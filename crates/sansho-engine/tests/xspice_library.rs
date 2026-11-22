//! R13 measurements through normal parsing, elaboration and analysis.
//! Provenance: original circuits based on ngspice 44 user manual §§8.2–8.5 and elementary circuit laws.
#![allow(clippy::unwrap_used)]
use sansho_engine::{RunConfig, run_with};
use sansho_netlist::{Netlist, SourceLoader, parse, parse_file, parse_with};
use sansho_output::AnalysisResult;
use std::path::Path;
fn run(n: &Netlist) -> AnalysisResult {
    run_with(n, &RunConfig { threads: 1 })
        .unwrap()
        .results
        .analyses
        .remove(0)
}
fn deck(name: &str) -> AnalysisResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../verify/r13-decks/r13_{name}.cir"));
    let n = parse_file(&path).unwrap();
    let a = run_with(&n, &RunConfig { threads: 1 }).unwrap();
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
    let b = run_with(&n, &RunConfig { threads: 2 }).unwrap();
    let (mut raw_a, mut raw_b) = (Vec::new(), Vec::new());
    sansho_output::write_raw_binary(&a.results, &mut raw_a).unwrap();
    sansho_output::write_raw_binary(&b.results, &mut raw_b).unwrap();
    assert!(raw_a == raw_b, "{name} produced different binary samples");
    assert_eq!(a.results, b.results);
    a.results.analyses.into_iter().next().unwrap()
}
fn near(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a:e} expected {b:e}");
}
fn sample(a: &AnalysisResult, key: &str, t: f64) -> f64 {
    let i = a
        .scale_values
        .partition_point(|v| *v < t)
        .min(a.scale_values.len() - 1);
    a.real(key).unwrap()[i]
}
#[test]
fn hysteresis_and_retrigger() {
    let a = deck("hyst");
    near(sample(&a, "v(out)", 1e-6), 0.1, 0.07);
    near(sample(&a, "v(out)", 3e-6), 0.9, 0.07);
    let a = deck("oneshot");
    for (ns, y) in [(0., 0.), (6., 1.), (17., 1.), (25., 0.)] {
        near(sample(&a, "v(out)", ns * 1e-9), y, 1e-8);
    }
    for (&t, &out) in a.scale_values.iter().zip(a.real("v(out)").unwrap()) {
        let ns = t * 1e9;
        let rise = ((ns - 2.55) / 2.).clamp(0., 1.);
        let fall = ((23.55 - ns) / 2.).clamp(0., 1.);
        near(out, rise.min(fall), 1e-10);
    }
}
#[test]
fn limited_supply_currents_and_zener() {
    let a = deck("ilimit");
    for (i, &v) in a.real("v(out)").unwrap().iter().enumerate() {
        let drive = a.scale_values[i];
        let resistance = if drive > 0. { 105. } else { 110. };
        near(v, 100. * (drive / resistance).clamp(-0.012, 0.008), 1e-6);
        near(
            v / 100. + a.real("i(vpos)").unwrap()[i] + a.real("i(vneg)").unwrap()[i],
            0.,
            1e-10,
        );
    }
    let a = deck("zener");
    for (&supply, &reverse) in a.scale_values.iter().zip(a.real("v(ref)").unwrap()) {
        let avalanche = 0.01 * ((reverse - 4.7) / (0.01 * 3.)).exp();
        let leakage = reverse * 0.2e-6 / 4.7;
        let predicted = avalanche + leakage;
        near(
            (supply - reverse) / 220.,
            predicted,
            1e-12 + 1e-3 * predicted,
        );
    }
}
#[test]
fn extremum_and_transformer() {
    let a = deck("multi_pwl");
    for (i, &x) in a.scale_values.iter().enumerate() {
        let f = |v: f64| v * if v < 0. { 0.5 } else { 2. };
        near(a.real("v(lo)").unwrap()[i], f(x.min(0.25)), 1e-10);
        near(a.real("v(hi)").unwrap()[i], f(x.max(0.25)), 1e-10);
    }
    let a = deck("transformer");
    let primary = a.real("v(winding)").unwrap();
    assert!(primary.iter().any(|v| *v > 0.15));
    for (p, s) in primary.iter().zip(a.real("v(out)").unwrap()) {
        near(*s, -0.6 * p, 1e-6);
    }
}
#[test]
fn machine_ram_lut_and_sampled_real() {
    let a = deck("state");
    for (ns, qa, qb) in [(2., 0., 0.), (7., 1., 0.), (16., 0., 1.), (24., 0., 0.)] {
        near(sample(&a, "v(a)", ns * 1e-9), qa, 1e-8);
        near(sample(&a, "v(b)", ns * 1e-9), qb, 1e-8);
    }
    let a = deck("ram");
    for (ns, v) in [
        (0., 0.),
        (4., 1.),
        (12., 0.),
        (8., 1.),
        (16., 0.),
        (20., 1.),
        (25., 0.45),
    ] {
        near(sample(&a, "v(out)", ns * 1e-9), v, 1e-8);
    }
    let a = deck("genlut");
    for (name, ns) in [("c", 20.6), ("d", 20.7)] {
        let &(time, ref symbol) = a.events[name].last().unwrap();
        assert_eq!(symbol, "Uu");
        near(time, ns * 1e-9, 1e-18);
    }
    for (ns, b, c) in [
        (0., 0., 0.),
        (7., 1., 1.),
        (12., 1., 0.),
        (17., 0., 1.),
        (23., 0.45, 0.45),
    ] {
        near(sample(&a, "v(first)", ns * 1e-9), b, 1e-8);
        near(sample(&a, "v(second)", ns * 1e-9), c, 1e-8);
    }
    let a = deck("real");
    for (ns, v) in [(0., 0.), (14., 0.5), (28., 2.5)] {
        near(sample(&a, "v(out)", ns * 1e-9), v, 1e-8);
    }
}
// Reuse the maintainer's original stimulus. The engine accepts the same
// transient analysis as a deck card in place of the interactive control block.
fn probe(source: &str, setting: f64) -> AnalysisResult {
    let (cards, control) = source.split_once(".control\n").unwrap();
    let tran = control.lines().next().unwrap();
    let parameter = cards
        .lines()
        .find(|l| l.starts_with(".param probe="))
        .unwrap();
    let cards = cards.replace(parameter, &format!(".param probe={setting:e}"));
    run(&parse(&format!("{cards}.{tran}\n.end")).unwrap())
}

fn event_sequence(a: &AnalysisResult, name: &str, expected: &[(f64, &str)]) {
    let trace = &a.events[name];
    assert_eq!(trace.len(), expected.len(), "{name}");
    for ((t, symbol), (ns, value)) in trace.iter().zip(expected) {
        assert_eq!(symbol, value, "{name}");
        near(*t, ns * 1e-9, 1e-18);
    }
}

#[test]
fn genlut_unknown_pin_order_and_release() {
    let source = include_str!("../../../verify/probes/r13_genlut_unknown.cir");
    for d in [0.1_f64, 0.5, 0.9] {
        let a = probe(source, d * 1e-9);
        for (pin, unknown, known) in [("p", d, d), ("q", 0.4, 0.4), ("r", d, d.max(0.4))] {
            event_sequence(
                &a,
                pin,
                &[
                    (0., "0s"),
                    (4.075 + unknown, "Uu"),
                    (8.075 + known, "1s"),
                    (11.275 + unknown, "Uu"),
                    (15.275 + known, "0s"),
                ],
            );
        }
        event_sequence(
            &a,
            "z",
            &[(0., "1s"), (7.275 + d, "Uz"), (12.075 + d, "1s")],
        );
        event_sequence(
            &a,
            "s",
            &[
                (0., "1s"),
                (3.275 + d, "Uu"),
                (7.475 + d, "0z"),
                (11.675 + d, "Uu"),
                (16.075 + d, "1s"),
            ],
        );
    }
    // Switching the other pin between two Z entries preserves the released level.
    let alternate = source.replace(
        "Aref b down",
        "Vaux aux 0 PWL(0 0 9n 0 9.1n 1)\nAaux [aux] [b] binary",
    );
    assert_eq!(
        probe(&alternate, 0.1e-9).events["s"],
        probe(source, 0.1e-9).events["s"]
    );
}

#[test]
fn genlut_release_carries_only_the_output_delay() {
    let source = include_str!("../../../verify/probes/r13d_genlut_release.cir");
    for fall in [0.1, 0.2, 0.5] {
        let a = probe(source, fall * 1e-9);
        for (name, rise) in [("base", 1.), ("rise", 2.), ("unusedpin", 1.)] {
            event_sequence(
                &a,
                name,
                &[
                    (0., "1s"),
                    (3.175 + fall, "Uu"),
                    (7.175 + 2. * fall, "0z"),
                    (11.175 + 3. * fall, "Uu"),
                    (15.175 + rise, "1s"),
                ],
            );
        }
        event_sequence(
            &a,
            "low",
            &[
                (0., "0s"),
                (4.175, "Uu"),
                (8.175 + fall, "0z"),
                (12.175 + 2. * fall, "Uu"),
                (15.175 + fall, "0s"),
            ],
        );
        event_sequence(
            &a,
            "direct",
            &[(0., "1s"), (7.175 + fall, "Uz"), (12.175, "1s")],
        );
    }
}

#[test]
fn ram_write_releases_the_stored_level() {
    let source = include_str!("../../../verify/probes/r13_ram_write.cir");
    for d in [0.25, 1., 2.] {
        let a = probe(source, d * 1e-9);
        event_sequence(
            &a,
            "q",
            &[
                (0., "0s"),
                (6.075 + d, "1z"),
                (11.075 + d, "0z"),
                (18.075 + d, "1z"),
                (22.075 + d, "1s"),
                (26.075 + d, "Uz"),
                (30.075 + d, "1s"),
            ],
        );
        event_sequence(
            &a,
            "bus",
            &[
                (0., "0s"),
                (6.075 + d, "0r"),
                (22.075 + d, "1s"),
                (26.075 + d, "0r"),
                (30.075 + d, "1s"),
            ],
        );
    }
}

fn check_real_steps(a: &AnalysisResult, events: &[(f64, f64)], tau: f64, gain: f64) {
    let out = a.real("v(out)").unwrap();
    let mut state = 0.;
    for (i, times) in a.scale_values.windows(2).enumerate() {
        let &(_, target) = events
            .iter()
            .rev()
            .find(|(at, _)| *at <= times[0] + 1e-18)
            .unwrap();
        let increment = (target - state) * (times[1] - times[0]) / tau;
        near(out[i + 1], state + gain * increment, 1e-11);
        state += increment;
    }
}

#[test]
fn real_gain_holds_ic_until_twice_its_own_delay() {
    let source = include_str!("../../../verify/probes/r13d_real_startup.cir");
    for delay in [0.1, 0.4, 0.7] {
        let a = probe(source, delay * 1e-9);
        for (node, edge) in [("fast", 4.0625), ("slow", 4.3625)] {
            event_sequence(&a, node, &[(0., "2.5e-1"), (edge, "1.25e0")]);
        }
        for (node, ic, edge) in [
            ("a", "0e0", 4.0625),
            ("b", "0e0", 4.3625),
            ("c", "-1e0", 4.0625),
        ] {
            event_sequence(
                &a,
                node,
                &[(0., ic), (2. * delay, "5e-1"), (edge + delay, "2.5e0")],
            );
        }
        check_real_steps(
            &a,
            &[
                (0., 0.),
                (2. * delay * 1e-9, 0.5),
                ((4.3625 + delay) * 1e-9, 2.5),
            ],
            0.2e-9,
            1.,
        );
    }
}

#[test]
fn real_bridge_short_steps_and_direct_gain() {
    let source = include_str!("../../../verify/probes/r13d_real_short.cir");
    for ns in [0., 0.025, 0.05, 0.1] {
        let a = probe(source, ns * 1e-9);
        event_sequence(
            &a,
            "value",
            &[(0., "5e-1"), (2.375, "1.5e0"), (3.375, "5e-1")],
        );
        let unity = a.real("v(unity)").unwrap();
        let scaled = a.real("v(scaled)").unwrap();
        near(unity[0], 0.5, 1e-12);
        near(scaled[0], 1., 1e-12);
        let high = a.events["value"][1].0..a.events["value"][2].0;
        if ns == 0. {
            for (i, &time) in a.scale_values.iter().enumerate() {
                near(scaled[i], 2. * unity[i], 1e-12);
                // The printed falling-event coordinate also has a pre-edge sample.
                if time != high.end {
                    near(
                        unity[i],
                        if high.contains(&time) { 1.5 } else { 0.5 },
                        1e-12,
                    );
                }
            }
        } else {
            // Unity exposes the internal state. Compare the gain-two output
            // from the first source corner onward, after the startup hold.
            for (i, ts) in a.scale_values.windows(2).enumerate() {
                if ts[0] < 2e-9 {
                    continue;
                }
                let input = if high.contains(&ts[0]) { 1.5 } else { 0.5 };
                let ratio = (ts[1] - ts[0]) / (ns * 1e-9);
                let next = unity[i] + ratio.min(1.) * (input - unity[i]);
                near(unity[i + 1], next, 1e-12);
                let emitted = if ts[1] - ts[0] >= ns * 1e-9 {
                    2. * input
                } else {
                    2. * next - unity[i]
                };
                near(scaled[i + 1], emitted, 1e-12);
            }
        }
    }
}

#[test]
fn real_bridge_gain_and_variable_step_history() {
    let source = include_str!("../../../verify/probes/r13_real_gain.cir");
    for gain in [0., 0.5, 1., 2.] {
        let a = probe(source, gain);
        event_sequence(&a, "value", &[(0., "2.5e-1"), (9.825, "1.25e0")]);
        event_sequence(
            &a,
            "scaled",
            &[(0., "0e0"), (0.8, "5e-1"), (10.225, "2.5e0")],
        );
        event_sequence(
            &a,
            "held",
            &[(0., "0e0"), (4.575, "5e-1"), (14.575, "2.5e0")],
        );
        check_real_steps(
            &a,
            &[(0., 0.), (4.575e-9, 0.5), (14.575e-9, 2.5)],
            0.4e-9,
            gain,
        );
    }
    let source = include_str!("../../../verify/probes/r13_real_transition.cir");
    for tau in [0.2e-9, 0.4e-9, 0.8e-9] {
        check_real_steps(
            &probe(source, tau),
            &[(0., 0.), (2.375e-9, 1.), (6.375e-9, 0.)],
            tau,
            1.,
        );
    }
}

#[test]
fn zener_wide_knee_normalization_and_forward_tangent() {
    let a = run(&parse(
        include_str!("../../../verify/probes/r13_zener_resistance.cir")
            .replace(".param probe=3", ".param probe=100")
            .as_str(),
    )
    .unwrap());
    let current = a.real("i(vrev)").unwrap();
    for (bias, measured, tolerance) in [
        (-0.1, -9.1830577573e-6, 6e-12),
        (0., 0., 1e-13),
        (0.5, 59.565376036e-6, 1e-9),
        (4.7, 10.000168516e-3, 4e-8),
    ] {
        let index = a
            .scale_values
            .iter()
            .position(|v| (v - bias).abs() < 1e-10)
            .unwrap();
        near(-current[index], measured, tolerance);
    }
}

#[test]
fn nonlinear_passives_and_meters() {
    let a = run(&parse(
        "passives
V1 a 0 2
Rload out 0 200
A1 a 0 a out sw
A2 0 w a pot
Ac a cf cm
Al a lf lm
C1 a 0 20p
L1 a l 40u
Rl l 0 1k
Az a 0 diode
Am a 0 mem
Acore a 0 core2
.model sw pswitch(r_on=200 r_off=1k)
.model pot potentiometer(r=10k position=0.25)
.model cm cmeter(gain=1e12)
.model lm lmeter(gain=1e6)
.model diode sidiode(ron=2 roff=1k vfwd=1)
.model mem memristor(rinit=2000)
.model core2 core(mode=2)
.op",
    )
    .unwrap());
    for (k, v) in [
        ("v(out)", 1.),
        ("v(w)", 0.5),
        ("v(cf)", 20.),
        ("v(lf)", 40.),
        ("i(az)", 0.501),
        ("i(am)", 0.001),
        ("i(acore)", 1.),
    ] {
        near(a.real(k).unwrap()[0], v, 1e-7);
    }
}
#[test]
fn delay_time_source_and_resistance_evolution() {
    let a = run(&parse(
        "ramp
At a wave
Ab a b null lag
V1 m 0 1
Am m 0 mem
R1 b 0 1k
.model wave pwlts(x_array=[0 1] y_array=[0 2] limit=true)
.model lag delay(delay=0.2 buffer_size=512)
.model mem memristor(rmin=100 rmax=1000 rinit=200 alpha=0 beta=100 vt=0)
.tran 0.01 1.3",
    )
    .unwrap());
    for (i, &t) in a.scale_values.iter().enumerate() {
        near(
            a.real("v(b)").unwrap()[i],
            2. * (t - 0.2).clamp(0., 1.),
            1e-9,
        );
        near(a.real("i(am)").unwrap()[i], 1. / (200. + 100. * t), 1e-8);
    }
}
struct Tables;
impl SourceLoader for Tables {
    fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
        Err("no includes".into())
    }
    fn waveform(&self, path: &Path, _: Option<&str>) -> Result<String, String> {
        match path.to_str() {
            Some("2D-table-model.txt") => Ok("2\n2\n0 2\n0 1\n0 4\n1 5".into()),
            Some("3D-table-model.txt") => Ok("2\n2\n2\n0 2\n0 1\n0 1\n0 4\n1 5\n-2 2\n-1 3".into()),
            Some("ac.s1p") => Ok("# kHz S RI R 50\n1 3 4\n3 5 0".into()),
            _ => Err("invalid file".into()),
        }
    }
}
#[test]
fn grids_and_frequency_interpolation() {
    let a = run(&parse_with(
        "tables
Vx x 0 1
Vy y 0 0.5
Vz z 0 0.25
At x y %v(a) surface
Au x y z %v(b) volume
.model surface table2d
.model volume table3d
.op",
        &Tables,
    )
    .unwrap());
    near(a.real("v(a)").unwrap()[0], 2.5, 1e-12);
    near(a.real("v(b)").unwrap()[0], 2., 1e-12);
    let a = run(&parse_with(
        "transfer
V1 x 0 DC 0 AC 1
A1 x a table
A2 x b file
.model table xfer(table=[1000 3 4 3000 5 0] r_i=true)
.model file xfer(file=\"ac.s1p\")
.ac lin 3 1k 3k",
        &Tables,
    )
    .unwrap());
    for name in ["v(a)", "v(b)"] {
        let value = a.complex(name).unwrap()[1];
        near(value.0, 4., 1e-10);
        near(value.1, 2., 1e-10);
    }
}
#[test]
fn bidirectional_drive_and_pwm() {
    let a = run(&parse(
        "bridges
Vinput a 0 2
Aread [a] [bus] NULL bridge
Awrite [b] [bus] NULL output
Rload b 0 310
Apwm a pulse pwm
.model bridge bidi_bridge(direction=1)
.model output bidi_bridge(direction=0)
.model pwm d_pwm(frequency=1meg cntl_array=[0 2] dc_array=[0.2 0.3])
.tran 5n 2u",
    )
    .unwrap());
    near(a.real("v(b)").unwrap()[0], 3.1, 1e-8);
    let pulse = &a.events["pulse"];
    assert!(pulse.len() >= 4);
    near(pulse[1].0, 301e-9, 1e-14);
}

#[test]
fn real_resolution_polarity_feedback_ic_and_bridge_domains() {
    let s = "real drivers
Ap bit up
Aa bit NULL net pos
Ab bit NULL ~net neg
Ag ~net corrected amp
Afb stored stored fb
Ao corrected %id(load 0) bridge
Rload load 0 10
.model up d_pullup
.model pos d_to_real(one=3)
.model neg d_to_real(one=1)
.model amp real_gain(gain=2 in_offset=1 out_offset=1)
.model fb real_gain(gain=1 ic=7)
.model bridge real_to_v
.op";
    let n = parse(s).unwrap();
    let r = run(&n);
    let roundtrip = run(&parse(&sansho_netlist::write(&n)).unwrap());
    assert_eq!(r, roundtrip);
    near(r.events["net"][0].1.parse().unwrap(), 2., 1e-12);
    near(r.events["stored"][0].1.parse().unwrap(), 7., 1e-12);
    near(r.real("v(load)").unwrap()[0], 10., 1e-12);
    let err = run_with(
        &parse(&s.replace("Ag ~net", "Ag ~bit")).unwrap(),
        &RunConfig { threads: 1 },
    )
    .unwrap_err();
    assert!(err.diagnostics.iter().any(|e| e.code == "E-XSPICE-PORT"));
}
#[test]
fn current_sense_controlled_delay_and_file_errors() {
    let s = "named current
V1 a 0 1
R1 a 0 2
Vc ctl 0 0.5
Ad %vnam V1 z ctl lag
.model lag delay(has_delay_cnt=true delmin=1n delmax=3n)
.tran 1n 4n";
    let r = run(&parse(s).unwrap());
    for v in r.real("v(z)").unwrap() {
        near(*v, -0.5, 1e-12);
    }
    for (kind, key) in [
        ("table2d", "file"),
        ("table3d", "file"),
        ("xfer", "file"),
        ("d_state", "state_file"),
    ] {
        let e = parse_with(
            &format!("bad path\n.model unsafe {kind}({key}=\"../outside\")"),
            &Tables,
        )
        .unwrap_err();
        assert!(
            e.iter()
                .any(|d| d.code == "E-XSPICE-FILE" && d.span.is_some())
        );
    }
}
