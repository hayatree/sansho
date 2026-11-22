//! Fully coupled silicon drift–diffusion elements.
//! Provenance: Scharfetter & Gummel (1969), Selberherr (1984), Gates (1993),
//! and ngspice 44 manual chapter 26. Equations and parameter sources in docs/cider.md.

mod cards;
mod equilibrium;
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
    port: Option<usize>,
    active: bool,
    fixed: f64,
    traps: Vec<cards::SurfaceTrap>,
}

#[derive(Debug)]
struct Edge {
    ends: [usize; 2],
    length: f64,
    dielectric: f64,
    conducting: f64,
    surface: Option<cards::Surface>,
}

#[derive(Debug)]
struct Base {
    index: usize,
    carrier: usize,
    terminal: Node,
    mobility_geometry: f64,
}

/// Numerical silicon/oxide device. Carrier unknowns belong to vertices touching silicon.
#[derive(Debug)]
pub struct Numerical {
    name: String,
    pub measurement_frequency: f64,
    points: Vec<Vertex>,
    edges: Vec<Edge>,
    base: Option<Base>,
    vt: f64,
    ports: usize,
    surface_params: [[f64; 3]; 2],
    transmob: bool,
    // Extrusion factor: area in cm² for a line, width in cm for a planar section.
    extrusion: f64,
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
        let mos = card.kind == "numos";
        let ports = if mos {
            4
        } else if bipolar {
            3
        } else {
            2
        };
        if pins.len() != ports {
            return Err(invalid("incorrect numerical device terminal count"));
        }
        if (bipolar && (s.option("diode", 0.) != 0. || s.option("resistor", 0.) != 0.))
            || (!bipolar && !mos && s.option("bipolar", 0.) != 0.)
        {
            return Err(invalid("incompatible structure type"));
        }
        let temp = inst.get("temp", temp) + 273.15;
        let nominal = s.option("tnom", nominal) + 273.15;
        let mask_area = s.option("defa", 1.);
        let factor = inst.get("area", 1.);
        let area = if s.plane.is_some() {
            if mos {
                inst.get("l", 1.);
            }
            let mask_length = s.option("defl", 1.);
            let default_width = s.option("defw", mask_area / mask_length);
            let width_factor = inst.get("w", 1.);
            if mask_length <= 0. || default_width <= 0. || width_factor <= 0. {
                return Err(invalid("mask dimensions and width factor must be positive"));
            }
            100. * default_width * width_factor * factor
        } else {
            1e4 * mask_area * factor
        };
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
        let mut points = Vec::with_capacity(s.net.len());
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
            let (port, volume, active, fixed) = if let Some(plane) = &s.plane {
                let pt = &plane.vertices[i];
                (pt.port, pt.volume, pt.active, pt.fixed)
            } else {
                let end = i + 1 == s.x.len();
                let port = if i == 0 {
                    Some(left)
                } else if end {
                    Some(right)
                } else {
                    None
                };
                (
                    port,
                    0.5 * (s.x[(i + 1).min(s.x.len() - 1)] - s.x[i.saturating_sub(1)]),
                    true,
                    0.,
                )
            };
            let electrode = port.and_then(|k| pins[k]);
            let nodes = if volume > 0. {
                [electrode; 3]
            } else {
                [electrode, None, None]
            };
            let psi0 = if let Some((plane, k)) = s.plane.as_ref().zip(port).filter(|_| volume == 0.)
            {
                // Vacuum-referenced work function of intrinsic Si minus that of the gate.
                s.material("affinity", 4.05) + vt * (nc / ni).ln() - plane.work_functions[k]
            } else {
                vt * (net / (2. * ni)).asinh()
            };
            points.push(Vertex {
                nodes,
                ni,
                net,
                psi0,
                mu,
                tau,
                volume,
                scale: [1.; 3],
                port,
                active,
                fixed,
                traps: s
                    .plane
                    .as_ref()
                    .map_or_else(Vec::new, |plane| plane.vertices[i].traps.clone()),
            });
            if s.plane.is_none() && i > 0 {
                edges.push(Edge {
                    ends: [i - 1, i],
                    length: s.x[i] - s.x[i - 1],
                    dielectric: eps,
                    conducting: 1.,
                    surface: None,
                })
            }
        }
        if let Some(plane) = &s.plane {
            for f in &plane.faces {
                let mut remaining = f.conducting;
                let mut dielectric = f.dielectric;
                // Two cells can contribute different mobilities to one control face.
                // Stamp its full displacement once and each conducting width once.
                let mut assigned = Vec::new();
                if s.enabled("surfmob") {
                    for &sf in &f.layers {
                        if assigned.contains(&sf.cell) {
                            continue;
                        }
                        let density = sf
                            .interface
                            .map(|i| points[i].net.abs())
                            .into_iter()
                            .fold(ni, f64::max);
                        let layer = if sf.layer > 0. {
                            sf.layer
                        } else {
                            (eps * vt / (CHARGE * density)).sqrt()
                        };
                        if sf.depth >= layer * (1. - 16. * f64::EPSILON) {
                            continue;
                        }
                        assigned.push(sf.cell);
                        edges.push(Edge {
                            ends: f.ends,
                            length: f.length,
                            dielectric,
                            conducting: sf.width,
                            surface: Some(sf),
                        });
                        dielectric = 0.;
                        remaining -= sf.width;
                    }
                }
                if remaining > f.conducting * f64::EPSILON || dielectric > 0. {
                    edges.push(Edge {
                        ends: f.ends,
                        length: f.length,
                        dielectric,
                        conducting: remaining.max(0.),
                        surface: None,
                    });
                }
            }
            equilibrium::planar(&mut points, &edges, vt)?;
        } else {
            equilibrium(&mut points, &edges, vt, eps)?;
        }
        let mut weights = vec![[0.; 2]; points.len()];
        if s.plane.is_some() {
            for e in &edges {
                for i in e.ends {
                    weights[i][0] += e.dielectric / e.length;
                    weights[i][1] += e.conducting / e.length;
                }
            }
        }
        for (i, p) in points
            .iter_mut()
            .enumerate()
            .filter(|(_, p)| p.active && p.port.is_none())
        {
            let n = p.ni * (p.psi0 / vt).exp();
            let h = p.ni * (-p.psi0 / vt).exp();
            if s.plane.is_some() {
                p.scale[0] = 1. / (area * weights[i][0]);
                if p.volume > 0. {
                    // One fixed density scale for the carrier rows remains well
                    // conditioned when the channel inverts under gate bias.
                    p.scale[1] = 1.
                        / (weights[i][1] * area * CHARGE * p.mu[0] * n.max(p.ni).max(p.net.abs()));
                    p.scale[2] = 1.
                        / (weights[i][1] * area * CHARGE * p.mu[1] * h.max(p.ni).max(p.net.abs()));
                }
            } else {
                p.scale = [
                    p.volume / (area * eps),
                    p.volume / (area * CHARGE * p.mu[0] * n),
                    p.volume / (area * CHARGE * p.mu[1] * h),
                ];
            }
        }
        let base = if bipolar && s.plane.is_none() {
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
                ElementKind::Diode { off: true, .. }
                    | ElementKind::Bjt { off: true, .. }
                    | ElementKind::Mosfet { off: true, .. }
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
            ports,
            surface_params: s.surface_params,
            transmob: s.enabled("transmob"),
            extrusion: area,
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

    fn declare(&self, b: &mut StampBuilder<'_>, nodes: impl Fn(usize) -> [Node; 3], base: Node) {
        for e in &self.edges {
            let fields = edge_nodes(e, &nodes);
            let extra;
            let fields = if let Some(sf) = &e.surface {
                extra = surface_nodes(fields, sf.stencil.map(|i| nodes(i)[0]));
                extra.as_slice()
            } else {
                fields.as_slice()
            };
            if e.ends.iter().any(|&i| self.points[i].port.is_some()) {
                b.charge_core(fields);
            } else {
                b.core(fields);
            }
        }
        for (i, _) in self
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.port.is_none() && p.volume > 0.)
        {
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
        let ports = self.ports;
        let mut size = ports;
        let nodes = self
            .points
            .iter()
            .map(|p| {
                let mut n = [None; 3];
                if p.active {
                    for v in n.iter_mut().take(if p.volume > 0. { 3 } else { 1 }) {
                        *v = p.port.or_else(|| {
                            size += 1;
                            Some(size - 1)
                        });
                    }
                }
                n
            })
            .collect::<Vec<_>>();
        let unknowns = Unknowns::default();
        let mut b = StampBuilder::new(&unknowns);
        self.declare(&mut b, |i| nodes[i], Some(1));
        (ports, size, b.finish())
    }

    fn edge<const N: usize>(
        &self,
        e: &Edge,
        v: [Dual<N>; N],
        normal: Dual<N>,
    ) -> ([Dual<N>; N], [Dual<N>; N]) {
        let [i, j] = e.ends;
        let (l, r) = (&self.points[i], &self.points[j]);
        let psi = [v[0] + l.psi0, v[3] + r.psi0];
        let dpsi = psi[1] - psi[0];
        let field = dpsi / e.length;
        let mut f = [Dual::cst(0.); N];
        let mut q = [Dual::cst(0.); N];
        let flux = -dpsi * (e.dielectric * self.extrusion / e.length);
        for (k, p, sign) in [(0, l, 1.), (3, r, -1.)] {
            if p.port.is_some() {
                q[k] = sign * flux
            } else {
                f[k] = sign * p.scale[0] * flux
            }
        }
        if e.conducting == 0. {
            return (f, q);
        }
        let mut generation = Dual::cst(0.);
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
            let mu = if e.surface.is_some() {
                let [mus, theta_a, theta_b] = self.surface_params[k - 1];
                if self.transmob {
                    mus / (1. + abs(normal) * theta_a + normal.sq() * theta_b)
                } else {
                    Dual::cst(mus)
                }
            } else {
                Dual::cst(mu)
            };
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
                mu
            };
            let current = diffusion
                * mobility
                * (CHARGE * self.vt * self.extrusion * e.conducting / e.length);
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
            if p.port.is_none() {
                f[k + 1] += generation * p.scale[1];
                f[k + 2] += -generation * p.scale[2]
            }
        }
        (f, q)
    }

    fn port_names(&self) -> &'static [&'static str] {
        match self.ports {
            4 => &["id", "ig", "is", "ib"],
            3 => &["ic", "ib", "ie"],
            _ => &["id", ""],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn stamp_edge<const N: usize>(
        &self,
        e: &Edge,
        nodes: [Node; N],
        indices: [usize; N],
        x: &[f64],
        state: &[f64],
        at: &mut SlotOffsets,
        out: &mut Slots,
    ) {
        let act = nodes.map(|n| volt(x, n));
        let lin = indices.map(|i| state[i]);
        let v = std::array::from_fn(|k| Dual::var(lin[k], k));
        let normal = if let Some(sf) = e.surface {
            let [a, b, c, d] = sf.stencil.map(|i| self.points[i].psi0);
            ((v[8] - v[6] + (c - a)) + (v[9] - v[7] + (d - b))) * sf.normal_inverse
                + sf.normal_offset
        } else {
            Dual::cst(0.)
        };
        let (f, q) = self.edge(e, v, normal);
        out.core(at, &f, &q, &act, &lin);
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
        let charge = CHARGE * self.extrusion * p.volume;
        let mut recomb = rate * excess * charge;
        for trap in &p.traps {
            let denominator = (n + p.ni) / trap.sp + (h + p.ni) / trap.sn;
            recomb += excess * (CHARGE * self.extrusion * trap.length) / denominator;
        }
        (
            [
                charge * p.scale[0] * (n - h - p.net) - self.extrusion * p.fixed * p.scale[0],
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

fn surface_nodes<T: Copy>(edge: [T; 6], normal: [T; 4]) -> [T; 10] {
    let [a, b, c, d, e, f] = edge;
    let [g, h, i, j] = normal;
    [a, b, c, d, e, f, g, h, i, j]
}
fn edge_nodes_indices(e: &Edge) -> [usize; 6] {
    std::array::from_fn(|i| 3 * e.ends[i / 3] + i % 3)
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
fn bernoulli<const N: usize>(x: Dual<N>) -> Dual<N> {
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
        // Both line and planar slabs select electron majority and hole minority.
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
        match self.ports {
            4 => "numos",
            3 => "nbjt",
            _ => "numd",
        }
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
            .filter(|(_, p)| p.active && p.port.is_none())
        {
            for (k, name) in ["psi", "fn", "fp"]
                .iter()
                .enumerate()
                .take(if p.volume > 0. { 3 } else { 1 })
            {
                p.nodes[k] = Some(u.alloc_operator_state(&self.name, &format!("{name}{i}")));
            }
        }
    }
    fn output_charges(&self, _: usize, name: &str) -> Vec<(usize, f64)> {
        let mut result = Vec::new();
        let mut offset = 0;
        for e in &self.edges {
            for (k, i) in e.ends.into_iter().enumerate() {
                if self.points[i]
                    .port
                    .is_some_and(|p| self.port_names()[p] == name)
                {
                    result.push((offset + 3 * k, 1.));
                }
            }
            offset += if e.surface.is_some() { 10 } else { 6 };
        }
        result
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
            let nodes = edge_nodes(e, |i| self.points[i].nodes);
            let indices = edge_nodes_indices(e);
            if let Some(sf) = e.surface {
                let nodes = surface_nodes(nodes, sf.stencil.map(|i| self.points[i].nodes[0]));
                let indices = surface_nodes(indices, sf.stencil.map(|i| 3 * i));
                self.stamp_edge(e, nodes, indices, x, state.current, &mut at, out);
            } else {
                self.stamp_edge(e, nodes, indices, x, state.current, &mut at, out);
            }
        }
        for (i, p) in self
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.port.is_none() && p.volume > 0.)
        {
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
            let mut currents = [0.; 4];
            let mut offset = 0;
            for e in &self.edges {
                for (k, i) in e.ends.into_iter().enumerate() {
                    if let Some(port) = self.points[i].port {
                        currents[port] += out.f[offset + 3 * k + 1] + out.f[offset + 3 * k + 2];
                    }
                }
                offset += if e.surface.is_some() { 10 } else { 6 };
            }
            if self.base.is_some() {
                currents[1] = out.f[at.f - 3];
            }
            values.extend(
                self.port_names()
                    .iter()
                    .copied()
                    .zip(currents)
                    .filter(|(name, _)| !name.is_empty()),
            );
        }
    }
}
