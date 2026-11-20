//! Independent voltage and current sources.
//!
//! Provenance: original work. Source semantics follow SPICE conventions: a voltage source's
//! branch current flows into its positive terminal; a current source drives current from its
//! positive terminal through the source to its negative terminal. See `docs/PROVENANCE.md`.

use sansho_diag::Diagnostic;
use sansho_netlist::SourceSpec;

use crate::{
    AcSlots, DeviceGroup, EvalCtx, Mode, Node, Slots, StampBuilder, TranParams, Unknowns, volt,
    waveform,
};

#[derive(Debug, Default)]
struct SourceSet {
    names: Vec<String>,
    p: Vec<Node>,
    n: Vec<Node>,
    specs: Vec<SourceSpec>,
    streams: Vec<Option<crate::random::Stream>>,
}

impl SourceSet {
    fn push(&mut self, name: &str, p: Node, n: Node, spec: SourceSpec, seed: u64) {
        self.streams.push(
            matches!(
                spec.tran,
                Some(sansho_netlist::TranFn::TrRandom(_) | sansho_netlist::TranFn::TrNoise(_))
            )
            .then(|| crate::random::Stream::new(name, seed)),
        );
        self.names.push(name.to_string());
        self.p.push(p);
        self.n.push(n);
        self.specs.push(spec);
    }

    fn next_breakpoint(&self, t: f64) -> f64 {
        self.specs
            .iter()
            .zip(&self.streams)
            .filter_map(|(s, stream)| Some(stream.as_ref()?.next(s.tran.as_ref()?, t)))
            .fold(f64::INFINITY, f64::min)
    }
    fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Source value for the given context.
    fn value(&self, i: usize, ctx: &EvalCtx) -> f64 {
        let s = &self.specs[i];
        let v = match (ctx.mode, &s.tran) {
            (Mode::Tran, Some(f)) => self.streams[i].as_ref().map_or_else(
                || waveform::value(f, ctx.time, &ctx.tran),
                |s| s.value(f, ctx.time),
            ),
            _ => dc_value(s),
        };
        v * ctx.source_factor
    }

    fn ac(&self, i: usize) -> (f64, f64) {
        let s = &self.specs[i];
        let ph = s.ac_phase_deg.to_radians();
        (s.ac_mag * ph.cos(), s.ac_mag * ph.sin())
    }

    fn breakpoints(&self, tp: &TranParams, out: &mut Vec<f64>) {
        for s in &self.specs {
            if let Some(f) = &s.tran {
                waveform::breakpoints(f, tp, out);
            }
        }
    }

    fn step_limits(&self, tp: &TranParams, out: &mut Vec<(f64, f64)>) {
        out.extend(
            self.specs
                .iter()
                .filter_map(|s| waveform::step_limit(s.tran.as_ref()?, tp)),
        );
    }

    fn set_dc(&mut self, instance: &str, param: &str, value: f64) -> bool {
        match self.index(instance) {
            Some(i) if param == "dc" => {
                self.specs[i].dc = Some(value);
                true
            }
            _ => false,
        }
    }

    fn get_dc(&self, instance: &str, param: &str) -> Option<f64> {
        let i = self.index(instance)?;
        (param == "dc").then(|| dc_value(&self.specs[i]))
    }
}

/// DC value of a source: the explicit DC value, otherwise the transient waveform at t = 0
/// (with the defaults that do not depend on the analysis), otherwise 0.
pub fn dc_value(s: &SourceSpec) -> f64 {
    match (s.dc, &s.tran) {
        (Some(v), _) => v,
        (None, Some(f)) => waveform::initial_value(f),
        (None, None) => 0.0,
    }
}

/// Independent voltage sources: branch equation `vp - vn = V`.
#[derive(Debug, Default)]
pub struct VoltageSources {
    set: SourceSet,
    k: Vec<usize>,
}

impl VoltageSources {
    /// Add an instance.
    pub fn push(&mut self, name: &str, p: Node, n: Node, spec: SourceSpec) {
        self.push_seeded(name, p, n, spec, 1);
    }
    pub fn push_seeded(&mut self, name: &str, p: Node, n: Node, spec: SourceSpec, seed: u64) {
        self.set.push(name, p, n, spec, seed);
    }
}

impl DeviceGroup for VoltageSources {
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.set.names.get(index).map(String::as_str)
    }
    fn type_name(&self) -> &'static str {
        "vsource"
    }
    fn len(&self) -> usize {
        self.set.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.k = self.set.names.iter().map(|n| u.alloc_branch(n)).collect();
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        if self.k.len() != self.len() {
            return Err(Diagnostic::error(
                "E-ELAB-INTERNAL",
                "voltage source branches were not allocated",
            ));
        }
        for i in 0..self.len() {
            let (p, n, k) = (self.set.p[i], self.set.n[i], Some(self.k[i]));
            b.g(p, k);
            b.g(n, k);
            b.g(k, p);
            b.g(k, n);
            b.f(p);
            b.f(n);
            b.f(k);
            b.b(k);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let cur = x[self.k[i]];
            let v = volt(x, self.set.p[i]) - volt(x, self.set.n[i]);
            out.g[4 * i..4 * i + 4].copy_from_slice(&[1.0, -1.0, 1.0, -1.0]);
            if let Some(values) = out.outputs.get_mut(i) {
                values.extend([("i", cur), ("p", cur * v)]);
            }
            out.f[3 * i..3 * i + 3].copy_from_slice(&[cur, -cur, v]);
            out.b[i] = self.set.value(i, ctx);
        }
    }
    fn has_random_waveform(&self) -> bool {
        self.set.streams.iter().any(Option::is_some)
    }
    fn next_random_breakpoint(&self, time: f64) -> f64 {
        self.set.next_breakpoint(time)
    }
    fn eval_ac(&self, out: &mut AcSlots) {
        for i in 0..self.len() {
            (out.re[i], out.im[i]) = self.set.ac(i);
        }
    }
    fn breakpoints(&self, tp: &TranParams, out: &mut Vec<f64>) {
        self.set.breakpoints(tp, out);
    }
    fn step_limits(&self, tp: &TranParams, out: &mut Vec<(f64, f64)>) {
        self.set.step_limits(tp, out);
    }
    fn set_param(&mut self, instance: &str, param: &str, value: f64) -> bool {
        self.set.set_dc(instance, param, value)
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        self.set.get_dc(instance, param)
    }
}

/// Independent current sources.
#[derive(Debug, Default)]
pub struct CurrentSources {
    set: SourceSet,
}

impl CurrentSources {
    /// Add an instance.
    pub fn push(&mut self, name: &str, p: Node, n: Node, spec: SourceSpec) {
        self.push_seeded(name, p, n, spec, 1);
    }
    pub fn push_seeded(&mut self, name: &str, p: Node, n: Node, spec: SourceSpec, seed: u64) {
        self.set.push(name, p, n, spec, seed);
    }
}

impl DeviceGroup for CurrentSources {
    fn ac_current(
        &self,
        i: usize,
        _: &Slots,
        _: &mut dyn FnMut(Node, f64, f64),
    ) -> Option<(f64, f64)> {
        Some(self.set.ac(i))
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.set.names.get(index).map(String::as_str)
    }
    fn type_name(&self) -> &'static str {
        "isource"
    }
    fn len(&self) -> usize {
        self.set.names.len()
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            b.b(self.set.p[i]);
            b.b(self.set.n[i]);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _state: crate::StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            // Current leaves node p into the source and enters node n.
            let v = self.set.value(i, ctx);
            if let Some(values) = out.outputs.get_mut(i) {
                values.extend([
                    ("i", v),
                    ("current", v),
                    ("p", v * (volt(x, self.set.p[i]) - volt(x, self.set.n[i]))),
                ]);
            }
            out.b[2 * i] = -v;
            out.b[2 * i + 1] = v;
        }
    }
    fn has_random_waveform(&self) -> bool {
        self.set.streams.iter().any(Option::is_some)
    }
    fn next_random_breakpoint(&self, time: f64) -> f64 {
        self.set.next_breakpoint(time)
    }
    fn eval_ac(&self, out: &mut AcSlots) {
        for i in 0..self.len() {
            let (re, im) = self.set.ac(i);
            out.re[2 * i] = -re;
            out.im[2 * i] = -im;
            out.re[2 * i + 1] = re;
            out.im[2 * i + 1] = im;
        }
    }
    fn breakpoints(&self, tp: &TranParams, out: &mut Vec<f64>) {
        self.set.breakpoints(tp, out);
    }
    fn step_limits(&self, tp: &TranParams, out: &mut Vec<(f64, f64)>) {
        self.set.step_limits(tp, out);
    }
    fn set_param(&mut self, instance: &str, param: &str, value: f64) -> bool {
        self.set.set_dc(instance, param, value)
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        self.set.get_dc(instance, param)
    }
}
