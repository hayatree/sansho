//! Rectangular control volumes, material faces and electrode geometry.
//! Provenance: Selberherr (1984), box integration; ngspice 44 manual §§26.2–26.7, 26.16–26.19.

use super::{Fields, bounds, id, invalid};
use crate::cider::EPS_SI;
use sansho_diag::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(in crate::cider) struct BoxVertex {
    pub active: bool,
    pub volume: f64,
    pub fixed: f64,
    pub traps: Vec<SurfaceTrap>,
    pub port: Option<usize>,
    domains: Vec<(u32, f64)>,
}

impl BoxVertex {
    pub(super) fn domain_fraction(&self, ids: &[u32]) -> f64 {
        if self.volume == 0. {
            return 0.;
        }
        if ids.is_empty() {
            return 1.;
        }
        self.domains
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|p| p.1)
            .sum::<f64>()
            / self.volume
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::cider) struct SurfaceTrap {
    pub length: f64,
    pub sn: f64,
    pub sp: f64,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::cider) struct Surface {
    // Interface doping determines the default depth. The field stencil is local
    // to the transport edge and uses the nearest silicon neighbors.
    pub interface: [usize; 2],
    pub normal_weights: [f64; 2],
    pub stencil: [usize; 4],
    pub depth: f64,
    pub layer: f64,
}

#[derive(Debug, Default)]
pub(in crate::cider) struct Face {
    pub ends: [usize; 2],
    pub length: f64,
    pub dielectric: f64,
    pub conducting: f64,
    pub layers: Vec<Surface>,
}

#[derive(Debug)]
pub(in crate::cider) struct Plane {
    pub vertices: Vec<BoxVertex>,
    pub faces: Vec<Face>,
    pub work_functions: Vec<f64>,
    pub silicon_material: Option<u32>,
    pub domain_ids: BTreeSet<u32>,
}

#[derive(Clone, Copy)]
struct Cell {
    id: u32,
    silicon: bool,
    eps: f64,
}

struct Rectangle {
    x: (f64, f64),
    y: (f64, f64),
}
impl Rectangle {
    fn new(f: &Fields, x: &[f64], y: &[f64]) -> Result<Self, Diagnostic> {
        Ok(Self {
            x: bounds(f, x, false, false)?,
            y: bounds(f, y, false, true)?,
        })
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        let inside = |v: f64, (lo, hi): (f64, f64)| {
            let tol = 16. * f64::EPSILON * (1. + v.abs() + lo.abs() + hi.abs());
            lo - tol <= v && v <= hi + tol
        };
        inside(x, self.x) && inside(y, self.y)
    }
}

impl Plane {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind(
        x: &[f64],
        y: &[f64],
        materials: &BTreeMap<u32, Fields>,
        domains: &[Fields],
        electrodes: &[Fields],
        contacts: &[Fields],
        interfaces: &[Fields],
        ports: usize,
    ) -> Result<Self, Diagnostic> {
        let nx = x.len();
        let ny = y.len();
        if nx < 2 || ny < 2 || nx * ny > 1024 {
            return Err(invalid(
                "2-D mesh requires at least two vertices per axis and at most 1024 in total",
            ));
        }
        if x.windows(2).chain(y.windows(2)).any(|p| p[1] <= p[0]) {
            return Err(invalid("2-D mesh positions must increase"));
        }
        let nc = nx - 1;
        let mut cells = vec![None; nc * (ny - 1)];
        let mut silicon_material = None;
        let mut domain_ids = BTreeSet::new();
        for f in domains {
            let domain = id(f, "number")?;
            let m = id(f, "material")?;
            let mat = materials
                .get(&m)
                .ok_or_else(|| invalid("undefined region material"))?;
            if mat.flag("oxide") && mat.flag("silicon") {
                return Err(invalid("material cannot be both silicon and oxide"));
            }
            let silicon = !mat.flag("oxide");
            if silicon && silicon_material.replace(m).is_some_and(|old| old != m) {
                return Err(invalid(
                    "only one silicon material is supported per 2-D device",
                ));
            }
            let eps = mat.positive(
                "permittivity",
                if silicon {
                    EPS_SI
                } else {
                    3.9 * 8.8541878128e-14
                },
            )?;
            let rect = Rectangle::new(f, x, y)?;
            domain_ids.insert(domain);
            for (j, ys) in y.windows(2).enumerate() {
                for (i, xs) in x.windows(2).enumerate() {
                    if rect.contains(0.5 * (xs[0] + xs[1]), 0.5 * (ys[0] + ys[1])) {
                        cells[i + j * nc] = Some(Cell {
                            id: domain,
                            silicon,
                            eps,
                        });
                    }
                }
            }
        }
        let mut mesh = Mesh { x, y, cells };
        let (mut vertices, mut faces) = mesh.control_volumes();
        let work_functions = mesh.electrodes(&mut vertices, electrodes, contacts, ports)?;
        if mesh.clip_contact_corners(&vertices) {
            let (mut clipped, clipped_faces) = mesh.control_volumes();
            for (p, old) in clipped.iter_mut().zip(vertices) {
                p.port = old.port.filter(|_| p.active);
            }
            if (0..ports).any(|k| !clipped.iter().any(|p| p.port == Some(k))) {
                return Err(invalid("electrode misses the active material"));
            }
            vertices = clipped;
            faces = clipped_faces;
        }
        let rules = interface_rules(interfaces, &domain_ids, x, y)?;
        mesh.apply_interfaces(&rules, &mut vertices, &mut faces);
        Ok(Self {
            vertices,
            faces: faces.into_values().collect(),
            work_functions,
            silicon_material,
            domain_ids,
        })
    }
}

type Faces = BTreeMap<[usize; 2], Face>;

struct Mesh<'a> {
    x: &'a [f64],
    y: &'a [f64],
    cells: Vec<Option<Cell>>,
}

impl Mesh<'_> {
    // R21b: at an exterior gate end a one-cell oxide bridge from the
    // insulated electrode to a side ohmic contact has no active gate area.
    // Identify the adjoining contacts from the uncut mesh before applying
    // this footprint to Poisson, electrode displacement and surface flux.
    fn clip_contact_corners(&mut self, vertices: &[BoxVertex]) -> bool {
        let nx = self.x.len();
        let ny = self.y.len();
        let mut changed = false;
        for (k, cell) in self.cells.iter_mut().enumerate() {
            if !cell.is_some_and(|c| !c.silicon) {
                continue;
            }
            let (i, j) = (k % (nx - 1), k / (nx - 1));
            let a = i + j * nx;
            for (boundary, outer, inner) in [
                (i == 0, [a, a + nx], [a + 1, a + nx + 1]),
                (i + 2 == nx, [a + 1, a + nx + 1], [a, a + nx]),
                (j == 0, [a, a + 1], [a + nx, a + nx + 1]),
                (j + 2 == ny, [a + nx, a + nx + 1], [a, a + 1]),
            ] {
                if !boundary {
                    continue;
                }
                let [u, v] = outer.map(|n| &vertices[n]);
                let [w, z] = inner.map(|n| &vertices[n]);
                let (gate, ohmic, gate_in, si_in) = if u.volume == 0. {
                    (u, v, w, z)
                } else {
                    (v, u, z, w)
                };
                if gate.volume == 0.
                    && gate_in.volume == 0.
                    && ohmic.volume > 0.
                    && si_in.volume > 0.
                    && gate.port.is_some()
                    && gate_in.port == gate.port
                    && ohmic.port.is_some()
                    && ohmic.port != gate.port
                    && si_in.port.is_none()
                {
                    *cell = None;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    fn control_volumes(&self) -> (Vec<BoxVertex>, Faces) {
        let (x, y, cells) = (self.x, self.y, &self.cells);
        let (nx, ny) = (x.len(), y.len());
        let nc = nx - 1;
        let mut vertices: Vec<_> = (0..nx * ny).map(|_| BoxVertex::default()).collect();
        let mut faces = BTreeMap::new();
        for (index, cell) in cells.iter().enumerate() {
            let Some(c) = cell else { continue };
            let (i, j) = (index % nc, index / nc);
            let a = i + j * nx;
            let dx = (x[i + 1] - x[i]) * 1e-4;
            let dy = (y[j + 1] - y[j]) * 1e-4;
            for k in [a, a + 1, a + nx, a + nx + 1] {
                let p = &mut vertices[k];
                p.active = true;
                if c.silicon {
                    let vol = 0.25 * dx * dy;
                    p.volume += vol;
                    p.domains.push((c.id, vol));
                }
            }
            for (start, stride, length, width) in [
                (a, 1, dx, dy),
                (a + nx, 1, dx, dy),
                (a, nx, dy, dx),
                (a + 1, nx, dy, dx),
            ] {
                let ends = [start, start + stride];
                let e = faces.entry(ends).or_insert(Face {
                    ends,
                    length,
                    ..Face::default()
                });
                e.dielectric += 0.5 * width * c.eps;
                if c.silicon {
                    e.conducting += 0.5 * width;
                }
            }
        }
        (vertices, faces)
    }

    fn electrodes(
        &self,
        vertices: &mut [BoxVertex],
        electrodes: &[Fields],
        contacts: &[Fields],
        ports: usize,
    ) -> Result<Vec<f64>, Diagnostic> {
        let (x, y) = (self.x, self.y);
        let nx = x.len();
        let mut used = vec![false; ports];
        for (seq, f) in electrodes.iter().enumerate() {
            let number = f.get("number", (seq + 1) as f64);
            let k = port(number, ports)?;
            let rect = Rectangle::new(f, x, y)?;
            let mut found = false;
            for (i, p) in vertices.iter_mut().enumerate() {
                if p.active && rect.contains(x[i % nx], y[i / nx]) {
                    if p.port.replace(k).is_some_and(|old| old != k) {
                        return Err(invalid("overlapping electrodes"));
                    }
                    found = true;
                }
            }
            if !found {
                return Err(invalid("electrode misses the active mesh"));
            }
            used[k] = true;
        }
        if used.contains(&false) {
            return Err(invalid("missing numbered electrode"));
        }
        let mut work_functions = vec![4.1; ports];
        for f in contacts {
            let k = port(id(f, "number")? as f64, ports)?;
            work_functions[k] = f.positive("work-function", work_functions[k])?;
        }

        Ok(work_functions)
    }

    fn apply_interfaces(
        &self,
        rules: &[Interface<'_>],
        vertices: &mut [BoxVertex],
        faces: &mut Faces,
    ) {
        let (x, y, cells) = (self.x, self.y, &self.cells);
        let (nx, ny) = (x.len(), y.len());
        let nc = nx - 1;
        // Visit silicon faces once, from the silicon side. First matching layer
        // has priority where strips from two different interfaces overlap.
        let mut strips = Vec::new();
        for (idx, c) in cells.iter().enumerate() {
            let Some(c) = c.filter(|v| v.silicon) else {
                continue;
            };
            let (i, j) = (idx % nc, idx / nc);
            let a = i + nx * j;
            for (di, dj, ends) in [
                (0, -1, [a, a + 1]),
                (0, 1, [a + nx, a + nx + 1]),
                (-1, 0, [a, a + nx]),
                (1, 0, [a + 1, a + 1 + nx]),
            ] {
                let neighbor = i
                    .checked_add_signed(di)
                    .zip(j.checked_add_signed(dj))
                    .filter(|&(u, v)| u < nc && v < ny - 1)
                    .and_then(|(u, v)| cells[u + v * nc]);
                if neighbor.is_some_and(|n| n.silicon) {
                    continue;
                }
                let [l, r] = ends;
                let xm = 0.5 * (x[l % nx] + x[r % nx]);
                let ym = 0.5 * (y[l / nx] + y[r / nx]);
                let matching = rules.iter().enumerate().filter(|(_, rule)| {
                    let n = neighbor.map(|n| n.id);
                    (rule.domain == c.id && (rule.neighbor.is_none() || rule.neighbor == n))
                        || (Some(rule.domain) == n && rule.neighbor.is_none_or(|o| o == c.id))
                });
                let tangent_x = dj != 0;
                let axis = if tangent_x { x } else { y };
                let start = if tangent_x { i } else { j };
                let normal = if tangent_x { ym } else { xm };
                let midpoint = 0.5 * (axis[start] + axis[start + 1]);
                for (vertex, lo, hi) in [(l, axis[start], midpoint), (r, midpoint, axis[start + 1])]
                {
                    integrate_boundary(
                        &mut vertices[vertex],
                        lo,
                        hi,
                        tangent_x,
                        normal,
                        matching.clone().map(|(_, r)| r),
                    );
                }
                let rule = matching.clone().find(|(_, r)| {
                    r.bounds.contains(xm, ym) && r.fields.values.contains_key("layer.width")
                });
                if neighbor.is_some()
                    && (rule.is_some()
                        || !matching
                            .clone()
                            .any(|(_, r)| r.fields.values.contains_key("layer.width")))
                {
                    let step = -di - dj * nx as isize;
                    let layer = rule.map_or(0., |(_, r)| r.fields.get("layer.width", 0.)) * 1e-4;
                    strips.push((
                        rule.map_or(usize::MAX, |r| r.0),
                        ends,
                        (i, j, di, dj, step),
                        layer,
                    ));
                }
            }
        }
        strips.sort_by_key(|s| s.0);
        for (_, ends, (mut i, mut j, di, dj, step), layer) in strips {
            let mut here = ends;
            let mut depth = 0.;
            let mut previous: Option<([usize; 2], f64)> = None;
            while cells[i + nc * j].is_some_and(|c| c.silicon) {
                let spacing = if di == 0 {
                    y[j + 1] - y[j]
                } else {
                    x[i + 1] - x[i]
                } * 1e-4;
                let inner = here.map(|k| k.saturating_add_signed(step));
                if layer > 0. && depth > layer {
                    break;
                }
                // Derivative of the quadratic interpolant on unequal intervals.
                let (outer, normal_weights) = if let Some((outer, h)) = previous {
                    (
                        outer,
                        [
                            -spacing / (h * (spacing + h)),
                            h / (spacing * (spacing + h)),
                        ],
                    )
                } else {
                    (here, [0., 1. / spacing])
                };
                let surface = Surface {
                    interface: ends,
                    normal_weights,
                    stencil: [outer[0], outer[1], inner[0], inner[1]],
                    depth,
                    layer,
                };
                if let Some(face) = faces.get_mut(&here) {
                    face.layers.push(surface);
                }
                let outer_edge = here;
                previous = Some((here, spacing));
                here = inner;
                depth += spacing;
                let next = i.checked_add_signed(-di).zip(j.checked_add_signed(-dj));
                let Some((u, v)) = next.filter(|&(u, v)| {
                    u < nc && v < ny - 1 && cells[u + nc * v].is_some_and(|c| c.silicon)
                }) else {
                    // One-sided silicon derivative on the far boundary.
                    if let Some(face) = faces.get_mut(&inner) {
                        face.layers.push(Surface {
                            normal_weights: [-1. / spacing, 0.],
                            stencil: [outer_edge[0], outer_edge[1], inner[0], inner[1]],
                            depth,
                            ..surface
                        });
                    }
                    break;
                };
                (i, j) = (u, v);
            }
        }
    }
}

struct Interface<'a> {
    domain: u32,
    neighbor: Option<u32>,
    bounds: Rectangle,
    fields: &'a Fields,
}
fn interface_rules<'a>(
    interfaces: &'a [Fields],
    domain_ids: &BTreeSet<u32>,
    x: &[f64],
    y: &[f64],
) -> Result<Vec<Interface<'a>>, Diagnostic> {
    interfaces
        .iter()
        .map(|f| {
            let dom = id(f, "domain")?;
            let neighbor = f
                .values
                .contains_key("neighbor")
                .then(|| id(f, "neighbor"))
                .transpose()?;
            if !domain_ids.contains(&dom)
                || neighbor.is_some_and(|n| !domain_ids.contains(&n) || n == dom)
            {
                return Err(invalid("invalid interface domain or neighbor"));
            }
            if f.get("sn", 0.) < 0. || f.get("sp", 0.) < 0. {
                return Err(invalid(
                    "surface recombination velocities must be nonnegative",
                ));
            }
            Ok(Interface {
                domain: dom,
                neighbor,
                bounds: Rectangle::new(f, x, y)?,
                fields: f,
            })
        })
        .collect()
}

// Integrate each half-face exactly even when a box ends between vertices.
fn integrate_boundary<'a>(
    p: &mut BoxVertex,
    lo: f64,
    hi: f64,
    tangent_x: bool,
    normal: f64,
    rules: impl Iterator<Item = &'a Interface<'a>> + Clone,
) {
    let mut cuts = vec![lo, hi];
    for rule in rules.clone() {
        let (start, end) = if tangent_x {
            rule.bounds.x
        } else {
            rule.bounds.y
        };
        for cut in [start, end] {
            if cut > lo && cut < hi {
                cuts.push(cut);
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    for pair in cuts.windows(2) {
        let length = (pair[1] - pair[0]) * 1e-4;
        let mid = 0.5 * (pair[0] + pair[1]);
        let (x, y) = if tangent_x {
            (mid, normal)
        } else {
            (normal, mid)
        };
        let mut params = [0.; 3];
        for rule in rules.clone().filter(|r| r.bounds.contains(x, y)) {
            for (v, key) in params.iter_mut().zip(["qf", "sn", "sp"]) {
                *v = rule.fields.get(key, *v);
            }
        }
        let [qf, sn, sp] = params;
        p.fixed += length * qf;
        if sn > 0. && sp > 0. {
            p.traps.push(SurfaceTrap { length, sn, sp });
        }
    }
}

fn port(number: f64, count: usize) -> Result<usize, Diagnostic> {
    if number.fract() != 0. || !(1. ..=count as f64).contains(&number) {
        return Err(invalid("electrode number outside device terminal range"));
    }
    Ok(number as usize - 1)
}
