//! Fully coupled one-dimensional silicon drift–diffusion elements.
//! Provenance: Scharfetter & Gummel (1969), Selberherr (1984), Gates (1993),
//! and ngspice 44 manual chapter 26. Equations and parameter sources in docs/cider.md.

mod cards;
use crate::{
    CHARGE, DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StampLayout, StateRef,
    Unknowns, dual::Dual, model::ModelParams, volt,
};
use cards::{Structure, invalid};
use sansho_diag::Diagnostic;
use sansho_netlist::{Element, ElementKind, ModelCard};

// The ideal diffusion slope resolves the historical k/q ratio (see docs/cider.md).
const VT_PER_K: f64 = 8.617087e-5;
const REFERENCE_K: f64 = 300.15;
const GAP_NOMINAL: f64 = 1.1245;
const GAP_SLOPE: f64 = 4.73e-4;
const GAP_TREF: f64 = 636.;

// Relative DOS masses from the five isothermal silicon material reports.
// Interpolate mass between reports and use the endpoint mass outside their range.
fn density_of_states(temp: f64) -> [f64; 2] {
    const MASSES: [(f64, [f64; 2]); 5] = [
        (273.15, [1.1712, 1.1159]),
        (REFERENCE_K, [1.1824, 1.1406]),
        (328.15, [1.1937, 1.1639]),
        (358.15, [1.2053, 1.1869]),
        (383.15, [1.2147, 1.2045]),
    ];
    let hi = MASSES
        .partition_point(|&(t, _)| t < temp)
        .clamp(1, MASSES.len() - 1);
    let (t0, m0) = MASSES[hi - 1];
    let (t1, m1) = MASSES[hi];
    let f = ((temp - t0) / (t1 - t0)).clamp(0., 1.);
    std::array::from_fn(|k| {
        let mass = m0[k] + f * (m1[k] - m0[k]);
        [3.22603e19, 3.05625e19][k] * (temp / REFERENCE_K * mass / MASSES[1].1[k]).powf(1.5)
    })
}

fn end_ports(bipolar: bool) -> [usize; 2] {
    // External Q pins are C/B/E. The physical mesh runs from E to C.
    if bipolar { [2, 0] } else { [0, 1] }
}

const EPS_SI: f64 = 11.7 * 8.8541878128e-14;
type D = Dual<6>;
const ZERO: D = D::cst(0.);

#[derive(Debug)]
struct Vertex {
    nodes: [Node; 3],
    psi0: f64,
    ni: f64,
    net: f64,
    mu: [f64; 2],
    tau: [f64; 2],
    volume: f64,
    scale: [f64; 3],
    contact: bool,
}

#[derive(Debug)]
struct Edge {
    ends: [usize; 2],
    length: f64,
}

#[derive(Debug)]
struct Base {
    index: usize,
    carrier: usize,
    terminal: Node,
    mobility_geometry: f64,
}

/// One numerical device. Each vertex has an electrostatic and two quasi-Fermi potentials.
#[derive(Debug)]
pub struct Numerical {
    name: String,
    pub measurement_frequency: f64,
    points: Vec<Vertex>,
    edges: Vec<Edge>,
    base: Option<Base>,
    vt: f64,
    eps: f64,
    area: f64,
    srh: bool,
    auger: [f64; 2],
    fieldmob: bool,
    vsat: [f64; 2],
    sg_field: [bool; 2],
    off: bool,
    avalanche: bool,
}

impl Numerical {
    pub fn new(
        instance: &Element,
        pins: &[Node],
        card: &ModelCard,
        inst: &ModelParams,
        temp: f64,
        nominal: f64,
        warnings: &mut Vec<Diagnostic>,
    ) -> Result<Self, Diagnostic> {
        let s = Structure::bind(card, warnings)?;
        let bipolar = card.kind == "nbjt";
        if pins.len() != if bipolar { 3 } else { 2 } {
            return Err(invalid("incorrect numerical device terminal count"));
        }
        if (bipolar && (s.option("diode", 0.) != 0. || s.option("resistor", 0.) != 0.))
            || (!bipolar && s.option("bipolar", 0.) != 0.)
        {
            return Err(invalid("incompatible structure type"));
        }
        let temp = inst.get("temp", temp) + 273.15;
        let nominal = s.option("tnom", nominal) + 273.15;
        let mask_area = s.option("defa", 1.);
        let factor = inst.get("area", 1.);
        let area = 1e4 * mask_area * factor;
        if factor <= 0. || mask_area <= 0. {
            return Err(invalid("area must be positive"));
        }
        let vt = VT_PER_K * temp;
        let eps = s.material("permittivity", EPS_SI);
        let gap_shift = |t: f64| {
            s.material("deg.dt", GAP_SLOPE) * t * t / (t + s.material("eg.tref", GAP_TREF))
        };
        let bulk_nominal = GAP_NOMINAL
            + GAP_SLOPE
                * (REFERENCE_K.powi(2) / (REFERENCE_K + GAP_TREF)
                    - nominal.powi(2) / (nominal + GAP_TREF));
        let eg = s.material("eg", bulk_nominal) + gap_shift(nominal) - gap_shift(temp);
        let dos = density_of_states(temp);
        // An explicitly specified nominal DOS represents a fixed effective mass.
        let t = temp / nominal;
        let dos_scale = t.powf(1.5);
        let nc = s.material("nc", dos[0] / dos_scale) * dos_scale;
        let nv = s.material("nv", dos[1] / dos_scale) * dos_scale;
        let ni = (nc * nv).sqrt() * (-eg / (2. * vt)).exp();
        let lifetime_scale = (0.1 / (VT_PER_K * nominal) * (1. / t - 1.)).exp() / t.sqrt();
        if [temp, nominal, area, eps, ni, nc, nv, eg]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.)
        {
            return Err(invalid("invalid temperature, area or silicon constants"));
        }
        let lifetimes =
            [s.material("tn", 30e-6), s.material("tp", 10e-6)].map(|tau| tau * lifetime_scale);
        let auger = [
            s.material("cn", 1.8e-31) * t.powf(0.14),
            s.material("cp", 8.3e-32) * t.powf(0.18),
        ];
        if auger.iter().any(|&a| !a.is_finite() || a < 0.) {
            return Err(invalid("invalid Auger coefficient"));
        }
        let vsat = [
            s.mobility[0][0].get("vsat", 1.38e7),
            s.mobility[1][0].get("vsat", 9e6),
        ];
        if s.report_material {
            warnings.push(Diagnostic::information("I-CIDER-MATERIAL", format!(
                "{} silicon at {temp:.2} K, permittivity {eps:.6e} F/cm, thermal voltage {vt:.6e} V, gap {eg:.6} eV, intrinsic density {ni:.6e}/cm³, Nc {nc:.6e}/cm³, Nv {nv:.6e}/cm³, electron/hole lifetimes {:.6e}/{:.6e} s, Auger coefficients {:.6e}/{:.6e} cm⁶/s, saturation velocities {:.6e}/{:.6e} cm/s",
                instance.name, lifetimes[0], lifetimes[1], auger[0], auger[1], vsat[0], vsat[1]
            )).with_span(card.span));
        }
        let [left, right] = end_ports(bipolar);
        let mut points = Vec::with_capacity(s.x.len());
        let mut edges = Vec::new();
        for (i, &net) in s.net.iter().enumerate() {
            let dop = s.impurities[i];
            let narrowing = if s.enabled("bgn") && net != 0. {
                let (factor, reference, slope, onset) = if net >= 0. {
                    ("deg.dn", "eg.nref", 0.012, 1e18)
                } else {
                    ("deg.dp", "eg.pref", 0.0097, 1e17)
                };
                let z = (net.abs() / s.material(reference, onset)).ln();
                s.material(factor, slope) * (z + (z * z + 0.5).sqrt())
            } else {
                0.
            };
            let ni = ni * (narrowing / (2. * vt)).exp();
            let mu = mobility(&s, dop, temp / 300.)?;
            let tau = if s.enabled("conctau") {
                std::array::from_fn(|k| {
                    lifetimes[k] / (1. + dop / s.material(["srh.nref", "srh.pref"][k], 1e17))
                })
            } else {
                lifetimes
            };
            if [ni, mu[0], mu[1], tau[0], tau[1]]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.)
            {
                return Err(invalid("invalid silicon mobility, doping or lifetime"));
            }
            let end = i + 1 == s.x.len();
            let contact = i == 0 || end;
            let nodes = if end {
                [pins[right]; 3]
            } else if i == 0 {
                [pins[left]; 3]
            } else {
                [None; 3]
            };
            let volume = 0.5 * (s.x[(i + 1).min(s.x.len() - 1)] - s.x[i.saturating_sub(1)]);
            points.push(Vertex {
                nodes,
                ni,
                net,
                psi0: vt * (net / (2. * ni)).asinh(),
                mu,
                tau,
                volume,
                scale: [1.; 3],
                contact,
            });
            if i > 0 {
                edges.push(Edge {
                    ends: [i - 1, i],
                    length: s.x[i] - s.x[i - 1],
                })
            }
        }
        equilibrium(&mut points, &edges, vt, eps)?;
        for p in points.iter_mut().filter(|p| !p.contact) {
            let n = p.ni * (p.psi0 / vt).exp();
            let h = p.ni * (-p.psi0 / vt).exp();
            p.scale = [
                p.volume / (area * eps),
                p.volume / (area * CHARGE * p.mu[0] * n),
                p.volume / (area * CHARGE * p.mu[1] * h),
            ];
        }
        let base = if bipolar {
            let boundary = s.net[0].signum();
            if s.net[0] == 0.
                || s.net[s.net.len() - 1] == 0.
                || s.net[s.net.len() - 1].signum() != boundary
            {
                return Err(invalid("NBJT requires like-doped collector and emitter"));
            }
            let interior = 1..s.x.len() - 1;
            let first = interior
                .clone()
                .find(|&i| s.net[i] * boundary < 0.)
                .ok_or_else(|| invalid("NBJT needs an opposite-doped interior base"))?;
            let last = interior
                .rev()
                .find(|&i| s.net[i] * boundary < 0.)
                .unwrap_or(first);
            if s.net[first..=last].iter().any(|v| v * boundary > 0.) {
                return Err(invalid("NBJT requires a single base region"));
            }
            let depth = s.option("base.depth", 0.5 * (s.x[first] + s.x[last]) * 1e4) * 1e-4;
            // Select the nearest vertex, resolving a midpoint toward increasing x.
            let hi = s.x.partition_point(|&x| x <= depth).clamp(1, s.x.len() - 1);
            let index = if depth < 0.5 * (s.x[hi - 1] + s.x[hi]) {
                hi - 1
            } else {
                hi
            };
            if s.net[index] * boundary >= 0. {
                return Err(invalid("base.depth must select a mesh point in the base"));
            }
            let carrier = if s.net[index] > 0. { 1 } else { 2 };
            let length = s.option("base.length", 1.) * 1e-4;
            let ratio = s.option("base.area", 1.);
            let mobility_geometry = CHARGE * points[index].mu[carrier - 1] * area * ratio / length;
            if length <= 0. || ratio <= 0. || !mobility_geometry.is_finite() {
                return Err(invalid("invalid base contact dimensions"));
            }
            Some(Base {
                index,
                carrier,
                terminal: pins[1],
                mobility_geometry,
            })
        } else {
            None
        };
        if points
            .iter()
            .any(|p| p.scale.iter().any(|v| !v.is_finite() || *v <= 0.))
        {
            return Err(invalid(
                "mesh charge or flux scaling outside numerical range",
            ));
        }
        Ok(Self {
            name: instance.name.clone(),
            off: matches!(
                instance.kind,
                ElementKind::Diode { off: true, .. } | ElementKind::Bjt { off: true, .. }
            ),
            vsat,
            sg_field: s
                .mobility
                .each_ref()
                .map(|p| p[0].get("fieldmodel", 0.) == 2.),
            measurement_frequency: s.frequency,
            points,
            edges,
            base,
            vt,
            eps,
            area,
            srh: s.enabled("srh"),
            auger: if s.enabled("auger") && s.enabled("srh") {
                auger
            } else {
                [0.; 2]
            },
            fieldmob: s.enabled("fieldmob"),
            avalanche: s.enabled("avalanche"),
        })
    }

    fn edge_nodes(&self, e: &Edge) -> [Node; 6] {
        edge_nodes(e, |i| self.points[i].nodes)
    }

    fn declare(&self, b: &mut StampBuilder<'_>, nodes: impl Fn(usize) -> [Node; 3], base: Node) {
        for e in &self.edges {
            let fields = edge_nodes(e, &nodes);
            if self.points[e.ends[0]].contact || self.points[e.ends[1]].contact {
                b.charge_core(&fields);
            } else {
                b.core(&fields);
            }
        }
        for i in 1..self.points.len() - 1 {
            b.core(&nodes(i));
        }
        if let Some(contact) = &self.base {
            let vertex = nodes(contact.index);
            b.core(&[base, vertex[contact.carrier], vertex[0]]);
        }
    }

    /// Independent electrodes for intrinsic admittance extraction, including grounded
    /// or externally shorted terminals. Slot order is identical to the circuit stamp.
    pub fn isolated_layout(&self) -> (usize, usize, StampLayout) {
        let ports = if self.base.is_some() { 3 } else { 2 };
        let [left, right] = end_ports(self.base.is_some());
        let size = ports + 3 * (self.points.len() - 2);
        let unknowns = Unknowns::default();
        let mut b = StampBuilder::new(&unknowns);
        self.declare(
            &mut b,
            |i| {
                if i == 0 {
                    [Some(left); 3]
                } else if i + 1 == self.points.len() {
                    [Some(right); 3]
                } else {
                    std::array::from_fn(|k| Some(ports + 3 * (i - 1) + k))
                }
            },
            Some(1),
        );
        (ports, size, b.finish())
    }

    fn edge(&self, e: &Edge, v: [D; 6]) -> ([D; 6], [D; 6]) {
        let [i, j] = e.ends;
        let (l, r) = (&self.points[i], &self.points[j]);
        let psi = [v[0] + l.psi0, v[3] + r.psi0];
        let dpsi = psi[1] - psi[0];
        let field = dpsi / e.length;
        let mut f = [ZERO; 6];
        let mut q = [ZERO; 6];
        let flux = -dpsi * (self.eps * self.area / e.length);
        for (k, p, sign) in [(0, l, 1.), (3, r, -1.)] {
            if p.contact {
                q[k] = sign * flux
            } else {
                f[k] = sign * p.scale[0] * flux
            }
        }
        let mut generation = ZERO;
        for k in 1..=2 {
            let sign = if k == 1 { 1. } else { -1. };
            let band = sign * dpsi / self.vt + (r.ni / l.ni).ln();
            let du = sign * (v[k + 3] - v[k]) / self.vt;
            // Factor the SG difference at the endpoint giving a nonpositive expm1 argument.
            let diffusion = if du.v >= 0. {
                let left = l.ni * (sign * (psi[0] - v[k]) / self.vt).exp();
                sign * left * bernoulli(-band) * (-du).exp_m1()
            } else {
                let right = r.ni * (sign * (psi[1] - v[k + 3]) / self.vt).exp();
                -sign * right * bernoulli(band) * du.exp_m1()
            };
            let mu = (l.mu[k - 1] + r.mu[k - 1]) * 0.5;
            let mobility = if self.fieldmob {
                let t = field * (mu / self.vsat[k - 1]);
                if self.sg_field[k - 1] {
                    (2. * mu) / (1. + (1. + 4. * t.sq()).sqrt())
                } else if k == 1 {
                    mu / (1. + t.sq()).sqrt()
                } else {
                    mu / (1. + abs(t))
                }
            } else {
                D::cst(mu)
            };
            let current = diffusion * mobility * (CHARGE * self.vt * self.area / e.length);
            f[k] += current * l.scale[k];
            f[k + 3] += -current * r.scale[k];
            if self.avalanche && field.v.abs() > 1. {
                let (a, b) = if k == 1 {
                    (7.03e5, 1.231e6)
                } else {
                    (1.582e6, 2.036e6)
                };
                generation += abs(current) * (a * e.length * 0.5) * (-b / abs(field)).exp();
            }
        }
        for (k, p) in [(0, l), (3, r)] {
            if !p.contact {
                f[k + 1] += generation * p.scale[1];
                f[k + 2] += -generation * p.scale[2]
            }
        }
        (f, q)
    }

    fn cell(&self, p: &Vertex, v: [Dual<3>; 3]) -> ([Dual<3>; 3], [Dual<3>; 3]) {
        let psi = v[0] + p.psi0;
        let n = p.ni * ((psi - v[1]) / self.vt).exp();
        let h = p.ni * ((v[2] - psi) / self.vt).exp();
        let mut rate = n * self.auger[0] + h * self.auger[1];
        if self.srh {
            rate += 1. / (p.tau[1] * (n + p.ni) + p.tau[0] * (h + p.ni))
        }
        let excess = p.ni * p.ni * ((v[2] - v[1]) / self.vt).exp_m1();
        let charge = CHARGE * self.area * p.volume;
        let recomb = rate * excess * charge;
        (
            [
                charge * p.scale[0] * (n - h - p.net),
                -recomb * p.scale[1],
                recomb * p.scale[2],
            ],
            [
                Dual::cst(0.),
                -charge * p.scale[1] * n,
                charge * p.scale[2] * h,
            ],
        )
    }
}

fn edge_nodes(e: &Edge, nodes: impl Fn(usize) -> [Node; 3]) -> [Node; 6] {
    let [l, r] = e.ends;
    let [lp, ln, lh] = nodes(l);
    let [rp, rn, rh] = nodes(r);
    [lp, ln, lh, rp, rn, rh]
}

fn abs<const N: usize>(x: Dual<N>) -> Dual<N> {
    if x.v == 0. {
        Dual::cst(0.)
    } else if x.v < 0. {
        -x
    } else {
        x
    }
}
fn bernoulli(x: D) -> D {
    if x.v.abs() < 1e-3 {
        1. - x * 0.5 + x.sq() * (1. / 12. - x.sq() / 720.)
    } else if x.v > 50. {
        x * (-x).exp() / (1. - (-x).exp())
    } else {
        x / x.exp_m1()
    }
}

fn mobility(s: &Structure, dop: f64, t: f64) -> Result<[f64; 2], Diagnostic> {
    let t = if s.enabled("tempmob") { t } else { 1. };
    let mut result = [0.; 2];
    for (k, mu) in result.iter_mut().enumerate() {
        // The 1-D transport pair stays fixed across polarity reversals.
        let p = &s.mobility[k][k];
        let min = p.get("mumin", 0.);
        let max = p.get("mumax", 0.);
        let reference = p.get("ntref", 0.);
        let power = p.get("ntexp", 0.);
        if min <= 0.
            || max < min
            || reference <= 0.
            || power <= 0.
            || s.mobility[k][0].get("vsat", 1e7) <= 0.
        {
            return Err(invalid("invalid bulk mobility parameters"));
        }
        let reference = reference * t.powf(2.4);
        let power = power * t.powf(-0.146);
        let scattering = if s.enabled("concmob") {
            1. + (dop / reference).powf(power)
        } else {
            1.
        };
        let exponent = if k == 0 { -2.33 } else { -2.23 };
        *mu = min * t.powf(-0.57) + (max - min) * t.powf(exponent) / scattering;
    }
    Ok(result)
}

// Dirichlet Poisson–Boltzmann solve. Eliminate along the line in fixed coordinate order.
fn equilibrium(p: &mut [Vertex], e: &[Edge], vt: f64, eps: f64) -> Result<(), Diagnostic> {
    let mut upper = vec![0.; p.len()];
    let mut step = upper.clone();
    for _ in 0..160 {
        for i in 1..p.len() - 1 {
            let a = eps / e[i - 1].length;
            let b = eps / e[i].length;
            let pt = &p[i];
            let n = pt.ni * (pt.psi0 / vt).exp();
            let h = pt.ni * (-pt.psi0 / vt).exp();
            let q = CHARGE * pt.volume;
            let diag = a + b + q * (n + h) / vt + a * upper[i - 1];
            let residual = a * (pt.psi0 - p[i - 1].psi0)
                + b * (pt.psi0 - p[i + 1].psi0)
                + q * (n - h - pt.net);
            upper[i] = -b / diag;
            step[i] = (a * step[i - 1] - residual) / diag;
        }
        for i in (1..p.len() - 1).rev() {
            step[i] -= upper[i] * step[i + 1]
        }
        let max = step.iter().map(|v| v.abs()).fold(0., f64::max);
        let scale = 1_f64.min(4. * vt / max);
        for (pt, &dv) in p.iter_mut().zip(&step) {
            pt.psi0 += scale * dv
        }
        if max < 1e-12 && p.iter().all(|pt| pt.psi0.is_finite()) {
            return Ok(());
        }
    }
    Err(invalid("equilibrium Poisson solve did not converge"))
}

impl DeviceGroup for Numerical {
    fn type_name(&self) -> &'static str {
        if self.base.is_some() { "nbjt" } else { "numd" }
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, i: usize) -> Option<&str> {
        (i == 0).then_some(&self.name)
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (i, p) in self
            .points
            .iter_mut()
            .enumerate()
            .filter(|(_, p)| !p.contact)
        {
            p.nodes = ["psi", "fn", "fp"]
                .map(|k| Some(u.alloc_operator_state(&self.name, &format!("{k}{i}"))))
        }
    }
    fn output_charges(&self, _: usize, name: &str) -> Vec<(usize, f64)> {
        match name {
            "id" | "ie" => vec![(0, 1.)],
            "ic" => vec![(6 * (self.edges.len() - 1) + 3, 1.)],
            _ => Vec::new(),
        }
    }
    fn state_len(&self) -> usize {
        self.points.len() * 3
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.declare(
            b,
            |i| self.points[i].nodes,
            self.base.as_ref().and_then(|b| b.terminal),
        );
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        let mut at = SlotOffsets::default();
        for (p, prev) in self.points.iter().zip(state.current.as_chunks_mut::<3>().0) {
            for (k, last) in prev.iter_mut().enumerate() {
                let value = volt(x, p.nodes[k]);
                let delta = value - *last;
                let limit = 3. * self.vt;
                if ctx.init_junctions && self.off {
                    *last = 0.;
                } else if ctx.limit && delta.abs() > limit {
                    *last += limit.copysign(delta);
                    out.noncon = true;
                } else {
                    *last = value
                }
            }
        }
        for e in &self.edges {
            let act = self.edge_nodes(e).map(|n| volt(x, n));
            let lin = std::array::from_fn(|i| state.current[3 * e.ends[i / 3] + i % 3]);
            let (f, q) = self.edge(e, std::array::from_fn(|i| D::var(lin[i], i)));
            out.core(&mut at, &f, &q, &act, &lin)
        }
        for (i, p) in self.points.iter().enumerate().filter(|(_, p)| !p.contact) {
            let act = p.nodes.map(|n| volt(x, n));
            let lin = std::array::from_fn(|k| state.current[i * 3 + k]);
            let (f, q) = self.cell(p, std::array::from_fn(|k| Dual::var(lin[k], k)));
            out.core(&mut at, &f, &q, &act, &lin)
        }
        if let Some(b) = &self.base {
            let pt = &self.points[b.index];
            let act = [b.terminal, pt.nodes[b.carrier], pt.nodes[0]].map(|node| volt(x, node));
            let lin = [
                act[0],
                state.current[3 * b.index + b.carrier],
                state.current[3 * b.index],
            ];
            let [terminal, fermi, psi] = std::array::from_fn(|k| Dual::<3>::var(lin[k], k));
            let sign = if b.carrier == 1 { 1. } else { -1. };
            let density = pt.ni * (sign * (psi + pt.psi0 - fermi) / self.vt).exp();
            let current = b.mobility_geometry * density * (terminal - fermi);
            out.core(
                &mut at,
                &[current, -current * pt.scale[b.carrier], Dual::cst(0.)],
                &[Dual::cst(0.); 3],
                &act,
                &lin,
            );
        }
        if let Some(values) = out.outputs.first_mut() {
            let first = out.f[1] + out.f[2];
            if self.base.is_some() {
                let last = 6 * (self.edges.len() - 1);
                values.extend([
                    ("ie", first),
                    ("ib", out.f[at.f - 3]),
                    ("ic", out.f[last + 4] + out.f[last + 5]),
                ]);
            } else {
                values.push(("id", first))
            }
        }
    }
}
