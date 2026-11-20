//! Parser for `.meas` cards.
//!
//! Supported forms (case-insensitive):
//!
//! ```text
//! .meas tran name TRIG sig VAL=v [RISE|FALL|CROSS=n|LAST] [TD=t] TARG sig VAL=v [...]
//! .meas tran name TRIG AT=t TARG ...
//! .meas tran name WHEN sig=v [RISE|FALL|CROSS=n|LAST] [TD=t]
//! .meas tran name FIND sig WHEN sig=v [...]
//! .meas tran name FIND sig AT=t
//! .meas tran name MAX|MIN|PP|AVG|RMS|INTEG sig [FROM=t1] [TO=t2]
//! ```
//!
//! Signals: `v(n)`, `v(n1,n2)`, `i(vsrc)` and, for AC, `vm vdb vp vr vi` / `im idb ip ir ii`.
//!
//! Provenance: original work; the card syntax follows the measurement statements commonly
//! documented for SPICE-family simulators. See `docs/PROVENANCE.md`.

use sansho_diag::{Diagnostic, Span};

use crate::lexer::{Line, Token};
use crate::number::parse_number;
use crate::{CrossDir, MeasAnalysis, MeasEvent, MeasKind, MeasStat, MeasureCard, Signal, SignalFn};

const KEYWORDS: &[&str] = &[
    "trig", "targ", "val", "rise", "fall", "cross", "td", "at", "when", "find", "from", "to",
];

struct Cursor<'a> {
    t: &'a [Token],
    i: usize,
    card: Span,
}

fn error(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-PARSE-MEAS", msg).with_span(span)
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<String> {
        self.t.get(self.i).map(Token::lower)
    }
    fn span(&self) -> Span {
        self.t.get(self.i).map_or(self.card, |t| t.span)
    }
    fn next(&mut self, what: &str) -> Result<&'a Token, Diagnostic> {
        let t = self
            .t
            .get(self.i)
            .ok_or_else(|| error(self.card, format!("missing {what}")))?;
        self.i += 1;
        Ok(t)
    }
    fn number(&mut self, what: &str) -> Result<f64, Diagnostic> {
        let t = self.next(what)?;
        parse_number(&t.text).ok_or_else(|| {
            error(
                t.span,
                format!("expected a number for {what}, found `{}`", t.text),
            )
        })
    }
    fn eat_eq(&mut self) -> Result<(), Diagnostic> {
        let t = self.next("`=`")?;
        if t.is_eq() {
            Ok(())
        } else {
            Err(error(t.span, format!("expected `=`, found `{}`", t.text)))
        }
    }
    /// `key = number`
    fn kv(&mut self, key: &str) -> Result<f64, Diagnostic> {
        self.i += 1;
        self.eat_eq()?;
        self.number(key)
    }

    fn signal(&mut self) -> Result<Signal, Diagnostic> {
        let f = self.next("a signal")?;
        let name = f.lower();
        if name.starts_with('@')
            || name.starts_with("onoise.")
            || matches!(name.as_str(), "onoise_spectrum" | "inoise_spectrum")
        {
            return Ok(Signal {
                bare: true,
                func: SignalFn::Plain,
                current: false,
                pos: name,
                neg: None,
            });
        }
        let (current, func) = match name.as_str() {
            "v" => (false, SignalFn::Plain),
            "vm" => (false, SignalFn::Mag),
            "vdb" => (false, SignalFn::Db),
            "vp" => (false, SignalFn::Phase),
            "vr" => (false, SignalFn::Re),
            "vi" => (false, SignalFn::Im),
            "i" => (true, SignalFn::Plain),
            "im" => (true, SignalFn::Mag),
            "idb" => (true, SignalFn::Db),
            "ip" => (true, SignalFn::Phase),
            "ir" => (true, SignalFn::Re),
            "ii" => (true, SignalFn::Im),
            _ => {
                return Err(error(
                    f.span,
                    format!(
                        "expected a signal such as v(node) or i(vsrc), found `{}`",
                        f.text
                    ),
                ));
            }
        };
        let first = self.next("a node name")?;
        let pos = first.lower();
        let pos = if pos == "gnd" {
            crate::GROUND.into()
        } else {
            pos
        };
        // An optional second node, unless the next token starts a keyword or `=`.
        let neg = match self.t.get(self.i) {
            Some(t)
                if !current
                    && !first.ends_group
                    && !t.is_eq()
                    && (t.ends_group
                        || (!KEYWORDS.contains(&t.lower().as_str())
                            && !self.t.get(self.i + 1).is_some_and(Token::is_eq))) =>
            {
                self.i += 1;
                let n = t.lower();
                Some(if n == "gnd" { crate::GROUND.into() } else { n })
            }
            _ => None,
        };
        Ok(Signal {
            bare: false,
            func,
            current,
            pos,
            neg,
        })
    }

    /// Optional `RISE|FALL|CROSS=n|LAST` and `TD=t` qualifiers.
    fn qualifiers(&mut self) -> Result<(CrossDir, Option<u32>, Option<f64>), Diagnostic> {
        let (mut dir, mut n, mut td) = (CrossDir::Cross, Some(1), None);
        let mut seen_dir = false;
        while let Some(k) = self.peek() {
            match k.as_str() {
                "rise" | "fall" | "cross" => {
                    if seen_dir {
                        return Err(error(
                            self.span(),
                            "only one of RISE, FALL, CROSS is allowed",
                        ));
                    }
                    seen_dir = true;
                    dir = match k.as_str() {
                        "rise" => CrossDir::Rise,
                        "fall" => CrossDir::Fall,
                        _ => CrossDir::Cross,
                    };
                    self.i += 1;
                    self.eat_eq()?;
                    let t = self.next("a crossing number")?;
                    n = if t.lower() == "last" {
                        None
                    } else {
                        match parse_number(&t.text) {
                            Some(v) if v >= 1.0 && v <= f64::from(u32::MAX) && v.fract() == 0.0 => {
                                Some(v as u32)
                            }
                            _ => {
                                return Err(error(
                                    t.span,
                                    format!(
                                        "expected a positive integer or LAST, found `{}`",
                                        t.text
                                    ),
                                ));
                            }
                        }
                    };
                }
                "td" if td.is_none() => td = Some(self.kv("TD")?),
                _ => break,
            }
        }
        Ok((dir, n, td))
    }

    /// `sig VAL=v [qualifiers]` (TRIG/TARG) or `AT=t`.
    fn trig_event(&mut self) -> Result<MeasEvent, Diagnostic> {
        if self.peek().as_deref() == Some("at") {
            return Ok(MeasEvent::At { at: self.kv("AT")? });
        }
        let signal = self.signal()?;
        if self.peek().as_deref() != Some("val") {
            return Err(error(self.span(), "expected VAL=<threshold>"));
        }
        let value = self.kv("VAL")?;
        let (dir, n, td) = self.qualifiers()?;
        Ok(MeasEvent::Cross {
            signal,
            value,
            reference: None,
            dir,
            n,
            td,
        })
    }

    /// `sig=v [qualifiers]` (WHEN) or `AT=t`.
    fn when_event(&mut self) -> Result<MeasEvent, Diagnostic> {
        if self.peek().as_deref() == Some("at") {
            return Ok(MeasEvent::At { at: self.kv("AT")? });
        }
        let signal = self.signal()?;
        self.eat_eq()?;
        let (value, reference) = if self
            .t
            .get(self.i)
            .and_then(|t| parse_number(&t.text))
            .is_some()
        {
            (self.number("the threshold")?, None)
        } else {
            (0., Some(self.signal()?))
        };
        let (dir, n, td) = self.qualifiers()?;
        Ok(MeasEvent::Cross {
            signal,
            value,
            reference,
            dir,
            n,
            td,
        })
    }
}

/// Parse one `.meas` line.
pub(crate) fn parse_meas(line: &Line) -> Result<MeasureCard, Diagnostic> {
    let span = line.span();
    let t = &line.tokens;
    if t.len() < 4 {
        return Err(
            error(span, ".meas needs an analysis, a name and a definition")
                .with_note("usage: .meas tran name MAX v(out) FROM=0 TO=1u"),
        );
    }
    let analysis = match t[1].lower().as_str() {
        "noise" => MeasAnalysis::Noise,
        "tran" => MeasAnalysis::Tran,
        "ac" => MeasAnalysis::Ac,
        "dc" => MeasAnalysis::Dc,
        _ => {
            return Err(error(
                t[1].span,
                format!(
                    "unsupported .meas analysis `{}` (use tran, ac, dc or noise)",
                    t[1].text
                ),
            ));
        }
    };
    let name = t[2].lower();
    let mut c = Cursor {
        t,
        i: 3,
        card: span,
    };
    let head = c.next("a measurement keyword")?.lower();
    let kind = match head.as_str() {
        "trig" => {
            let trig = c.trig_event()?;
            if c.peek().as_deref() != Some("targ") {
                return Err(error(
                    c.span(),
                    "expected TARG after the TRIG specification",
                ));
            }
            c.i += 1;
            let targ = c.trig_event()?;
            MeasKind::TrigTarg { trig, targ }
        }
        "when" => MeasKind::When {
            event: c.when_event()?,
        },
        "param" => {
            c.eat_eq()?;
            let (_, expression) = line
                .raw
                .split_once('=')
                .ok_or_else(|| error(span, "expected PARAM=expression"))?;
            crate::parse_expr(expression).map_err(|e| error(span, e))?;
            c.i = t.len();
            MeasKind::Param {
                expression: expression.trim().into(),
            }
        }
        "deriv" | "derivative" | "find" => {
            let signal = c.signal()?;
            let event = match c.peek().as_deref() {
                Some("when") => {
                    c.i += 1;
                    c.when_event()?
                }
                Some("at") => MeasEvent::At { at: c.kv("AT")? },
                _ => {
                    return Err(error(
                        c.span(),
                        "expected WHEN or AT after the measured signal",
                    ));
                }
            };
            if head == "find" {
                MeasKind::Find { signal, event }
            } else {
                MeasKind::Deriv { signal, event }
            }
        }
        "max_at" | "min_at" | "max" | "min" | "pp" | "avg" | "rms" | "integ" | "integral" => {
            let stat = match head.as_str() {
                "max_at" => MeasStat::MaxAt,
                "min_at" => MeasStat::MinAt,
                "max" => MeasStat::Max,
                "min" => MeasStat::Min,
                "pp" => MeasStat::Pp,
                "avg" => MeasStat::Avg,
                "rms" => MeasStat::Rms,
                _ => MeasStat::Integ,
            };
            let signal = c.signal()?;
            let (mut from, mut to) = (None, None);
            while let Some(k) = c.peek() {
                match k.as_str() {
                    "from" if from.is_none() => from = Some(c.kv("FROM")?),
                    "to" if to.is_none() => to = Some(c.kv("TO")?),
                    _ => break,
                }
            }
            MeasKind::Stat {
                stat,
                signal,
                from,
                to,
            }
        }
        other => {
            return Err(
                error(t[3].span, format!("unsupported .meas form `{other}`"))
                    .with_note("supported: TRIG/TARG, WHEN, FIND, DERIV, PARAM, MAX, MAX_AT, MIN, MIN_AT, PP, AVG, RMS, INTEG"),
            );
        }
    };
    if let Some(extra) = t.get(c.i) {
        return Err(error(
            extra.span,
            format!("unexpected token `{}`", extra.text),
        ));
    }
    Ok(MeasureCard {
        name,
        analysis,
        kind,
        span,
    })
}

/// Parse a batch print request using the measurement signal grammar.
pub(crate) fn parse_print(line: &Line) -> Result<crate::PrintCard, Diagnostic> {
    let span = line.span();
    let t = &line.tokens;
    let analysis = t.get(1).map(Token::lower).unwrap_or_default();
    if !matches!(analysis.as_str(), "tran" | "ac" | "dc" | "op" | "noise") || t.len() < 3 {
        return Err(
            Diagnostic::error("E-PRINT", "expected .print analysis signal ...").with_span(span),
        );
    }
    let mut c = Cursor {
        t,
        i: 2,
        card: span,
    };
    let mut signals = Vec::new();
    while c.i < t.len() {
        signals.push(c.signal()?);
    }
    Ok(crate::PrintCard {
        analysis,
        signals,
        span,
    })
}

#[cfg(test)]
mod tests {
    use crate::{MeasEvent, MeasKind, parse};

    #[test]
    fn parses_measure_forms() {
        let src = "t
V1 a 0 1
R1 a b 1k
C1 b 0 1n
.meas tran tdelay TRIG v(a) VAL=0.5 RISE=1 TARG v(b) VAL=0.5 RISE=1
.meas tran t2 WHEN v(b)=0.9 CROSS=LAST TD=1u
.meas tran vb FIND v(a,b) AT=2u
.meas ac gain MAX vdb(b) FROM=1k TO=1meg
.meas dc x FIND i(v1) WHEN v(b)=0.3 FALL=2
";
        let nl = parse(src).unwrap_or_else(|d| panic!("{d:#?}"));
        assert_eq!(nl.measures.len(), 5);
        assert!(matches!(&nl.measures[0].kind, MeasKind::TrigTarg { .. }));
        match &nl.measures[1].kind {
            MeasKind::When {
                event: MeasEvent::Cross { n, td, .. },
            } => {
                assert_eq!(*n, None);
                assert_eq!(*td, Some(1e-6));
            }
            k => panic!("{k:?}"),
        }
        match &nl.measures[2].kind {
            MeasKind::Find { signal, .. } => assert_eq!(signal.neg.as_deref(), Some("b")),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn rejects_malformed_cards() {
        let err = parse("t\nR1 a 0 1\n.meas tran x TRIG v(a) RISE=1\n")
            .err()
            .unwrap_or_default();
        assert_eq!(err[0].code, "E-PARSE-MEAS");
    }
}
