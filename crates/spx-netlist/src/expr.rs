//! Parameter expressions: `{...}` / `'...'` values, `.param` and `.func`.
//!
//! Grammar (lowest to highest precedence): `c ? a : b`, `||`, `&&`, `== !=`, `< <= > >=`,
//! `+ -`, `* / %`, unary `- + !`, `^` / `**` (right associative), primary (number with SPICE
//! scale suffix, identifier, function call, parenthesized expression). Booleans are 1 / 0.
//!
//! Provenance: original work. Operator set and built-in functions follow commonly documented
//! SPICE parameter-expression conventions (e.g. the ngspice user manual's description of
//! `.param` expressions). See `docs/PROVENANCE.md`.

use std::collections::HashMap;

use crate::number::parse_number;

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Number.
    Num(f64),
    /// Parameter reference (lower-cased).
    Var(String),
    /// Unary operator.
    Unary(char, Box<Expr>),
    /// Binary operator (`+ - * / % ^ < > l(<=) g(>=) = (==) n(!=) & (&&) | (||)`).
    Binary(char, Box<Expr>, Box<Expr>),
    /// `c ? a : b`.
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Function call (lower-cased name).
    Call(String, Vec<Expr>),
}

/// A user function from `.func name(args) = expr`.
#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    /// Argument names (lower-cased).
    pub args: Vec<String>,
    /// Body.
    pub body: Expr,
}

/// User functions by lower-cased name.
pub type Funcs = HashMap<String, Func>;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(&'static str),
}

const OPS: &[&str] = &[
    "**", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "^", "<", ">", "!", "?",
    ":", "(", ")", ",",
];

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            // Mantissa, optional exponent, then any letters (scale suffix / unit).
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            let text = &s[start..i];
            out.push(Tok::Num(
                parse_number(text).ok_or_else(|| format!("invalid number `{text}`"))?,
            ));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Id(s[start..i].to_ascii_lowercase()));
        } else if let Some(op) = OPS.iter().find(|op| s[i..].starts_with(**op)) {
            out.push(Tok::Op(op));
            i += op.len();
        } else {
            return Err(format!(
                "unexpected character `{}`",
                s[i..].chars().next().unwrap_or('?')
            ));
        }
    }
    Ok(out)
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek_op(&self) -> Option<&'static str> {
        match self.t.get(self.i) {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn eat(&mut self, op: &str) -> bool {
        if self.peek_op() == Some(op) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, op: &str) -> Result<(), String> {
        if self.eat(op) {
            Ok(())
        } else {
            Err(format!("expected `{op}`"))
        }
    }

    fn cond(&mut self) -> Result<Expr, String> {
        let c = self.binary(0)?;
        if self.eat("?") {
            let a = self.cond()?;
            self.expect(":")?;
            let b = self.cond()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    /// Precedence climbing over the binary operator levels.
    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: &[&[(&str, char)]] = &[
            &[("||", '|')],
            &[("&&", '&')],
            &[("==", '='), ("!=", 'n')],
            &[("<=", 'l'), (">=", 'g'), ("<", '<'), (">", '>')],
            &[("+", '+'), ("-", '-')],
            &[("*", '*'), ("/", '/'), ("%", '%')],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for &(text, op) in LEVELS[level] {
                if self.eat(text) {
                    let rhs = self.binary(level + 1)?;
                    lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        for op in ['-', '+', '!'] {
            if self.eat(&op.to_string()) {
                return Ok(Expr::Unary(op, Box::new(self.unary()?)));
            }
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.primary()?;
        if self.eat("^") || self.eat("**") {
            let exp = self.unary()?;
            return Ok(Expr::Binary('^', Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Num(v)) => {
                self.i += 1;
                Ok(Expr::Num(v))
            }
            Some(Tok::Id(name)) => {
                self.i += 1;
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.cond()?);
                            if self.eat(")") {
                                break;
                            }
                            self.expect(",")?;
                        }
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(Tok::Op("(")) => {
                self.i += 1;
                let e = self.cond()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Op(o)) => Err(format!("unexpected `{o}`")),
            None => Err("unexpected end of expression".into()),
        }
    }
}

/// Whether `s` is a single numeric literal (optionally signed, with scale suffix).
pub fn is_number_literal(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    matches!(tokenize(body).as_deref(), Ok([Tok::Num(_)]))
}

/// Strip one level of `{...}` or `'...'` delimiters.
pub fn strip_delims(s: &str) -> &str {
    let t = s.trim();
    if (t.starts_with('{') && t.ends_with('}'))
        || (t.len() >= 2 && t.starts_with('\'') && t.ends_with('\''))
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// Parse an expression (delimiters `{}` / `''` are accepted).
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let text = strip_delims(s).trim();
    let text = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(text);
    let mut p = P {
        t: tokenize(text)?,
        i: 0,
    };
    let e = p.cond()?;
    if p.i != p.t.len() {
        return Err("unexpected trailing input".into());
    }
    Ok(e)
}

fn builtin(name: &str, a: &[f64]) -> Option<Result<f64, String>> {
    let arity = |n: usize| -> Result<(), String> {
        if a.len() == n {
            Ok(())
        } else {
            Err(format!("`{name}` expects {n} argument(s), got {}", a.len()))
        }
    };
    let one = |f: fn(f64) -> f64| arity(1).map(|()| f(a[0]));
    let two = |f: fn(f64, f64) -> f64| arity(2).map(|()| f(a[0], a[1]));
    Some(match name {
        "sqrt" => one(f64::sqrt),
        "exp" => one(f64::exp),
        "ln" | "log" => one(f64::ln),
        "log10" => one(f64::log10),
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "sinh" => one(f64::sinh),
        "cosh" => one(f64::cosh),
        "tanh" => one(f64::tanh),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "int" => one(f64::trunc),
        "nint" | "round" => one(f64::round),
        "sgn" | "sign" => one(|x| {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }),
        "pow" | "pwr" => two(f64::powf),
        "atan2" => two(f64::atan2),
        "min" => two(f64::min),
        "max" => two(f64::max),
        "gauss" | "agauss" => arity(3).and_then(|()| {
            if a[1] == 0. || a[2] == 0. {
                Ok(a[0])
            } else {
                Err(format!(
                    "{name} with active statistical variation is unsupported"
                ))
            }
        }),
        "aunif" | "unif" => arity(2).and_then(|()| {
            if a[1] == 0. {
                Ok(a[0])
            } else {
                Err(format!(
                    "{name} with active statistical variation is unsupported"
                ))
            }
        }),
        "limit" => arity(3).map(|()| a[0].clamp(a[1].min(a[2]), a[1].max(a[2]))),
        _ => return None,
    })
}

/// Maximum nesting of user function calls (guards against recursion).
const MAX_DEPTH: usize = 64;

/// Evaluate `e`; `var` resolves parameter names.
pub fn eval(
    e: &Expr,
    var: &dyn Fn(&str) -> Result<f64, String>,
    funcs: &Funcs,
) -> Result<f64, String> {
    eval_depth(e, var, funcs, 0)
}

fn eval_depth(
    e: &Expr,
    var: &dyn Fn(&str) -> Result<f64, String>,
    funcs: &Funcs,
    depth: usize,
) -> Result<f64, String> {
    let ev = |x: &Expr| eval_depth(x, var, funcs, depth);
    let b = |x: bool| if x { 1.0 } else { 0.0 };
    Ok(match e {
        Expr::Num(v) => *v,
        Expr::Var(n) => match var(n) {
            Ok(v) => v,
            Err(_) if n == "pi" => std::f64::consts::PI,
            Err(err) => return Err(err),
        },
        Expr::Unary(op, x) => {
            let v = ev(x)?;
            match op {
                '-' => -v,
                '!' => b(v == 0.0),
                _ => v,
            }
        }
        Expr::Binary(op, l, r) => {
            let (x, y) = (ev(l)?, ev(r)?);
            match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => x / y,
                '%' => x % y,
                '^' => x.powf(y),
                '<' => b(x < y),
                '>' => b(x > y),
                'l' => b(x <= y),
                'g' => b(x >= y),
                '=' => b(x == y),
                'n' => b(x != y),
                '&' => b(x != 0.0 && y != 0.0),
                _ => b(x != 0.0 || y != 0.0),
            }
        }
        Expr::Cond(c, a, z) => {
            if ev(c)? != 0.0 {
                ev(a)?
            } else {
                ev(z)?
            }
        }
        Expr::Call(name, args) => {
            let vals = args.iter().map(ev).collect::<Result<Vec<_>, _>>()?;
            if let Some(f) = funcs.get(name) {
                if depth >= MAX_DEPTH {
                    return Err(format!("function `{name}` nests too deeply (recursion?)"));
                }
                if f.args.len() != vals.len() {
                    return Err(format!(
                        "`{name}` expects {} argument(s), got {}",
                        f.args.len(),
                        vals.len()
                    ));
                }
                let local = |n: &str| match f.args.iter().position(|a| a == n) {
                    Some(i) => Ok(vals[i]),
                    None => var(n),
                };
                return eval_depth(&f.body, &local, funcs, depth + 1);
            }
            if matches!(
                (name.as_str(), vals.len()),
                ("agauss" | "gauss", 3) | ("aunif" | "unif", 2)
            ) {
                let switches = [var("mc_mm_switch"), var("mc_pr_switch")];
                if switches.iter().any(|v| matches!(v, Ok(0.0)))
                    && switches.iter().all(|v| match v {
                        Ok(x) => *x == 0.,
                        Err(e) => e.starts_with("unknown parameter"),
                    })
                {
                    return Ok(vals[0]);
                }
            }
            match builtin(name, &vals) {
                Some(r) => r?,
                None => return Err(format!("unknown function `{name}`")),
            }
        }
    })
}

/// Evaluate a delimited or bare expression with no parameters.
pub fn eval_const(s: &str) -> Result<f64, String> {
    eval(
        &parse_expr(s)?,
        &|n| Err(format!("unknown parameter `{n}`")),
        &Funcs::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval_const(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1+2*3"), 7.0);
        assert_eq!(ev("{(1+2)*3}"), 9.0);
        assert_eq!(ev("'2^3^2'"), 512.0);
        assert_eq!(ev("-2**2"), -4.0);
        assert_eq!(ev("2*-3"), -6.0);
        assert_eq!(ev("1k*2u"), 2e-3);
        assert_eq!(ev("10meg/2"), 5e6);
        assert_eq!(ev("1e-3*2"), 2e-3);
        assert_eq!(ev("3 > 2 ? 10 : 20"), 10.0);
        assert_eq!(ev("1 < 2 && 2 <= 2 || 0"), 1.0);
        assert_eq!(ev("max(1, min(5, 3)) + abs(-2)"), 5.0);
        assert!((ev("2*pi") - std::f64::consts::TAU).abs() < 1e-15);
    }

    #[test]
    fn variables_and_functions() {
        let mut funcs = Funcs::new();
        funcs.insert(
            "sq".into(),
            Func {
                args: vec!["x".into()],
                body: parse_expr("x*x").unwrap(),
            },
        );
        let var = |n: &str| match n {
            "w" => Ok(2e-6),
            _ => Err(format!("unknown `{n}`")),
        };
        let e = parse_expr("{sq(W/1u) + 1}").unwrap();
        assert_eq!(eval(&e, &var, &funcs).unwrap(), 5.0);
        assert!(eval(&parse_expr("nope + 1").unwrap(), &var, &funcs).is_err());
        assert!(parse_expr("1 +").is_err());
        assert!(parse_expr("(1").is_err());
    }
}
