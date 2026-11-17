//! Netlist writer: [`Netlist`] → SPICE text that both spx and ngspice accept.
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
    match &s.tran {
        Some(TranFn::Pulse(v)) => {
            let _ = write!(out, " pulse({})", list(v));
        }
        Some(TranFn::Sin(v)) => {
            let _ = write!(out, " sin({})", list(v));
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

fn signal(s: &Signal) -> String {
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

fn qualifiers(dir: CrossDir, n: Option<u32>, td: f64) -> String {
    let d = match dir {
        CrossDir::Rise => "rise",
        CrossDir::Fall => "fall",
        CrossDir::Cross => "cross",
    };
    let n = n.map_or("last".to_string(), |n| n.to_string());
    let mut s = format!(" {d}={n}");
    if td != 0.0 {
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
            dir,
            n,
            td,
        } => {
            let sep = if when { "=" } else { " val=" };
            format!(
                "{}{sep}{}{}",
                signal(s),
                num(*value),
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
    };
    let body = match &m.kind {
        MeasKind::TrigTarg { trig, targ } => {
            format!("trig {} targ {}", event(trig, false), event(targ, false))
        }
        MeasKind::When { event: e } => format!("when {}", event(e, true)),
        MeasKind::Find {
            signal: s,
            event: e,
        } => match e {
            MeasEvent::At { .. } => format!("find {} {}", signal(s), event(e, true)),
            MeasEvent::Cross { .. } => format!("find {} when {}", signal(s), event(e, true)),
        },
        MeasKind::Stat {
            stat,
            signal: s,
            from,
            to,
        } => {
            let st = match stat {
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
        AnalysisCard::Op { .. } => ".op".into(),
        AnalysisCard::Dc {
            source,
            start,
            stop,
            step,
            ..
        } => format!(".dc {source} {} {} {}", num(*start), num(*stop), num(*step)),
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
            ElementKind::Va {
                nodes,
                model,
                params: ps,
            } => {
                let _ = write!(l, "{name} {} {model}", nodes.join(" "));
                params(&mut l, ps);
            }
            ElementKind::Resistor { p, n, value }
            | ElementKind::Capacitor { p, n, value }
            | ElementKind::Inductor { p, n, value } => {
                let _ = write!(l, "{name} {p} {n} {}", num(*value));
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
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {c} {b} {em}");
                if let Some(s) = s {
                    let _ = write!(l, " {s}");
                }
                let _ = write!(l, " {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
            ElementKind::Mosfet {
                d,
                g,
                s,
                b,
                model,
                params: ps,
                off,
            } => {
                let _ = write!(l, "{name} {d} {g} {s} {b} {model}");
                params(&mut l, ps);
                if *off {
                    l += " off";
                }
            }
        }
        let _ = writeln!(out, "{l}");
    }
    for m in &nl.models {
        let mut l = format!(".model {} {} (", m.name, m.kind);
        let ps: Vec<String> = m
            .params
            .iter()
            .map(|(k, v)| format!("{k}={}", num(*v)))
            .collect();
        l += &ps.join(" ");
        l += ")";
        let _ = writeln!(out, "{l}");
    }
    if !nl.options.is_empty() {
        let mut l = String::from(".options");
        for o in &nl.options {
            match o.value {
                // The integration method is stored as a flag; ngspice spells it `method=`.
                None if o.name == "gear" || o.name == "trap" => {
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
    if !nl.saves.is_empty() {
        let _ = writeln!(out, ".save {}", nl.saves.join(" "));
    }
    for m in &nl.measures {
        let _ = writeln!(out, "{}", write_measure(m));
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
            crate::parse("t\n.model m unknown\n")
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
