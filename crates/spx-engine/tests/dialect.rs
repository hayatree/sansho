//! Netlist dialect features end to end: parameters, subcircuits, libraries, binning,
//! `.ic`, `.nodeset` and UIC.
//!
//! Provenance: original test circuits; expected values follow from circuit theory.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use spx_engine::run;
use spx_netlist::{SourceLoader, parse, parse_with};
use spx_output::SimResults;

fn simulate(src: &str) -> SimResults {
    let nl = parse(src).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    run(&nl)
        .unwrap_or_else(|e| panic!("{:#?}", e.diagnostics))
        .results
}

fn op(r: &SimResults, name: &str) -> f64 {
    r.analyses[0].real(name).unwrap()[0]
}

#[test]
fn parameterized_subcircuit_divider() {
    let r = simulate(
        "div\n.param rtop=3k gain='1/4'\n\
         .subckt div in out params: r1=1k r2={r1*gain/(1-gain)}\nR1 in out {r1}\nR2 out 0 {r2}\n.ends\n\
         V1 a 0 4\nX1 a b div r1=rtop\nX2 a c div\n.op\n",
    );
    assert!((op(&r, "v(b)") - 1.0).abs() < 1e-12);
    assert!((op(&r, "v(c)") - 1.0).abs() < 1e-12);
}

#[test]
fn internal_subcircuit_nodes_and_branches_are_named_hierarchically() {
    let r = simulate(
        "h\n.subckt cell a b\nR1 a m 1k\nR2 m b 1k\nVs m 0 0.5\n.ends\nV1 x 0 1\nX1 x 0 cell\n.op\n",
    );
    assert!((op(&r, "v(x1.m)") - 0.5).abs() < 1e-12);
    assert!((op(&r, "i(v.x1.vs)") - 0.0).abs() < 1e-6);
}

#[test]
fn binned_models_are_selected_by_length() {
    let src = "bins\nVD d 0 1\nVG g 0 1\nMS d g 0 0 n l=0.5u w=1u\nML d g 0 0 n l=2u w=1u\n\
               .model n.1 nmos lmin=0.1u lmax=1u vto=0.2 kp=100u\n\
               .model n.2 nmos lmin=1u lmax=10u vto=0.6 kp=100u\n.op\n";
    let nl = parse(src).unwrap();
    let models: Vec<&str> = nl.elements[2..]
        .iter()
        .map(|e| e.kind.model().unwrap())
        .collect();
    assert_eq!(models, ["n.1", "n.2"]);
    let bad = parse(&src.replace("l=2u", "l=20u")).unwrap_err();
    assert!(bad.iter().any(|d| d.code == "E-PARSE-MODEL"));
}

#[test]
fn nodeset_selects_the_latch_state() {
    let latch = "latch\nVDD vdd 0 1.8\nMP1 q qb vdd vdd p l=1u w=2u\nMN1 q qb 0 0 n l=1u w=1u\n\
                 MP2 qb q vdd vdd p l=1u w=2u\nMN2 qb q 0 0 n l=1u w=1u\n\
                 .model n nmos vto=0.5 kp=100u\n.model p pmos vto=-0.5 kp=40u\n.op\n";
    let hi = simulate(&format!("{latch}.nodeset v(q)=1.8 v(qb)=0\n"));
    assert!(op(&hi, "v(q)") > 1.79 && op(&hi, "v(qb)") < 0.01);
    let lo = simulate(&format!("{latch}.nodeset v(q)=0 v(qb)=1.8\n"));
    assert!(op(&lo, "v(q)") < 0.01 && op(&lo, "v(qb)") > 1.79);
}

#[test]
fn initial_conditions_with_and_without_uic() {
    // RC discharge from 1 V: with UIC the capacitor starts at its .ic value.
    let r = simulate("rc\nR1 a 0 1k\nC1 a 0 1n\n.ic v(a)=1\n.tran 10n 3u uic\n");
    let a = &r.analyses[0];
    let (t, v) = (&a.scale_values, a.real("v(a)").unwrap());
    let k = t.iter().position(|&x| x >= 1e-6).unwrap();
    assert!(
        (v[k] - (-t[k] / 1e-6).exp()).abs() < 5e-3,
        "{} at {}",
        v[k],
        t[k]
    );
    // Without UIC, .ic holds the node during the operating point.
    let r = simulate("rc\nV1 s 0 0\nR1 s a 1k\nC1 a 0 1n\n.ic v(a)=1\n.tran 10n 3u\n");
    assert!((r.analyses[0].real("v(a)").unwrap()[0] - 1.0).abs() < 1e-6);
}

struct Mem;

impl SourceLoader for Mem {
    fn load(&self, path: &str, _from: Option<&str>) -> Result<(String, String), String> {
        match path {
            "lib.spi" => Ok((
                path.into(),
                ".lib slow\n.param r=2k\n.endl\n.lib fast\n.param r=500\n.endl\n".into(),
            )),
            "cell.inc" => Ok((path.into(), "R2 out 0 {r}\n".into())),
            _ => Err(format!("no such file `{path}`")),
        }
    }
}

#[test]
fn libraries_and_includes_through_a_loader() {
    let nl = parse_with(
        "t\n.lib lib.spi fast\nV1 in 0 1\nR1 in out 500\n.include cell.inc\n.op\n",
        &Mem,
    )
    .unwrap();
    assert_eq!(nl.sources, ["", "lib.spi", "cell.inc"]);
    let r = run(&nl).unwrap().results;
    assert!((op(&r, "v(out)") - 0.5).abs() < 1e-12);
    let e = parse_with("t\n.lib lib.spi typ\n", &Mem).unwrap_err();
    assert_eq!(e[0].code, "E-PARSE-INCLUDE");
}
