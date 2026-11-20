//! Session API: the design loop (load → simulate → measure → alter → rerun) and
//! reproducibility guarantees.
//!
//! Provenance: original test circuits.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use sansho_api::{Change, Reduce, Session, SessionOptions, Status};

const RC: &str = "rc lowpass
V1 in 0 PULSE(0 1 0 1n 1n 1 2)
R1 in out 1k
C1 out 0 1n
.tran 10n 10u
.meas tran tr trig v(out) val=0.1 rise=1 targ v(out) val=0.9 rise=1
.end
";

fn session(threads: usize) -> Session {
    Session::new(SessionOptions {
        threads,
        ..SessionOptions::default()
    })
}

fn tr(s: &sansho_api::JobSummary) -> f64 {
    s.measures
        .iter()
        .find(|m| m.name == "tr")
        .unwrap()
        .value
        .unwrap()
}

#[test]
fn design_loop() {
    let mut s = session(2);
    let c = s.load(RC).unwrap();
    assert_eq!(c.stats.unknowns, 3);
    let j1 = s.simulate(&c.circuit_id, None).unwrap();
    assert_eq!(j1.status, Status::Ok);
    // Rise time of an RC low-pass: ln(9)·RC.
    let expect = 9f64.ln() * 1e-6;
    assert!((tr(&j1) - expect).abs() < 1e-2 * expect, "{}", tr(&j1));
    assert_eq!(j1.measures[0].unit, "s");

    // Halve R: the rise time halves.
    let a = s
        .alter(
            &c.circuit_id,
            &[Change {
                target: "R1".into(),
                param: "value".into(),
                value: 500.0,
            }],
        )
        .unwrap();
    assert_eq!(a.applied[0].old, Some(1000.0));
    assert_ne!(a.input_hash, c.input_hash);
    let j2 = s.simulate(&c.circuit_id, None).unwrap();
    assert!((tr(&j2) - expect / 2.0).abs() < 1e-2 * expect);
    assert_ne!(j1.result_hash, j2.result_hash);

    // Measurements after the fact.
    let m = s
        .measure(&j2.job_id, &["tran vmax max v(out)".into()])
        .unwrap();
    assert!((m[0].value.unwrap() - 1.0).abs() < 1e-3);
    assert_eq!(m[0].unit, "V");

    // Waveform with a point budget keeps the end points.
    let w = s
        .waveform(&j2.job_id, "v(out)", None, Reduce::Real, None, None, 50)
        .unwrap();
    assert!(w.downsampled && w.x.len() == 50 && w.source_points > 50);
    assert_eq!(w.x[0], 0.0);
    assert!((w.x[49] - 10e-6).abs() < 1e-15);
    assert_eq!((w.unit, w.x_unit), ("V", "s"));

    // Analysis override.
    let j3 = s
        .simulate(&c.circuit_id, Some(&["ac dec 10 1k 10meg".into()]))
        .unwrap();
    assert_eq!(j3.analyses[0].kind, sansho_output::AnalysisKind::Ac);
    let w = s
        .waveform(&j3.job_id, "v(out)", None, Reduce::Db, None, None, 1000)
        .unwrap();
    assert!(!w.downsampled && w.unit == "dB");
}

#[test]
fn results_are_reproducible_across_thread_counts() {
    let (mut a, mut b) = (session(1), session(6));
    let ca = a.load(RC).unwrap();
    let cb = b.load(RC).unwrap();
    assert_eq!(ca.input_hash, cb.input_hash);
    let ja = a.simulate(&ca.circuit_id, None).unwrap();
    let jb = b.simulate(&cb.circuit_id, None).unwrap();
    assert_eq!(ja.result_hash, jb.result_hash);
    assert_eq!(ja.result_hash.len(), 16);
}

#[test]
fn sweep_runs_in_parallel_and_matches_single_runs() {
    let mut s = session(4);
    let c = s.load(RC).unwrap();
    let values = [250.0, 500.0, 1000.0, 2000.0];
    let rep = s.sweep(&c.circuit_id, "r1", "value", &values, &[]).unwrap();
    assert_eq!(rep.rows.len(), 4);
    for (row, &r) in rep.rows.iter().zip(&values) {
        let v = row.measures[0].value.unwrap();
        let expect = 9f64.ln() * r * 1e-9;
        assert!((v - expect).abs() < 2e-2 * expect, "R={r}: {v}");
    }
    // The stored circuit is unchanged by a sweep.
    let j = s.simulate(&c.circuit_id, None).unwrap();
    let single = s.job_summary(&rep.rows[2].job_id).unwrap();
    assert_eq!(j.result_hash, single.result_hash);
}

#[test]
fn failures_are_structured() {
    let mut s = session(1);
    let v = s.validate("t\nR1 a 0 1k\nQ1 a b\n");
    assert!(!v.ok && v.diagnostics[0].is_error());
    let e = s.load("t\nV1 a 0 1\nC1 a b 1n\nC2 b 0 1n\n.op\n").unwrap();
    let j = s.simulate(&e.circuit_id, None).unwrap();
    assert_eq!(j.status, Status::Error);
    assert_eq!(j.diagnostics[0].code, "E-SOLVE-SINGULAR");
    let x = s.explain(&j.job_id).unwrap();
    assert!(x.codes.iter().any(|c| c.code == "E-SOLVE-SINGULAR"));
    assert!(!s.explain("E-CONV-OP").unwrap().codes[0].remedies.is_empty());
    let bad = s.alter(
        &e.circuit_id,
        &[Change {
            target: "nosuch".into(),
            param: "value".into(),
            value: 1.0,
        }],
    );
    assert_eq!(bad.unwrap_err().diagnostics[0].code, "E-API-ALTER");
    assert_eq!(
        s.simulate("c99", None).unwrap_err().diagnostics[0].code,
        "E-API-UNKNOWN-ID"
    );
    assert_eq!(
        s.waveform(&j.job_id, "v(zz)", None, Reduce::Real, None, None, 10)
            .unwrap_err()
            .diagnostics[0]
            .code,
        "E-API-UNKNOWN-SIGNAL"
    );
}

#[test]
fn export_round_trips() {
    let mut s = session(1);
    let c = s.load(RC).unwrap();
    let deck = s.export(&c.circuit_id).unwrap();
    let c2 = s.load(&deck).unwrap();
    assert_eq!(c.input_hash, c2.input_hash);
    let a = s.simulate(&c.circuit_id, None).unwrap();
    let b = s.simulate(&c2.circuit_id, None).unwrap();
    assert_eq!(a.result_hash, b.result_hash);
}

#[test]
fn model_and_option_alterations() {
    let mut s = session(1);
    let c = s
        .load("d\nV1 a 0 0.7\nD1 a 0 dm\n.model dm d is=1e-14\n.op\n")
        .unwrap();
    let i0 = s.simulate(&c.circuit_id, None).unwrap().analyses[0].values[1].value;
    s.alter(
        &c.circuit_id,
        &[
            Change {
                target: "dm".into(),
                param: "is".into(),
                value: 2e-14,
            },
            Change {
                target: ".options".into(),
                param: "reltol".into(),
                value: 1e-6,
            },
        ],
    )
    .unwrap();
    let j = s.simulate(&c.circuit_id, None).unwrap();
    assert_eq!(j.options.reltol, 1e-6);
    let i1 = j.analyses[0].values[1].value;
    assert!((i1 / i0 - 2.0).abs() < 1e-3, "{i0} {i1}");
}

#[test]
fn load_hdl_file_and_alter_instance() {
    let dir = std::env::temp_dir().join(format!("sansho-api-va-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("models/nested")).unwrap();
    std::fs::write(
        dir.join("models/nested/params.va"),
        "parameter real r=1000 from (0:inf);",
    )
    .unwrap();
    std::fs::write(
        dir.join("models/resistor.va"),
        r#"
        `include "disciplines.vams"
        module resistor(p,n); electrical p,n;
        `include "nested/params.va"
        analog I(p,n)<+V(p,n)/r; endmodule
    "#,
    )
    .unwrap();
    std::fs::write(dir.join("included.cir"), ".hdl \"models/resistor.va\"\n").unwrap();
    let path = dir.join("deck.cir");
    std::fs::write(
        &path,
        "va\n.include included.cir\n.model rm resistor r=2k\nV1 a 0 1\nN1 a 0 rm\n.op\n",
    )
    .unwrap();
    let mut s = session(1);
    let loaded = s.load_file(&path).unwrap();
    let first = s.simulate(&loaded.circuit_id, None).unwrap();
    assert_eq!(first.status, Status::Ok);
    let current = first.analyses[0]
        .values
        .iter()
        .find(|v| v.name == "i(v1)")
        .unwrap()
        .value;
    assert!((current + 0.0005).abs() < 1e-12);
    s.alter(
        &loaded.circuit_id,
        &[Change {
            target: "n1".into(),
            param: "r".into(),
            value: 1000.,
        }],
    )
    .unwrap();
    let second = s.simulate(&loaded.circuit_id, None).unwrap();
    assert_eq!(second.status, Status::Ok);
    let current = second.analyses[0]
        .values
        .iter()
        .find(|v| v.name == "i(v1)")
        .unwrap()
        .value;
    assert!((current + 0.001).abs() < 1e-12);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn save_projects_results_and_preserves_measurement_inputs() {
    let mut s = session(1);
    let text = RC.replace(".end", ".save i(V1)\n.save v(IN) i(v1)\n.end");
    let c = s.load(&text).unwrap();
    let job = s.simulate(&c.circuit_id, None).unwrap();
    assert!(tr(&job) > 0.);
    let a = &s.results(&job.job_id).unwrap().unwrap().analyses[0];
    assert_eq!(
        a.variables
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        ["v(in)", "i(v1)"]
    );
    assert!(
        s.waveform(&job.job_id, "v(out)", None, Reduce::Real, None, None, 20)
            .is_err()
    );
    let m = s
        .measure(&job.job_id, &["tran vmax max v(out)".into()])
        .unwrap();
    assert!((m[0].value.unwrap() - 1.).abs() < 1e-3);
    let c = s.load(&text.replace(".end", ".save all\n.end")).unwrap();
    let job = s.simulate(&c.circuit_id, None).unwrap();
    assert!(
        s.results(&job.job_id).unwrap().unwrap().analyses[0]
            .real("v(out)")
            .is_some()
    );
}

#[test]
fn subcircuit_save_resolves_local_current_and_port_voltage() {
    let mut s = session(1);
    let c=s.load("nested save\n.subckt load p\nVprobe p mid 0\nR1 mid 0 1k\n.save i(Vprobe) v(p)\n.ends\nX1 a load\nV1 a 0 1\n.op\n").unwrap();
    let job = s.simulate(&c.circuit_id, None).unwrap();
    let a = &s.results(&job.job_id).unwrap().unwrap().analyses[0];
    assert_eq!(a.variables.len(), 2);
    assert_eq!(a.real("V(a)").unwrap(), [1.]);
    assert!((a.real("I(v.x1.vprobe)").unwrap()[0] - 0.001).abs() < 1e-14);
}

#[test]
fn dialect_lint_and_failure_explanations() {
    let s = session(1);
    for (card, code) in [
        ("K1 L1 L2 1.5", "E-COUPLING-K"),
        ("K1 L1 V1 0.5", "E-COUPLING-L"),
        ("B1 b 0 V={v(missing)}", "E-BSOURCE-EXPR"),
        ("B1 b 0 V={i(no_branch)}", "E-BSOURCE-EXPR"),
        ("S1 a 0 b 0 absent", "E-SWITCH-MODEL"),
        ("W1 a 0 v99 m\n.model m csw", "E-SWITCH-MODEL"),
    ] {
        let report = s.validate(&format!("bad deck\nV1 a 0 1\nL1 b 0 1m\n{card}\n.op\n"));
        assert!(
            report.diagnostics.iter().any(|d| d.code == code),
            "{report:?}"
        );
        assert!(!s.explain(code).unwrap().codes[0].remedies.is_empty());
    }
}

#[test]
fn noise_override_reports_ranked_contributors_and_reproducible_hash() {
    let mut hashes = vec![];
    for threads in [1, 4] {
        let mut s = session(threads);
        let c = s
            .load("divider\nV1 in 0 1\nR1 in out 1k\nR2 out 0 4k\n.op\n")
            .unwrap();
        let job = s
            .simulate(
                &c.circuit_id,
                Some(&["noise v(out) v1 dec 8 1 10k".into(), "tf v(out) v1".into()]),
            )
            .unwrap();
        assert_eq!(job.status, Status::Ok);
        hashes.push(job.result_hash.clone());
        let top = &job.analyses[1].noise_contributors;
        assert_eq!((&*top[0].name, &*top[1].name), ("r1", "r2"));
        assert!(top[0].value > top[1].value);
        let w = s
            .waveform(
                &job.job_id,
                "onoise_total",
                Some(1),
                Reduce::Auto,
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!((w.y.len(), w.unit), (1, "V"));
    }
    assert_eq!(hashes[0], hashes[1]);
}

#[test]
fn digital_waveform_query_preserves_strength() {
    let mut s = session(2);
    let c = s
        .load("bus\nAone bus pull\n.model pull d_pullup\n.op")
        .unwrap();
    let j = s.simulate(&c.circuit_id, None).unwrap();
    assert_eq!(j.analyses[0].digital_nodes, ["bus"]);
    let wave = s
        .waveform(&j.job_id, "bus", None, Reduce::Auto, None, None, 100)
        .unwrap();
    assert_eq!(wave.unit, "digital");
    assert_eq!(wave.symbols, ["1r"]);
    assert_eq!(wave.x, [0.]);
    assert_eq!(wave.y, [4.]);
}
