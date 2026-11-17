//! Binding-specialized register code for scalar analog equations.
//! Provenance: original work from Aho et al., Compilers, and Griewank & Walther, Evaluating Derivatives.

mod execute;
mod history;
mod optimize;
use super::*;
use crate::math::Math;
pub(crate) use history::history;
use std::collections::{HashMap, HashSet};

type Reg = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pair {
    f: Reg,
    q: Reg,
}

#[derive(Clone, Debug)]
enum Op {
    Math(Reg, Math, Reg, Reg),
    Charge(Reg, Math, Reg, Reg),
    Nop,
    Unknown(Reg, usize),
    History(Reg, usize, bool),
    Ddx(Reg, Reg, usize),
    CheckZeroProduct(Reg, bool),
    Noise(Reg),
    IntegralInitial(Reg),
    LimitEnabled(Reg),
    Event(Reg, bool, bool),
    Previous(Reg, usize),
    LimitValue {
        dst: Reg,
        junction: bool,
        new: Reg,
        old: Reg,
        arg: Reg,
        critical: Reg,
    },
    Limited {
        dst: Reg,
        slot: usize,
        new: Reg,
        limited: Reg,
    },
    Time(Reg),
    Simparam(Reg, String, Option<Reg>),
    Analysis(Reg, String),
    IfFalse(Reg, usize),
    Jump(usize),
    Phi(Reg, Reg, Reg, Reg),
    Stamp(usize, Pair, f64),
    Remember(usize, Pair, Ty),
}
#[derive(Default, Debug)]
pub(super) struct Machine {
    key: Option<(u64, u64)>,
    program: Option<Program>,
    queries: Vec<Option<f64>>,
}
#[derive(Default, Debug)]
struct Program {
    ops: Vec<Op>,
    values: Vec<f64>,
    rows: Vec<f64>,
    gradient_count: usize,
}

struct Builder<'a> {
    m: &'a CompiledModule,
    b: &'a Bound,
    input: &'a EvalInput<'a>,
    integrals_seen: Vec<bool>,
    fuel: usize,
    depth: usize,
    compact_charge: bool,
    types: Vec<Ty>,
    // The two slots after the variables for each branch hold its mode and sum.
    vars: Vec<Option<Pair>>,
    constants: HashMap<u64, Reg>,
    known: Vec<Option<f64>>,
    cse: HashMap<(Math, Reg, Reg), Reg>,
    ops: Vec<Op>,
}
impl Builder<'_> {
    fn reg(&mut self) -> Reg {
        let i = self.known.len();
        self.known.push(None);
        i
    }
    fn num(&mut self, v: f64) -> Reg {
        if let Some(&r) = self.constants.get(&v.to_bits()) {
            return r;
        }
        let r = self.reg();
        self.known[r] = Some(v);
        self.constants.insert(v.to_bits(), r);
        r
    }
    fn scalar(&mut self, r: Reg) -> Pair {
        Pair {
            f: r,
            q: self.num(0.),
        }
    }
    fn math(&mut self, op: Math, a: Reg, b: Reg) -> Reg {
        if let (Some(x), Some(y)) = (self.known[a], self.known[b]) {
            return self.num(op.apply(x, y).0);
        }
        if let Some(&r) = self.cse.get(&(op, a, b)) {
            return r;
        }
        let r = self.reg();
        self.ops.push(Op::Math(r, op, a, b));
        self.cse.insert((op, a, b), r);
        r
    }
    // On a nonreactive path the charge channel vanishes. IEEE multiplication
    // by zero can still produce NaN for an infinite primal or partial. Guard
    // that assumption and use the full two-channel program on exceptional input.
    fn charge_product(&mut self, op: Math, a: Reg, b: Reg) -> Reg {
        if !self.compact_charge || (self.known[a].is_some() && self.known[b].is_some()) {
            return self.math(op, a, b);
        }
        let r = self.reg();
        self.ops.push(Op::Charge(r, op, a, b));
        r
    }
    fn phi(&mut self, c: Reg, a: Reg, b: Reg) -> Reg {
        if a == b {
            return a;
        }
        let r = self.reg();
        self.ops.push(Op::Phi(r, c, a, b));
        r
    }
    fn merge(&mut self, c: Reg, a: Pair, b: Pair) -> Pair {
        Pair {
            f: self.phi(c, a.f, b.f),
            q: self.phi(c, a.q, b.q),
        }
    }
    fn load(&mut self, i: usize) -> Pair {
        if let Some(v) = self.vars[i] {
            return v;
        }
        let f = self.reg();
        let q = self.reg();
        self.ops.push(Op::History(f, i, false));
        self.ops.push(Op::History(q, i, true));
        let v = Pair { f, q };
        self.vars[i] = Some(v);
        v
    }
    fn voltage(&mut self, i: Option<usize>) -> Reg {
        if let Some(j) = i.and_then(|i| self.b.layout.index(i)) {
            let r = self.reg();
            self.ops.push(Op::Unknown(r, j));
            r
        } else {
            self.num(0.)
        }
    }
    fn cond(
        &mut self,
        c: Reg,
        a: impl FnOnce(&mut Self) -> Option<Pair>,
        b: impl FnOnce(&mut Self) -> Option<Pair>,
    ) -> Option<Pair> {
        if let Some(v) = self.known[c] {
            return if v != 0. { a(self) } else { b(self) };
        }
        let old = self.vars.clone();
        let branch = self.ops.len();
        self.ops.push(Op::IfFalse(c, 0));
        let x = a(self)?;
        let then = std::mem::replace(&mut self.vars, old);
        let end = self.ops.len();
        self.ops.push(Op::Jump(0));
        self.cse.clear();
        self.ops[branch] = Op::IfFalse(c, self.ops.len());
        let y = b(self)?;
        self.ops[end] = Op::Jump(self.ops.len());
        self.cse.clear();
        for (i, a) in then.into_iter().enumerate() {
            let b = self.vars[i];
            if a != b {
                self.vars[i] = Some(self.merge(c, a?, b?))
            }
        }
        Some(self.merge(c, x, y))
    }
    fn expr(&mut self, e: &Expr) -> Option<Pair> {
        if e.ty == Ty::String {
            return None;
        }
        self.fuel = self.fuel.checked_sub(1)?;
        let z = self.num(0.);
        let f = match &e.kind {
            E::Num(v) => self.num(*v),
            E::Param(i) if self.b.values[*i].text.is_none() => self.num(self.b.values[*i].f.v),
            E::Given(i) => self.num(f64::from(self.b.given[*i])),
            E::Connected(i) => self.num(f64::from(*i < self.b.connected_ports)),
            E::Var(i) => return Some(self.load(*i)),
            E::Voltage(a, b) => {
                let x = self.voltage(*a);
                let y = self.voltage(*b);
                self.math(Math::Sub, x, y)
            }
            E::Current(i) => self.voltage(self.m.branches[*i].current),
            E::Ddx(e, i) => {
                let a = self.expr(e)?.f;
                if let Some(j) = self.b.layout.index(*i) {
                    let r = self.reg();
                    self.ops.push(Op::Ddx(r, a, j));
                    r
                } else {
                    z
                }
            }
            E::Select(c, a, b) => {
                let c = self.expr(c)?.f;
                let p = self.cond(c, |s| s.expr(a), |s| s.expr(b))?;
                return Some(self.cast(p, e.ty));
            }
            E::Unary(op, a) => {
                let a = self.expr(a)?;
                match op.as_str() {
                    "+" => return Some(a),
                    "-" => {
                        let p = Pair {
                            f: self.math(Math::Neg, a.f, z),
                            q: self.math(Math::Neg, a.q, z),
                        };
                        return Some(self.cast(p, e.ty));
                    }
                    "!" => self.math(Math::Not, a.f, z),
                    _ => return None,
                }
            }
            E::Binary(op, a, b) => {
                let ints = a.ty == Ty::Integer && b.ty == Ty::Integer;
                let a = self.expr(a)?;
                if op == "&&" || op == "||" {
                    let eval = |s: &mut Self| {
                        let b = s.expr(b)?.f;
                        let f = s.math(Math::Bool, b, z);
                        Some(s.scalar(f))
                    };
                    return if op == "&&" {
                        self.cond(a.f, eval, |s| Some(s.scalar(z)))
                    } else {
                        self.cond(
                            a.f,
                            |s| {
                                let one = s.num(1.);
                                Some(s.scalar(one))
                            },
                            eval,
                        )
                    };
                }
                let b = self.expr(b)?;
                let op = Math::named(op)?;
                if ints && op == Math::Div && self.known[b.f].is_none_or(|v| v == 0.) {
                    return None;
                }
                let f = self.math(op, a.f, b.f);
                let q = match op {
                    Math::Add | Math::Sub => self.math(op, a.q, b.q),
                    Math::Mul => {
                        let x = self.charge_product(Math::Mul, a.q, b.f);
                        let y = self.charge_product(Math::Mul, a.f, b.q);
                        self.math(Math::Add, x, y)
                    }
                    Math::Div if !ints => self.charge_product(Math::Div, a.q, b.f),
                    _ => z,
                };
                return Some(self.cast(Pair { f, q }, e.ty));
            }
            E::Function(i, args) => {
                let fun = &self.m.functions[*i];
                let mut values = Vec::with_capacity(args.len());
                for (a, (_, dir)) in args.iter().zip(&fun.args) {
                    values.push(if dir == "output" {
                        self.scalar(z)
                    } else {
                        self.expr(a)?
                    });
                }
                let result = self.function(*i, &values)?;
                for (a, (slot, dir)) in args.iter().zip(&fun.args) {
                    if dir != "input" {
                        let E::Var(target) = a.kind else { return None };
                        let v = self.vars[*slot]?;
                        self.types[target] = a.ty;
                        self.vars[target] = Some(self.cast(v, a.ty));
                    }
                }
                return Some(result);
            }
            E::Integral(k, u, ic, _) => {
                if std::mem::replace(&mut self.integrals_seen[*k], true) {
                    return None;
                }
                let state = self.voltage(Some(self.m.integrals.start + k));
                let state = self.scalar(state);
                let c = self.reg();
                self.ops.push(Op::IntegralInitial(c));
                let residual = self.cond(
                    c,
                    |s| {
                        let ic = s.expr(ic)?.f;
                        let r = s.math(Math::Sub, state.f, ic);
                        Some(s.scalar(r))
                    },
                    |s| {
                        let u = s.expr(u)?.f;
                        let r = s.math(Math::Neg, u, z);
                        Some(s.scalar(r))
                    },
                )?;
                self.vars[self.m.slots + 2 * self.m.branches.len() + k] = Some(Pair {
                    f: residual.f,
                    q: state.f,
                });
                return Some(state);
            }
            E::Limit(slot, name, args, func) => {
                let mut args = args
                    .iter()
                    .map(|a| self.expr(a))
                    .collect::<Option<Vec<_>>>()?;
                let mut new = args[0];
                if name == "typedpnjlim_new" {
                    new.f = self.math(Math::Mul, new.f, args[3].f);
                }
                let old = self.reg();
                self.ops.push(Op::Previous(old, *slot));
                let c = self.reg();
                self.ops.push(Op::LimitEnabled(c));
                let limited = if name == "dummy" {
                    new
                } else {
                    self.cond(
                        c,
                        |s| {
                            if let Some(i) = func {
                                args.insert(1, s.scalar(old));
                                s.function(*i, &args)
                            } else {
                                let dst = s.reg();
                                s.ops.push(Op::LimitValue {
                                    dst,
                                    junction: name != "fetlim",
                                    new: new.f,
                                    old,
                                    arg: args[1].f,
                                    critical: args.get(2).map_or(z, |a| a.f),
                                });
                                Some(s.scalar(dst))
                            }
                        },
                        |_| Some(new),
                    )?
                };
                let dst = self.reg();
                self.ops.push(Op::Limited {
                    dst,
                    slot: *slot,
                    new: new.f,
                    limited: limited.f,
                });
                return Some(Pair { f: dst, q: new.q });
            }
            E::Call(name, args) => match name.as_str() {
                "ddt" => {
                    return Some(Pair {
                        f: z,
                        q: self.expr(&args[0])?.f,
                    });
                }
                "$temperature" => self.num(self.input.temperature),
                "$vt" => {
                    let k = self.num(K_OVER_Q);
                    if args.is_empty() {
                        self.num(K_OVER_Q * self.input.temperature)
                    } else {
                        let a = self.expr(&args[0])?.f;
                        self.math(Math::Mul, a, k)
                    }
                }
                "$mfactor" => self.num(
                    self.input.mfactor
                        * if self.m.implicit_m {
                            self.b.values[0].f.v
                        } else {
                            1.
                        },
                ),
                "$abstime" => {
                    let r = self.reg();
                    self.ops.push(Op::Time(r));
                    r
                }
                "$simparam" | "analysis" => {
                    let E::Str(key) = &args[0].kind else {
                        return None;
                    };
                    let default = if let Some(a) = args.get(1) {
                        Some(self.expr(a)?.f)
                    } else {
                        None
                    };
                    let r = self.reg();
                    self.ops.push(if name == "analysis" {
                        Op::Analysis(r, key.to_string())
                    } else {
                        Op::Simparam(r, key.to_string(), default)
                    });
                    r
                }
                "white_noise" | "flicker_noise" | "noise_table" => {
                    let r = self.reg();
                    self.ops.push(Op::Noise(r));
                    r
                }
                _ => {
                    let op = Math::named(name)?;
                    let a = self.expr(args.first()?)?;
                    let b = if let Some(b) = args.get(1) {
                        self.expr(b)?
                    } else {
                        self.scalar(z)
                    };
                    if matches!(op, Math::Min | Math::Max) {
                        let c =
                            self.math(if op == Math::Min { Math::Le } else { Math::Ge }, a.f, b.f);
                        return Some(self.choose(c, a, b));
                    }
                    self.math(op, a.f, b.f)
                }
            },
            _ => return None,
        };
        Some(self.scalar(f))
    }
    fn choose(&mut self, c: Reg, a: Pair, b: Pair) -> Pair {
        if let Some(v) = self.known[c] {
            if v != 0. { a } else { b }
        } else {
            self.merge(c, a, b)
        }
    }
    fn cast(&mut self, a: Pair, ty: Ty) -> Pair {
        if ty != Ty::Integer {
            return a;
        }
        let z = self.num(0.);
        Pair {
            f: self.math(Math::Trunc, a.f, z),
            q: self.math(Math::Trunc, a.q, z),
        }
    }
    fn function(&mut self, i: usize, args: &[Pair]) -> Option<Pair> {
        // Semantic analysis rejects recursion and assigns every local a distinct slot.
        if self.depth >= 128 {
            return None;
        }
        self.depth += 1;
        let f = &self.m.functions[i];
        for (j, v) in &f.locals {
            self.init(*j, v)?;
        }
        for ((j, dir), &value) in f.args.iter().zip(args) {
            if dir != "output" {
                let ty = f.locals.iter().find(|(k, _)| k == j)?.1.ty;
                self.vars[*j] = Some(self.cast(value, ty));
            }
        }
        for s in &f.body {
            self.stmt(s)?
        }
        self.depth -= 1;
        self.vars[f.result]
    }
    fn init(&mut self, i: usize, v: &Variable) -> Option<()> {
        if v.ty == Ty::String || !v.dimensions.is_empty() {
            return None;
        }
        let value = if let Some(e) = &v.init {
            self.expr(e)?
        } else {
            let z = self.num(0.);
            self.scalar(z)
        };
        self.types[i] = v.ty;
        self.vars[i] = Some(self.cast(value, v.ty));
        Some(())
    }
    fn unroll(&mut self, c: &Expr, body: &Stmt, step: Option<&Stmt>) -> Option<()> {
        for _ in 0..=64 {
            let r = self.expr(c)?.f;
            if self.known[r]? == 0. {
                return Some(());
            }
            self.stmt(body)?;
            if let Some(s) = step {
                self.stmt(s)?
            }
        }
        None
    }
    fn stmt(&mut self, s: &Stmt) -> Option<()> {
        self.fuel = self.fuel.checked_sub(1)?;
        match s {
            Stmt::Block(vars, body) => {
                if self.depth > 0 {
                    for (i, v) in vars {
                        self.init(*i, v)?
                    }
                } else if vars.iter().any(|(_, v)| {
                    v.init.is_some() || !v.dimensions.is_empty() || v.ty == Ty::String
                }) {
                    return None;
                }
                for s in body {
                    self.stmt(s)?
                }
            }
            Stmt::Assign(a, b) => {
                let E::Var(i) = a.kind else { return None };
                if a.ty == Ty::String {
                    return None;
                }
                let v = self.expr(b)?;
                self.types[i] = a.ty;
                self.vars[i] = Some(self.cast(v, a.ty));
            }
            Stmt::Contrib(i, voltage, sign, e) => {
                let v = self.expr(e)?;
                let branch = &self.m.branches[*i];
                if branch
                    .current
                    .and_then(|j| self.b.layout.index(j))
                    .is_some()
                {
                    let mode = self.m.slots + 2 * i;
                    let old = self.vars[mode]?;
                    let sum = self.vars[mode + 1]?;
                    let flag = self.num(f64::from(*voltage));
                    let same = self.math(Math::Eq, old.f, flag);
                    let z = self.num(0.);
                    let zero = self.scalar(z);
                    let sum = self.choose(same, sum, zero);
                    let op = if *sign == 1. { Math::Add } else { Math::Sub };
                    self.vars[mode + 1] = Some(Pair {
                        f: self.math(op, sum.f, v.f),
                        q: self.math(op, sum.q, v.q),
                    });
                    self.vars[mode] = Some(self.scalar(flag));
                } else if !voltage {
                    if let Some(row) = branch.a.and_then(|j| self.b.layout.index(j)) {
                        self.ops.push(Op::Stamp(row, v, *sign))
                    }
                    if let Some(row) = branch.b.and_then(|j| self.b.layout.index(j)) {
                        self.ops.push(Op::Stamp(row, v, -sign))
                    }
                }
            }
            Stmt::If(c, a, b) => {
                let c = self.expr(c)?.f;
                let r = self.num(0.);
                let z = self.scalar(r);
                self.cond(
                    c,
                    |s| {
                        s.stmt(a)?;
                        Some(z)
                    },
                    |s| {
                        if let Some(b) = b {
                            s.stmt(b)?
                        }
                        Some(z)
                    },
                )?;
            }
            Stmt::While(c, body) => self.unroll(c, body, None)?,
            Stmt::For(init, c, step, body) => {
                self.stmt(init)?;
                self.unroll(c, body, Some(step))?
            }
            Stmt::Repeat(count, body) => {
                let c = self.expr(count)?.f;
                let count = self.known[c]?.trunc().max(0.);
                if count > 64. {
                    return None;
                }
                self.fuel = self.fuel.checked_sub(count as usize)?;
                for _ in 0..count as usize {
                    self.stmt(body)?
                }
            }
            Stmt::Case(selector, arms) => {
                let x = self.expr(selector)?.f;
                let key = self.known[x]?;
                let mut selected = None;
                let mut default = None;
                'arms: for (labels, stmt) in arms {
                    if labels.is_empty() {
                        default = Some(stmt)
                    }
                    for label in labels {
                        let r = self.expr(label)?.f;
                        if self.known[r]? == key {
                            selected = Some(stmt);
                            break 'arms;
                        }
                    }
                }
                if let Some(stmt) = selected.or(default) {
                    self.stmt(stmt)?
                }
            }
            Stmt::Event {
                initial,
                last,
                body,
                ..
            } => {
                // The initialization call has already run in the general evaluator.
                if *initial || *last {
                    let c = self.reg();
                    self.ops.push(Op::Event(c, *initial, *last));
                    let z = self.num(0.);
                    let zero = self.scalar(z);
                    self.cond(
                        c,
                        |s| {
                            s.stmt(body)?;
                            Some(zero)
                        },
                        |_| Some(zero),
                    )?;
                }
            }
            Stmt::Empty => {}
            Stmt::Task(name, _) if !matches!(name.as_str(), "$error" | "$fatal" | "$finish") => {}
            _ => return None,
        }
        Some(())
    }
}
impl Program {
    fn compile(
        m: &CompiledModule,
        b: &Bound,
        input: &EvalInput<'_>,
        compact_charge: bool,
    ) -> Option<Self> {
        let mut builder = Builder {
            m,
            b,
            input,
            depth: 0,
            integrals_seen: vec![false; m.integrals.len()],
            fuel: ITERATION_CAP,
            compact_charge,
            types: vec![Ty::Real; m.slots],
            vars: vec![None; m.slots],
            constants: HashMap::new(),
            known: vec![],
            cse: HashMap::new(),
            ops: vec![],
        };
        for i in 0..m.slots {
            builder.load(i);
        }
        let z = builder.num(0.);
        let zero = builder.scalar(z);
        builder
            .vars
            .resize(m.slots + 2 * m.branches.len(), Some(zero));
        for id in m.integrals.clone() {
            let r = builder.voltage(Some(id));
            let v = builder.scalar(r);
            builder.vars.push(Some(v));
        }
        for s in &m.body {
            builder.stmt(s)?
        }
        for (k, id) in m.integrals.clone().enumerate() {
            if let Some(row) = b.layout.index(id) {
                builder.ops.push(Op::Stamp(
                    row,
                    builder.vars[m.slots + 2 * m.branches.len() + k]?,
                    1.,
                ))
            }
        }
        for (k, br) in m.branches.iter().enumerate() {
            if let Some(row) = br.current.and_then(|i| b.layout.index(i)) {
                let r = builder.voltage(br.current);
                let current = builder.scalar(r);
                for (terminal, sign) in [(br.a, 1.), (br.b, -1.)] {
                    if let Some(i) = terminal.and_then(|i| b.layout.index(i)) {
                        builder.ops.push(Op::Stamp(i, current, sign));
                    }
                }
                let a = builder.voltage(br.a);
                let c = builder.voltage(br.b);
                let v = builder.math(Math::Sub, a, c);
                let v = builder.scalar(v);
                let mode = m.slots + 2 * k;
                let lhs = if br.contributed {
                    builder.choose(builder.vars[mode]?.f, v, current)
                } else {
                    v
                };
                builder.ops.push(Op::Stamp(row, lhs, 1.));
                builder
                    .ops
                    .push(Op::Stamp(row, builder.vars[mode + 1]?, -1.));
            }
        }
        for &i in &m.history {
            let v = builder.vars[i]?;
            if v.f != i * 2 || v.q != i * 2 + 1 {
                builder.ops.push(Op::Remember(i, v, builder.types[i]))
            }
        }
        Some(Self::finish(builder, b.layout.unknown_len() + 1))
    }
}

pub(super) fn evaluate(
    m: &CompiledModule,
    b: &Bound,
    input: &EvalInput<'_>,
    scratch: &mut Scratch,
    out: &mut EvalOutput,
    state: &mut StateIo<'_>,
) -> bool {
    let key = (input.temperature.to_bits(), input.mfactor.to_bits());
    if scratch.machine.key != Some(key) {
        scratch.machine.program = Program::compile(m, b, input, m.states == 0);
        if let Some(p) = &scratch.machine.program {
            scratch.machine.queries.clear();
            scratch.machine.queries.reserve(
                p.ops
                    .iter()
                    .filter(|op| matches!(op, Op::Simparam(..)))
                    .count(),
            );
        }
        scratch.machine.key = Some(key);
    }
    let machine = &mut scratch.machine;
    let Some(p) = &mut machine.program else {
        return false;
    };
    machine.queries.clear();
    if p.run(b, input, &mut scratch.slots, out, state, |key| {
        let value = (input.simparams)(key);
        machine.queries.push(value);
        value
    }) {
        return true;
    }
    // The guarded pass has not committed retained variables. Replay simulator
    // queries so even a stateful embedding observes each call exactly once.
    out.f.fill(0.);
    out.q.fill(0.);
    out.df.fill(0.);
    out.dq.fill(0.);
    machine.program = Program::compile(m, b, input, false);
    if let Some(p) = &mut machine.program {
        let mut values = machine.queries.iter().copied();
        p.run(b, input, &mut scratch.slots, out, state, |_| {
            values.next().flatten()
        });
        true
    } else {
        false
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::Cell;

    // Force the general evaluator in the second workspace. Compare the complete
    // state and stamps on every call, including binding and temperature changes.
    fn compare(source: &str, inputs: &[f64], exceptional: bool) {
        let ast = crate::parse_va(source, &crate::NoVaFiles).unwrap();
        let m = crate::compile_module(&ast, None).unwrap();
        let b = m.bind(&[]).unwrap();
        let mut work = [Scratch::default(), Scratch::default()];
        let mut current = [vec![0.; m.states], vec![0.; m.states]];
        let accepted = vec![0.; m.states];
        let mut out = [EvalOutput::default(), EvalOutput::default()];
        for (k, &bias) in inputs.iter().enumerate() {
            let x = vec![bias; b.layout.unknown_len()];
            let temperature = if k % 4 == 1 || k % 4 == 2 { 300. } else { 330. };
            let calls = [Cell::new(0), Cell::new(0)];
            for path in 0..2 {
                if path == 1 {
                    work[path].machine.key = Some((f64::to_bits(temperature), 1f64.to_bits()));
                    work[path].machine.program = None;
                }
                m.eval_with_scratch(
                    &b,
                    &EvalInput {
                        x: &x,
                        temperature,
                        analysis: if k % 3 == 0 {
                            Analysis::Dc
                        } else {
                            Analysis::Tran
                        },
                        initial_step: k % 3 == 0,
                        final_step: k % 3 == 2,
                        abstime: k as f64,
                        gmin: 1e-12,
                        limit: k % 2 == 1,
                        mfactor: 1.,
                        simparams: &|_| {
                            calls[path].set(calls[path].get() + 1);
                            Some(0.1 * f64::from(calls[path].get()))
                        },
                    },
                    StateIo {
                        accepted: &accepted,
                        current: &mut current[path],
                    },
                    &mut work[path],
                    &mut out[path],
                );
            }
            if k > 0 {
                assert!(
                    work[0].machine.program.is_some(),
                    "expected register execution"
                );
            }
            assert_eq!(calls[0].get(), calls[1].get());
            assert_eq!(out[0].error, None);
            assert_eq!(out[0].noncon, out[1].noncon);
            for (left, right) in [
                (&current[0], &current[1]),
                (&out[0].f, &out[1].f),
                (&out[0].q, &out[1].q),
                (&out[0].df, &out[1].df),
                (&out[0].dq, &out[1].dq),
            ] {
                for (a, b) in left.iter().zip(right) {
                    assert!(
                        a.to_bits() == b.to_bits() || (exceptional && a.is_nan() && b.is_nan()),
                        "call {k}: {a} versus {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn register_stamps_match_general_execution() {
        compare(
            r#"module m(p); electrical p; real t,y,v; integer j;
        analog function real bump; inout a; real a; begin a=a+1; bump=a*a; end endfunction
        analog begin
          @(initial_instance) t=3;
          @(initial_step) t=t+1;
          v=$limit(V(p),"fetlim",0.4);
          y=V(p)>0 ? bump(t) : -v;
          for(j=0;j<3;j=j+1) y=y+v;
          if (analysis("tran")) y=y+idt(v,0.2);
          if (V(p)>0.5) y=y+$simparam("k",0);
          @(final_step) y=y+t;
          I(p)<+y*$vt+ddt(v*v);
        end endmodule"#,
            &[0., 1., 1.2, -2., 0.3, 1.1],
            false,
        );
    }

    #[test]
    fn exceptional_charge_replays_queries_once() {
        compare(
            r#"module m(p); electrical p; real a;
        analog begin a=$simparam("a",0);
          if(V(p)>0.) a=a*$simparam("b",0);
          I(p)<+a/V(p);
        end endmodule"#,
            &[1., 0., -1., 1e-240],
            true,
        );
    }
}
