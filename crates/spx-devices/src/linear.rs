//! Linear passive and controlled-source devices.
//!
//! Provenance: original work. Stamps follow the modified nodal formulation of Ho, Ruehli &
//! Brennan (1975); sign conventions follow SPICE (branch current flows into the positive
//! terminal). See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;

use crate::{DeviceGroup, Node, Slots, StampBuilder, Unknowns, volt};

fn missing_branch(name: &str) -> Diagnostic {
    Diagnostic::error(
        "E-ELAB-INTERNAL",
        format!("branch unknown of `{name}` was not allocated"),
    )
}

/// Resistors (`f = (vp - vn)/R`).
#[derive(Debug, Default)]
pub struct Resistors {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    g: Vec<f64>,
}

impl Resistors {
    /// Add an instance with resistance `r` (non-zero).
    pub fn push(&mut self, name: &str, p: Node, n: Node, r: f64) {
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.g.push(1.0 / r);
    }
}

impl DeviceGroup for Resistors {
    fn type_name(&self) -> &'static str {
        "resistor"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            b.g2(self.p[i], self.n[i]);
            b.f(self.p[i]);
            b.f(self.n[i]);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let g = self.g[i];
            let cur = g * (volt(x, self.p[i]) - volt(x, self.n[i]));
            out.g[4 * i..4 * i + 4].copy_from_slice(&[g, -g, -g, g]);
            out.f[2 * i] = cur;
            out.f[2 * i + 1] = -cur;
        }
    }
    fn set_param(&mut self, instance: &str, param: &str, value: f64) -> bool {
        match self.names.iter().position(|n| n == instance) {
            Some(i) if (param == "r" || param == "resistance") && value != 0.0 => {
                self.g[i] = 1.0 / value;
                true
            }
            _ => false,
        }
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        let i = self.names.iter().position(|n| n == instance)?;
        (param == "r" || param == "resistance").then(|| 1.0 / self.g[i])
    }
}

/// Capacitors (`q = C (vp - vn)`).
#[derive(Debug, Default)]
pub struct Capacitors {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    c: Vec<f64>,
}

impl Capacitors {
    /// Add an instance.
    pub fn push(&mut self, name: &str, p: Node, n: Node, c: f64) {
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.c.push(c);
    }
}

impl DeviceGroup for Capacitors {
    fn type_name(&self) -> &'static str {
        "capacitor"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            let (p, n) = (self.p[i], self.n[i]);
            b.c(p, p);
            b.c(p, n);
            b.c(n, p);
            b.c(n, n);
            b.tracked_q(p);
            b.q(n);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let c = self.c[i];
            let q = c * (volt(x, self.p[i]) - volt(x, self.n[i]));
            out.c[4 * i..4 * i + 4].copy_from_slice(&[c, -c, -c, c]);
            out.q[2 * i] = q;
            out.q[2 * i + 1] = -q;
        }
    }
}

/// Inductors. Branch equation `vp - vn - d/dt (L i) = 0`, i.e. `f = vp - vn`, `q = -L i`.
#[derive(Debug, Default)]
pub struct Inductors {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    l: Vec<f64>,
    k: Vec<usize>,
}

impl Inductors {
    /// Add an instance.
    pub fn push(&mut self, name: &str, p: Node, n: Node, l: f64) {
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.l.push(l);
    }
}

impl DeviceGroup for Inductors {
    fn type_name(&self) -> &'static str {
        "inductor"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.k = self.names.iter().map(|n| u.alloc_branch(n)).collect();
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        if self.k.len() != self.len() {
            return Err(missing_branch(self.type_name()));
        }
        for i in 0..self.len() {
            let (p, n, k) = (self.p[i], self.n[i], Some(self.k[i]));
            b.g(p, k);
            b.g(n, k);
            b.g(k, p);
            b.g(k, n);
            b.c(k, k);
            b.f(p);
            b.f(n);
            b.f(k);
            b.q(k);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let cur = x[self.k[i]];
            let v = volt(x, self.p[i]) - volt(x, self.n[i]);
            out.g[4 * i..4 * i + 4].copy_from_slice(&[1.0, -1.0, 1.0, -1.0]);
            out.c[i] = -self.l[i];
            out.f[3 * i..3 * i + 3].copy_from_slice(&[cur, -cur, v]);
            out.q[i] = -self.l[i] * cur;
        }
    }
}

/// Voltage-controlled voltage sources (E): `vp - vn = gain (vcp - vcn)`.
#[derive(Debug, Default)]
pub struct Vcvs {
    names: Vec<String>,
    nodes: Vec<[Node; 4]>,
    gain: Vec<f64>,
    k: Vec<usize>,
}

impl Vcvs {
    /// Add an instance.
    pub fn push(&mut self, name: &str, nodes: [Node; 4], gain: f64) {
        self.names.push(name.to_string());
        self.nodes.push(nodes);
        self.gain.push(gain);
    }
}

impl DeviceGroup for Vcvs {
    fn type_name(&self) -> &'static str {
        "vcvs"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.k = self.names.iter().map(|n| u.alloc_branch(n)).collect();
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        if self.k.len() != self.len() {
            return Err(missing_branch(self.type_name()));
        }
        for i in 0..self.len() {
            let [p, n, cp, cn] = self.nodes[i];
            let k = Some(self.k[i]);
            for (r, c) in [(p, k), (n, k), (k, p), (k, n), (k, cp), (k, cn)] {
                b.g(r, c);
            }
            b.f(p);
            b.f(n);
            b.f(k);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let [p, n, cp, cn] = self.nodes[i];
            let a = self.gain[i];
            let cur = x[self.k[i]];
            out.g[6 * i..6 * i + 6].copy_from_slice(&[1.0, -1.0, 1.0, -1.0, -a, a]);
            let branch = volt(x, p) - volt(x, n) - a * (volt(x, cp) - volt(x, cn));
            out.f[3 * i..3 * i + 3].copy_from_slice(&[cur, -cur, branch]);
        }
    }
}

/// Voltage-controlled current sources (G): current `gm (vcp - vcn)` flows from `p` through the
/// source to `n`.
#[derive(Debug, Default)]
pub struct Vccs {
    names: Vec<String>,
    nodes: Vec<[Node; 4]>,
    gm: Vec<f64>,
}

impl Vccs {
    /// Add an instance.
    pub fn push(&mut self, name: &str, nodes: [Node; 4], gm: f64) {
        self.names.push(name.to_string());
        self.nodes.push(nodes);
        self.gm.push(gm);
    }
}

impl DeviceGroup for Vccs {
    fn type_name(&self) -> &'static str {
        "vccs"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for &[p, n, cp, cn] in &self.nodes {
            for (r, c) in [(p, cp), (p, cn), (n, cp), (n, cn)] {
                b.g(r, c);
            }
            b.f(p);
            b.f(n);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let [_, _, cp, cn] = self.nodes[i];
            let gm = self.gm[i];
            let cur = gm * (volt(x, cp) - volt(x, cn));
            out.g[4 * i..4 * i + 4].copy_from_slice(&[gm, -gm, -gm, gm]);
            out.f[2 * i] = cur;
            out.f[2 * i + 1] = -cur;
        }
    }
}

/// Current-controlled current sources (F): current `gain · i(control)` flows from `p` through
/// the source to `n`.
#[derive(Debug, Default)]
pub struct Cccs {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    control: Vec<String>,
    gain: Vec<f64>,
    kc: Vec<usize>,
}

impl Cccs {
    /// Add an instance controlled by the branch current of device `control`.
    pub fn push(&mut self, name: &str, p: Node, n: Node, control: &str, gain: f64) {
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.control.push(control.to_string());
        self.gain.push(gain);
    }
}

fn resolve_controls(
    names: &[String],
    controls: &[String],
    b: &StampBuilder<'_>,
) -> Result<Vec<usize>, Diagnostic> {
    names
        .iter()
        .zip(controls)
        .map(|(name, c)| {
            b.branch(c).ok_or_else(|| {
                Diagnostic::error(
                    "E-ELAB-CONTROL",
                    format!(
                        "`{name}` is controlled by `{c}`, which is not a voltage source or inductor"
                    ),
                )
                .with_note(
                    "current-controlled sources must reference a V source or inductor by name",
                )
            })
        })
        .collect()
}

impl DeviceGroup for Cccs {
    fn type_name(&self) -> &'static str {
        "cccs"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.kc = resolve_controls(&self.names, &self.control, b)?;
        for i in 0..self.len() {
            let kc = Some(self.kc[i]);
            b.g(self.p[i], kc);
            b.g(self.n[i], kc);
            b.f(self.p[i]);
            b.f(self.n[i]);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let a = self.gain[i];
            let cur = a * x[self.kc[i]];
            out.g[2 * i] = a;
            out.g[2 * i + 1] = -a;
            out.f[2 * i] = cur;
            out.f[2 * i + 1] = -cur;
        }
    }
}

/// Current-controlled voltage sources (H): `vp - vn = r · i(control)`.
#[derive(Debug, Default)]
pub struct Ccvs {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    control: Vec<String>,
    r: Vec<f64>,
    k: Vec<usize>,
    kc: Vec<usize>,
}

impl Ccvs {
    /// Add an instance controlled by the branch current of device `control`.
    pub fn push(&mut self, name: &str, p: Node, n: Node, control: &str, r: f64) {
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.control.push(control.to_string());
        self.r.push(r);
    }
}

impl DeviceGroup for Ccvs {
    fn type_name(&self) -> &'static str {
        "ccvs"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.k = self.names.iter().map(|n| u.alloc_branch(n)).collect();
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        if self.k.len() != self.len() {
            return Err(missing_branch(self.type_name()));
        }
        self.kc = resolve_controls(&self.names, &self.control, b)?;
        for i in 0..self.len() {
            let (p, n, k, kc) = (self.p[i], self.n[i], Some(self.k[i]), Some(self.kc[i]));
            for (r, c) in [(p, k), (n, k), (k, p), (k, n), (k, kc)] {
                b.g(r, c);
            }
            b.f(p);
            b.f(n);
            b.f(k);
        }
        Ok(())
    }
    fn eval(&self, _ctx: &crate::EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let r = self.r[i];
            let cur = x[self.k[i]];
            out.g[5 * i..5 * i + 5].copy_from_slice(&[1.0, -1.0, 1.0, -1.0, -r]);
            let branch = volt(x, self.p[i]) - volt(x, self.n[i]) - r * x[self.kc[i]];
            out.f[3 * i..3 * i + 3].copy_from_slice(&[cur, -cur, branch]);
        }
    }
}
