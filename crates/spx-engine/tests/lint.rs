//! Pre-run lint checks.
//!
//! Provenance: original test circuits.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use spx_engine::check;
use spx_netlist::parse;

fn codes(src: &str) -> Vec<&'static str> {
    let nl = parse(src).unwrap();
    check(&nl)
        .unwrap()
        .warnings
        .iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn flags_likely_mistakes() {
    let c = codes(
        "lint\nV1 in 0 1\nR1 in out 10M\nC1 out 0 10\nM1 out in 0 0 nm\n.model nm nmos\n\
         .model unused d\n.meas tran tr trig v(in) val=0.5 rise=1 targ v(otu) val=0.5 rise=1\n\
         .tran 1 1n\n",
    );
    for want in [
        "W-LINT-UNIT-MILLI",
        "W-LINT-UNIT-RANGE",
        "W-LINT-UNUSED-MODEL",
        "W-LINT-DEFAULT-GEOM",
        "W-LINT-MEAS-SIGNAL",
        "W-LINT-TRAN-STEP",
    ] {
        assert!(c.contains(&want), "missing {want} in {c:?}");
    }
}

#[test]
fn clean_netlist_has_no_lint() {
    let c = codes(
        "clean\nV1 in 0 1\nR1 in out 10meg\nR2 out 0 1m\nC1 out 0 10p\n\
         M1 out in 0 0 nm l=1u w=2u\n.model nm nmos\n\
         .meas tran vmax max v(out)\n.tran 1n 1u\n",
    );
    assert!(c.is_empty(), "{c:?}");
}

#[test]
fn unused_models_are_reported_once_per_main_file_definition() {
    struct Library;
    impl spx_netlist::SourceLoader for Library {
        fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok((path.into(), ".model lib_unused d\n".into()))
        }
    }
    let nl = spx_netlist::parse_with("lint\n.include models.lib\n.subckt cell n\n.model unused d\nR1 n 0 1k\n.ends\nX1 a cell\nX2 a cell\nV1 a 0 1\n.op\n", &Library).unwrap();
    let warnings = check(&nl).unwrap().warnings;
    let unused: Vec<_> = warnings
        .iter()
        .filter(|d| d.code == "W-LINT-UNUSED-MODEL")
        .collect();
    assert_eq!(unused.len(), 1);
    assert_eq!(unused[0].span.unwrap().file, 0);
}
