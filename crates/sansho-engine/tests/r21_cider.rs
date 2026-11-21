//! Planar numerical devices through the public mixed-level circuit solver.
//! Provenance: rectangular box integration, the Poisson–Boltzmann MOS capacitor,
//! the long-channel gradual-channel approximation and ngspice 44 manual chapter 26.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use sansho_netlist::{parse, write};
use sansho_output::AnalysisResult;

const DIODE: &str = include_str!("../../../verify/decks/r21_diode_dc.cir");
const MOS: &str = include_str!("../../../verify/decks/r21_numos_gate_ac.cir");
const CAP: &str = include_str!("../../../verify/probes/r21_workfunction.cir");
const EPS0: f64 = 8.8541878128e-14;
const VT: f64 = 300.15 * 8.617087e-5;
const COX: f64 = 3.9 * EPS0 / 2e-6;

fn run(deck: &str, threads: usize) -> AnalysisResult {
    let nl = parse(deck).unwrap();
    let result = run_with(&nl, &RunConfig { threads }).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    result.results.analyses.into_iter().next().unwrap()
}

#[test]
fn uniform_extrusion_preserves_diode_transport_and_admittance() {
    let line = DIODE
        .lines()
        .filter(|l| !l.starts_with("+ y.mesh") && !l.starts_with("+ electrode"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("level=2", "level=1")
        .replace("defw=10u", "defa=1e-11");
    let analysis = ".dc Vbias -0.2 0.55 0.05";
    for frequency in [false, true] {
        let (planar, line) = if frequency {
            (
                DIODE.replace(analysis, ".ac dec 2 1k 10meg"),
                line.replace(analysis, ".ac dec 2 1k 10meg"),
            )
        } else {
            (DIODE.to_owned(), line.clone())
        };
        let a = run(&planar, 1);
        let b = run(&line, 1);
        if frequency {
            for (&(x, y), &(u, v)) in a
                .complex("i(vbias)")
                .unwrap()
                .iter()
                .zip(b.complex("i(vbias)").unwrap())
            {
                assert!((x - u).hypot(y - v) < 1e-17 + 1e-5 * u.hypot(v));
            }
        } else {
            for (&i, &j) in a
                .real("i(vbias)")
                .unwrap()
                .iter()
                .zip(b.real("i(vbias)").unwrap())
            {
                assert!((i - j).abs() < 1e-17 + 1e-5 * j.abs(), "{i} {j}");
            }
        }
    }
}

#[test]
fn mos_capacitor_accumulation_flatband_and_depletion() {
    let acceptors = 1e16;
    let ni = 1.1376e10;
    let eps = 11.7 * EPS0;
    let flatband =
        4.8 - 4.05 - VT * (3.22603e19_f64 / ni).ln() - VT * (acceptors / (2. * ni)).asinh();
    // Integrate Poisson once in a semi-infinite p-type slab. At these surface
    // potentials the minority charge is negligible. Cg is Cs in series with Cox.
    for u in [-3.0_f64, 0., 3., 8.] {
        let q = eps * sansho_devices::CHARGE * acceptors;
        let sheet = (2. * q * VT * ((-u).exp_m1() + u)).sqrt().copysign(u);
        let cs = if u == 0. {
            (q / VT).sqrt()
        } else {
            -q * (-u).exp_m1() / sheet
        };
        let vg = flatband + VT * u + sheet / COX;
        let expected = 2e-7 / (1. / cs + 1. / COX);
        let deck = CAP
            .replace("{probe}", "4.8")
            .replace("0.05 ac", &format!("{vg} ac"));
        let c = -run(&deck, 1).complex("i(vgate)").unwrap()[0].1 / (std::f64::consts::TAU * 1e5);
        assert!(
            (c / expected - 1.).abs() < 0.012,
            "u={u}, C={c}, expected {expected}"
        );
    }
}

#[test]
fn strong_inversion_square_law() {
    let s = include_str!("../../../verify/decks/r21_numos_transfer.cir")
        .replace(" w=1 l=2u", "")
        .replace("Vd d 0 0.05", "Vd d 0 1.8")
        .replace("0.0 1.2 0.1", "1.2 1.8 0.2");
    let result = run(&s, 1);
    let currents = result.real("i(vd)").unwrap();
    // At saturation d(sqrt(Id))/dVg = sqrt(mu Cox W / (2 L)).
    // Contacted donor vertices end at x=0.2 and resume at x=2.6 microns.
    let slope = (1340. * COX * 5. / (2. * 2.4)).sqrt();
    for pair in currents.windows(2) {
        let observed = ((-pair[1]).sqrt() - (-pair[0]).sqrt()) / 0.2;
        assert!((observed / slope - 1.).abs() < 0.08, "{observed} {slope}");
    }
}

#[test]
fn exterior_gate_ends_match_the_active_oxide_footprint() {
    let clipped = MOS.replace("mat=2 y.h=0", "mat=2 y.h=0 ix.l=2 ix.h=14");
    let rotate = |s: &str| {
        s.replace("x.", "SWAP.")
            .replace("y.", "x.")
            .replace("SWAP.", "y.")
    };
    for (a, b) in [
        (MOS.to_owned(), clipped.clone()),
        (rotate(MOS), rotate(&clipped)),
    ] {
        let original = run(&a, 1);
        let explicit = run(&b, 1);
        for port in ["i(vg)", "i(vd)"] {
            assert_eq!(original.complex(port), explicit.complex(port));
        }
    }
}

#[test]
fn gate_drive_matches_all_four_port_reactions_and_kcl() {
    let deck=MOS.replace("i(vg) i(vd)","i(vg) i(vd) @mchannel[id] @mchannel[ig] @mchannel[is] @mchannel[ib] @mchannel[y12] @mchannel[y22] @mchannel[y32] @mchannel[y42]");
    let deck = deck.replace(
        "+ end",
        "+ interface dom=1 neigh=2 layer.width=0.04\n+ models surfmob transmob fieldmob\n+ end",
    );
    let result = run(&write(&parse(&deck).unwrap()), 1);
    let threaded = run(&deck, 2);
    for (k, _) in result.scale_values.iter().enumerate() {
        let mut sum = (0., 0.);
        for (port, name) in ["id", "ig", "is", "ib"].iter().enumerate() {
            let key = format!("@mchannel[{name}]");
            let (a, b) = result.complex(&key).unwrap()[k];
            assert_eq!(
                result.complex(&key).unwrap(),
                threaded.complex(&key).unwrap()
            );
            let (g, h) = result
                .complex(&format!("@mchannel[y{}2]", port + 1))
                .unwrap()[k];
            assert!(
                (a - g).hypot(b - h) < 1e-14 + 1e-7 * a.hypot(b),
                "port {port} k {k} circuit {a} {b} reference {g} {h}"
            );
            if port < 2 {
                let (c, d) = result.complex(["i(vd)", "i(vg)"][port]).unwrap()[k];
                assert!(
                    (a + c).hypot(b + d) < 1e-14 + 1e-7 * a.hypot(b),
                    "port {port} k {k} circuit {a} {b} reference {g} {h}"
                );
            }
            sum.0 += a;
            sum.1 += b;
        }
        assert!(sum.0.hypot(sum.1) < 2e-12, "{:?}", sum);
    }
}

#[test]
fn invalid_planar_geometry_is_diagnosed_before_solve() {
    for (from, to) in [
        ("loc=1 n=3", "loc=1 n=33"),
        ("ix.l=32", "ix.l=1"),
        ("num=2 ix.l=32", "num=1 ix.l=32"),
        ("num=2 ix.l=32", "num=3 ix.l=32"),
        ("+ y.mesh loc=0 n=1\n+ y.mesh loc=1 n=3", ""),
    ] {
        let deck = DIODE.replace(from, to);
        let failure = run_with(&parse(&deck).unwrap(), &RunConfig { threads: 1 }).unwrap_err();
        assert!(
            failure
                .diagnostics
                .iter()
                .any(|d| d.code == "E-CIDER-MODEL"),
            "{:?}",
            failure
        );
    }
}

#[test]
fn split_gate_and_sheet_charge_shift_flatband() {
    let base = CAP.replace("{probe}", "4.8");
    // The measured physical sheet on a Si-first face is twice q times the input.
    let sheet = 1e11;
    let charged = base.replace(
        "+ end",
        &format!("+ interface dom=1 neigh=2 qf={sheet}\n+ end"),
    );
    let shifted = base
        .replace(
            "workf=4.8",
            &format!("workf={}", 4.8 - 2. * sansho_devices::CHARGE * sheet / COX),
        )
        .replace(
            "num=1 iy.h=1",
            "num=1 iy.h=1 ix.h=2\n+ electrode num=1 iy.h=1 ix.l=2",
        );
    let capacitance = |deck| run(deck, 1).complex("i(vgate)").unwrap()[0].1;
    let reversed = charged.replace("dom=1 neigh=2", "dom=2 neigh=1");
    assert_eq!(capacitance(&reversed), capacitance(&base));
    assert!((capacitance(&charged) / capacitance(&shifted) - 1.).abs() < 1e-7);
}

#[test]
fn pure_dielectric_scales_with_mask_and_instance_width() {
    let deck = "oxide sandwich\nVa a 0 ac 1\nDoxide a 0 m area=2 w=3\n.model m numd level=2
+ options capacitor defw=5u
+ x.mesh width=2 number=3
+ y.mesh width=0.1 number=2
+ region num=1 mat=1
+ material num=1 oxide
+ electrode iy.h=1
+ electrode iy.l=3
.ac lin 1 1meg 1meg
.save i(va) @doxide[c11]";
    let ac = run(deck, 1);
    let c = ac.complex("@doxide[c11]").unwrap()[0].0;
    let expected = 3.9 * EPS0 * 60e-8 / 1e-5;
    assert!((c / expected - 1.).abs() < 1e-9);
}

#[test]
fn inverter_gate_displacement_and_drain_current() {
    let deck = include_str!("../../../verify/decks/r21_numos_inverter_tran.cir")
        .replace("Cout d 0 30f", "Cout d 0 30f\nVsense d drain 0")
        .replace("Mchannel d g", "Mchannel drain g")
        .replace(
            ".save v(g) v(d)",
            ".save v(d) i(vsense) i(vg) @mchannel[id] @mchannel[ig]",
        );
    let result = run(&deck, 1);
    for (port, branch, sign) in [("id", "i(vsense)", 1.), ("ig", "i(vg)", -1.)] {
        let internal = result.real(&format!("@mchannel[{port}]")).unwrap();
        for (&i, &j) in internal.iter().zip(result.real(branch).unwrap()) {
            assert!(
                (i - sign * j).abs() < 1e-10 + 1e-4 * i.abs(),
                "{port}: {i} {j}"
            );
        }
    }
    let v = result.real("v(d)").unwrap();
    assert!(v[0] > 0.28 && v.iter().any(|&x| x < 0.05) && v.last().unwrap() > &0.24);
}

#[test]
fn full_depth_surface_strip_has_two_bulk_contact_columns_in_series() {
    let deck =
        include_str!("../../../verify/probes/r21b_surface_depth.cir").replace("{probe}", "0.002");
    let ac = run(&deck, 1);
    let conductance = |name| -ac.complex(name).unwrap()[0].0;
    let bulk = conductance("i(vbulk)");
    assert_eq!(conductance("i(vautomatic)"), bulk);
    assert_eq!(conductance("i(vox)"), bulk);
    // Four equal columns. The two middle columns have MUS = 300.
    let ratio = 2. / (1. + 1340. / 300.);
    assert!((conductance("i(vsi)") / bulk - ratio).abs() < 1e-8);
}

#[test]
fn surface_field_jacobian_matches_bias_perturbation() {
    let deck = include_str!("../../../verify/probes/r21_transverse_field.cir")
        .replace("{probe}", "3e5")
        .replace(".dc Vg 0.4 1.2 0.1", ".dc Vg 0.8999 0.9001 0.0001")
        .replace(".save i(vd)", ".save i(vd) @mchannel[g12]");
    let result = run(&deck, 1);
    let current = result.real("i(vd)").unwrap();
    let slope = -(current[2] - current[0]) / 2e-4;
    let g = result.real("@mchannel[g12]").unwrap()[1];
    assert!((g / slope - 1.).abs() < 1e-3, "{g} {slope}");
}

#[test]
fn transposing_profile_and_mesh_preserves_slab_admittance() {
    let deck =
        include_str!("../../../verify/probes/r21_lateral_doping.cir").replace("{probe}", "0.5");
    let rotated = deck
        .replace("x.", "SWAP.")
        .replace("y.", "x.")
        .replace("SWAP.", "y.")
        .replace("gauss lat.erfc", "gauss x.axis lat.erfc");
    let input = |s| run(s, 1).complex("i(vbias)").unwrap()[0];
    let (a, b) = input(&deck);
    let (c, d) = input(&rotated);
    assert!((a / c - 1.).abs() < 1e-7 && (b / d - 1.).abs() < 1e-7);
}
