//! End-to-end event, mixed-signal and scheduling regressions.
//! Provenance: original stimuli, Boolean truth tables, edge timing and circuit laws.
#![allow(clippy::unwrap_used)]
use sansho_engine::{RunConfig, run_with};
use sansho_netlist::{SourceLoader, parse, parse_file, parse_with};
use std::path::Path;

#[test]
fn mixed_decks_are_bit_identical() {
    for name in ["bus", "counter", "latch", "mixed", "ring"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../verify/r12-decks/{name}.cir"));
        let nl = parse_file(&path).unwrap();
        let simulate = |threads| {
            let out = run_with(&nl, &RunConfig { threads }).unwrap();
            assert!(out.warnings.is_empty(), "{name}: {:?}", out.warnings);
            out.results
        };
        let one = simulate(1);
        let two = simulate(2);
        let raw = |r| {
            let mut bytes = vec![];
            sansho_output::write_raw_binary(r, &mut bytes).unwrap();
            bytes
        };
        assert_eq!(raw(&one), raw(&two));
        assert_eq!(one, two);
        let a = &one.analyses[0];
        assert!(a.events.values().all(|trace| {
            trace
                .windows(2)
                .all(|p| p[0].0 < p[1].0 && p[0].1 != p[1].1)
        }));
        let key = match name {
            "bus" => "bus",
            "counter" => "q1",
            "mixed" | "latch" => "q",
            _ => "last",
        };
        let states: Vec<_> = a.events[key].iter().map(|(_, v)| v.as_str()).collect();
        match name {
            "bus" => assert_eq!(states, ["1r", "0s", "Us", "1s", "1r"]),
            "counter" => assert_eq!(states, ["0s", "1s", "0s"]),
            "latch" => {
                assert_eq!(states, ["0s", "1s", "0s"]);
                let input = &a.events["ds"];
                let buffered = &a.events["clean"];
                assert_eq!(input.len(), 5);
                assert_eq!(buffered.len(), input.len());
                for ((t, data), (out, delayed)) in input[1..].iter().zip(&buffered[1..]) {
                    assert_eq!(data, delayed);
                    assert!((out - t - 2e-9).abs() < 1e-18);
                }
            }
            "mixed" => assert_eq!(states, ["0s", "1s", "0s", "1s", "0s", "1s"]),
            _ => {
                for p in a.events[key][1..11].windows(2) {
                    assert!((p[1].0 - p[0].0 - 6e-9).abs() < 1e-16)
                }
            }
        }
    }
}

#[test]
fn gate_truth_tables_storage_controls_and_scoped_ports() {
    let mut deck =
        "logic\nAh h up\nAl l down\n.model up d_pullup\n.model down d_pulldown\n".to_string();
    for (i, kind) in ["and", "nand", "or", "nor", "xor", "xnor"]
        .iter()
        .enumerate()
    {
        deck += &format!("Ag{i} %d[h l] n{i} m{i}\n.model m{i} d_{kind}\n");
    }
    deck +=
        "Ac h oc collector\nAe h oe emitter\n.model collector d_open_c\n.model emitter d_open_e\n";
    deck += "Aj l h h h l jq NULL jk\nAs l h h NULL NULL sq NULL sr\nAd h h NULL NULL dq NULL lat\n.model jk d_jkff\n.model sr d_srff(ic=1)\n.model lat d_dlatch\n";
    deck +=
        ".subckt stage in out\nAinv in out mi\n.model mi d_inverter\n.ends\nXscope h zz stage\n.op";
    let nl = parse(&deck).unwrap();
    let r = run_with(&nl, &RunConfig { threads: 2 }).unwrap();
    let a = &r.results.analyses[0];
    for (n, expected) in [
        ("n0", "0s"),
        ("n1", "1s"),
        ("n2", "1s"),
        ("n3", "0s"),
        ("n4", "1s"),
        ("n5", "0s"),
        ("oc", "1z"),
        ("oe", "1s"),
        ("jq", "1s"),
        ("sq", "1s"),
        ("dq", "1s"),
        ("zz", "0s"),
    ] {
        assert_eq!(a.events[n][0].1, expected, "{n}");
    }
    assert!(a.variables.is_empty());
}

struct Stimulus(&'static str);
const BITS: Stimulus = Stimulus("0 0s 1r\n3n 1s 0z\n8n Uu 1u");
impl SourceLoader for Stimulus {
    fn waveform(&self, path: &Path, _: Option<&str>) -> Result<String, String> {
        if path != Path::new("stim.dat") {
            return Err("unexpected path".into());
        }
        Ok(self.0.into())
    }
    fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
        Err("no includes".into())
    }
}
#[test]
fn file_and_lut_changes_land_between_analog_steps() {
    let src = "file\nAst [a b] src\nAlut [a b] y lut\n.model src d_source(input_file=\"stim.dat\")\n.model lut d_lut(table_values=\"0101\" rise_delay=1n fall_delay=1n)\n.tran 5n 12n 0 5n";
    for deck in [
        src.into(),
        sansho_netlist::write(&parse_with(src, &BITS).unwrap()),
    ] {
        let nl = parse_with(&deck, &BITS).unwrap();
        let r = run_with(&nl, &RunConfig { threads: 1 }).unwrap();
        let a = &r.results.analyses[0];
        let y = &a.events["y"];
        assert_eq!(
            y.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(),
            ["0s", "1s", "Us"]
        );
        for (entry, ns) in y.iter().zip([0., 4., 9.]) {
            assert!((entry.0 - ns * 1e-9).abs() < 1e-18)
        }
        assert_eq!(a.events["b"][2].1, "1u");
    }
    for filename in ["stim.dat", "../secret", "/etc/file"] {
        let errs = parse(&src.replace("stim.dat", filename)).unwrap_err();
        assert!(errs.iter().any(|d| d.code == "E-XSPICE-FILE"));
    }
}

#[test]
fn dc_sweep_resolves_both_bridge_domains() {
    let s = "sweep\nV1 a 0 0\nAa [a] [d] adc\nAd [d] [out] dac\n.model adc adc_bridge(in_low=0.25 in_high=0.75)\n.model dac dac_bridge(out_low=-1 out_high=3 out_undef=0.2)\n.dc V1 0 1 0.25";
    let r = run_with(&parse(s).unwrap(), &RunConfig { threads: 1 }).unwrap();
    for (&actual, expected) in r.results.analyses[0]
        .real("v(out)")
        .unwrap()
        .iter()
        .zip([-1., -1., 0.2, 3., 3.])
    {
        assert!((actual - expected).abs() < 1e-12);
    }
}

#[test]
fn type_errors_and_zero_delay_oscillation_are_structured() {
    for cards in [
        "A1 x up\nR1 x 0 1k",
        "R1 x 0 1k\nA1 x up",
        "A1 %v x up",
        "A1 0 up",
    ] {
        let nl = parse(&format!("bad pins\n{cards}\n.model up d_pullup\n.op")).unwrap();
        let err = run_with(&nl, &RunConfig { threads: 1 }).unwrap_err();
        assert!(
            err.diagnostics
                .iter()
                .any(|d| d.code == "E-XSPICE-PORT" && d.span.is_some())
        );
    }
    let src = "osc\nV1 x 0 PWL(0 0 1n 1)\nAadc [x] [go] adc\nAn [go y] y nand\n.model adc adc_bridge(in_low=0.5 in_high=0.5 rise_delay=0 fall_delay=0)\n.model nand d_nand(rise_delay=0 fall_delay=0)\n.tran 1n 3n";
    let err = run_with(&parse(src).unwrap(), &RunConfig { threads: 1 }).unwrap_err();
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == "E-XSPICE-OSCILLATION")
    );
}

#[test]
fn oscillator_edges_and_asymmetric_ramps() {
    let s = "osc\nVf f 0 1\nAo f q osc\nAd [q] [out] dac\n.model osc d_osc(cntl_array=[0 2] freq_array=[0.5G 1.5G] duty_cycle=0.25 rise_delay=0 fall_delay=0)\n.model dac dac_bridge(t_rise=0.2n t_fall=0.1n)\n.tran 0.5n 3n";
    let r = run_with(&parse(s).unwrap(), &RunConfig { threads: 1 }).unwrap();
    let a = &r.results.analyses[0];
    for ((t, _), ns) in a.events["q"].iter().zip([0., 0.75, 1., 1.75, 2., 2.75, 3.]) {
        assert!((t - ns * 1e-9).abs() < 1e-19)
    }
    for (ns, expected) in [(0.95, 1.), (1.1, 0.), (1.95, 1.)] {
        let i = a
            .scale_values
            .iter()
            .position(|t| (t - ns * 1e-9).abs() < 1e-18)
            .unwrap();
        assert!((a.real("v(out)").unwrap()[i] - expected).abs() < 1e-9);
    }
}

#[test]
fn uic_charges_use_the_initial_dac_voltage() {
    let s = "initial\nAp d up\nAd [d] [out] dac\nCout out 0 2p\n.model up d_pullup\n.model dac dac_bridge(out_low=0 out_high=2)\n.tran 1n 5n uic";
    let r = run_with(&parse(s).unwrap(), &RunConfig { threads: 1 }).unwrap();
    assert!(
        r.results.analyses[0]
            .real("v(out)")
            .unwrap()
            .iter()
            .all(|v| (v - 2.).abs() < 1e-12)
    );
}

#[test]
fn repeated_vector_pins_add_to_the_digital_load() {
    let nl=parse("loads\nAg [a a a] q gate\nAd [a] [out] dac\n.model gate d_and(input_load=2p)\n.model dac dac_bridge(input_load=4p)\n.op").unwrap();
    let (c, _) = sansho_engine::compile(&nl, &Default::default()).unwrap();
    assert!((c.digital_load("A").unwrap() - 10e-12).abs() < 1e-25);
    assert_eq!(c.digital_load("out"), None);
}

#[test]
fn concurrent_op_and_transient_keep_separate_event_histories() {
    let s = "two analyses\nV1 x 0 DC 0 PULSE(0 1 3n 2n 2n 5n 20n)\nA1 [x] [d] m\n.model m adc_bridge(in_low=0.25 in_high=0.75 rise_delay=0 fall_delay=0)\n.op\n.tran 5n 16n 0 5n";
    let nl = parse(s).unwrap();
    let one = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    let two = run_with(&nl, &RunConfig { threads: 2 }).unwrap().results;
    assert_eq!(one, two);
    assert_eq!(two.analyses[0].events["d"].len(), 1);
    let changes = &two.analyses[1].events["d"];
    let tran = &two.analyses[1];
    let mut sampled = Vec::new();
    for (&t, &v) in tran.scale_values.iter().zip(tran.real("v(x)").unwrap()) {
        let state = if v <= 0.25 {
            "0s"
        } else if v >= 0.75 {
            "1s"
        } else {
            "Us"
        };
        if sampled.last().is_none_or(|(_, last)| last != state) {
            sampled.push((t, state.to_string()));
        }
    }
    assert_eq!(*changes, sampled);
    assert_eq!(
        changes.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(),
        ["0s", "Us", "1s", "Us", "0s"]
    );
}

#[test]
fn same_time_file_data_and_delayed_clock_are_sampled_together() {
    let s = "simultaneous\nAs [trigger data] src\nAc trigger clock buf\nAff data clock NULL NULL q NULL flip\n.model src d_source(input_file=\"stim.dat\")\n.model buf d_buffer(rise_delay=1)\n.model flip d_dff(ic=0 clk_delay=0 rise_delay=0 fall_delay=0)\n.tran 0.5 3";
    let nl = parse_with(s, &Stimulus("0 0s 0s\n1 1s 0s\n2 1s 1s")).unwrap();
    let a = run_with(&nl, &RunConfig { threads: 1 })
        .unwrap()
        .results
        .analyses
        .remove(0);
    assert_eq!(
        a.events["q"],
        [(0., "0s".to_owned()), (2., "1s".to_owned())]
    );
}

/// Run the first transient of a probe with its own circuit and sweep parameter.
fn probe_transient(deck: &str, value: &str) -> sansho_output::AnalysisResult {
    let (circuit, commands) = deck.split_once(".control\n").unwrap();
    let tran = commands.lines().find(|s| s.starts_with("tran ")).unwrap();
    let mut deck = String::new();
    for card in circuit.lines() {
        if card.starts_with(".param probe=") {
            deck.push_str(&format!(".param probe={value}"));
        } else {
            deck.push_str(card);
        }
        deck.push('\n');
    }
    deck.push_str(&format!(".{tran}\n.end"));
    run_with(&parse(&deck).unwrap(), &RunConfig { threads: 1 })
        .unwrap()
        .results
        .analyses
        .remove(0)
}

#[test]
fn short_pulse_arrival_order_is_shared_by_buffers_and_inverters() {
    let a = probe_transient(
        include_str!("../../../verify/probes/r12_buffer.cir"),
        "0.5n",
    );
    for (output, states) in [("buf", ["0s", "1s", "0s"]), ("inv", ["1s", "0s", "1s"])] {
        let trace = &a.events[output];
        assert_eq!(trace.len(), 3);
        for ((t, v), (ns, symbol)) in trace.iter().zip([0., 5.125, 5.825].into_iter().zip(states)) {
            assert_eq!(v, symbol);
            assert!((t - ns * 1e-9).abs() < 1e-18);
        }
    }
    assert_eq!(a.events["asym"], [(0., "0s".into())]);
}

#[test]
fn divider_phase_follows_the_count_after_each_rising_edge() {
    let probe = include_str!("../../../verify/probes/r12_divider.cir");
    for initial in 0..4 {
        let a = probe_transient(probe, &initial.to_string());
        for (name, high) in [("narrow", 1), ("wide", 3)] {
            let mut expected = Vec::new();
            for (i, (t, _)) in a.events["clk"]
                .iter()
                .filter(|(t, s)| *t == 0. || s == "1s")
                .enumerate()
            {
                let count = (initial + i) % 4;
                let state = if (1..=high).contains(&count) {
                    "1s"
                } else {
                    "0s"
                };
                if expected.last().is_none_or(|(_, prev)| *prev != state) {
                    expected.push((if i == 0 { 0. } else { t + 1e-9 }, state));
                }
            }
            assert_eq!(expected.len(), a.events[name].len());
            for ((time, state), (t, v)) in expected.iter().zip(&a.events[name]) {
                assert_eq!(state, v);
                assert!((t - time).abs() < 1e-18);
            }
        }
    }
}

#[test]
fn bridge_window_equality_and_interrupted_ramp() {
    let a = probe_transient(
        include_str!("../../../verify/probes/r12_bridges.cir"),
        "1.5",
    );
    for (name, edges) in [
        ("instant", &[0., 3.15, 3.95, 44.15, 44.95][..]),
        ("d", &[0., 4.15, 4.95, 45.15, 45.95]),
        ("cmp", &[0., 14.01, 33.]),
    ] {
        assert_eq!(a.events[name].len(), edges.len());
        for ((t, _), ns) in a.events[name].iter().zip(edges) {
            assert!((t - ns * 1e-9).abs() < 1e-18);
        }
    }
    let slew = a.real("v(slew)").unwrap();
    for (&t, &v) in a.scale_values.iter().zip(slew) {
        let ns = t * 1e9;
        let expected = if ns < 45.15 {
            (-1. + 0.2 * (ns - 4.15).max(0.)).min(3.)
        } else {
            3. - 0.5 * (ns - 45.15)
        };
        assert!((v - expected).abs() < 1e-12);
    }
}

#[test]
fn boundary_probe_matches_directional_delays_and_oscillator_phase() {
    let a = probe_transient(
        include_str!("../../../verify/probes/r12c_boundaries.cir"),
        "0.1",
    );
    let adc = ["0s", "Us", "1s", "Us", "0s"];
    for (name, times, states) in [
        ("d", &[0., 5.01, 16., 22.15, 22.9][..], adc.as_slice()),
        ("w", &[0., 7.01, 18., 22.4, 23.15], &adc),
        ("q", &[0., 7.01, 18., 22.4, 23.15], &adc),
        (
            "osc",
            &[0., 7.5, 10., 17.5, 20., 27.5, 30.],
            &["0u", "1s", "0s", "1s", "0s", "1s", "0s"],
        ),
    ] {
        let trace = &a.events[name];
        assert_eq!(trace.len(), times.len(), "{name}");
        for ((t, state), (ns, expected)) in trace.iter().zip(times.iter().zip(states)) {
            assert_eq!(state, expected, "{name}");
            assert!((t - ns * 1e-9).abs() < 1e-18, "{name} at {t:e}");
        }
    }
    let out = a.real("v(out)").unwrap();
    let start = a.scale_values.partition_point(|&t| t < 6e-9);
    let end = a.scale_values.partition_point(|&t| t <= 15e-9);
    assert!(start < end);
    assert!(out[start..end].iter().all(|&v| (v - 1.).abs() < 1e-12));
}

#[test]
fn oscillator_drives_only_after_its_first_low_phase() {
    let a = probe_transient(include_str!("../../../verify/probes/r12_osc.cir"), "0.25");
    assert_eq!(a.events["delayed"], a.events["clk"]);
    assert_eq!(a.events["clk"][0], (0., "0u".into()));
    assert_eq!(a.events["clk"].len(), 7);
    assert!((a.events["clk"][1].0 - 7.5e-9).abs() < 1e-18);
    let a = probe_transient(include_str!("../../../verify/probes/r12_storage.cir"), "2");
    for (name, initial) in [
        ("loop", "0s"),
        ("qi", "Us"),
        ("openq", "1s"),
        ("openqb", "0s"),
    ] {
        assert_eq!(a.events[name][0].1, initial);
    }
}

#[test]
fn counter_edges_stay_within_the_observed_sampling_difference() {
    let probe = include_str!("../../../verify/probes/r12_counter_grid.cir");
    let a = probe_transient(probe, "0.2n");
    for (name, times) in [
        ("q0", &[0., 4.64, 14.64, 24.64, 34.65, 44.64][..]),
        ("carry", &[0., 4.64, 14.64, 24.64, 34.65, 44.64]),
        ("q1", &[0., 15.04, 35.05]),
    ] {
        assert_eq!(a.events[name].len(), times.len());
        for ((t, _), ns) in a.events[name].iter().zip(times) {
            let budget = if *ns == 34.65 || *ns == 35.05 {
                10e-12
            } else {
                0.
            };
            assert!((t - ns * 1e-9).abs() <= budget + 1e-18);
        }
    }
}
