//! Control execution tests through the parser, session, filesystem and numerical engine.
//! Provenance: original circuits, elementary circuit laws and ngspice user manual syntax.
#![allow(clippy::unwrap_used)]

use sansho_api::{
    Session, SessionOptions, Status,
    control::{self, ControlOptions, ControlOutput},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

fn script(deck: &str, body: &str, opts: &ControlOptions) -> ControlOutput {
    let nl=sansho_netlist::parse(&format!("batch test\nV1 in 0 1 ac 1\nR1 in out 1k\nR2 out 0 1k\n{deck}\n.control\n{body}\n.endc\n.end")).unwrap();
    control::execute_with(&nl, opts, 1)
}
fn success(o: &ControlOutput) {
    assert!(
        !o.diagnostics.iter().any(|d| d.is_error()),
        "{:?}",
        o.diagnostics
    )
}
fn values(o: &ControlOutput) -> Vec<f64> {
    o.transcript
        .prints
        .iter()
        .flat_map(|p| p.columns.iter().flatten().map(|z| z.0))
        .collect()
}
#[test]
fn scopes_substitution_branching_and_limits() {
    let opts = ControlOptions {
        iteration_limit: 30,
        ..Default::default()
    };
    let out = script(
        ".param k=2\n.csparam twice={2*k}",
        "
set steps = ( 0 1 2 )
let total = 0
foreach k $steps
  if $k eq 1
    continue
  else
    let total = total + $k
  end
end
dowhile total < 4
  let total = total + 1
end
while total < 100
  if total >= 5
    break
  end
  let total = total + 1
end
goto finish
shell
label finish
print total twice $steps[2] $#steps
quit
!",
        &opts,
    );
    success(&out);
    assert_eq!(values(&out), [5., 4., 1., 3.]);
    for body in [
        "while 1\nend",
        "repeat 100\nend",
        "dowhile 1\nend",
        "repeat 3\nforeach v 1 2 3 4 5 6 7 8 9 10 11\nend\nend",
        "label here\ngoto here",
    ] {
        let o = script("", body, &opts);
        assert_eq!(o.diagnostics.last().unwrap().code, "E-CONTROL-LIMIT");
    }
}
#[test]
fn expression_precedence_complex_vectors_and_seed() {
    let o = script(
        "",
        "let a = vector(4)\nlet a[2]=8\nprint a[3,1] (2+3*4) (-2^2) (2^3^2) mag(3+j(4)) imag(j(2)) pos(-2)\nsetseed 42\nlet draw=rnd(unitvec(12)*10)\nsetseed 42\nprint vecmax(abs(draw-rnd(unitvec(12)*10)))",
        &Default::default(),
    );
    success(&o);
    assert_eq!(values(&o), [3., 8., 1., 14., -4., 512., 5., 2., 0., 0.]);
    for expr in [
        "rnd(2)",
        "vector(3)[4]",
        "vector(3)+vector(2)",
        "bogus(1)",
        "vector(1e20)",
    ] {
        let o = script("", &format!("print {expr}"), &Default::default());
        assert!(
            o.diagnostics
                .iter()
                .any(|d| d.is_error() && d.span.is_some())
        );
    }
}
#[test]
fn analysis_cards_plot_lifetime_and_session_isolation() {
    let mut s = Session::new(SessionOptions {
        threads: 2,
        ..Default::default()
    });
    let c=s.load("job\nV1 a 0 1\nR1 a 0 1k\n.op\n.control\nrun\nalter v1=2\nop\nprint op1.v(a) op2.v(a) i(v1) @r1[resistance]\nsetplot op1\ndestroy op1\nop\n.endc").unwrap();
    let j = s.simulate(&c.circuit_id, None).unwrap();
    assert_eq!(j.status, Status::Ok);
    let t = j.control.unwrap();
    assert_eq!(t.plots, ["op2", "op3"]);
    assert_eq!(t.current_plot, "op3");
    assert_eq!(
        t.prints[0].columns,
        [[(1., 0.)], [(2., 0.)], [(-0.002, 0.)], [(1000., 0.)]]
    );
    assert_eq!(
        s.simulate(&c.circuit_id, None).unwrap().result_hash,
        j.result_hash
    );
    let explicit = s.simulate(&c.circuit_id, Some(&["op".into()])).unwrap();
    assert!(explicit.control.is_none());
    assert_eq!(explicit.analyses.len(), 1);
    let out = script(".op", "echo no implicit analysis", &Default::default());
    success(&out);
    assert!(out.results.analyses.is_empty());
    let out = script("", "op\ndestroy op1 missing", &Default::default());
    assert!(out.diagnostics.last().unwrap().is_error());
}
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "r10-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir(&d).unwrap();
        Self(d)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn forbidden_commands_and_file_boundaries() {
    let dir = Dir::new();
    let root = dir.0.join("output");
    fs::create_dir(&root).unwrap();
    let opts = ControlOptions {
        output_dir: Some(root.clone()),
        ..Default::default()
    };
    let outside = dir.0.join("outside");
    fs::write(&outside, b"keep").unwrap();
    for cmd in [
        "shell touch marker",
        "! touch marker",
        "cd ..",
        "source /etc/passwd",
        "sysinfo",
        "rusage",
        "circbyline R1 x y 1k",
    ] {
        let out = script("", cmd, &opts);
        assert!(
            out.diagnostics
                .last()
                .unwrap()
                .message
                .contains(cmd.split_whitespace().next().unwrap())
        )
    }
    for cmd in [
        "write ../outside v(out)",
        "wrdata ../outside v(out)",
        "print v(out)>../outside",
    ] {
        let out = script("", &format!("op\n{cmd}"), &opts);
        assert_eq!(out.diagnostics.last().unwrap().code, "E-CONTROL-OUTPUT");
    }
    let out = script(
        "",
        &format!("op\nwrite {} v(out)", outside.display()),
        &opts,
    );
    assert_eq!(out.diagnostics.last().unwrap().code, "E-CONTROL-OUTPUT");
    for cmd in [
        "write result.raw",
        "wrdata result.txt v(in)",
        "print v(in)>table",
    ] {
        let out = script("", &format!("op\n{cmd}"), &Default::default());
        assert_eq!(out.diagnostics.last().unwrap().code, "E-CONTROL-OUTPUT");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, root.join("leaf")).unwrap();
        std::os::unix::fs::symlink(&dir.0, root.join("link")).unwrap();
        for file in ["link/outside", "leaf"] {
            let out = script("", &format!("print pi > {file}"), &opts);
            assert_eq!(out.diagnostics.last().unwrap().code, "E-CONTROL-OUTPUT");
        }
        fs::hard_link(&outside, root.join("hard")).unwrap();
        success(&script("", "print pi > hard", &opts));
    }
    assert_eq!(fs::read(outside).unwrap(), b"keep");
}
#[test]
fn verification_decks_and_determinism() {
    let dir = Dir::new();
    let opts = ControlOptions {
        output_dir: Some(dir.0.clone()),
        ..Default::default()
    };
    for (name, expected) in [
        ("alter_meas", vec![1., 4. / 3., 1.5]),
        ("alterparam", vec![0.5, 1. / 3., 0.2]),
        ("foreach_temp", vec![1. / 1.892, 0.5, 1. / 2.212]),
        ("mos_model", vec![-0.00055, -0.00035]),
        ("ac_vectors", vec![]),
        ("setplot", vec![]),
        ("linearize", vec![]),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../verify/decks/r10_{name}.cir"));
        let nl = sansho_netlist::parse_file(&path).unwrap();
        let a = control::execute_with(&nl, &opts, 1);
        success(&a);
        let files: Vec<_> = a
            .transcript
            .written_files
            .iter()
            .map(|n| fs::read(dir.0.join(n)).unwrap())
            .collect();
        let b = control::execute_with(&nl, &opts, 2);
        assert_eq!(a.transcript, b.transcript);
        assert_eq!(a.results, b.results);
        for (n, v) in b.transcript.written_files.iter().zip(files) {
            assert_eq!(fs::read(dir.0.join(n)).unwrap(), v);
        }
        assert_eq!(expected.len(), a.results.measures.len());
        for (v, m) in expected.iter().zip(a.results.measures) {
            assert!((m.value.unwrap() - v).abs() < 2e-9)
        }
        match name {
            "setplot" => {
                let plot = a.results.analyses.last().unwrap();
                assert_eq!(plot.real("delta").unwrap().last(), Some(&0.25));
            }
            "linearize" => {
                let plot = a.results.analyses.last().unwrap();
                assert_eq!(plot.len(), 41);
                for (&t, &v) in plot.scale_values.iter().zip(plot.real("v(out)").unwrap()) {
                    assert!((v - t * 250000.).abs() < 1e-12)
                }
            }
            "ac_vectors" => {
                let plot = a.results.analyses.last().unwrap();
                let z = 2. * std::f64::consts::PI * 1e-5 * 1000.;
                let gain = -10. * (1. + z * z).log10();
                let phase = -z.atan();
                let value = |n| plot.complex(n).unwrap()[0].0;
                assert!(
                    (value("gain") - gain).abs() < 1e-12 && (value("phase") - phase).abs() < 1e-12
                );
                let slope = -20. * z * z / (1000. * (1. + z * z) * std::f64::consts::LN_10);
                assert!((value("slope") / slope - 1.).abs() < 0.0001);
            }
            _ => {}
        }
    }
}

#[test]
fn derived_measurement_scale_and_grid_interpolation() {
    let out = script(
        "",
        "dc v1 0 1 .25\nlet gain = -v(out)\nmeas dc minimum min gain\nprint minimum deriv(v(out))[2] integ(v(out))[4] v-sweep[2]\ndc v1 0 1 .5\nprint interpolate(dc1.gain)[1]",
        &Default::default(),
    );
    success(&out);
    assert_eq!(values(&out), [-0.5, 0.5, 0.25, 0.5, -0.25]);
}
#[test]
fn malformed_blocks_are_source_located() {
    for body in [
        ".control\nrepeat 3\nop\n.endc",
        ".control\nelse\n.endc",
        ".control\nop",
    ] {
        let errors = sansho_netlist::parse(&format!("demo\nV1 n 0 1\n{body}")).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == "E-CONTROL" && e.span.is_some())
        );
    }
}

#[test]
fn analysis_commands_use_the_card_solvers() {
    for name in [
        "noise_divider",
        "tf_divider",
        "sens_rc_ac",
        "pz_lead_lag",
        "four_clipped",
        "r9_sp_filter",
        "r9_disto_diode",
    ] {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../verify/decks/{name}.cir"));
        let mut nl = sansho_netlist::parse_file(&path).unwrap();
        let commands = nl
            .analyses
            .iter()
            .map(|a| {
                sansho_netlist::write_analysis(a)
                    .trim_start_matches('.')
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let template =
            sansho_netlist::parse(&format!("commands\n.control\n{commands}\n.endc")).unwrap();
        let reference =
            sansho_engine::run_with(&nl, &sansho_engine::RunConfig { threads: 1 }).unwrap();
        nl.analyses.clear();
        nl.controls = template.controls;
        let out = control::execute_with(&nl, &Default::default(), 1);
        success(&out);
        assert_eq!(out.results.analyses, reference.results.analyses, "{name}");
    }
}

#[test]
fn literal_device_observables_and_model_parameters() {
    let out = script(
        ".model nch nmos(vto=.4 kp=100u)\nM1 in in 0 0 nch l=1u w=2u",
        "op\nprint @m1[gm] @nch[vto] @m1[w]",
        &Default::default(),
    );
    success(&out);
    let v = values(&out);
    assert!((v[0] - 0.00012).abs() < 1e-10);
    assert_eq!(&v[1..], &[0.4, 2e-6]);
}

#[test]
fn conditional_blocks_preserve_deck_order_and_parameter_source_exports() {
    let input = "order\n.param val=1000\nV1 in 0 1\nR1 in 0 {val}\n.if (1)\n.control\necho first\n.endc\n.endif\n.control\nalterparam val=2000\nreset\nop\nprint i(v1)\n.endc";
    let mut s = Session::new(SessionOptions {
        threads: 1,
        ..Default::default()
    });
    let report = s.load(input).unwrap();
    let exported = s.export(&report.circuit_id).unwrap();
    let copy = s.load(&exported).unwrap();
    for id in [report.circuit_id, copy.circuit_id] {
        let j = s.simulate(&id, None).unwrap();
        assert_eq!(j.status, Status::Ok);
        let t = j.control.unwrap();
        assert_eq!(t.lines[0], "first");
        assert_eq!(t.prints[0].columns[0][0], (-0.0005, 0.));
    }
}

#[test]
fn probe_substitution_and_angle_units() {
    let out = script(
        "",
        "op\nset units=degrees\nprint $&v(out) $&@r1[resistance] ph(j(1)) sin(90) atan(1)",
        &Default::default(),
    );
    success(&out);
    assert_eq!(values(&out), [0.5, 1000., 90., 1., 45.]);
}

#[test]
fn digital_control_lists_nodes_and_prints_strengths() {
    let o = script(
        "Ap bus pull\n.model pull d_pullup",
        "op\nedisplay\neprint bus",
        &Default::default(),
    );
    success(&o);
    assert!(o.transcript.lines.iter().any(|s| s == "bus digital"));
    assert!(
        o.transcript
            .lines
            .iter()
            .any(|s| s == "0.000000e+00 bus 1r")
    );
}

#[test]
fn showmod_lists_current_model_parameters_without_an_analysis() {
    let out = script(
        ".model clock d_osc(cntl_array=[0 2] freq_array=[1 3] duty_cycle=0.5)",
        "altermod clock duty_cycle=0.25\nshowmod\necho done",
        &Default::default(),
    );
    success(&out);
    assert_eq!(
        out.transcript.lines,
        [
            ".model clock d_osc (duty_cycle=2.500000e-01 cntl_array=[0.000000e+00 2.000000e+00] freq_array=[1.000000e+00 3.000000e+00])",
            "done",
        ]
    );
    assert!(out.results.analyses.is_empty());
}

#[test]
fn batch_precision_distinguishes_print_wrdata_measurement_and_echo() {
    let dir = Dir::new();
    let opts = ControlOptions {
        output_dir: Some(dir.0.clone()),
        ..Default::default()
    };
    let o = script(
        "",
        "set numdgt=4\ndc v1 0 1 .5\nlet z=v(out)+j(-v(out))\nset wr_vecnames\nwrdata fields z\nprint col z > columns\nmeas dc endpoint find i(v1) at=1\necho $&endpoint\nset numdgt=-6\nprint col z > defaults\nset numdgt=3\nprint line 9.9999e99",
        &opts,
    );
    success(&o);
    let file = fs::read_to_string(dir.0.join("fields")).unwrap();
    assert_eq!(
        file.lines().nth(2),
        Some(" 5.0000e-01  2.5000e-01 -2.5000e-01 ")
    );
    assert_eq!(
        file.lines().next(),
        Some(" v-sweep     z           z          ")
    );
    let defaults = fs::read_to_string(dir.0.join("defaults")).unwrap();
    assert_eq!(
        defaults.lines().nth(2),
        Some("1\t5.000000e-01\t2.500000e-01,\t-2.50000e-01\t")
    );
    let cols = fs::read_to_string(dir.0.join("columns")).unwrap();
    assert_eq!(
        cols.lines().nth(2),
        Some("1\t5.0000e-01\t2.5000e-01,\t-2.500e-01\t")
    );
    assert!(
        o.transcript
            .lines
            .iter()
            .any(|s| s == "endpoint            =  -5.000000e-04")
    );
    assert!(o.transcript.lines.iter().any(|s| s == "-0.0005"));
    assert!(
        o.transcript
            .lines
            .last()
            .unwrap()
            .ends_with(" = 1.000e+100")
    );
    assert_eq!(o.transcript.prints[0].columns[0][1], (0.25, -0.25));
}
