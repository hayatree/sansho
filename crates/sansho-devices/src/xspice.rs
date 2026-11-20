//! Continuous analog signal ports and their modified nodal equations.
//! Provenance: XSPICE analog model conventions and standard controllable companion realization.

use crate::{
    DeviceGroup, EvalCtx, Mode, ModelParams, Node, Slots, StampBuilder, StateRef, Unknowns,
    dual::Dual, volt,
};
use sansho_diag::Diagnostic;
use sansho_netlist::{CodePort, ModelCard};

#[path = "xspice_oscillator.rs"]
mod oscillator;
use oscillator::Oscillator;
type D = Dual<1>;
fn invalid(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-ELAB-PARAM", msg)
}
fn port_error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-XSPICE-PORT", msg)
}

#[derive(Debug)]
struct Terminal {
    pair: [Node; 2],
    current: bool,
    branch: Node,
}
impl Terminal {
    fn controls(&self) -> [Node; 2] {
        if self.current {
            [self.branch, None]
        } else {
            self.pair
        }
    }
}

/// A C1 corner between two lines. Both tangents meet at the specified center.
fn corner(x: D, center: D, value: D, left: f64, right: f64, width: D) -> D {
    let dx = x - center;
    if dx.v <= -width.v {
        value + left * dx
    } else if dx.v >= width.v {
        value + right * dx
    } else {
        value + left * dx + (right - left) * (dx + width).sq() / (4. * width)
    }
}

#[derive(Debug)]
struct Limits {
    lo: f64,
    hi: f64,
    range: f64,
    fraction: bool,
}
impl Limits {
    fn new(p: &ModelParams, lo: f64, hi: f64) -> Result<Self, Diagnostic> {
        let s = Self {
            lo,
            hi,
            range: p.get("limit_range", 1e-6),
            fraction: p.get("fraction", 0.) != 0.,
        };
        if lo >= hi || s.range < 0. || s.width(D::cst(lo), D::cst(hi)).v * 2. > hi - lo {
            return Err(invalid("invalid bounds or overlapping smoothing intervals"));
        }
        Ok(s)
    }
    fn width(&self, lo: D, hi: D) -> D {
        if self.fraction {
            self.range * (hi - lo)
        } else {
            D::cst(self.range)
        }
    }
    fn eval(&self, x: D, lo: D, hi: D) -> D {
        let width = self.width(lo, hi);
        if x.v <= lo.v - width.v {
            lo
        } else if x.v >= hi.v + width.v {
            hi
        } else if x.v >= lo.v + width.v && x.v <= hi.v - width.v {
            x
        } else if x.v < 0.5 * lo.v + 0.5 * hi.v {
            corner(x, lo, lo, 0., 1., width)
        } else {
            corner(x, hi, hi, 1., 0., width)
        }
    }
    fn fixed(&self, x: D) -> D {
        self.eval(x, D::cst(self.lo), D::cst(self.hi))
    }
}

#[derive(Debug)]
struct Curve {
    x: Vec<f64>,
    y: Vec<f64>,
    slopes: Vec<f64>,
    widths: Vec<f64>,
}
impl Curve {
    fn new(x: Vec<f64>, y: Vec<f64>, domain: f64, fraction: bool) -> Result<Self, Diagnostic> {
        if x.len() < 2 || x.len() != y.len() || x.windows(2).any(|v| v[1] <= v[0]) || domain < 0. {
            return Err(invalid(
                "pwl requires matching arrays, increasing x and nonnegative input_domain",
            ));
        }
        let slopes = x
            .windows(2)
            .zip(y.windows(2))
            .map(|(a, b)| (b[1] - b[0]) / (a[1] - a[0]))
            .collect();
        let mut widths = vec![0.; x.len()];
        for (i, triple) in x.windows(3).enumerate() {
            widths[i + 1] = if fraction {
                domain * (triple[1] - triple[0]).min(triple[2] - triple[1])
            } else {
                domain
            };
        }
        if widths
            .windows(2)
            .zip(x.windows(2))
            .any(|(w, t)| w[0] + w[1] > t[1] - t[0])
        {
            return Err(invalid("pwl smoothing regions overlap"));
        }
        Ok(Self {
            x,
            y,
            slopes,
            widths,
        })
    }
    fn eval(&self, u: D) -> D {
        let i = self
            .x
            .partition_point(|&x| x <= u.v)
            .saturating_sub(1)
            .min(self.x.len() - 2);
        for j in [i, i + 1] {
            if j > 0 && j + 1 < self.x.len() && (u.v - self.x[j]).abs() < self.widths[j] {
                return corner(
                    u,
                    D::cst(self.x[j]),
                    D::cst(self.y[j]),
                    self.slopes[j - 1],
                    self.slopes[j],
                    D::cst(self.widths[j]),
                );
            }
        }
        self.y[i] + (u - self.x[i]) * self.slopes[i]
    }
}

#[derive(Debug)]
enum Transfer {
    Oscillator(Oscillator),
    Slew {
        rise: f64,
        fall: f64,
    },
    Switch {
        off: f64,
        on: f64,
        roff: f64,
        ron: f64,
        log: bool,
    },
    Gain,
    Summer,
    Mult,
    Divide {
        minimum: f64,
        domain: f64,
    },
    Limit(Limits),
    ControlledLimit {
        limits: Limits,
        lower_delta: f64,
        upper_delta: f64,
    },
    Curve(Curve),
    Derivative {
        limits: Limits,
        offset: f64,
    },
    Integral {
        initial: f64,
        limits: Limits,
    },
    Filter {
        den: Vec<f64>,
        num: Vec<f64>,
        frequency: f64,
        initial: Vec<f64>,
    },
}
#[derive(Debug)]
pub struct Analog {
    name: String,
    inputs: Vec<Terminal>,
    output: Terminal,
    offsets: Vec<f64>,
    gains: Vec<f64>,
    out_gain: f64,
    out_offset: f64,
    transfer: Transfer,
    states: Vec<Node>,
}

impl Analog {
    pub fn new(
        name: &str,
        ports: &[CodePort],
        m: &ModelCard,
        node: impl Fn(&str) -> Node,
    ) -> Result<Self, Diagnostic> {
        let k = m.kind.as_str();
        m.check_code_model_params()?;
        let nports = match k {
            "divide" => 3,
            "climit" => 4,
            _ => 2,
        };
        if ports.len() != nports {
            return Err(port_error(format!("{k} needs {nports} ports")));
        }
        let mut ins = Vec::new();
        let mut outs = Vec::new();
        for (index, p) in ports.iter().enumerate() {
            let vector = index == 0 && matches!(k, "summer" | "mult");
            if p.vector != vector || p.nodes.is_empty() {
                return Err(port_error(format!(
                    "{k} port {} needs a connected {} port",
                    index + 1,
                    if vector { "vector" } else { "scalar" }
                )));
            }
            let current = if k == "aswitch" && index == 1 {
                if !matches!(p.modifier.as_str(), "" | "g" | "gd" | "h" | "hd") {
                    return Err(port_error(
                        "aswitch requires a conductance or resistance port",
                    ));
                }
                true
            } else {
                match p.modifier.as_str() {
                    "" | "v" | "vd" => false,
                    "i" | "id" => true,
                    _ => return Err(port_error(format!("%{} is invalid for {k}", p.modifier))),
                }
            };
            let size = if p.differential() { 2 } else { 1 };
            let terminals = if index + 1 == nports {
                &mut outs
            } else {
                &mut ins
            };
            if !p.nodes.len().is_multiple_of(size) || !vector && p.nodes.len() != size {
                return Err(port_error("incorrect port width"));
            }
            for t in p.nodes.chunks_exact(size) {
                terminals.push(Terminal {
                    pair: [node(&t[0]), t.get(1).and_then(|n| node(n))],
                    current,
                    branch: None,
                });
            }
        }
        let output = outs.pop().ok_or_else(|| port_error("missing output"))?;
        if k == "aswitch" {
            ins.push(Terminal {
                pair: output.pair,
                current: false,
                branch: None,
            });
        }
        let p = ModelParams::new(&m.params);
        let mut used_arrays = Vec::new();
        let mut array = |key: &'static str, default: Vec<f64>| {
            used_arrays.push(key);
            m.arrays
                .iter()
                .rev()
                .find(|(n, _)| n == key)
                .map_or(default, |(_, v)| v.clone())
        };
        let count = ins.len();
        let mut offsets = vec![0.; count];
        let mut gains = vec![1.; count];
        let mut out_gain = 1.;
        let mut out_offset = 0.;
        let mut order = 0;
        let transfer = match k {
            "sine" | "triangle" | "square" => {
                order = 1;
                Transfer::Oscillator(Oscillator::new(
                    k,
                    &p,
                    array("cntl_array", vec![]),
                    array("freq_array", vec![]),
                )?)
            }
            "slew" => {
                let rise = p.get("rise_slope", 1e9);
                let fall = p.get("fall_slope", 1e9);
                if rise <= 0. || fall <= 0. {
                    return Err(invalid("slew needs positive rise and fall slopes"));
                }
                Transfer::Slew { rise, fall }
            }
            "aswitch" => {
                let on = p.get("cntl_on", 1.);
                let off = p.get("cntl_off", 0.);
                let ron = p.get("r_on", 1.);
                let roff = p.get("r_off", 1e6);
                if on == off || ron <= 0. || roff <= 0. {
                    return Err(invalid(
                        "aswitch needs distinct control levels and positive resistances",
                    ));
                }
                Transfer::Switch {
                    off,
                    on,
                    roff,
                    ron,
                    log: p.get("log", 1.) != 0.,
                }
            }
            "summer" | "mult" => {
                offsets = array("in_offset", offsets);
                gains = array("in_gain", gains);
                out_gain = p.get("out_gain", 1.);
                out_offset = p.get("out_offset", 0.);
                if k == "summer" {
                    Transfer::Summer
                } else {
                    Transfer::Mult
                }
            }
            "divide" => {
                offsets = vec![p.get("num_offset", 0.), p.get("den_offset", 0.)];
                gains = vec![p.get("num_gain", 1.), p.get("den_gain", 1.)];
                out_gain = p.get("out_gain", 1.);
                out_offset = p.get("out_offset", 0.);
                let minimum = p.get("den_lower_limit", 1e-10);
                let domain = p.get("den_domain", 1e-16);
                if minimum <= 0. || domain < 0. || domain > minimum {
                    return Err(invalid("invalid divide denominator limits"));
                }
                Transfer::Divide { minimum, domain }
            }
            "pwl" => Transfer::Curve(Curve::new(
                array("x_array", vec![]),
                array("y_array", vec![]),
                p.get("input_domain", 0.01),
                p.get("fraction", 1.) != 0.,
            )?),
            "s_xfer" => {
                let mut den = array("den_coeff", vec![]);
                let mut num = array("num_coeff", vec![]);
                if num.is_empty() || num.len() > den.len() || den.first().is_none_or(|v| *v == 0.) {
                    return Err(invalid(
                        "s_xfer needs a proper rational transfer with nonzero leading denominator",
                    ));
                }
                let lead = den[0];
                for a in den.iter_mut().chain(&mut num) {
                    *a /= lead;
                }
                den.reverse();
                num.reverse();
                num.resize(den.len(), 0.);
                order = den.len() - 1;
                let initial = array("int_ic", vec![0.; order]);
                let frequency = p.get("denormalized_freq", 1.);
                if frequency <= 0. || initial.len() != order {
                    return Err(invalid("invalid s_xfer frequency or integrator IC count"));
                }
                out_gain = p.get("gain", 1.);
                offsets[0] = p.get("in_offset", 0.);
                Transfer::Filter {
                    den,
                    num,
                    frequency,
                    initial,
                }
            }
            "gain" | "limit" | "climit" | "int" | "d_dt" => {
                offsets[0] = p.get("in_offset", 0.);
                gains[0] = p.get("gain", 1.);
                let fixed_limits = || {
                    Limits::new(
                        &p,
                        p.get("out_lower_limit", -1.),
                        p.get("out_upper_limit", 1.),
                    )
                };
                match k {
                    "gain" => {
                        out_offset = p.get("out_offset", 0.);
                        Transfer::Gain
                    }
                    "limit" => Transfer::Limit(fixed_limits()?),
                    "climit" => Transfer::ControlledLimit {
                        limits: Limits::new(&p, -1., 1.)?,
                        lower_delta: p.get("out_lower_delta", 0.),
                        upper_delta: p.get("out_upper_delta", 0.),
                    },
                    "int" => {
                        order = 1;
                        Transfer::Integral {
                            initial: p.get("out_ic", 0.),
                            limits: fixed_limits()?,
                        }
                    }
                    _ => {
                        order = 1;
                        Transfer::Derivative {
                            limits: fixed_limits()?,
                            offset: p.get("out_offset", 0.),
                        }
                    }
                }
            }
            _ => {
                return Err(Diagnostic::error(
                    "E-XSPICE-MODEL",
                    format!("unsupported code model {k}"),
                ));
            }
        };
        if offsets.len() != count || gains.len() != count {
            return Err(invalid("input gains and offsets must match vector width"));
        }
        if let Some(key) = p.unused().first().cloned().or_else(|| {
            m.arrays
                .iter()
                .find(|(key, _)| !used_arrays.contains(&key.as_str()))
                .map(|(k, _)| k.clone())
        }) {
            return Err(invalid(format!("unsupported {k} parameter {key}")));
        }
        if m.params
            .iter()
            .map(|(_, v)| v)
            .chain(m.arrays.iter().flat_map(|(_, v)| v))
            .any(|v| !v.is_finite())
        {
            return Err(invalid("nonfinite code-model parameter"));
        }
        Ok(Self {
            name: name.into(),
            inputs: ins,
            output,
            offsets,
            gains,
            out_gain,
            out_offset,
            transfer,
            states: vec![None; order],
        })
    }

    fn input(&self, i: usize, x: &[f64], seed: Option<usize>) -> D {
        let [a, b] = self.inputs[i].controls();
        let value = volt(x, a) - volt(x, b);
        let v = if seed == Some(i) {
            D::var(value, 0)
        } else {
            D::cst(value)
        };
        (v + self.offsets[i]) * self.gains[i]
    }
    fn equation(
        &self,
        row: usize,
        ctx: &EvalCtx,
        x: &[f64],
        seed: Option<usize>,
        history: &[f64],
    ) -> (D, D) {
        let input = |i| self.input(i, x, seed);
        let state = |i: usize| {
            let v = volt(x, self.states[i]);
            if seed == Some(self.inputs.len() + i) {
                D::var(v, 0)
            } else {
                D::cst(v)
            }
        };
        let u = input(0);
        let zero = D::cst(0.);
        let startup = matches!(ctx.mode, Mode::Dc) || ctx.mode == Mode::Tran && ctx.time == 0.;
        if row != 0 {
            let i = row - 1;
            let z = state(i);
            return match &self.transfer {
                Transfer::Oscillator(o) => (
                    if ctx.mode == Mode::Tran && !startup {
                        -o.frequency(u)
                    } else {
                        z
                    },
                    z,
                ),
                Transfer::Derivative { .. } => (z, -u),
                Transfer::Integral { initial, .. } => (if startup { z - *initial } else { -u }, z),
                Transfer::Filter {
                    den,
                    frequency,
                    initial,
                    ..
                } => {
                    let f = if i + 1 < self.states.len() {
                        -state(i + 1)
                    } else {
                        (0..self.states.len()).fold(-u, |acc, j| acc + den[j] * state(j))
                    };
                    (
                        if startup && den[0] == 0. {
                            z - initial[i]
                        } else {
                            f
                        },
                        z / *frequency,
                    )
                }
                _ => (zero, zero),
            };
        }
        let value = match &self.transfer {
            Transfer::Oscillator(o) => o.value(ctx.mode, u, state(0)),
            Transfer::Slew { rise, fall } => {
                if ctx.mode == Mode::Tran && ctx.time > 0. {
                    let dt = (ctx.time - history[0]).max(0.);
                    let lo = history[1] - dt * fall;
                    let hi = history[1] + dt * rise;
                    if u.v < lo || u.v > hi {
                        D::cst(u.v.clamp(lo, hi))
                    } else {
                        u
                    }
                } else {
                    u
                }
            }
            Transfer::Switch {
                off,
                on,
                roff,
                ron,
                log,
            } => {
                let t = (u - *off) / (on - off);
                let r = if t.v <= 0. {
                    D::cst(*roff)
                } else if t.v >= 1. {
                    D::cst(*ron)
                } else if *log {
                    (roff.ln() + t * (ron.ln() - roff.ln())).exp()
                } else {
                    *roff + t * (ron - roff)
                };
                input(1) / r
            }
            Transfer::Gain => u,
            Transfer::Summer => (0..self.inputs.len()).fold(zero, |a, i| a + input(i)),
            Transfer::Mult => (0..self.inputs.len()).fold(D::cst(1.), |a, i| a * input(i)),
            Transfer::Divide { minimum, domain } => {
                let v = input(1);
                let sign = if v.v < 0. { -1. } else { 1. };
                u / (sign
                    * corner(
                        sign * v,
                        D::cst(*minimum),
                        D::cst(*minimum),
                        0.,
                        1.,
                        D::cst(*domain),
                    ))
            }
            Transfer::Limit(l) => l.fixed(u),
            Transfer::ControlledLimit {
                limits,
                lower_delta,
                upper_delta,
            } => {
                let hi = input(1) - *upper_delta;
                let lo = input(2) + *lower_delta;
                if hi.v <= lo.v {
                    (hi + lo) * 0.5
                } else {
                    limits.eval(u, lo, hi)
                }
            }
            Transfer::Curve(c) => c.eval(u),
            Transfer::Integral { limits, .. } => limits.fixed(state(0)),
            Transfer::Derivative { limits, offset } => limits.fixed(state(0) + *offset),
            Transfer::Filter { den, num, .. } => {
                let n = self.states.len();
                (0..n).fold(num[n] * u, |acc, i| {
                    acc + (num[i] - num[n] * den[i]) * state(i)
                })
            }
        };
        (self.out_offset + self.out_gain * value, zero)
    }
    fn variables(&self) -> impl Iterator<Item = [Node; 2]> + '_ {
        self.inputs
            .iter()
            .map(Terminal::controls)
            .chain(self.states.iter().map(|&n| [n, None]))
    }
}
impl DeviceGroup for Analog {
    fn type_name(&self) -> &'static str {
        "analog code model"
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn len(&self) -> usize {
        1
    }
    fn is_linear(&self) -> bool {
        matches!(
            self.transfer,
            Transfer::Gain | Transfer::Summer | Transfer::Filter { .. }
        )
    }
    fn state_len(&self) -> usize {
        if matches!(self.transfer, Transfer::Slew { .. }) {
            2
        } else {
            0
        }
    }
    fn step_limits(&self, _: &crate::TranParams, out: &mut Vec<(f64, f64)>) {
        if let Transfer::Oscillator(o) = &self.transfer {
            out.push((0., o.max_step()));
        }
    }
    fn has_crossings(&self) -> bool {
        matches!(
            self.transfer,
            Transfer::Oscillator(_)
                | Transfer::Slew { .. }
                | Transfer::Curve(_)
                | Transfer::Limit(_)
                | Transfer::ControlledLimit { .. }
                | Transfer::Switch { .. }
        )
    }
    fn crossing(&self, old: &[f64], new: &[f64], accepted: &[f64], current: &[f64]) -> Option<f64> {
        let endpoints = [old, new];
        let input = endpoints.map(|x| self.input(0, x, None).v);
        let mut first: Option<f64> = None;
        // Interpolate the signed distance from an edge, including moving rails.
        let mut edge = |threshold: [f64; 2]| {
            let a = input[0] - threshold[0];
            let b = input[1] - threshold[1];
            if a != 0. && (b == 0. || a.is_sign_positive() != b.is_sign_positive()) {
                let fraction = a / (a - b);
                if fraction > 64. * f64::EPSILON {
                    first = Some(first.map_or(fraction, |v| v.min(fraction)));
                }
            }
        };
        match &self.transfer {
            Transfer::Slew { rise, fall } => {
                // Locate where the rate-limited ramp catches the linearly interpolated input.
                let [time, output] = [accepted[0], accepted[1]];
                let dt = current[0] - time;
                if input[0] > output {
                    edge([output, output + rise * dt]);
                } else if input[0] < output {
                    edge([output, output - fall * dt]);
                }
            }
            Transfer::Oscillator(o) => {
                let phase = endpoints.map(|x| volt(x, self.states[0]));
                let frequency = input.map(|v| o.frequency(D::cst(v)).v);
                return o.crossing(phase, frequency);
            }
            Transfer::Switch { off, on, .. } => {
                edge([*off; 2]);
                edge([*on; 2]);
            }
            Transfer::Curve(c) => {
                for i in 1..c.x.len() - 1 {
                    edge([c.x[i] - c.widths[i]; 2]);
                    edge([c.x[i] + c.widths[i]; 2]);
                }
            }
            Transfer::Limit(l) | Transfer::ControlledLimit { limits: l, .. } => {
                let bounds = endpoints.map(|x| {
                    let (lo, hi) = match self.transfer {
                        Transfer::ControlledLimit {
                            lower_delta,
                            upper_delta,
                            ..
                        } => (
                            self.input(2, x, None).v + lower_delta,
                            self.input(1, x, None).v - upper_delta,
                        ),
                        _ => (l.lo, l.hi),
                    };
                    let width = l.width(D::cst(lo), D::cst(hi)).v;
                    [lo - width, lo + width, hi - width, hi + width]
                });
                for i in 0..4 {
                    edge([bounds[0][i], bounds[1][i]]);
                }
            }
            _ => {}
        }
        first
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (i, p) in self
            .inputs
            .iter_mut()
            .enumerate()
            .filter(|(_, p)| p.current)
        {
            p.branch = Some(u.alloc_branch(&format!("{}:sense{i}", self.name)));
        }
        self.output.branch = Some(u.alloc_branch(&self.name));
        for (i, z) in self.states.iter_mut().enumerate() {
            *z = Some(u.alloc_operator_state(&self.name, &format!("state{i}")));
        }
    }
    fn initial_values(&self, x: &mut [f64], _: &mut Vec<(Node, Node, f64)>) {
        for (i, &z) in self.states.iter().enumerate() {
            if let Some(j) = z {
                x[j] = match &self.transfer {
                    Transfer::Integral { initial, .. } => *initial,
                    Transfer::Filter { initial, .. } => initial[i],
                    _ => 0.,
                };
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for p in self
            .inputs
            .iter()
            .filter(|p| p.current)
            .chain([&self.output])
        {
            let [a, n] = p.pair;
            b.f(a);
            b.f(n);
            b.f(p.branch);
            b.g(a, p.branch);
            b.g(n, p.branch);
            b.g(p.branch, a);
            b.g(p.branch, n);
            b.g(p.branch, p.branch);
        }
        for (r, z) in std::iter::once(self.output.branch)
            .chain(self.states.iter().copied())
            .enumerate()
        {
            if r != 0 {
                b.f(z);
                b.tracked_q(z);
            }
            for pair in self.variables() {
                for c in pair {
                    b.g(z, c);
                    if r != 0 {
                        b.c(z, c);
                    }
                }
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], history: StateRef<'_>, out: &mut Slots) {
        let mut f = 0;
        let mut g = 0;
        for (p, current_output) in self
            .inputs
            .iter()
            .filter(|p| p.current)
            .map(|p| (p, false))
            .chain([(&self.output, self.output.current)])
        {
            let i = volt(x, p.branch);
            let v = volt(x, p.pair[0]) - volt(x, p.pair[1]);
            out.f[f..f + 3].copy_from_slice(&[i, -i, if current_output { i } else { v }]);
            out.g[g..g + 5].copy_from_slice(if current_output {
                &[1., -1., 0., 0., 1.]
            } else {
                &[1., -1., 1., -1., 0.]
            });
            f += 3;
            g += 5;
        }
        let mut c = 0;
        for r in 0..=self.states.len() {
            let (v, q) = self.equation(r, ctx, x, None, history.accepted);
            let sign = if r == 0 { -1. } else { 1. };
            if r == 0 {
                if matches!(self.transfer, Transfer::Slew { .. }) {
                    history.current.copy_from_slice(&[ctx.time, v.v]);
                }
                out.f[f - 1] -= v.v;
            } else {
                out.f[f] = v.v;
                f += 1;
                out.q[r - 1] = q.v;
            }
            for j in 0..self.inputs.len() + self.states.len() {
                let (v, q) = self.equation(r, ctx, x, Some(j), history.accepted);
                let d = sign * v.d[0];
                out.g[g..g + 2].copy_from_slice(&[d, -d]);
                g += 2;
                if r != 0 {
                    out.c[c..c + 2].copy_from_slice(&[q.d[0], -q.d[0]]);
                    c += 2;
                }
            }
        }
        if out
            .f
            .iter()
            .chain(&out.q)
            .chain(&out.g)
            .chain(&out.c)
            .any(|v| !v.is_finite())
        {
            out.error = Some(invalid(format!(
                "{}: nonfinite code-model equation",
                self.name
            )));
        }
    }
}

/// Translate each file column to an independent source, sharing the native time-grid logic.
pub fn file_sources(
    ports: &[CodePort],
    model: &ModelCard,
) -> Result<Vec<sansho_netlist::SourceSpec>, Diagnostic> {
    model.check_code_model_params()?;
    let [port] = ports else {
        return Err(port_error("filesource requires one output vector"));
    };
    if !port.vector || !matches!(port.modifier.as_str(), "" | "v" | "vd" | "i" | "id") {
        return Err(port_error(
            "filesource requires a voltage or current output vector",
        ));
    }
    let count = port.nodes.len() / if port.differential() { 2 } else { 1 };
    let table = model
        .table
        .as_ref()
        .ok_or_else(|| invalid("filesource needs file"))?;
    let p = ModelParams::new(&model.params);
    let relative = p.get("timerelative", 0.) != 0.;
    let step = p.get("amplstep", 0.) != 0.;
    let scale = p.get("timescale", 1.);
    let offset = p.get("timeoffset", 0.);
    let amplitudes = |key: &str, default: f64| -> Result<Vec<f64>, Diagnostic> {
        let v = model
            .arrays
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map_or_else(|| vec![default; count], |(_, a)| a.clone());
        if v.len() != count {
            return Err(invalid(format!("{key} must match the number of channels")));
        }
        Ok(v)
    };
    let gains = amplitudes("amplscale", 1.)?;
    let offsets = amplitudes("amploffset", 0.)?;
    if !p.unused().is_empty()
        || model
            .arrays
            .iter()
            .any(|(k, _)| !matches!(k.as_str(), "amploffset" | "amplscale"))
    {
        return Err(invalid("unsupported filesource parameter"));
    }
    let mut cols = vec![Vec::<(f64, f64)>::new(); count];
    let mut source_time = 0.;
    let mut previous = f64::NEG_INFINITY;
    for row in &table.rows {
        if row.len() != count + 1 {
            return Err(invalid(
                "filesource output count differs from waveform columns",
            ));
        }
        source_time = if relative {
            source_time + row[0]
        } else {
            row[0]
        };
        let t = offset + scale * source_time;
        if !t.is_finite() || t <= previous {
            return Err(invalid("filesource times must be finite and increasing"));
        }
        for (j, col) in cols.iter_mut().enumerate() {
            let value = offsets[j] + gains[j] * row[j + 1];
            if !value.is_finite() {
                return Err(invalid("nonfinite scaled filesource amplitude"));
            }
            if step
                && let Some(&(_, old)) = col.last()
                && t.next_down() > previous
            {
                col.push((t.next_down(), old));
            }
            col.push((t, value));
        }
        previous = t;
    }
    Ok(cols
        .into_iter()
        .map(|pts| sansho_netlist::SourceSpec {
            tran: Some(sansho_netlist::TranFn::Pwl(pts)),
            ..Default::default()
        })
        .collect())
}
