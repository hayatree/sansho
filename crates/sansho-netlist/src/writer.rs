//! Netlist writer: [`Netlist`] → SPICE text that both Sansho and ngspice accept.
//!
//! The output is canonical (lower-case names, one card per line, shortest round-trip number
//! formatting), so it doubles as the basis of input hashes. Parsing the output yields the same
//! netlist up to source spans.
//!
//! Provenance: original work, following the SPICE input format described in Nagel (1975).
//! See `docs/PROVENANCE.md`.

use std::fmt::Write;

use crate::{
    AcSweep, AnalysisCard, CrossDir, ElementKind, MeasAnalysis, MeasEvent, MeasKind, MeasStat,
    MeasureCard, Netlist, Signal, SignalFn, SourceSpec, TranFn,
};

/// Shortest representation that parses back to the same `f64`.
fn num(v: f64) -> String {
    let s = format!("{v:e}");
    // `1e0` → `1`, keep everything else as is (`2.5e-9`).
    s.strip_suffix("e0").map_or(s.clone(), str::to_string)
}

fn list(vals: &[f64]) -> String {
    vals.iter().map(|&v| num(v)).collect::<Vec<_>>().join(" ")
}

fn params(out: &mut String, ps: &[(String, f64)]) {
    for (k, v) in ps {
        let _ = write!(out, " {k}={}", num(*v));
    }
}

fn source(out: &mut String, s: &SourceSpec) {
    if let Some(dc) = s.dc {
        let _ = write!(out, " dc {}", num(dc));
    }
    if s.ac_mag != 0.0 || s.ac_phase_deg != 0.0 {
        let _ = write!(out, " ac {} {}", num(s.ac_mag), num(s.ac_phase_deg));
    }
    if let Some((n, r)) = s.port {
        let _ = write!(out, " portnum={n} z0={}", num(r));
    }
    for (i, stim) in s.disto.iter().enumerate() {
        if let Some((mag, phase)) = stim {
            let _ = write!(out, " distof{} {} {}", i + 1, num(*mag), num(*phase));
        }
    }
    match &s.tran {
        Some(TranFn::Pulse(v)) => {
            let _ = write!(out, " pulse({})", list(v));
        }
        Some(TranFn::TrNoise(v)) => {
            let _ = write!(out, " trnoise({})", list(v));
        }
        Some(TranFn::TrRandom(v)) => {
            let _ = write!(out, " trrandom({})", list(v));
        }
        Some(TranFn::Sin(v)) => {
            let _ = write!(out, " sin({})", list(v));
        }
        Some(TranFn::Sffm(v)) => {
            let _ = write!(out, " sffm({})", list(v));
        }
        Some(TranFn::Am(v)) => {
            let _ = write!(out, " am({})", list(v));
        }
        Some(TranFn::Exp(v)) => {
            let _ = write!(out, " exp({})", list(v));
        }
        Some(TranFn::Pwl(pts)) => {
            let flat: Vec<f64> = pts.iter().flat_map(|&(t, v)| [t, v]).collect();
            let _ = write!(out, " pwl({})", list(&flat));
        }
        None => {}
    }
}

pub(crate) fn signal(s: &Signal) -> String {
    if s.is_vector() {
        return s.pos.clone();
    }
    let base = if s.current { "i" } else { "v" };
    let f = match s.func {
        SignalFn::Plain => "",
        SignalFn::Mag => "m",
        SignalFn::Db => "db",
        SignalFn::Phase => "p",
        SignalFn::Re => "r",
        SignalFn::Im => "i",
    };
    match &s.neg {
        Some(n) => format!("{base}{f}({},{n})", s.pos),
        None => format!("{base}{f}({})", s.pos),
    }
}

fn qualifiers(dir: CrossDir, n: Option<u32>, td: Option<f64>) -> String {
    let d = match dir {
        CrossDir::Rise => "rise",
        CrossDir::Fall => "fall",
        CrossDir::Cross => "cross",
    };
    let n = n.map_or("last".to_string(), |n| n.to_string());
    let mut s = format!(" {d}={n}");
    if let Some(td) = td {
        let _ = write!(s, " td={}", num(td));
    }
    s
}

/// `TRIG`/`TARG` event (`sig val=v ...`) or `WHEN` event (`sig=v ...`).
fn event(e: &MeasEvent, when: bool) -> String {
    match e {
        MeasEvent::At { at } => format!("at={}", num(*at)),
        MeasEvent::Cross {
            signal: s,
            value,
            reference,
            dir,
            n,
            td,
        } => {
            let sep = if when { "=" } else { " val=" };
            format!(
                "{}{sep}{}{}",
                signal(s),
                reference.as_ref().map_or_else(|| num(*value), signal),
                qualifiers(*dir, *n, *td)
            )
        }
    }
}

/// Write one `.meas` card.
pub fn write_measure(m: &MeasureCard) -> String {
    let a = match m.analysis {
        MeasAnalysis::Tran => "tran",
        MeasAnalysis::Ac => "ac",
        MeasAnalysis::Dc => "dc",
        MeasAnalysis::Noise => "noise",
    };
    let body = match &m.kind {
        MeasKind::TrigTarg { trig, targ } => {
            format!("trig {} targ {}", event(trig, false), event(targ, false))
        }
        MeasKind::When { event: e } => format!("when {}", event(e, true)),
        MeasKind::Param { expression } => format!("param={expression}"),
        MeasKind::Find {
            signal: s,
            event: e,
        }
        | MeasKind::Deriv {
            signal: s,
            event: e,
        } => {
            let op = if matches!(m.kind, MeasKind::Find { .. }) {
                "find"
            } else {
                "deriv"
            };
            let when = if matches!(e, MeasEvent::At { .. }) {
                ""
            } else {
                "when "
            };
            format!("{op} {} {when}{}", signal(s), event(e, true))
        }
        MeasKind::Stat {
            stat,
            signal: s,
            from,
            to,
        } => {
            let st = match stat {
                MeasStat::MaxAt => "max_at",
                MeasStat::MinAt => "min_at",
                MeasStat::Max => "max",
                MeasStat::Min => "min",
                MeasStat::Pp => "pp",
                MeasStat::Avg => "avg",
                MeasStat::Rms => "rms",
                MeasStat::Integ => "integ",
            };
            let mut b = format!("{st} {}", signal(s));
            if let Some(f) = from {
                let _ = write!(b, " from={}", num(*f));
            }
            if let Some(t) = to {
                let _ = write!(b, " to={}", num(*t));
            }
            b
        }
    };
    format!(".meas {a} {} {body}", m.name)
}

/// Write one analysis card.
pub fn write_analysis(a: &AnalysisCard) -> String {
    match a {
        AnalysisCard::Pss { spec: p, .. } => format!(
            ".pss {} {} {} {} {} {} {}{}{}",
            num(p.gfreq),
            num(p.tstab),
            p.oscnob,
            p.points,
            p.harmonics,
            p.iterations,
            num(p.steadycoeff),
            if p.driven { " driven" } else { "" },
            if p.uic { " uic" } else { "" }
        ),
        AnalysisCard::Sp { grid, .. } | AnalysisCard::Disto { grid, .. } => {
            let (name, extra) = match a {
                AnalysisCard::Disto { ratio, .. } => ("disto", ratio.map(num)),
                AnalysisCard::Sp { noise: true, .. } => ("sp", Some("1".into())),
                _ => ("sp", None),
            };
            let sweep = match grid.sweep {
                AcSweep::Dec => "dec",
                AcSweep::Oct => "oct",
                AcSweep::Lin => "lin",
            };
            let mut card = format!(
                ".{name} {sweep} {} {} {}",
                grid.points,
                num(grid.fstart),
                num(grid.fstop)
            );
            if let Some(extra) = extra {
                let _ = write!(card, " {extra}");
            }
            card
        }
        AnalysisCard::Op { .. } => ".op".into(),
        AnalysisCard::Tf { output, source, .. } => format!(".tf {output} {source}"),
        AnalysisCard::Sens { output, ac, .. } => {
            let mut line = format!(".sens {output}");
            if let Some(grid) = ac {
                let mode = match grid.sweep {
                    AcSweep::Dec => "dec",
                    AcSweep::Oct => "oct",
                    AcSweep::Lin => "lin",
                };
                let _ = write!(
                    line,
                    " ac {mode} {} {} {}",
                    grid.points,
                    num(grid.fstart),
                    num(grid.fstop)
                );
            }
            line
        }
        AnalysisCard::Four {
            frequency, outputs, ..
        } => format!(
            ".four {} {}",
            num(*frequency),
            outputs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        AnalysisCard::Pz {
            nodes,
            voltage,
            poles,
            zeros,
            ..
        } => format!(
            ".pz {} {} {}",
            nodes.join(" "),
            if *voltage { "vol" } else { "cur" },
            if *poles && *zeros {
                "pz"
            } else if *poles {
                "pol"
            } else {
                "zer"
            }
        ),
        AnalysisCard::Noise {
            output,
            source,
            sweep,
            points,
            fstart,
            fstop,
            summary,
            ..
        } => {
            let sweep = match sweep {
                AcSweep::Dec => "dec",
                AcSweep::Oct => "oct",
                AcSweep::Lin => "lin",
            };
            format!(
                ".noise {output} {source} {sweep} {points} {} {} {summary}",
                num(*fstart),
                num(*fstop)
            )
        }
        AnalysisCard::Dc { inner, outer, .. } => {
            let mut text = String::from(".dc");
            for axis in std::iter::once(inner).chain(outer) {
                let _ = write!(
                    text,
                    " {} {} {} {}",
                    axis.source,
                    num(axis.start),
                    num(axis.stop),
                    num(axis.step)
                );
            }
            text
        }
        AnalysisCard::Ac {
            sweep,
            points,
            fstart,
            fstop,
            ..
        } => {
            let s = match sweep {
                AcSweep::Dec => "dec",
                AcSweep::Oct => "oct",
                AcSweep::Lin => "lin",
            };
            format!(".ac {s} {points} {} {}", num(*fstart), num(*fstop))
        }
        AnalysisCard::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            uic,
            ..
        } => {
            let mut s = format!(".tran {} {}", num(*tstep), num(*tstop));
            if *tstart != 0.0 || tmax.is_some() {
                let _ = write!(s, " {}", num(*tstart));
            }
            if let Some(m) = tmax {
                let _ = write!(s, " {}", num(*m));
            }
            if *uic {
                s += " uic";
            }
            s
        }
    }
}

/// Format a model card with its explicit parameters.
pub fn write_model(m: &crate::ModelCard) -> String {
    write_model_with(m, num)
}

/// Write a model listing using the caller's numeric display precision.
pub fn write_model_with(m: &crate::ModelCard, num: impl Fn(f64) -> String) -> String {
    if matches!(m.kind.as_str(), "numd" | "nbjt" | "numos") {
        let mut line = format!(
            ".model {} {} level={}",
            m.name,
            m.kind,
            m.params
                .iter()
                .find(|(k, _)| k == "level")
                .map_or(1., |p| p.1)
        );
        for card in &m.cider {
            let _ = write!(line, "\n+ {}", card.join(" "));
        }
        return line;
    }
    let mut l = format!(".model {} {} (", m.name, m.kind);
    let ps: Vec<String> = m
        .params
        .iter()
        .map(|(k, v)| {
            if m.kind == "multi_input_pwl" && k == "model" {
                format!("model=\"{}\"", if *v == 0. { "and" } else { "or" })
            } else {
                format!("{k}={}", num(*v))
            }
        })
        .collect();
    if let Some(v) = &m.version {
        let _ = write!(l, "version={v} ");
    }
    if let Some(t) = &m.table {
        let key = if m.kind == "d_state" {
            "state_file"
        } else if m.kind == "d_source" {
            "input_file"
        } else {
            "file"
        };
        let _ = write!(l, "{key}=\"{}\" ", t.filename);
    }
    l += &ps.join(" ");
    for (k, values) in &m.arrays {
        if matches!(m.kind.as_str(), "d_genlut" | "d_lut") && k == "table_values" {
            let table: String = values
                .iter()
                .map(|&v| {
                    if v == 0. {
                        '0'
                    } else if v == 1. {
                        '1'
                    } else {
                        if v == 8. {
                            'Z'
                        } else if m.kind == "d_genlut" {
                            'X'
                        } else {
                            'U'
                        }
                    }
                })
                .collect();
            let _ = write!(l, " {k}=\"{table}\"");
        } else {
            let vector = values
                .iter()
                .copied()
                .map(&num)
                .collect::<Vec<_>>()
                .join(" ");
            if m.kind == "cpl" {
                let _ = write!(l, " {k}={vector}");
            } else {
                let _ = write!(l, " {k}=[{vector}]");
            }
        }
    }
    l += ")";
    l
}

/// Write a complete deck (title line through `.end`).
pub fn write(nl: &Netlist) -> String {
    let mut out = String::new();
    let title = nl.title.lines().next().unwrap_or("");
    let _ = writeln!(out, "{}", if title.is_empty() { "*" } else { title });
    for h in &nl.hdl {
        let _ = writeln!(out, ".hdl \"{}\"", h.path);
    }
    for e in &nl.elements {
        let name = &e.name;
        let mut l = String::new();
        match &e.kind {
            ElementKind::CodeModel { ports, model } => {
                let ports = ports
                    .iter()
                    .map(crate::CodePort::text)
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = write!(l, "{name} {ports} {model}");
            }
            ElementKind::Kspice {
                ports: nodes,
                model,
                params: ps,
            }
            | ElementKind::Va {
                nodes,
                model,
                params: ps,
            } => {
                let _ = write!(l, "{name} {} {model}", nodes.join(" "));
                params(&mut l, ps);
            }
            ElementKind::Ltra { ports, model } => {
                let _ = write!(l, "{name} {} {model}", ports.join(" "));
            }
            ElementKind::Urc {
                ports,
                model,
                params: ps,
            } => {
                let _ = write!(l, "{name} {} {model}", ports.join(" "));
                params(&mut l, ps);
            }
            ElementKind::Line { ports, z0, td } => {
                let _ = write!(
                    l,
                    "{name} {} z0={} td={}",
                    ports.join(" "),
                    num(*z0),
                    num(*td)
                );
            }
            ElementKind::Polynomial {
                p,
                n,
                controls,
                coefficients,
                ..
            } => {
                let _ = write!(l, "{name} {p} {n} poly({})", controls.len());
                for ctrl in controls {
                    match ctrl {
                        crate::SwitchControl::Voltage(a, b) => {
                            let _ = write!(l, " {a} {b}");
                        }
                        crate::SwitchControl::Current(v) => {
                            let _ = write!(l, " {v}");
                        }
                    }
                }
                let _ = write!(l, " {}", list(coefficients));
            }
            ElementKind::Jfet {
                d,
                g,
                s,
                model,
                params: ps,
                off,
            }
            | ElementKind::Mesfet {
                d,
                g,
                s,
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {d} {g} {s} {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
            ElementKind::Vdmos {
                d,
                g,
                s,
                model,
                thermal_pins,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {d} {g} {s}");
                if let Some([tj, tc]) = thermal_pins {
                    let _ = write!(l, " {tj} {tc}");
                }
                let _ = write!(l, " {model}");
                for (k, v) in ps {
                    if k == "thermal" && *v == 1. {
                        l += " thermal";
                    } else {
                        let _ = write!(l, " {k}={}", num(*v));
                    }
                }
                if *off {
                    l += " off";
                }
            }
            ElementKind::Coupling { first, second, k } => {
                let _ = write!(l, "{name} {first} {second} {}", num(*k));
            }
            ElementKind::Behavioral {
                p,
                n,
                voltage,
                expr,
                params: ps,
                table,
            } => {
                let key = if name.starts_with(['r', 'l', 'c']) {
                    &name[..1]
                } else if !table.is_empty() {
                    "table"
                } else if !name.starts_with('b') {
                    "value"
                } else if *voltage {
                    "v"
                } else {
                    "i"
                };
                let _ = write!(l, "{name} {p} {n} {key}={{{expr}}}");
                params(&mut l, ps);
                for (x, y) in table {
                    let _ = write!(l, " ({},{})", num(*x), num(*y));
                }
            }
            ElementKind::Switch {
                p,
                n,
                control,
                model,
                on,
            } => {
                let ctrl = match control {
                    crate::SwitchControl::Voltage(a, b) => format!("{a} {b}"),
                    crate::SwitchControl::Current(v) => v.clone(),
                };
                let _ = write!(l, "{name} {p} {n} {ctrl} {model}");
                if *on {
                    l.push_str(" on");
                }
            }
            ElementKind::Resistor {
                p,
                n,
                value,
                model,
                params: ps,
                ..
            }
            | ElementKind::Capacitor {
                p,
                n,
                value,
                model,
                params: ps,
            }
            | ElementKind::Inductor {
                p,
                n,
                value,
                model,
                params: ps,
            } => {
                let mult = match &e.kind {
                    ElementKind::Resistor { mult, .. } => *mult,
                    _ => 1.,
                };
                let _ = write!(l, "{name} {p} {n} {}", num(*value * mult));
                if let Some(model) = model {
                    let _ = write!(l, " {model}");
                }
                if mult != 1. {
                    let _ = write!(l, " m={}", num(mult));
                    for (key, v) in ps {
                        let _ =
                            write!(l, " {key}={}", num(if key == "ac" { v * mult } else { *v }));
                    }
                } else {
                    params(&mut l, ps);
                }
            }
            ElementKind::VoltageSource { p, n, source: s }
            | ElementKind::CurrentSource { p, n, source: s } => {
                let _ = write!(l, "{name} {p} {n}");
                source(&mut l, s);
            }
            ElementKind::Vcvs { p, n, cp, cn, gain } => {
                let _ = write!(l, "{name} {p} {n} {cp} {cn} {}", num(*gain));
            }
            ElementKind::Vccs { p, n, cp, cn, gm } => {
                let _ = write!(l, "{name} {p} {n} {cp} {cn} {}", num(*gm));
            }
            ElementKind::Cccs {
                p,
                n,
                control,
                gain,
            } => {
                let _ = write!(l, "{name} {p} {n} {control} {}", num(*gain));
            }
            ElementKind::Ccvs { p, n, control, r } => {
                let _ = write!(l, "{name} {p} {n} {control} {}", num(*r));
            }
            ElementKind::Diode {
                p,
                n,
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {p} {n} {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
            ElementKind::Bjt {
                c,
                b,
                e: em,
                s,
                tj,
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {c} {b} {em}");
                if let Some(s) = s {
                    let _ = write!(l, " {s}");
                }
                if let Some(tj) = tj {
                    let _ = write!(l, " {tj}");
                }
                let _ = write!(l, " {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
            ElementKind::Mosfet {
                extra,
                d,
                g,
                s,
                b,
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {d} {g} {s} {b}");
                for pin in extra {
                    let _ = write!(l, " {pin}");
                }
                let _ = write!(l, " {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
        }
        if !e.ic.is_empty() {
            let _ = write!(
                l,
                " ic={}",
                e.ic.iter().map(|&v| num(v)).collect::<Vec<_>>().join(",")
            );
        }
        let _ = writeln!(out, "{l}");
    }
    for m in &nl.models {
        let _ = writeln!(out, "{}", write_model(m));
    }
    if !nl.options.is_empty() {
        let mut l = String::from(".options");
        for o in &nl.options {
            match o.value {
                // The integration method is stored as a flag; ngspice spells it `method=`.
                None if matches!(o.name.as_str(), "gear" | "trap" | "trapezoidal") => {
                    let _ = write!(l, " method={}", o.name);
                }
                Some(v) => {
                    let _ = write!(l, " {}={}", o.name, num(v));
                }
                None => {
                    let _ = write!(l, " {}", o.name);
                }
            }
        }
        let _ = writeln!(out, "{l}");
    }
    for (card, list) in [(".ic", &nl.initial_conditions), (".nodeset", &nl.nodesets)] {
        if !list.is_empty() {
            let items: Vec<String> = list
                .iter()
                .map(|v| format!("v({})={}", v.node, num(v.value)))
                .collect();
            let _ = writeln!(out, "{card} {}", items.join(" "));
        }
    }
    for a in &nl.analyses {
        let _ = writeln!(out, "{}", write_analysis(a));
    }
    for card in &nl.prints {
        let _ = writeln!(
            out,
            ".print {} {}",
            card.analysis,
            card.signals
                .iter()
                .map(signal)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    if !nl.saves.is_empty() {
        let _ = writeln!(out, ".save {}", nl.saves.join(" "));
    }
    for m in &nl.measures {
        let _ = writeln!(out, "{}", write_measure(m));
    }
    for (name, value) in &nl.csparams {
        out += &format!(".csparam {name}={value}\n");
    }
    for block in &nl.controls {
        out += ".control\n";
        crate::control::write(block, &mut out);
        out += ".endc\n";
    }
    out += ".end\n";
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn numbers_round_trip() {
        for v in [1.0, 0.5, 1e-9, 2.5e-12, -3.3, 1e6, 0.1 + 0.2, 123456.789] {
            assert_eq!(crate::parse_number(&num(v)), Some(v), "{v}");
        }
        assert_eq!(num(1.0), "1");
    }

    #[test]
    fn soi_optional_pins_survive_flattening_and_round_trip() {
        let text = "SOI terminals
.subckt cell a z p t
.model film nmos level=10
M1 a a z z p b t film tnodeout ic=.1,.2,.3,.4,.5
.ends
X1 rail 0 contact rise cell
.end";
        let a = parse(text).unwrap();
        let b = parse(&write(&a)).unwrap();
        assert_eq!(a.elements[0].kind, b.elements[0].kind);
        assert_eq!(b.elements[0].ic, [0.1, 0.2, 0.3, 0.4, 0.5]);
        let nodes = a.elements[0].kind.nodes();
        assert_eq!(nodes.len(), 7);
        assert_eq!(nodes[4], "contact");
        assert_ne!(nodes[5], "b");
        assert_eq!(nodes[6], "rise");
        for (terminals, card) in [
            ("d g s e p", "nmos level=54"),
            ("d g s e p b t", "nmos level=10"),
        ] {
            let deck = format!("bad pins\nM1 {terminals} dev\n.model dev {card}\n");
            assert!(
                parse(&deck)
                    .unwrap_err()
                    .iter()
                    .any(|d| d.code == "E-PARSE-ARITY")
            );
        }
    }

    #[test]
    fn hdl_and_va_round_trip() {
        struct Loader;
        impl crate::SourceLoader for Loader {
            fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
                Ok((
                    path.into(),
                    "module r(p,n); electrical p,n; endmodule".into(),
                ))
            }
        }
        let src = "va roundtrip\n.hdl \"model dir/r.va\"\n.model rm r r=2k\nN1 a 0 rm r=1k\n";
        let a = crate::parse_with(src, &Loader).unwrap();
        let once = write(&a);
        let b = crate::parse_with(&once, &Loader).unwrap();
        assert_eq!(write(&b), once);
        assert_eq!(a.elements[0].kind, b.elements[0].kind);
        assert_eq!(a.hdl[0].path, "model dir/r.va");
        assert_eq!(a.hdl[0].text, b.hdl[0].text);
        assert_eq!(crate::parse(src).unwrap_err()[0].code, "E-PARSE-INCLUDE");
        assert!(
            crate::parse("t\n.model m unknown\nD1 a 0 m\n")
                .unwrap_err()
                .iter()
                .any(|d| d.code == "E-PARSE-MODEL")
        );
    }

    #[test]
    fn written_deck_is_a_fixed_point() {
        let src = "test deck\nV1 in 0 DC 1 AC 1 0 PULSE(0 1 1n 1n 1n 5n 10n)\nI1 0 x SIN(0 1m 1meg)\n\
                   V2 y 0 PWL(0 0 1n 1 2n 0)\nR1 in out 1k\nC1 out 0 1p\nL1 out x 1u\n\
                   E1 e 0 out 0 2\nG1 g 0 out 0 1m\nF1 f 0 v1 3\nH1 h 0 v1 5\n\
                   R2 e 0 1\nR3 g 0 1\nR4 f 0 1\nR5 h 0 1\nR6 x 0 1\nR7 y 0 1\n\
                   D1 out 0 dm 2\nQ1 out in 0 qn off\nM1 out in 0 0 nm l=1u w=2u\n\
                   .model dm d (is=1e-14 n=1.5)\n.model qn npn bf=100\n.model nm nmos vto=0.5\n\
                   .options reltol=1e-4 gear\n.ic v(out)=0.5 v(x)=-1\n.nodeset v(in)=1\n.op\n.dc v1 0 1 0.1\n.ac dec 10 1 1g\n\
                   .tran 1n 100n 0 1n uic\n\
                   .meas tran tr trig v(out) val=0.1 rise=1 targ v(out) val=0.9 rise=last td=1n\n\
                   .meas tran tw when v(out)=0.5 cross=2\n.meas tran pd pp v(out,in)\n\
                   .meas tran vf find i(v1) at=5n\n.meas ac g find vdb(out) when vp(out)=-1\n\
                   .meas tran av avg v(out) from=1n to=5n\n.end\n";
        let nl = parse(src).unwrap_or_else(|d| panic!("{d:#?}"));
        let once = write(&nl);
        let nl2 = parse(&once).unwrap_or_else(|d| panic!("{d:#?}\n{once}"));
        assert_eq!(write(&nl2), once);
        assert_eq!(nl.elements.len(), nl2.elements.len());
        for (a, b) in nl.elements.iter().zip(&nl2.elements) {
            assert_eq!(a.kind, b.kind);
        }
        assert_eq!(nl.initial_conditions.len(), 2);
        assert_eq!(nl2.nodesets[0].node, "in");
        for (a, b) in nl.measures.iter().zip(&nl2.measures) {
            assert_eq!(a.kind, b.kind);
        }
    }
}
