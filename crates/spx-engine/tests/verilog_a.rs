//! Verilog-A integration against authored models, built-ins and analytic circuit theory.
//! Provenance: original tests; Accellera LRM 2.4 and repository diode/junction equations.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use spx_engine::{RunConfig, run_with};
use spx_netlist::{Netlist, SourceLoader, parse_with};
use spx_output::{AnalysisResult, SimResults};

struct Models;
impl SourceLoader for Models {
    fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
        let text = match path {
            "resistor.va" => include_str!("va/resistor.va"),
            "capacitor.va" => include_str!("va/capacitor.va"),
            "diode.va" => include_str!("va/diode.va"),
            "init.va" => {
                r#"module init(p,n); electrical p,n; real r;
                analog begin @(initial_step) r=1000; I(p,n)<+V(p,n)/r; end endmodule"#
            }
            "rates.va" => {
                r#"module rates(p,u,w); electrical p,u,w; real rate;
                analog begin rate=ddt(V(u)); V(p)<+(1+V(w))*rate+sin(ddt(V(w))); end endmodule"#
            }
            "bad.va" => {
                r#"module bad(p,n); electrical p,n; analog $error("authored failure"); endmodule"#
            }
            _ => return Err(format!("unknown test source {path}")),
        };
        Ok((path.into(), text.into()))
    }
}
fn deck(text: &str) -> Netlist {
    parse_with(text, &Models).unwrap_or_else(|d| panic!("{d:#?}"))
}
fn simulate(text: &str) -> SimResults {
    run_with(&deck(text), &RunConfig { threads: 1 })
        .unwrap_or_else(|e| panic!("{:#?}", e.diagnostics))
        .results
}
fn close(a: f64, b: f64, rel: f64, abs: f64) {
    assert!((a - b).abs() <= abs + rel * b.abs(), "{a:e} != {b:e}");
}
fn interp(a: &AnalysisResult, signal: &str, t: f64) -> f64 {
    let v = a.real(signal).unwrap();
    let k = a.scale_values.partition_point(|x| *x < t);
    if k == 0 {
        return v[0];
    }
    if k == v.len() {
        return v[k - 1];
    }
    let lo = a.scale_values[k - 1];
    let hi = a.scale_values[k];
    v[k - 1] + (v[k] - v[k - 1]) * (t - lo) / (hi - lo)
}

/// Quadratic interpolation through the three nearest samples.
fn interp2(a: &AnalysisResult, signal: &str, t: f64) -> f64 {
    let v = a.real(signal).unwrap();
    let x = &a.scale_values;
    let k = x.partition_point(|s| *s < t).clamp(1, x.len() - 2);
    let (x0, x1, x2) = (x[k - 1], x[k], x[k + 1]);
    v[k - 1] * (t - x1) * (t - x2) / ((x0 - x1) * (x0 - x2))
        + v[k] * (t - x0) * (t - x2) / ((x1 - x0) * (x1 - x2))
        + v[k + 1] * (t - x0) * (t - x1) / ((x2 - x0) * (x2 - x1))
}

#[test]
fn resistor_op_dc_subcircuit_and_overrides() {
    let va = simulate(
        "va
.subckt cell a b
.model local vares r=3k
N1 a mid local r=1k
N2 mid b local
.ends
V1 in 0 2
X1 in 0 cell
.hdl resistor.va
.op
.dc v1 -1 2 0.1
",
    );
    let builtin = simulate(
        "builtin
V1 in 0 2
R1 in mid 1k
R2 mid 0 3k
.op
.dc v1 -1 2 0.1
",
    );
    for (a, b) in va.analyses.iter().zip(&builtin.analyses) {
        for signal in ["i(v1)"] {
            for (&x, &y) in a.real(signal).unwrap().iter().zip(b.real(signal).unwrap()) {
                close(x, y, 1e-9, 1e-12);
            }
        }
    }
    close(
        va.analyses[0].real("v(x1.mid)").unwrap()[0],
        1.5,
        1e-9,
        1e-12,
    );
}

#[test]
fn rc_transient_and_ac() {
    let va = simulate(
        "va rc
.hdl resistor.va
.hdl capacitor.va
.model r vares r=1k
.model c vacap c=1n
V1 in 0 AC 1 PULSE(0 1 0 1n 1n 1 2)
N1 in out r
N2 out 0 c
.options reltol=1e-7
.tran 1n 5u 0 1n
.ac dec 5 1 1meg
",
    );
    let builtin = simulate(
        "rc
V1 in 0 AC 1 PULSE(0 1 0 1n 1n 1 2)
R1 in out 1k
C1 out 0 1n
.options reltol=1e-7
.tran 1n 5u 0 1n
.ac dec 5 1 1meg
",
    );
    let a = &va.analyses[0];
    for (&t, &v) in a.scale_values.iter().zip(a.real("v(out)").unwrap()) {
        let tau = 1e-6;
        let tr = 1e-9;
        let exact = if t <= tr {
            (t - tau * (1. - (-t / tau).exp())) / tr
        } else {
            1. - (tau / tr) * ((tr / tau).exp() - 1.) * (-t / tau).exp()
        };
        close(v, exact, 0., 1e-6);
        close(v, interp(&builtin.analyses[0], "v(out)", t), 1e-6, 1e-9);
    }
    for (&x, &y) in va.analyses[1]
        .complex("v(out)")
        .unwrap()
        .iter()
        .zip(builtin.analyses[1].complex("v(out)").unwrap())
    {
        close(x.0, y.0, 1e-9, 1e-12);
        close(x.1, y.1, 1e-9, 1e-12);
    }
}

#[test]
fn diode_op_and_dc_match_builtin() {
    for rs in [0., 2., 100.] {
        let cards = ".op
.dc v1 -1 1 0.02
.options reltol=1e-10 abstol=1e-15 vntol=1e-12
";
        let va = simulate(&format!(
            "va diode
.hdl diode.va
.model dm vadiode IS=1e-14 N=1.2 RS={rs} TT=1n CJ0=2p
V1 in 0 1
R1 in out 1k
N1 out 0 dm
{cards}"
        ));
        let builtin = simulate(&format!(
            "diode
.model dm d is=1e-14 n=1.2 rs={rs} tt=1n cj0=2p
V1 in 0 1
R1 in out 1k
D1 out 0 dm
{cards}"
        ));
        for (a, b) in va.analyses.iter().zip(&builtin.analyses) {
            for signal in ["v(out)", "i(v1)"] {
                for (&x, &y) in a.real(signal).unwrap().iter().zip(b.real(signal).unwrap()) {
                    close(x, y, 1e-9, 1e-12);
                }
            }
        }
    }
}

#[test]
fn rectifier_transient_matches_builtin() {
    let common = "V1 in 0 SIN(0 1 1meg)
R1 in out 1k
C1 out 0 10p
.options reltol=1e-8 abstol=1e-16 chgtol=1e-20
.tran 0.1n 2u 0 0.1n
";
    let va = simulate(&format!(
        "va
.hdl diode.va
.model dm vadiode IS=1e-14 N=1 RS=2 TT=1n CJ0=2p
N1 out 0 dm
{common}"
    ));
    let builtin = simulate(&format!(
        "builtin
.model dm d is=1e-14 n=1 rs=2 tt=1n cj0=2p
D1 out 0 dm
{common}"
    ));
    for (&t, &v) in va.analyses[0]
        .scale_values
        .iter()
        .zip(va.analyses[0].real("v(out)").unwrap())
    {
        // The two runs may take different time grids (a 1-ulp difference in libm `exp`
        // can flip a step decision). Linear interpolation would add up to ~1e-7 V, so the
        // built-in waveform is interpolated quadratically.
        close(v, interp2(&builtin.analyses[0], "v(out)", t), 1e-6, 1e-9);
    }
}

#[test]
fn parallel_determinism_and_independent_initialization() {
    let mut text = String::from(
        "parallel
.hdl init.va
.model r init
V1 in 0 2
",
    );
    for k in 0..260 {
        text.push_str(&format!(
            "N{k} in n{k} r
R{k} n{k} 0 1k
"
        ));
    }
    text.push_str(
        ".op
.dc v1 0 2 0.5
.tran 1n 3n
",
    );
    let nl = deck(&text);
    let a = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    let b = run_with(&nl, &RunConfig { threads: 4 }).unwrap().results;
    assert_eq!(a, b);
    close(a.analyses[0].real("v(n0)").unwrap()[0], 1., 1e-9, 1e-12);
}

#[test]
fn structured_errors() {
    for (model, instance) in [
        ("missing", "N1 a 0 dm"),
        ("vares", "N1 a 0 b dm"),
        ("vares r=-1", "N1 a 0 dm"),
        ("vares", "N1 a 0 dm r=0"),
    ] {
        let nl = deck(&format!(
            "errors
.hdl resistor.va
.model dm {model}
V1 a 0 1
{instance}
.op
"
        ));
        let ds = spx_engine::check(&nl).unwrap_err();
        assert_eq!(
            ds[0].code,
            if model.contains('=') || instance.contains('=') {
                "E-VA-PARAM"
            } else {
                "E-ELAB-PARAM"
            }
        );
        assert!(ds[0].span.is_some());
    }
    let nl = deck(
        "runtime
.hdl bad.va
.model dm bad
V1 a 0 1
N1 a 0 dm
.op
",
    );
    let error = spx_engine::run(&nl).unwrap_err();
    assert_eq!(error.diagnostics[0].code, "E-VA-EVAL");
    assert!(error.diagnostics[0].message.contains("authored failure"));
}

#[test]
fn va_dc_parameter_rebinding() {
    // DC sweeps address the conventional "dc" parameter; the group rebinds the instance.
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok(("source.va".into(), "module source(p,n); electrical p,n; parameter real dc=1; analog V(p,n)<+dc; endmodule".into()))
        }
    }
    let nl = parse_with(
        "sweep
.hdl source.va
.model src source
N1 a 0 src
R1 a 0 1k
.dc n1 0 2 0.5
",
        &Loader,
    )
    .unwrap();
    let r = spx_engine::run(&nl).unwrap().results;
    assert_eq!(r.analyses[0].real("v(a)").unwrap(), &[0., 0.5, 1., 1.5, 2.]);
}

#[test]
fn limiter_states_internal_unknowns_and_parallel_chunks() {
    let mut text = String::from(
        "diode chunks\n.hdl diode.va\n.hdl diode.va\n.model d vadiode RS=2\nV1 in 0 1\n",
    );
    for k in 0..260 {
        text.push_str(&format!("R{k} in n{k} 1k\nN{k} n{k} 0 d\n"));
    }
    text.push_str(".op\n.dc v1 0 1 0.5\n");
    let nl = deck(&text);
    let a = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    let b = run_with(&nl, &RunConfig { threads: 4 }).unwrap().results;
    assert_eq!(a, b);
    let a = &a.analyses[0];
    assert!(a.real("v(n0#j)").unwrap()[0] > 0.5);
    close(
        a.real("i(n0#series)").unwrap()[0],
        a.real("i(n259#series)").unwrap()[0],
        0.,
        0.,
    );
}

#[test]
fn multiple_modules_and_va_diagnostic_locations() {
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
            let text = match path {
                "two.va" => {
                    "module a(p,n); electrical p,n; analog I(p,n)<+V(p,n)/1000; endmodule\nmodule b(p,n); electrical p,n; analog I(p,n)<+V(p,n)/2000; endmodule"
                }
                "invalid.va" => {
                    "module bad(p,n); electrical p,n; analog I(p,n)<+missing; endmodule"
                }
                _ => return Err("no source".into()),
            };
            Ok((path.into(), text.into()))
        }
    }
    let nl = parse_with(
        "two\n.hdl two.va\n.model a1 a\n.model b1 b\nV1 in 0 1\nN1 in 0 a1\nN2 in 0 b1\n.op\n",
        &Loader,
    )
    .unwrap();
    let r = spx_engine::run(&nl).unwrap().results;
    close(
        r.analyses[0].real("i(v1)").unwrap()[0],
        -0.0015,
        1e-9,
        1e-12,
    );
    let nl = parse_with("invalid\n.hdl invalid.va\nV1 a 0 1\n.op\n", &Loader).unwrap();
    let d = spx_engine::check(&nl).unwrap_err();
    assert_eq!(d[0].code, "E-VA-SEMA");
    assert_eq!(d[0].span.unwrap().line, 2);
    assert!(d[0].notes.iter().any(|n| n.contains("invalid.va:1:")));
    let mut duplicate = nl.clone();
    duplicate.hdl = vec![
        spx_netlist::HdlSource {
            path: "a.va".into(),
            text: "module a(p); electrical p; endmodule".into(),
            span: spx_netlist::Span::new(2, 1),
        },
        spx_netlist::HdlSource {
            path: "b.va".into(),
            text: "module a(p); electrical p; endmodule".into(),
            span: spx_netlist::Span::new(3, 1),
        },
    ];
    let d = spx_engine::check(&duplicate).unwrap_err();
    assert_eq!(d[0].code, "E-VA-SEMA");
    assert!(d[0].message.contains("duplicate"));
}

#[test]
fn psp103_dc_transfer() {
    let root = std::env::var_os("SPX_VA_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../va-models/ihp")
        });
    let file = root.join("psp103/psp103.va");
    if !file.try_exists().unwrap() {
        eprintln!("skipping missing {}", file.display());
        return;
    }
    let text = format!(
        "PSP transfer\n.hdl \"{}\"\n.model nmod psp103_va type=1\nN1 d g 0 0 nmod l=1u w=1u\nVD d 0 1.2\nVG g 0 0\n.dc vg 0 1.2 0.1\n",
        file.canonicalize().unwrap().display()
    );
    let nl = parse_with(&text, &spx_netlist::FsLoader { base: root }).unwrap();
    let output = spx_engine::run(&nl).unwrap();
    let dc = &output.results.analyses[0];
    let id = dc.real("i(vd)").unwrap();
    assert_eq!(id.len(), 13);
    assert!(id.iter().all(|v| v.is_finite()));
    assert!(
        id.windows(2).all(|w| w[0] >= w[1]) && id[12] < id[0],
        "{id:?}"
    );
}

#[test]
fn mixed_instance_layouts_and_external_port_shorts() {
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok((
                path.into(),
                format!(
                    "{}\nmodule strap(a,b); electrical a,b; analog V(a,b)<+0; endmodule",
                    include_str!("../../spx-va/tests/models/collapse.va")
                ),
            ))
        }
    }
    let nl=parse_with("shorts\n.hdl devices.va\n.model d series_junction\n.model link strap\nVD p 0 0.45\nNlink p a link\nNshort a 0 d\nNseries a q d rs=0.0001\nNreturn q 0 link\n.op\n",&Loader).unwrap();
    let checked = spx_engine::check(&nl).unwrap();
    assert_eq!(checked.circuit.dim(), 3); // supply voltage, supply current, one junction
    let r = spx_engine::run(&nl).unwrap().results;
    let op = &r.analyses[0];
    assert_eq!(op.real("v(a)"), op.real("v(p)"));
    assert_eq!(op.real("v(q)").unwrap(), &[0.]);
    let expected = 1e-12 * ((0.45_f64 / 0.026).exp() - 1.);
    close(-op.real("i(vd)").unwrap()[0], 2. * expected, 1e-6, 1e-12);
    assert!(op.real("v(nseries#junction)").is_some());
    assert!(op.real("v(nshort#junction)").is_none());
}

#[test]
fn derivative_expressions_in_dc_ac_and_transient() {
    let text = "independent derivatives
.hdl rates.va
.model rates_model rates
Vu u 0 dc 2 ac 1 pwl(0 2 0.1 2 0.9 2.4)
Vw w 0 dc 1 ac 0.5 pwl(0 1 0.1 1 0.9 1.2)
N1 p u w rates_model
Rp p 0 1k
.op
.ac lin 1 10 10
.tran 0.01 0.8
";
    for method in ["trap", "gear"] {
        let results = simulate(&format!("{text}.options method={method} rshunt=10\n"));
        close(results.analyses[0].real("v(p)").unwrap()[0], 0., 0., 1e-12);
        let ac = results.analyses[1].complex("v(p)").unwrap()[0];
        close(ac.0, 0., 0., 1e-12);
        close(ac.1, 2.5 * std::f64::consts::TAU * 10., 1e-12, 0.);
        let tran = &results.analyses[2];
        for (&t, &v) in tran.scale_values.iter().zip(tran.real("v(p)").unwrap()) {
            if t > 0.2 {
                close(
                    v,
                    (2. + 0.25 * (t - 0.1)) * 0.5 + 0.25_f64.sin(),
                    1e-9,
                    1e-12,
                );
            }
        }
    }
}

#[test]
fn integral_has_an_initial_condition_and_a_dynamic_state_equation() {
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok((
                "integral.va".into(),
                "module integral(p,u); electrical p,u; analog V(p)<+idt(V(u),0.25); endmodule"
                    .into(),
            ))
        }
    }
    let nl=parse_with("integral\n.hdl integral.va\n.model int integral\nVu u 0 2 AC 1\nN1 p u int\nRp p 0 1k\n.op\n.tran 0.01 0.2\n.ac lin 1 100 100\n",&Loader).unwrap();
    let results = spx_engine::run(&nl).unwrap().results;
    close(
        results.analyses[0].real("v(p)").unwrap()[0],
        0.25,
        1e-12,
        1e-12,
    );
    let tran = &results.analyses[1];
    for (&t, &v) in tran.scale_values.iter().zip(tran.real("v(p)").unwrap()) {
        close(v, 0.25 + 2. * t, 1e-8, 1e-12);
    }
    let ac = results.analyses[2].complex("v(p)").unwrap()[0];
    close(ac.0, 0., 0., 1e-12);
    close(ac.1, -1. / (std::f64::consts::TAU * 100.), 1e-8, 1e-12);
}

#[test]
fn parallel_va_instances_scale_terminal_currents_and_charges_once() {
    struct Loader;
    impl SourceLoader for Loader {
        fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok((
                "parallel.va".into(),
                "module plain(p,n); electrical p,n; analog I(p,n)<+V(p,n)+ddt(V(p,n)); endmodule
                 module own(p,n); electrical p,n; parameter real MuLt=1;
                 analog I(p,n)<+MuLt*V(p,n)+MuLt*ddt(V(p,n)); endmodule
                 module meter(p,n); electrical p,n; analog V(p,n)<+$mfactor; endmodule"
                    .into(),
            ))
        }
    }
    // Both the ordinary current and the capacitive susceptance triple.
    // A voltage contribution exposes $mfactor without scaling its voltage.
    for model in ["plain", "own"] {
        for key in ["m", "mult"] {
            let text = format!(
                "copies\n.model active {model}\n.hdl parallel.va\nV1 p 0 1 AC 1\nN1 p 0 active {key}=3\n.model sense meter\nNs b 0 sense m=4\nRs b 0 1k\n.op\n.ac lin 1 1 1\n"
            );
            let nl = parse_with(&text, &Loader).unwrap();
            let r = spx_engine::run(&nl).unwrap().results;
            close(r.analyses[0].real("i(v1)").unwrap()[0], -3., 1e-12, 0.);
            close(r.analyses[0].real("v(b)").unwrap()[0], 4., 1e-12, 0.);
            let (g, b) = r.analyses[1].complex("i(v1)").unwrap()[0];
            close(g, -3., 1e-12, 0.);
            close(b, -3. * std::f64::consts::TAU, 1e-12, 0.);
        }
    }
}

#[test]
fn ihp_corner_transfer_and_inverter_switching() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root
        .join("va-models/ihp/ngspice-models/cornerMOSlv.lib")
        .try_exists()
        .unwrap()
        || !root
            .join("va-models/ihp/psp103/psp103.va")
            .try_exists()
            .unwrap()
    {
        eprintln!("skipping IHP circuit tests: local SG13G2 inputs absent");
        return;
    }
    let run = |name: &str| {
        let nl = spx_netlist::parse_file(&root.join("verify/va-decks").join(name)).unwrap();
        spx_engine::run(&nl).unwrap()
    };
    let dc = run("psp_nmos_idvg.cir");
    let i = dc.results.analyses[0].real("i(vd)").unwrap();
    assert_eq!(i.len(), 121);
    assert!(i.windows(2).all(|x| x[1] <= x[0]));
    assert!(i[0].abs() < 1e-8 && (1e-4..1e-3).contains(&-i[120]));
    assert_eq!(dc.dc_stats[0].extra_solves, 0);
    let tran = run("psp_inverter_tran.cir").results;
    let a = &tran.analyses[0];
    assert!(interp(a, "v(out)", 100e-12) > 1.15);
    assert!(interp(a, "v(out)", 600e-12) < 0.05);
    assert!(interp(a, "v(out)", 1.5e-9) > 1.15);
    assert!((1e-12..200e-12).contains(&tran.measures[0].value.unwrap()));
}

#[test]
fn parallel_va_assembly_is_bit_identical() {
    let mut text = String::from("VA fanout\n.hdl resistor.va\n.model r vares\nV1 in 0 1\n");
    for i in 0..96 {
        text.push_str(&format!("N{i} in out r r={}\n", 1000 + i));
    }
    text.push_str("C1 out 0 1p\nR1 out 0 1k\n.tran 1n 10n\n");
    let nl = deck(&text);
    let reference = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    for threads in [2, 5] {
        let result = run_with(&nl, &RunConfig { threads }).unwrap().results;
        let expected = &reference.analyses[0];
        let actual = &result.analyses[0];
        assert_eq!(expected.variables, actual.variables);
        let bits = |xs: &[f64]| xs.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&expected.scale_values), bits(&actual.scale_values));
        for v in &expected.variables {
            assert_eq!(
                bits(expected.real(&v.name).unwrap()),
                bits(actual.real(&v.name).unwrap())
            );
        }
    }
}

#[cfg(feature = "jit")]
#[test]
fn ihp_jit_and_interpreter_match_every_vector() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for dir in [
        "psp103/psp103.va",
        "vbic_1p3/vbic_1p3.va",
        "r3_cmc/r3_cmc.va",
        "ngspice-models/cornerMOSlv.lib",
        "ngspice-models/cornerHBT.lib",
        "ngspice-models/cornerRES.lib",
    ] {
        if !root.join("va-models/ihp").join(dir).try_exists().unwrap() {
            eprintln!("skipping IHP vector comparison, missing {dir}");
            return;
        }
    }
    compare_va_vectors(&root, false);
}

#[cfg(feature = "jit")]
#[test]
fn cmg_jit_and_interpreter_match_every_vector() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if root.join("va-models/bsimcmg/bsimcmg.va").is_file() {
        compare_va_vectors(&root, true);
    } else {
        eprintln!("skipping missing BSIM-CMG model");
    }
}

#[cfg(feature = "jit")]
fn compare_va_vectors(root: &std::path::Path, cmg: bool) {
    let bits = |xs: &[f64]| xs.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    for file in root.join("verify/va-decks").read_dir().unwrap() {
        let path = file.unwrap().path();
        if path.extension().is_none_or(|e| e != "cir")
            || path
                .file_name()
                .is_some_and(|s| s.to_string_lossy().starts_with("cmg_"))
                != cmg
        {
            continue;
        }
        let mut nl = spx_netlist::parse_file(&path).unwrap();
        nl.saves.clear();
        let native = run_with(&nl, &RunConfig { threads: 5 }).unwrap().results;
        nl.options.push(spx_netlist::OptionCard {
            name: "va_jit".into(),
            value: Some(0.),
            span: spx_netlist::Span::new(1, 1),
        });
        assert!(!spx_engine::SimOptions::from_cards(&nl.options).0.va_jit);
        let other = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
        assert_eq!(native.analyses.len(), other.analyses.len());
        for (a, b) in native.analyses.iter().zip(&other.analyses) {
            assert_eq!(a.variables, b.variables);
            assert_eq!(bits(&a.scale_values), bits(&b.scale_values), "{path:?}");
            for v in &a.variables {
                if let Some(column) = a.real(&v.name) {
                    assert_eq!(
                        bits(column),
                        bits(b.real(&v.name).unwrap()),
                        "{path:?} {}",
                        v.name
                    );
                } else {
                    let complex = |r: &AnalysisResult| {
                        r.complex(&v.name)
                            .unwrap()
                            .iter()
                            .map(|(x, y)| (x.to_bits(), y.to_bits()))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(complex(a), complex(b), "{path:?} {}", v.name);
                }
            }
        }
    }
}

#[test]
fn monte_carlo_va_binding_with_concurrent_analysis_cards() {
    let src = "private VA run data\n.hdl resistor.va\n.model m vares r={aunif(1000,100)}\nN1 a 0 m\nV1 a 0 dc 1 ac 1\n.op\n.dc v1 0 1 1\n.ac lin 2 1 10\n.meas dc dc_i find i(v1) at=1\n.meas ac ac_i find im(v1) at=1\n.end";
    let plan = spx_engine::monte_carlo::MonteCarlo::prepare(src, &Models).unwrap();
    let serial = plan.batch(193, 5, true, &RunConfig { threads: 1 }).unwrap();
    assert!(
        serial
            .runs
            .iter()
            .all(|r| r.success && r.measures.iter().all(|m| m.value.is_some())),
        "{serial:?}"
    );
    assert_ne!(serial.runs[0].measures, serial.runs[1].measures);
    assert_eq!(
        plan.batch(193, 5, true, &RunConfig { threads: 3 }).unwrap(),
        serial
    );
}
