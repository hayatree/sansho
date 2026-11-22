//! KSPICE single and coupled uniform transmission lines.
//! Provenance: ngspice 44 manual §§6.4, 27.4.1, 27.4.4 (interface),
//! Paul, Analysis of Multiconductor Transmission Lines (modal coordinates);
//! Lin and Kuh (1992), delay extraction and recursive convolution.

mod coupled;
mod filter;
mod matrix;

use filter::{Kernel, Memory};
use matrix::Matrix;
use sansho_devices::{
    DeviceGroup, EvalCtx, Mode, ModelParams, Node, Slots, StampBuilder, StateRef, Unknowns, volt,
};
use sansho_diag::Diagnostic;
use sansho_netlist::ModelCard;
use sansho_sparse::c64;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub(crate) struct Constants {
    r: Matrix<f64>,
    l: Matrix<f64>,
    g: Matrix<f64>,
    c: Matrix<f64>,
    impedance: f64,
}
impl Constants {
    /// Coefficients of V - Zref I = S (V + Zref I), interleaved by port.
    pub fn coefficients(&self, omega: f64) -> Vec<c64> {
        let n = self.r.n;
        let z = Matrix::build(n, |i, j| {
            c64::new(self.r[(i, j)], omega * self.l[(i, j)]) / self.impedance
        });
        let y = Matrix::build(n, |i, j| {
            c64::new(self.g[(i, j)], omega * self.c[(i, j)]) * self.impedance
        });
        let s = matrix::scattering(&z, &y);
        let mut rows = Vec::with_capacity(2 * s.data.len());
        for i in 0..2 * n {
            for j in 0..2 * n {
                let identity = c64::from(f64::from(i == j));
                rows.extend([
                    identity - s[(i, j)],
                    (identity + s[(i, j)]) * (-self.impedance),
                ]);
            }
        }
        rows
    }
}

#[derive(Debug)]
struct LineMode {
    voltage: Vec<f64>,
    current: Vec<f64>,
    dual: bool,
    kernel: Kernel,
}
impl LineMode {
    fn sample(&self, port: &[[f64; 2]]) -> [f64; 2] {
        let mut v = 0.;
        let mut i = 0.;
        for ((p, a), b) in port.iter().zip(&self.voltage).zip(&self.current) {
            v += p[0] * a;
            i += p[1] * b;
        }
        if self.dual { [i, v] } else { [v, i] }
    }
}

#[derive(Debug)]
pub(crate) struct KLine {
    name: String,
    kind: &'static str,
    ports: Vec<[Node; 2]>,
    currents: Vec<Node>,
    pub constants: Constants,
    dc: Vec<f64>,
    modes: Vec<LineMode>,
    coupled_loss: Option<(Matrix<f64>, Matrix<f64>)>,
}

fn unpack(
    card: &ModelCard,
    p: &ModelParams,
    key: &str,
    n: usize,
    length: f64,
) -> Result<Matrix<f64>, Diagnostic> {
    let loss = matches!(key, "r" | "g");
    let zeros = vec![0.; n];
    let scalar = [p.get(key, 0.)];
    let values = if card.kind == "txl" {
        scalar.as_slice()
    } else {
        card.arrays
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map_or(zeros.as_slice(), |(_, v)| v.as_slice())
    };
    let mut m = Matrix::build(n, |_, _| 0.);
    if values.len() == n && loss {
        for (i, &v) in values.iter().enumerate() {
            m[(i, i)] = v * length;
        }
    } else if values.len() == n * (n + 1) / 2 {
        let mut k = 0;
        for i in 0..n {
            for j in i..n {
                m[(i, j)] = values[k] * length;
                m[(j, i)] = m[(i, j)];
                k += 1;
            }
        }
    } else {
        return Err(Diagnostic::error(
            "E-LINE-MATRIX",
            format!(
                "{}: {key} has {} entries for {n} conductors",
                card.name,
                values.len()
            ),
        ));
    }
    let (eig, _) = m.eigen();
    let scale = m.data.iter().map(|v| v.abs()).fold(0., f64::max);
    if m.data.iter().any(|v| !v.is_finite())
        || eig.iter().any(|&v| {
            !v.is_finite()
                || if loss {
                    v < -8. * f64::EPSILON * scale
                } else {
                    v <= 0.
                }
        })
    {
        let requirement = if loss {
            "positive semidefinite"
        } else {
            "positive definite"
        };
        return Err(Diagnostic::error(
            "E-LINE-MATRIX",
            format!("{}: {key} must be finite and {requirement}", card.name),
        ));
    }
    Ok(m)
}

/// A repeated lossless velocity leaves its modal basis undetermined. Resolve
/// that freedom using the first-order attenuation within the eigenspace.
fn resolve_equal_delays(times_squared: &[f64], rm: &Matrix<f64>, gm: &Matrix<f64>) -> Matrix<f64> {
    let n = times_squared.len();
    let mut rotation = Matrix::build(n, |i, j| f64::from(i == j));
    let mut assigned = vec![false; n];
    for i in 0..n {
        if assigned[i] {
            continue;
        }
        let group: Vec<_> = (i..n)
            .filter(|&j| {
                !assigned[j]
                    && (times_squared[i] - times_squared[j]).abs()
                        <= 32. * f64::EPSILON * times_squared[i]
            })
            .collect();
        if group.len() < 2 {
            continue;
        }
        let attenuation = Matrix::build(group.len(), |i, j| {
            let (a, b) = (group[i], group[j]);
            rm[(a, b)] + gm[(a, b)] / (times_squared[a].sqrt() * times_squared[b].sqrt())
        });
        let (_, q) = attenuation.eigen();
        for (k, &a) in group.iter().enumerate() {
            assigned[a] = true;
            for (j, &b) in group.iter().enumerate() {
                rotation[(a, b)] = q[(k, j)];
            }
        }
    }
    rotation
}

impl KLine {
    pub fn from_group(g: &dyn DeviceGroup) -> Option<&Self> {
        (g as &dyn std::any::Any).downcast_ref()
    }
    pub fn new(
        name: &str,
        nodes: &[Node],
        card: &ModelCard,
        p: &ModelParams,
        inst: &ModelParams,
    ) -> Result<Self, Diagnostic> {
        let n = nodes.len() / 2;
        let error = |msg: &str| Diagnostic::error("E-LINE-PARAM", format!("{name}: {msg}"));
        if !(2..=9).contains(&n) || nodes.len() != 2 * n || (card.kind == "txl" && n != 2) {
            return Err(error("invalid terminal count"));
        }
        let ports = nodes
            .chunks_exact(n)
            .flat_map(|end| end[..n - 1].iter().map(|&v| [v, end[n - 1]]))
            .collect::<Vec<_>>();
        let n = n - 1;
        let model_length = p.get("length", 0.);
        let length = inst.opt(&["len", "length"]).unwrap_or(model_length);
        if ![model_length, length]
            .iter()
            .all(|&v| v.is_finite() && v > 0.)
        {
            return Err(error(
                "model LENGTH and effective length must be finite and positive",
            ));
        }
        let r = unpack(card, p, "r", n, length)?;
        let l = unpack(card, p, "l", n, length)?;
        let g = unpack(card, p, "g", n, length)?;
        let c = unpack(card, p, "c", n, length)?;

        // L = B B^T. Diagonalize the symmetric B^T C B. The columns of
        // B Q transform modal voltages; B^-T Q is its power-dual for currents.
        let (inductance, u) = l.eigen();
        let b = Matrix::build(n, |i, j| u[(i, j)] * inductance[j].sqrt());
        let dual = Matrix::build(n, |i, j| u[(i, j)] / inductance[j].sqrt());
        let (times_squared, q) = b.transpose().product(&c).product(&b).eigen();
        let tv = b.product(&q);
        let ti = dual.product(&q);
        let rm = ti.transpose().product(&r).product(&ti);
        let gm = tv.transpose().product(&g).product(&tv);
        let rotation = resolve_equal_delays(&times_squared, &rm, &gm);
        let tv = tv.product(&rotation);
        let ti = ti.product(&rotation);
        let rm = rotation.transpose().product(&rm).product(&rotation);
        let gm = rotation.transpose().product(&gm).product(&rotation);
        let mut modes = Vec::with_capacity(n);
        for j in 0..n {
            let tau = times_squared[j].sqrt();
            let a = rm[(j, j)].max(0.);
            let b = gm[(j, j)].max(0.) / times_squared[j];
            if !(tau > 0. && tau.is_finite() && a.is_finite() && b.is_finite()) {
                return Err(error(
                    "modal flight time or attenuation is outside numerical range",
                ));
            }
            modes.push(LineMode {
                voltage: (0..n).map(|i| ti[(i, j)] * tau.sqrt()).collect(),
                current: (0..n).map(|i| tv[(i, j)] / tau.sqrt()).collect(),
                dual: a < b,
                kernel: Kernel::new(tau, a.max(b), a.min(b)),
            });
        }
        // In power-normalized coordinates the lossless Z and Y are both
        // diag(tau)*s. Only simultaneously diagonal losses admit scalar filters.
        let normalize = |m: &Matrix<f64>, series: bool| {
            Matrix::build(n, |i, j| {
                let scale = modes[i].kernel.delay.sqrt() * modes[j].kernel.delay.sqrt();
                if series {
                    m[(i, j)] * scale
                } else {
                    m[(i, j)] / scale
                }
            })
        };
        let series = normalize(&rm, true);
        let shunt = normalize(&gm, false);
        let coupled = |m: &Matrix<f64>| {
            let norm = m.data.iter().map(|v| v.abs()).fold(0., f64::max);
            (0..n).any(|i| (0..i).any(|j| m[(i, j)].abs() > 32. * f64::EPSILON * norm))
        };
        let coupled_loss = (coupled(&series) || coupled(&shunt)).then_some((series, shunt));
        let impedance = l[(0, 0)].sqrt() / c[(0, 0)].sqrt();
        let constants = Constants {
            r,
            l,
            g,
            c,
            impedance,
        };
        let dc: Vec<_> = constants.coefficients(0.).iter().map(|v| v.re).collect();
        if dc.iter().any(|v| !v.is_finite()) {
            return Err(error("static line equations exceed numerical range"));
        }
        Ok(Self {
            name: name.into(),
            kind: if card.kind == "txl" { "txl" } else { "cpl" },
            currents: vec![None; 2 * n],
            ports,
            constants,
            dc,
            modes,
            coupled_loss,
        })
    }

    pub fn delays(&self) -> impl Iterator<Item = f64> + '_ {
        self.modes.iter().map(|m| m.kernel.delay)
    }

    fn sample(&self, x: &[f64]) -> Vec<[f64; 2]> {
        self.ports
            .iter()
            .zip(&self.currents)
            .map(|(&[p, n], &i)| [volt(x, p) - volt(x, n), volt(x, i)])
            .collect()
    }

    // First two slots in each row inject the physical branch current into
    // KCL. Each remaining triple multiplies V+, Vref, and I respectively.
    pub fn entries(&self) -> impl Iterator<Item = (Node, Node, usize, f64)> + '_ {
        self.currents.iter().enumerate().flat_map(move |(r, &row)| {
            self.ports
                .iter()
                .zip(&self.currents)
                .enumerate()
                .flat_map(move |(c, (&[p, n], &i))| {
                    let k = 2 * (r * self.ports.len() + c);
                    [(row, p, k, 1.), (row, n, k, -1.), (row, i, k + 1, 1.)]
                })
        })
    }
    fn stamp(&self, coefficients: &[f64], out: &mut Slots) {
        for (row, values) in out
            .g
            .chunks_exact_mut(2 + 3 * self.ports.len())
            .zip(coefficients.chunks_exact(2 * self.ports.len()))
        {
            for (dst, src) in row[2..]
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(values.as_chunks::<2>().0)
            {
                dst.copy_from_slice(&[src[0], -src[0], src[1]]);
            }
        }
    }
}

impl DeviceGroup for KLine {
    fn type_name(&self) -> &'static str {
        self.kind
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (j, i) in self.currents.iter_mut().enumerate() {
            *i = Some(u.alloc_branch(&format!("{}#{}", self.name, j + 1)));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (&[p, n], &row) in self.ports.iter().zip(&self.currents) {
            b.g(p, row);
            b.g(n, row);
            for (&[p, n], &i) in self.ports.iter().zip(&self.currents) {
                b.g(row, p);
                b.g(row, n);
                b.g(row, i);
            }
            b.f(p);
            b.f(n);
            b.f(row);
            b.b(row);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        let ac = matches!(ctx.mode, Mode::Ac | Mode::Noise);
        if ac {
            out.g.fill(0.);
        }
        if !ac && (ctx.mode != Mode::Tran || ctx.time == 0.) {
            self.stamp(&self.dc, out);
        }
        if ctx.mode != Mode::Tran || ctx.time == 0. {
            out.b.fill(0.);
        }
        let sample = self.sample(x);
        for (i, row) in out.g.chunks_exact_mut(2 + 3 * sample.len()).enumerate() {
            row[..2].copy_from_slice(&[1., -1.]);
            let equation = row[2..]
                .as_chunks::<3>()
                .0
                .iter()
                .zip(&sample)
                .map(|(g, p)| g[0] * p[0] + g[2] * p[1])
                .sum();
            out.f[3 * i..3 * i + 3].copy_from_slice(&[sample[i][1], -sample[i][1], equation]);
        }
    }
}

struct ModeHistory {
    memory: [Memory; 2],
    samples: VecDeque<(f64, [f64; 2])>,
}
pub(crate) struct History<'a> {
    pub line: &'a KLine,
    initial: Vec<[f64; 2]>,
    modes: Vec<ModeHistory>,
    coupled: Option<coupled::Coupled>,
    time: f64,
}
impl<'a> History<'a> {
    pub fn new(line: &'a KLine, x: &[f64], uic: bool, step: f64) -> Self {
        let initial = if uic {
            vec![[0.; 2]; line.ports.len()]
        } else {
            line.sample(x)
        };
        let modes = line
            .modes
            .iter()
            .filter(|_| line.coupled_loss.is_none())
            .map(|m| ModeHistory {
                memory: std::array::from_fn(|_| m.kernel.memory()),
                samples: VecDeque::new(),
            })
            .collect();
        let coupled = line
            .coupled_loss
            .as_ref()
            .map(|(r, g)| coupled::Coupled::new(r, g, line.delays().collect(), step));
        let mut h = Self {
            coupled,
            line,
            initial,
            modes,
            time: 0.,
        };
        h.accept(0., x);
        h
    }
    pub fn step_limit(&self) -> f64 {
        self.coupled
            .as_ref()
            .map_or(f64::INFINITY, coupled::Coupled::step_limit)
    }
    pub fn accept(&mut self, time: f64, x: &[f64]) {
        let mut sample = self.line.sample(x);
        for (p, i) in sample.iter_mut().zip(&self.initial) {
            p[0] -= i[0];
            p[1] -= i[1];
        }
        let n = self.line.modes.len();
        if let Some(coupled) = &mut self.coupled {
            let outgoing = sample
                .chunks_exact(n)
                .flat_map(|port| {
                    self.line
                        .modes
                        .iter()
                        .map(|m| m.sample(port).into_iter().sum())
                })
                .collect();
            coupled.accept(time, outgoing);
            return;
        }
        for (m, h) in self.line.modes.iter().zip(&mut self.modes) {
            let waves = std::array::from_fn(|k| {
                let [drive, other] = m.sample(&sample[k * n..(k + 1) * n]);
                m.kernel
                    .accept(&mut h.memory[k], time - self.time, drive, other)
            });
            h.samples.push_back((time, waves));
            while h.samples.len() > 1 && h.samples[1].0 <= time - m.kernel.delay {
                h.samples.pop_front();
            }
        }
        self.time = time;
    }
    pub fn trial(&self, t: f64, out: &mut Slots) {
        if let Some(coupled) = &self.coupled {
            for j in 0..self.line.modes.len() {
                for side in 0..2 {
                    self.stamp_mode(j, side, 1., coupled.incoming(t, side, j), false, out);
                }
            }
            return;
        }
        for (j, (m, h)) in self.line.modes.iter().zip(&self.modes).enumerate() {
            let retarded = t - m.kernel.delay;
            let next = h.samples.partition_point(|(t, _)| *t <= retarded);
            let (t0, left) = h.samples[next.saturating_sub(1)];
            let (t1, right) = h.samples.get(next).copied().unwrap_or((t0, left));
            let frac = if t1 > t0 {
                (retarded - t0) / (t1 - t0)
            } else {
                0.
            };
            for (side, memory) in h.memory.iter().enumerate() {
                let (offset, slope) = m.kernel.local(memory, t - self.time);
                let other = 1 - side;
                let incoming = if retarded < 0. {
                    0.
                } else {
                    left[other] + frac * (right[other] - left[other])
                };
                let sign = if m.dual { -1. } else { 1. };
                self.stamp_mode(j, side, slope, sign * incoming - offset, m.dual, out);
            }
        }
    }

    fn stamp_mode(&self, j: usize, side: usize, slope: f64, rhs: f64, dual: bool, out: &mut Slots) {
        let n = self.line.modes.len();
        let m = &self.line.modes[j];
        let row = side * n + j;
        out.b[row] = rhs;
        let width = 2 + 6 * n;
        let slots = &mut out.g[row * width + 2..(row + 1) * width];
        slots.fill(0.);
        for i in 0..n {
            let k = side * n + i;
            let (v, cur) = if dual {
                (-m.voltage[i], slope * m.current[i])
            } else {
                (slope * m.voltage[i], -m.current[i])
            };
            slots[3 * k..3 * k + 3].copy_from_slice(&[v, -v, cur]);
            out.b[row] += v * self.initial[k][0] + cur * self.initial[k][1];
        }
    }
}
