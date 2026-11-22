//! End-to-end SOI film, thermal port and parallel stamping regressions.
//! Provenance: original Sansho circuits and BSIMSOIv4.4 Users' Manual §§2.3 and 5.2.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use std::fmt::Write;

#[test]
fn floating_film_completes_turnoff_and_cools() {
    let text = include_str!("../../../verify/probes/r17_soi_forward_film.cir");
    let deck = sansho_netlist::parse(text).unwrap();
    let run = sansho_engine::run(&deck).unwrap();
    assert!(run.warnings.is_empty(), "{:?}", run.warnings);
    let tran = &run.results.analyses[0];
    let film = tran.real("@m1[vbs]").unwrap();
    let rise = tran.real("v(tj)").unwrap();
    let peak = rise.iter().copied().fold(0., f64::max);
    assert!(film.iter().copied().fold(0., f64::max) > 0.35);
    assert!(peak > 0.2 && rise.last().unwrap() < &(peak * 0.1));
    assert!((tran.scale_values.last().unwrap() - 35e-6).abs() < 1e-15);
}

#[test]
fn parallel_soi_chunks_replay_the_same_drain_current() {
    let mut text = String::from("SOI array\nVd d 0 .7\nVg g 0 .8\n");
    for k in 0..257 {
        writeln!(text, "M{k} d g 0 0 0 dev l=.48u w={}u", 1 + k % 4).unwrap();
    }
    text.push_str(".model dev nmos(level=58 vth0=.48)\n.dc Vg .3 1 .1\n.save i(Vd)\n");
    let deck = sansho_netlist::parse(&text).unwrap();
    let currents = |threads| {
        let result = run_with(&deck, &RunConfig { threads }).unwrap();
        result.results.analyses[0]
            .real("i(vd)")
            .unwrap()
            .iter()
            .map(|x| x.to_bits())
            .collect::<Vec<_>>()
    };
    let serial = currents(1);
    assert_eq!(serial.len(), 8);
    assert_ne!(serial[0], serial[7]);
    assert_eq!(serial, currents(2));
}

#[test]
fn unused_instance_and_model_parameters_identify_their_owner() {
    let text = "SOI card diagnostics\nVd d 0 1\nM1 d d 0 0 dev spare=3\n.model dev nmos(level=10 ltox=1)\n.op\n";
    let deck = sansho_netlist::parse(text).unwrap();
    let errors = sansho_engine::check(&deck).unwrap_err();
    assert!(errors.iter().any(|d| d.code == "E-ELAB-PARAM"
        && d.message.contains("m1")
        && d.message.contains("spare")));
    let valid = sansho_netlist::parse(&text.replace(" spare=3", "")).unwrap();
    let check = sansho_engine::check(&valid).unwrap();
    assert!(check.warnings.iter().any(|d| d.code == "W-MODEL-IGNORED"
        && d.message.contains("dev")
        && d.message.contains("ltox")));
}
