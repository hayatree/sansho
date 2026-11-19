//! Nonlinear controlled sources with directional forward-mode differentiation.
//! Provenance: original work from the ngspice user manual (nonlinear dependent
//! sources), Ho, Ruehli & Brennan (MNA), and standard calculus.

use crate::{
    DeviceGroup, EvalCtx, Node, Slots, StampBuilder, StateRef, Unknowns, dual::Dual, volt,
};
use spx_diag::Diagnostic;
use spx_netlist::Expr;

type D = Dual<1>;
#[derive(Debug)]
enum Term {
    Number(f64),
    Polynomial {
        variables: Vec<Term>,
        coefficients: Vec<f64>,
    },
    Time,
    Temperature,
    Input(usize),
    Unary(char, Box<Term>),
    Binary(char, Box<Term>, Box<Term>),
    If(Box<Term>, Box<Term>, Box<Term>),
    Function(String, Vec<Term>),
}
#[derive(Debug, PartialEq)]
enum Input {
    Voltage(Node),
    Branch(String),
}

fn error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-BSOURCE-EXPR", msg)
}

impl Term {
    fn bind(
        e: Expr,
        inputs: &mut Vec<Input>,
        node: &dyn Fn(&str) -> Option<Node>,
    ) -> Result<Self, Diagnostic> {
        let mut bind = |e| Self::bind(e, inputs, node).map(Box::new);
        Ok(match e {
            Expr::Num(v) => Self::Number(v),
            Expr::Var(s) if s == "time" => Self::Time,
            Expr::Var(s) if s == "temper" => Self::Temperature,
            Expr::Var(s) => {
                let (kind, tail) = s
                    .split_once('(')
                    .ok_or_else(|| error(format!("unknown variable {s}")))?;
                let mut input = |s: &str| -> Result<Self, Diagnostic> {
                    let probe = if kind == "v" {
                        Input::Voltage(node(s).ok_or_else(|| error(format!("unknown node {s}")))?)
                    } else {
                        Input::Branch(s.into())
                    };
                    let index = inputs.iter().position(|p| p == &probe).unwrap_or_else(|| {
                        inputs.push(probe);
                        inputs.len() - 1
                    });
                    Ok(Self::Input(index))
                };
                let args = tail.trim_end_matches(')');
                if let Some((a, b)) = args.split_once(',') {
                    Self::Binary('-', Box::new(input(a)?), Box::new(input(b)?))
                } else {
                    input(args)?
                }
            }
            Expr::Unary(op, x) => Self::Unary(op, bind(*x)?),
            Expr::Binary(op, a, b) => Self::Binary(op, bind(*a)?, bind(*b)?),
            Expr::Cond(c, a, b) => Self::If(bind(*c)?, bind(*a)?, bind(*b)?),
            Expr::Call(name, args) => {
                let valid = match name.as_str() {
                    "pow" | "pwr" | "min" | "max" | "atan2" => args.len() == 2,
                    "if" | "ternary_fcn" | "limit" => args.len() == 3,
                    "pwl" | "table" => args.len() >= 3 && args.len() % 2 == 1,
                    "exp" | "ln" | "log" | "log10" | "sqrt" | "abs" | "tanh" | "sin" | "cos"
                    | "tan" | "u" | "sinh" | "cosh" | "asin" | "acos" | "atan" | "floor"
                    | "ceil" | "int" | "round" | "nint" | "sgn" | "sign" => args.len() == 1,
                    _ => false,
                };
                if !valid {
                    return Err(error(format!(
                        "unknown function or invalid argument count for {name}"
                    )));
                }
                let args = args
                    .into_iter()
                    .map(|a| Self::bind(a, inputs, node))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(name.as_str(), "pwl" | "table") {
                    let mut previous = f64::NEG_INFINITY;
                    for [coordinate, _] in args[1..].as_chunks::<2>().0 {
                        let value = coordinate
                            .constant()
                            .ok_or_else(|| error("lookup coordinates must be constants"))?;
                        if !value.is_finite() || value <= previous {
                            return Err(error(
                                "lookup coordinates must be finite and strictly increasing",
                            ));
                        }
                        previous = value;
                    }
                }
                Self::Function(name, args)
            }
            Expr::Draw(..) => return Err(error("unbound statistical expression")),
        })
    }
    fn constant(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Unary('-', e) => Some(-e.constant()?),
            Self::Unary('+', e) => e.constant(),
            Self::Binary(op, a, b) => {
                let (a, b) = (a.constant()?, b.constant()?);
                Some(match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    '^' => a.powf(b),
                    _ => return None,
                })
            }
            _ => None,
        }
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], inputs: &[Node], seed: Option<usize>) -> D {
        let ev = |t: &Term| t.eval(ctx, x, inputs, seed);
        let truth = |b: bool| D::cst(f64::from(b));
        match self {
            Self::Polynomial {
                variables,
                coefficients,
            } => {
                let values: Vec<_> = variables.iter().map(ev).collect();
                polynomial(&values, coefficients)
            }
            Self::Number(v) => D::cst(*v),
            Self::Time => D::cst(ctx.time),
            Self::Temperature => D::cst(ctx.temp - 273.15),
            Self::Input(i) => D {
                v: volt(x, inputs[*i]),
                d: [f64::from(seed == Some(*i))],
            },
            Self::If(c, a, b) => {
                if ev(c).v != 0. {
                    ev(a)
                } else {
                    ev(b)
                }
            }
            Self::Unary(op, x) => {
                let a = ev(x);
                match op {
                    '-' => -a,
                    '!' => truth(a.v == 0.),
                    _ => a,
                }
            }
            Self::Binary(op, a, b) => {
                let (a, b) = (ev(a), ev(b));
                match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    '/' => a / b,
                    '%' => D {
                        v: a.v % b.v,
                        d: [a.d[0] - (a.v / b.v).trunc() * b.d[0]],
                    },
                    '^' => power(a, b),
                    '<' => truth(a.v < b.v),
                    '>' => truth(a.v > b.v),
                    'l' => truth(a.v <= b.v),
                    'g' => truth(a.v >= b.v),
                    '=' => truth(a.v == b.v),
                    'n' => truth(a.v != b.v),
                    '&' => truth(a.v != 0. && b.v != 0.),
                    _ => truth(a.v != 0. || b.v != 0.),
                }
            }
            Self::Function(f, args) => {
                let a = ev(&args[0]);
                let b = || ev(&args[1]);
                let c = || ev(&args[2]);
                let chain = |v, slope| D {
                    v,
                    d: [if a.d[0] == 0. { 0. } else { a.d[0] * slope }],
                };
                match f.as_str() {
                    "if" | "ternary_fcn" => {
                        if a.v != 0. {
                            b()
                        } else {
                            c()
                        }
                    }
                    "pow" => power(a, b()),
                    "pwr" => {
                        if a.v < 0. {
                            -power(-a, b())
                        } else {
                            power(a, b())
                        }
                    }
                    "exp" => a.exp(),
                    "ln" | "log" => a.ln(),
                    "log10" => chain(a.v.log10(), 1. / (a.v * std::f64::consts::LN_10)),
                    "sqrt" => {
                        if a.v == 0. {
                            D::cst(0.)
                        } else {
                            a.sqrt()
                        }
                    }
                    "abs" => {
                        if a.v < 0. {
                            -a
                        } else {
                            a
                        }
                    }
                    "sin" => chain(a.v.sin(), a.v.cos()),
                    "cos" => chain(a.v.cos(), -a.v.sin()),
                    "tan" => chain(a.v.tan(), 1. / a.v.cos().powi(2)),
                    "tanh" => chain(a.v.tanh(), 1. - a.v.tanh().powi(2)),
                    "sinh" => chain(a.v.sinh(), a.v.cosh()),
                    "cosh" => chain(a.v.cosh(), a.v.sinh()),
                    "asin" => chain(a.v.asin(), 1. / (1. - a.v * a.v).sqrt()),
                    "acos" => chain(a.v.acos(), -1. / (1. - a.v * a.v).sqrt()),
                    "atan" => chain(a.v.atan(), 1. / (1. + a.v * a.v)),
                    "atan2" => {
                        let b = b();
                        D {
                            v: a.v.atan2(b.v),
                            d: [(b.v * a.d[0] - a.v * b.d[0]) / (a.v * a.v + b.v * b.v)],
                        }
                    }
                    "floor" => D::cst(a.v.floor()),
                    "ceil" => D::cst(a.v.ceil()),
                    "int" => D::cst(a.v.trunc()),
                    "round" | "nint" => D::cst(a.v.round()),
                    "sgn" | "sign" => D::cst(if a.v == 0. { 0. } else { a.v.signum() }),
                    "u" => truth(a.v > 0.),
                    "min" => {
                        let b = b();
                        if a.v <= b.v { a } else { b }
                    }
                    "max" => {
                        let b = b();
                        if a.v >= b.v { a } else { b }
                    }
                    "limit" => {
                        let (mut lo, mut hi) = (b(), c());
                        if lo.v > hi.v {
                            std::mem::swap(&mut lo, &mut hi);
                        }
                        if a.v < lo.v {
                            lo
                        } else if a.v > hi.v {
                            hi
                        } else {
                            a
                        }
                    }
                    _ => {
                        let (mut left, mut y) = (b(), c());
                        if a.v < left.v {
                            return y;
                        }
                        for pair in args[3..].as_chunks::<2>().0 {
                            let right = ev(&pair[0]);
                            let next = ev(&pair[1]);
                            if right.v <= left.v {
                                return D::cst(f64::NAN);
                            }
                            if a.v < right.v {
                                return y + (a - left) * (next - y) / (right - left);
                            }
                            (left, y) = (right, next);
                        }
                        y
                    }
                }
            }
        }
    }
}

/// Enumerate products by increasing degree, with nondecreasing variable indices
/// within each product (1, x, y, x², xy, y², ...).
fn polynomial(variables: &[D], coefficients: &[f64]) -> D {
    if let [x] = variables {
        if let [gain] = coefficients {
            return *x * *gain;
        }

        return coefficients
            .iter()
            .rev()
            .fold(D::cst(0.), |y, &c| y * *x + c);
    }
    let mut factors = Vec::new();
    let mut sum = D::cst(0.);
    for &c in coefficients {
        if c != 0. {
            sum += factors
                .iter()
                .fold(D::cst(c), |term, &i: &usize| term * variables[i]);
        }
        if let Some(k) = factors.iter().rposition(|&i| i + 1 < variables.len()) {
            let next = factors[k] + 1;
            factors[k..].fill(next);
        } else {
            factors.fill(0);
            factors.push(0);
        }
    }
    sum
}

fn power(a: D, b: D) -> D {
    if b.d[0] == 0. {
        if b.v == 0. { D::cst(1.) } else { a.powf(b.v) }
    } else {
        (b * a.ln()).exp()
    }
}

#[derive(Debug)]
pub struct Behavioral {
    name: String,
    pub span: Option<spx_diag::Span>,
    p: Node,
    n: Node,
    voltage: bool,
    branch: Node,
    term: Term,
    probes: Vec<Input>,
    inputs: Vec<Node>,
}
impl Behavioral {
    pub fn polynomial(
        name: &str,
        [p, n]: [Node; 2],
        voltage: bool,
        controls: &[spx_netlist::SwitchControl],
        coefficients: &[f64],
        node: impl Fn(&str) -> Node,
    ) -> Result<Self, Diagnostic> {
        if controls.is_empty() || coefficients.is_empty() {
            return Err(error("empty polynomial"));
        }
        let mut probes = Vec::new();
        let mut input = |probe| {
            let i = probes.len();
            probes.push(probe);
            Term::Input(i)
        };
        let variables = controls
            .iter()
            .map(|c| match c {
                spx_netlist::SwitchControl::Voltage(a, b) => Term::Binary(
                    '-',
                    Box::new(input(Input::Voltage(node(a)))),
                    Box::new(input(Input::Voltage(node(b)))),
                ),
                spx_netlist::SwitchControl::Current(v) => input(Input::Branch(v.clone())),
            })
            .collect();
        Ok(Self {
            name: name.into(),
            span: None,
            p,
            n,
            voltage,
            branch: None,
            term: Term::Polynomial {
                variables,
                coefficients: coefficients.to_vec(),
            },
            probes,
            inputs: vec![],
        })
    }
    pub fn new(
        name: &str,
        [p, n]: [Node; 2],
        voltage: bool,
        text: &str,
        table: &[(f64, f64)],
        node: impl Fn(&str) -> Option<Node>,
    ) -> Result<Self, Diagnostic> {
        let mut probes = vec![];
        let term = Term::bind(
            spx_netlist::parse_behavior(text).map_err(error)?,
            &mut probes,
            &node,
        )?;
        let term = if table.is_empty() {
            term
        } else {
            let mut args = vec![term];
            for &(x, y) in table {
                args.extend([Term::Number(x), Term::Number(y)]);
            }
            Term::Function("table".into(), args)
        };
        Ok(Self {
            span: None,
            name: name.into(),
            p,
            n,
            voltage,
            branch: None,
            term,
            probes,
            inputs: vec![],
        })
    }
}
impl DeviceGroup for Behavioral {
    fn type_name(&self) -> &'static str {
        if matches!(self.term, Term::Polynomial { .. }) {
            "poly"
        } else {
            "behavioral"
        }
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn instance_span(&self) -> Option<spx_diag::Span> {
        self.span
    }
    fn probe_nodes(&self) -> Vec<usize> {
        // POLY control terminals already occur in ElementKind::nodes.
        if matches!(self.term, Term::Polynomial { .. }) {
            return vec![];
        }
        self.probes
            .iter()
            .filter_map(|p| if let Input::Voltage(i) = p { *i } else { None })
            .collect()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        if self.voltage {
            self.branch = Some(u.alloc_branch(&self.name));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.inputs = self
            .probes
            .iter()
            .map(|probe| match probe {
                Input::Voltage(n) => Ok(*n),
                Input::Branch(name) => b.branch(name).map(Some).ok_or_else(|| {
                    error(format!("{} references unknown branch {name}", self.name))
                }),
            })
            .collect::<Result<_, _>>()?;
        b.f(self.p);
        b.f(self.n);
        if self.voltage {
            b.f(self.branch);
            for (r, c) in [
                (self.p, self.branch),
                (self.n, self.branch),
                (self.branch, self.p),
                (self.branch, self.n),
            ] {
                b.g(r, c);
            }
        }
        for &input in &self.inputs {
            if self.voltage {
                b.g(self.branch, input)
            } else {
                b.g(self.p, input);
                b.g(self.n, input)
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        let ev = |seed| self.term.eval(ctx, x, &self.inputs, seed);
        let v = ev(None).v;
        if self.voltage {
            let i = volt(x, self.branch);
            out.f
                .copy_from_slice(&[i, -i, volt(x, self.p) - volt(x, self.n) - v]);
            out.g[..4].copy_from_slice(&[1., -1., 1., -1.]);
            for (i, g) in out.g[4..].iter_mut().enumerate() {
                *g = -ev(Some(i)).d[0];
            }
        } else {
            out.f.copy_from_slice(&[v, -v]);
            for (i, g) in out.g.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let d = ev(Some(i)).d[0];
                g.copy_from_slice(&[d, -d]);
            }
        }
        if !v.is_finite() || out.g.iter().any(|v| !v.is_finite()) {
            if ctx.init_junctions {
                out.noncon = true;
                if self.voltage {
                    out.f[2] = volt(x, self.p) - volt(x, self.n);
                    out.g[4..].fill(0.);
                } else {
                    out.f.fill(0.);
                    out.g.fill(0.);
                }
                return;
            }
            out.error = Some(error(format!(
                "{} has non-finite expression value or derivative",
                self.name
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_functions_have_exact_directional_derivatives() {
        let ctx = EvalCtx::dc();
        let a = 0.25_f64;
        for (s, v, d) in [
            ("pwr(-v(a),2)", -a * a, -2. * a),
            ("pow(v(a),3)", a.powi(3), 3. * a * a),
            ("exp(v(a))", a.exp(), a.exp()),
            ("ln(v(a))", a.ln(), 1. / a),
            ("log(v(a))", a.ln(), 1. / a),
            ("sqrt(v(a))", a.sqrt(), 0.5 / a.sqrt()),
            ("abs(-v(a))", a, 1.),
            ("min(v(a),0.5)+max(v(a),1)", a + 1., 1.),
            ("limit(2*v(a),-0.2,0.2)", 0.2, 0.),
            ("tanh(v(a))", a.tanh(), 1. - a.tanh().powi(2)),
            ("sin(v(a))+cos(v(a))", a.sin() + a.cos(), a.cos() - a.sin()),
            ("if(u(v(a)),v(a),ln(-1))", a, 1.),
            ("v(a)>0?v(a):0", a, 1.),
            ("pwl(v(a),0,-1,1,1)", -0.5, 2.),
        ] {
            let expr = spx_netlist::parse_behavior(s).unwrap();
            let term = Term::bind(expr, &mut vec![], &|_| Some(Some(0))).unwrap();
            let r = term.eval(&ctx, &[a], &[Some(0)], Some(0));
            assert!(
                (r.v - v).abs() < 1e-14 && (r.d[0] - d).abs() < 1e-14,
                "{s}: {r:?}"
            );
        }
    }
}

#[cfg(test)]
mod polynomial_tests {
    use super::*;

    #[test]
    fn graded_products_and_partial_derivatives_at_zero() {
        let vars = [D::var(0., 0), D::cst(-2.), D::cst(3.)];
        // Degree three starts at index 10: xxx, xxy, xxz, xyy, xyz, xzz, ...
        let mut c = [0.; 16];
        c[5] = 1.; // xy
        c[14] = 2.; // xyz
        let value = polynomial(&vars, &c);
        assert_eq!(value, D { v: 0., d: [-14.] });
        assert_eq!(polynomial(&[D::var(4., 0)], &[2.]), D { v: 8., d: [2.] });
        assert_eq!(polynomial(&vars, &[2.]), D::cst(2.));
    }
}
