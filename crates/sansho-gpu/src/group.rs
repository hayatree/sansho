//! Device group adapter that routes evaluations to a [`Coordinator`].
//!
//! Provenance: original implementation. See `docs/PROVENANCE.md`.

use crate::coordinator::Coordinator;
use sansho_devices::{
    AcSlots, DeviceGroup, EvalCtx, FrequencyStamp, Node, NoiseSource, Slots, StampBuilder,
    StateRef, TranParams, Unknowns, bsim4, distortion, line, ltra,
};
use sansho_diag::Diagnostic;
use std::sync::Arc;

/// A bound device group whose Newton evaluations run on the GPU.
///
/// All other queries go to the wrapped CPU group. Output and startup-bias evaluations, which
/// are rare, also run on the CPU.
#[derive(Debug)]
pub struct Batched {
    inner: Arc<dyn DeviceGroup>,
    coordinator: Arc<Coordinator>,
    lane: usize,
    group: usize,
}

impl Batched {
    /// Wrap `inner`, whose parameters `lane` has already uploaded for plan group `group`.
    pub fn new(
        inner: Arc<dyn DeviceGroup>,
        coordinator: Arc<Coordinator>,
        lane: usize,
        group: usize,
    ) -> Self {
        Self {
            inner,
            coordinator,
            lane,
            group,
        }
    }
}

impl DeviceGroup for Batched {
    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.inner.instance_name(index)
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn is_linear(&self) -> bool {
        self.inner.is_linear()
    }
    fn distortion(
        &self,
        ctx: &EvalCtx,
        bias: &[f64],
        out: &mut Vec<distortion::Term>,
    ) -> Result<(), Diagnostic> {
        self.inner.distortion(ctx, bias, out)
    }
    fn line(&self) -> Option<&line::Line> {
        self.inner.line()
    }
    fn ltra(&self) -> Option<&ltra::Ltra> {
        self.inner.ltra()
    }
    fn instance_ic(
        &self,
        index: usize,
        ic: &[f64],
        x: &mut [f64],
        voltages: &mut Vec<(Node, Node, f64)>,
    ) {
        self.inner.instance_ic(index, ic, x, voltages);
    }
    fn output_charges(&self, index: usize, param: &str) -> Vec<(usize, f64)> {
        self.inner.output_charges(index, param)
    }
    fn initial_values(&self, x: &mut [f64], voltages: &mut Vec<(Node, Node, f64)>) {
        self.inner.initial_values(x, voltages);
    }
    fn instance_span(&self) -> Option<sansho_diag::Span> {
        self.inner.instance_span()
    }
    fn probe_nodes(&self) -> Vec<usize> {
        self.inner.probe_nodes()
    }
    fn settle(&self, x: &[f64], state: &mut [f64]) -> bool {
        self.inner.settle(x, state)
    }
    fn has_crossings(&self) -> bool {
        self.inner.has_crossings()
    }
    fn crossing(&self, old: &[f64], new: &[f64], accepted: &[f64], current: &[f64]) -> Option<f64> {
        self.inner.crossing(old, new, accepted, current)
    }
    fn state_len(&self) -> usize {
        self.inner.state_len()
    }
    fn setup(&mut self, _b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        // Only bound, already set-up groups are wrapped.
        Ok(())
    }
    fn alloc_unknowns(&mut self, _u: &mut Unknowns) {}
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        if !out.outputs.is_empty() || !out.initial_conditions.is_empty() {
            self.inner.eval(ctx, x, state, out);
        } else {
            self.coordinator
                .eval(self.lane, self.group, ctx, x, state, out);
        }
    }
    fn eval_cached(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        _cache: &mut bsim4::CoreCache,
        _tolerance: f64,
    ) {
        self.eval(ctx, x, state, out);
    }
    fn eval_fixed_voltage(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        _node: usize,
    ) {
        self.eval(ctx, x, state, out);
    }
    fn noise_components(&self) -> &'static [&'static str] {
        self.inner.noise_components()
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        slots: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        self.inner.noise(ctx, x, slots, out)
    }
    fn frequency_table(&self) -> Option<FrequencyStamp<'_>> {
        self.inner.frequency_table()
    }
    fn transport_delays(&self, state: &[f64], emit: &mut dyn FnMut(f64, Node, Node, f64)) {
        self.inner.transport_delays(state, emit);
    }
    fn ac_current(
        &self,
        i: usize,
        slots: &Slots,
        emit: &mut dyn FnMut(Node, f64, f64),
    ) -> Option<(f64, f64)> {
        self.inner.ac_current(i, slots, emit)
    }
    fn eval_ac(&self, out: &mut AcSlots) {
        self.inner.eval_ac(out);
    }
    fn step_limits(&self, tran: &TranParams, out: &mut Vec<(f64, f64)>) {
        self.inner.step_limits(tran, out);
    }
    fn exponential_step_limits(&self, reltol: f64, out: &mut Vec<(Node, Node, f64)>) {
        self.inner.exponential_step_limits(reltol, out);
    }
    fn has_random_waveform(&self) -> bool {
        self.inner.has_random_waveform()
    }
    fn next_random_breakpoint(&self, time: f64) -> f64 {
        self.inner.next_random_breakpoint(time)
    }
    fn breakpoints(&self, tran: &TranParams, out: &mut Vec<f64>) {
        self.inner.breakpoints(tran, out);
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        self.inner.param(instance, param)
    }
}
