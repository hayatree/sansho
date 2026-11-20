//! Vector expression syntax and pointwise/reduction operations.
//! Provenance: original implementation from the ngspice user manual's vectors and expressions.

use super::{Error, Result, State, error};
use sansho_sparse::c64 as C;
use std::{
    f64::consts::{PI, TAU},
    sync::Arc,
};

pub(super) const MAX_POINTS: usize = 1_000_000;
#[derive(Debug, Clone)]
pub(super) struct Vector {
    pub values: Vec<C>,
    pub complex: bool,
    pub scale: Option<Arc<[f64]>>,
}

impl Vector {
    pub fn real(values: impl IntoIterator<Item = f64>) -> Self {
        Self {
            values: values.into_iter().map(C::from).collect(),
            complex: false,
            scale: None,
        }
    }
    pub fn scalar(x: f64) -> Self {
        Self::real([x])
    }
    pub fn number(&self) -> Result<f64> {
        match self.values.as_slice() {
            [v] if v.im == 0. && v.re.is_finite() => Ok(v.re),
            _ => Err(error("expected one finite real value")),
        }
    }
    fn map(mut self, f: impl Fn(C) -> C) -> Self {
        self.values.iter_mut().for_each(|x| *x = f(*x));
        self
    }
}

#[derive(Debug)]
enum Expr {
    Name(String),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
}

#[derive(Clone, Copy)]
struct Token<'a> {
    word: &'a str,
    offset: usize,
}
/// Include the bracketed terminal/parameter name in one lexical atom.
pub(super) fn probe_end(s: &str, mut n: usize) -> Result<usize> {
    if s[..n].rsplit('.').next().is_some_and(|t| {
        ["v", "i", "temp", "res"]
            .iter()
            .any(|k| t.eq_ignore_ascii_case(k))
    }) && s[n..]
        .get(..6)
        .is_some_and(|s| s.eq_ignore_ascii_case("-sweep"))
    {
        n += 6;
    }
    let name = &s[..n];
    let stem = name.rsplit('.').next().unwrap_or(name);
    let close = if (stem.eq_ignore_ascii_case("v") || stem.eq_ignore_ascii_case("i"))
        && s[n..].starts_with('(')
    {
        Some(')')
    } else if name.contains('@') && s[n..].starts_with('[') {
        Some(']')
    } else {
        None
    };
    if let Some(end) = close {
        n += s[n..].find(end).ok_or_else(|| error("unclosed probe"))? + 1;
    }
    Ok(n)
}
fn lex(input: &str) -> Result<Vec<Token<'_>>> {
    let mut s = input;
    let mut toks = vec![];
    let special = |c: char| c.is_whitespace() || "+-*/^%<>=&|~!()[],".contains(c);
    while !s.trim_start().is_empty() {
        s = s.trim_start();
        let start = s;
        let first = s
            .chars()
            .next()
            .ok_or_else(|| error("missing expression"))?;
        let mut n = first.len_utf8();
        if !special(first) {
            n = s.find(special).unwrap_or(s.len());
            if (first.is_ascii_digit() || first == '.')
                && s[..n].ends_with(['e', 'E'])
                && s[n..].starts_with(['+', '-'])
                && s.as_bytes().get(n + 1).is_some_and(u8::is_ascii_digit)
            {
                n += 1;
                n += s[n..].find(special).unwrap_or(s.len() - n);
            }
            n = probe_end(s, n)?;
        } else if ["<=", ">=", "<>", "!=", "==", "&&", "||", "**"]
            .iter()
            .any(|op| s.starts_with(op))
        {
            n = 2;
        }
        toks.push(Token {
            word: &start[..n],
            offset: input.len() - start.len(),
        });
        if toks.len() > 256 {
            return Err(error("expression token limit exceeded"));
        }
        s = &s[n..];
    }
    Ok(toks)
}
struct Parser<'a> {
    tokens: &'a [Token<'a>],
    i: usize,
}
impl<'a> Parser<'a> {
    fn peek(&self) -> &'a str {
        self.tokens.get(self.i).map_or("", |t| t.word)
    }
    fn eat(&mut self, word: &str) -> bool {
        if self.peek() == word {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn need(&mut self, word: &str) -> Result<()> {
        if self.eat(word) {
            Ok(())
        } else {
            Err(error(format!("expected {word}")))
        }
    }
    fn parse(&mut self, min: u8, depth: usize) -> Result<Expr> {
        if depth >= 48 {
            return Err(error("expression nesting limit exceeded"));
        }
        let name = self.peek().to_ascii_lowercase();
        self.i += 1;
        let mut e = match name.as_str() {
            "" | ")" | "]" | "," => return Err(error("expected vector expression")),
            "+" | "-" | "!" | "~" | "not" => Expr::Unary(name, Box::new(self.parse(7, depth + 1)?)),
            "(" => {
                let e = self.parse(0, depth + 1)?;
                self.need(")")?;
                e
            }
            _ if self.eat("(") => {
                let mut args = vec![self.parse(0, depth + 1)?];
                while self.eat(",") {
                    args.push(self.parse(0, depth + 1)?);
                }
                self.need(")")?;
                Expr::Call(name, args)
            }
            _ => Expr::Name(name),
        };
        loop {
            if self.eat("[") {
                let a = self.parse(0, depth + 1)?;
                let b = if self.eat(",") {
                    Some(Box::new(self.parse(0, depth + 1)?))
                } else {
                    None
                };
                self.need("]")?;
                e = Expr::Index(Box::new(e), Box::new(a), b);
                continue;
            }
            let op = self.peek().to_ascii_lowercase();
            let prec = match op.as_str() {
                "or" | "|" | "||" => 1,
                "and" | "&" | "&&" => 2,
                "=" | "==" | "eq" | "<>" | "!=" | "ne" | "<" | ">" | "<=" | ">=" | "lt" | "gt"
                | "le" | "ge" => 3,
                "+" | "-" => 4,
                "*" | "/" | "%" => 5,
                "^" | "**" => 7,
                _ => break,
            };
            if prec < min {
                break;
            }
            self.i += 1;
            let rhs = self.parse(if prec == 7 { prec } else { prec + 1 }, depth + 1)?;
            e = Expr::Binary(op, Box::new(e), Box::new(rhs));
        }
        Ok(e)
    }
}

pub(super) fn expressions(state: &mut State<'_>, input: &str) -> Result<Vec<(String, Vector)>> {
    let tokens = lex(input)?;
    let mut p = Parser {
        tokens: &tokens,
        i: 0,
    };
    let mut out = vec![];
    while let Some(t) = tokens.get(p.i) {
        let expr = p.parse(0, 0)?;
        let end = tokens.get(p.i).map_or(input.len(), |x| x.offset);
        out.push((input[t.offset..end].trim().to_string(), expr.eval(state)?));
    }
    Ok(out)
}

pub(super) fn eval(state: &mut State<'_>, input: &str) -> Result<Vector> {
    let tokens = lex(input)?;
    let mut p = Parser {
        tokens: &tokens,
        i: 0,
    };
    let expr = p.parse(0, 0)?;
    if p.i != tokens.len() {
        return Err(error(format!("unexpected token {}", p.peek())));
    }
    expr.eval(state)
}

pub(super) fn integer(n: f64, max: usize) -> Result<usize> {
    if n >= 0. && n <= max as f64 && n.fract() == 0. {
        Ok(n as usize)
    } else {
        Err(error(format!("expected an integer in 0..={max}")))
    }
}

impl Expr {
    fn eval(&self, state: &mut State<'_>) -> Result<Vector> {
        match self {
            Self::Name(n) => state.lookup(n),
            Self::Unary(op, x) => {
                let v = x.eval(state)?;
                match op.as_str() {
                    "+" => Ok(v),
                    "-" => Ok(v.map(|z| -z)),
                    _ => Ok(Vector::real(
                        v.values.iter().map(|z| f64::from(*z == C::default())),
                    )),
                }
            }
            Self::Binary(op, a, b) => binary(op, a.eval(state)?, b.eval(state)?),
            Self::Index(v, a, b) => {
                let v = v.eval(state)?;
                let index = |x: &Expr, s: &mut State<'_>| {
                    integer(x.eval(s)?.number()?, v.values.len().saturating_sub(1))
                };
                let a = index(a, state)?;
                let b = match b {
                    Some(x) => index(x, state)?,
                    None => a,
                };
                let mut values = v
                    .values
                    .get(a.min(b)..=a.max(b))
                    .ok_or_else(|| error("index outside vector"))?
                    .to_vec();
                if b < a {
                    values.reverse()
                }
                Ok(Vector {
                    values,
                    complex: v.complex,
                    scale: None,
                })
            }
            Self::Call(f, args) => {
                let v = args[0].eval(state)?;
                match args.as_slice() {
                    [_] => function(state, f, v),
                    [_, b] if matches!(f.as_str(), "max" | "min") => binary(f, v, b.eval(state)?),
                    _ => Err(error(format!("invalid arguments to {f}"))),
                }
            }
        }
    }
}

fn log(z: C) -> C {
    C::new(z.norm().ln(), z.im.atan2(z.re))
}
fn exp(z: C) -> C {
    C::new(z.im.cos(), z.im.sin()) * z.re.exp()
}
fn sqrt(z: C) -> C {
    if z.im == 0. && z.re >= 0. {
        C::from(z.re.sqrt())
    } else {
        exp(log(z) * 0.5)
    }
}
pub(super) fn binary(op: &str, a: Vector, b: Vector) -> Result<Vector> {
    let n = a.values.len().max(b.values.len());
    if a.values.is_empty()
        || b.values.is_empty()
        || (a.values.len() != b.values.len() && a.values.len() != 1 && b.values.len() != 1)
    {
        return Err(error("incompatible vector lengths"));
    }
    let mut complex = a.complex || b.complex;
    let mut values = Vec::with_capacity(n);
    for i in 0..n {
        let x = a.values[i % a.values.len()];
        let y = b.values[i % b.values.len()];
        let z = match op {
            "+" => x + y,
            "-" => x - y,
            "*" => x * y,
            "/" => x / y,
            "^" | "**" if x.im == 0. && y.im == 0. && (x.re >= 0. || y.re.fract() == 0.) => {
                C::from(x.re.powf(y.re))
            }
            "^" | "**" => exp(log(x) * y),
            "%" => C::from(x.re % y.re),
            "max" => C::from(x.re.max(y.re)),
            "min" => C::from(x.re.min(y.re)),
            _ => {
                complex = false;
                C::from(f64::from(match op {
                    "=" | "==" | "eq" => x == y,
                    "!=" | "<>" | "ne" => x != y,
                    ">" | "gt" => x.re > y.re,
                    "<" | "lt" => x.re < y.re,
                    ">=" | "ge" => x.re >= y.re,
                    "<=" | "le" => x.re <= y.re,
                    "&" | "&&" | "and" => x != C::default() && y != C::default(),
                    "|" | "||" | "or" => x != C::default() || y != C::default(),
                    _ => return Err(error(format!("unknown operator {op}"))),
                }))
            }
        };
        complex |= z.im != 0.;
        values.push(z);
    }
    Ok(Vector {
        values,
        complex,
        scale: a.scale.or(b.scale),
    })
}

pub(super) fn resample(v: &Vector, target: &[f64]) -> Result<Vector> {
    let xs = v
        .scale
        .as_deref()
        .ok_or_else(|| error("interpolation needs a source scale"))?;
    if xs.len() != v.values.len() || xs.is_empty() || xs.windows(2).any(|p| p[1] <= p[0]) {
        return Err(error("interpolation needs an increasing scale"));
    }
    let values = target
        .iter()
        .map(|x| {
            let i = xs.partition_point(|a| a < x);
            if i == 0 {
                v.values[0]
            } else if i == xs.len() {
                v.values[i - 1]
            } else {
                v.values[i - 1]
                    + (v.values[i] - v.values[i - 1]) * ((*x - xs[i - 1]) / (xs[i] - xs[i - 1]))
            }
        })
        .collect();
    Ok(Vector {
        values,
        complex: v.complex,
        scale: Some(target.into()),
    })
}

fn function(state: &mut State<'_>, f: &str, mut v: Vector) -> Result<Vector> {
    let j = C::new(0., 1.);
    let n = v.values.len();
    if n == 0 {
        return Err(error("empty vector"));
    }
    match f {
        "vector" | "unitvec" => {
            let size = integer(v.number()?, MAX_POINTS)?;
            if size == 0 {
                return Err(error("zero vector length"));
            }
            return Ok(Vector::real(
                (0..size).map(|i| if f == "vector" { i as f64 } else { 1. }),
            ));
        }
        "length" => return Ok(Vector::scalar(n as f64)),
        "vecmax" | "max" | "vecmin" | "min" => {
            let result = v
                .values
                .iter()
                .map(|z| z.re)
                .reduce(if matches!(f, "vecmax" | "max") {
                    f64::max
                } else {
                    f64::min
                })
                .ok_or_else(|| error("empty vector"))?;
            return Ok(Vector::scalar(result));
        }
        "mean" => {
            let z = v.values.iter().copied().sum::<C>() / n as f64;
            v.values = vec![z];
            v.scale = None;
            return Ok(v);
        }
        "avg" => {
            let mut total = C::default();
            for (i, z) in v.values.iter_mut().enumerate() {
                total += *z;
                *z = total / (i + 1) as f64
            }
            return Ok(v);
        }
        "norm" => {
            let max = v.values.iter().map(|z| z.norm()).fold(0., f64::max);
            return Ok(if max == 0. { v } else { v.map(|z| z / max) });
        }
        "interpolate" => return resample(&v, &state.current_scale()?),
        "deriv" | "integ" => {
            let xs = v
                .scale
                .as_deref()
                .ok_or_else(|| error("function needs a scale"))?;
            if xs.len() != n || n < 2 || xs.windows(2).any(|x| x[1] == x[0]) {
                return Err(error("invalid scale for calculus"));
            }
            let mut total = C::default();
            v.values = (0..n)
                .map(|i| {
                    if f == "integ" {
                        if i > 0 {
                            total += (v.values[i] + v.values[i - 1]) * ((xs[i] - xs[i - 1]) * 0.5)
                        }
                        total
                    } else {
                        let a = i.saturating_sub(1);
                        let b = (i + 1).min(n - 1);
                        (v.values[b] - v.values[a]) / (xs[b] - xs[a])
                    }
                })
                .collect();
            return Ok(v);
        }
        "rnd" => {
            let seed = state
                .seed
                .as_mut()
                .ok_or_else(|| error("rnd requires setseed or set seed"))?;
            for z in &mut v.values {
                *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *z = C::from(((*seed >> 11) as f64 / ((1u64 << 53) as f64) * z.re).floor())
            }
            v.complex = false;
            return Ok(v);
        }
        _ => {}
    }
    let radians_per_unit = if state
        .vars
        .get("units")
        .is_some_and(|s| s.eq_ignore_ascii_case("degrees"))
    {
        PI / 180.
    } else {
        1.
    };
    let mut last = 0.;
    let mut offset = 0.;
    for z in &mut v.values {
        let x = *z;
        let real = match f {
            "mag" | "abs" => Some(x.norm()),
            "real" => Some(x.re),
            "imag" => Some(x.im),
            "db" => Some(20. * x.norm().log10()),
            "ph" | "cph" => {
                let phase = x.im.atan2(x.re);
                if f == "cph" {
                    let d = phase - last;
                    if d > PI {
                        offset -= TAU
                    } else if d < -PI {
                        offset += TAU
                    }
                    last = phase;
                }
                Some((phase + offset) / radians_per_unit)
            }
            "floor" => Some(x.re.floor()),
            "ceil" => Some(x.re.ceil()),
            "round" => Some(x.re.round()),
            "sgn" => Some(if x.re == 0. { 0. } else { x.re.signum() }),
            "pos" => Some(f64::from(x.re > 0.)),
            _ => None,
        };
        if let Some(r) = real {
            *z = C::from(r);
            v.complex = false;
            continue;
        }
        let sin = |x: C| C::new(x.re.sin() * x.im.cosh(), x.re.cos() * x.im.sinh());
        let cos = |x: C| C::new(x.re.cos() * x.im.cosh(), -x.re.sin() * x.im.sinh());
        *z = match f {
            "j" => j * x,
            "sqrt" => sqrt(x),
            "exp" => exp(x),
            "ln" => log(x),
            "log" | "log10" => log(x) / std::f64::consts::LN_10,
            "sin" => sin(x * radians_per_unit),
            "cos" => cos(x * radians_per_unit),
            "tan" => sin(x * radians_per_unit) / cos(x * radians_per_unit),
            "sinh" => (exp(x) - exp(-x)) * 0.5,
            "cosh" => (exp(x) + exp(-x)) * 0.5,
            "tanh" => (exp(x) - exp(-x)) / (exp(x) + exp(-x)),
            "atan" if x.im == 0. => C::from(x.re.atan()),
            "atan" => (log(1. + j * x) - log(1. + (-j) * x)) / (j * 2.),
            "asin" if x.im == 0. && x.re.abs() <= 1. => C::from(x.re.asin()),
            "acos" if x.im == 0. && x.re.abs() <= 1. => C::from(x.re.acos()),
            "asin" | "acos" => {
                let a = -j * log(j * x + sqrt(C::from(1.) - x * x));
                if f == "asin" { a } else { C::from(PI / 2.) - a }
            }
            "asinh" if x.im == 0. => C::from(x.re.asinh()),
            "asinh" => log(x + sqrt(1. + x * x)),
            "acosh" => log(x + sqrt(x * x - C::from(1.))),
            "atanh" => (log(1. + x) - log(1. + (-x))) * 0.5,
            _ => return Err(Error::error("E-CONTROL", format!("unknown function {f}"))),
        };
        if matches!(f, "asin" | "acos" | "atan") {
            *z = *z / radians_per_unit;
        }
        v.complex |= z.im != 0. || f == "j";
    }
    Ok(v)
}
