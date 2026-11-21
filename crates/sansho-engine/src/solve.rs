//! Deterministic assembly and the Newton–Raphson solver.
//!
//! The circuit equations are `F(x) = f(x) + a0·q(x) + h − b = 0` with Jacobian
//! `J = G + a0·C`, where `a0` and the history vector `h` come from the integration method
//! (`a0 = 0`, `h = 0` for DC). Convergence uses SPICE-style per-unknown tolerances.
//!
//! Provenance: original work. Newton iteration and convergence criteria follow Nagel (1975).
//! See `docs/PROVENANCE.md`.

use sansho_devices::{EvalCtx, Mode, Slots, StateRef, TranParams, UnknownKind};
use sansho_diag::{Confidence, Culprit, CulpritKind, Diagnostic};
use sansho_sparse::{NewtonLu, SolverError};

use rayon::prelude::*;
use std::time::{Duration, Instant};

use crate::circuit::{Circuit, DISCARD, Gather};
use crate::options::SimOptions;

/// Integration terms for one timepoint.
#[derive(Debug, Clone)]
pub(crate) struct Integration {
    /// Coefficient of `q` (0 for DC).
    pub a0: f64,
    /// Nodal history terms for the Newton residual.
    pub hist: Vec<f64>,
}

impl Integration {
    pub(crate) fn dc(n: usize) -> Self {
        Self {
            a0: 0.0,
            hist: vec![0.0; n],
        }
    }
}

/// Assembled vectors and matrix values at one point.
#[derive(Debug)]
pub(crate) struct Assembled {
    /// Values of `G` in pattern order.
    pub g: Vec<f64>,
    /// Values of `C` in pattern order.
    pub c: Vec<f64>,
    pub f: Vec<f64>,
    pub q: Vec<f64>,
    pub b: Vec<f64>,
}

/// Reusable per-job workspace.
#[derive(Debug)]
pub(crate) struct Workspace {
    pub events: crate::digital::Events,
    timed: bool,
    detailed: bool,
    assembly_time: Duration,
    symbolic_time: Duration,
    full_factors: usize,
    repivot_time: Duration,
    refactor_time: Duration,
    gathered: u64,
    stamp_sizes: [u64; 3],
    refactors: usize,
    solve_time: Duration,
    last_linear_jac: Option<Vec<f64>>,
    identical_factors: usize,
    factor_time: std::time::Duration,
    group_time: std::time::Duration,
    pub assemblies: usize,
    pub newton_calls: usize,
    pub newton_iterations: usize,
    pub slots: Vec<Slots>,
    core_cache: Vec<sansho_devices::bsim4::CoreCache>,
    pub asm: Assembled,
    jac: Vec<f64>,
    rhs: Vec<f64>,
    lu: Option<NewtonLu>,
    /// Per-group device state of the current iteration.
    pub state: Vec<Vec<f64>>,
    /// Per-group device state at the last accepted solution.
    pub accepted: Vec<Vec<f64>>,
    /// Whether any device limited its controlling voltages in the last assembly.
    pub noncon: bool,
    /// Nodes held at a voltage through [`CLAMP_G`] (`.ic` / `.nodeset`): (unknown, volts).
    pub clamp: Vec<(usize, f64)>,
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if sansho_diag::profile_enabled() {
            use std::io::Write;
            let mut err = std::io::stderr().lock();
            for (name, duration) in [
                ("device", self.group_time),
                ("assembly", self.assembly_time),
                ("symbolic", self.symbolic_time),
                ("numeric", self.factor_time),
                ("solve", self.solve_time),
            ] {
                let _ = writeln!(
                    err,
                    "sansho-phase {name} {:.6}",
                    duration.as_secs_f64() * 1000.
                );
            }
            let _ = writeln!(
                err,
                "sansho-lu-ns {} {}",
                self.repivot_time.as_nanos(),
                self.refactor_time.as_nanos()
            );
            let _ = writeln!(err, "sansho-assembly-stamps {}", self.gathered);
            if self.gathered > 0 {
                let _ = writeln!(
                    err,
                    "sansho-assembly-ns-per-stamp {:.3}",
                    self.assembly_time.as_secs_f64() * 1e9 / self.gathered as f64
                );
            }
            let _ = writeln!(err, "sansho-workers {}", rayon::current_num_threads());
            let _ = sansho_devices::bsim4::CoreCache::report_profile(&self.core_cache, &mut err);
            let _ = writeln!(err, "sansho-identical {}", self.identical_factors);
            let _ = writeln!(
                err,
                "sansho-factors {} {}",
                self.full_factors, self.refactors
            );
        }
        if self.detailed {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-groups {} {} {}",
                rayon::current_num_threads(),
                self.assemblies,
                self.group_time.as_nanos()
            );
        }
    }
}

/// Conductance that holds `.ic` / `.nodeset` nodes at their value (S).
pub(crate) const CLAMP_G: f64 = 1e10;

impl Workspace {
    pub(crate) fn new(ckt: &Circuit) -> Self {
        let n = ckt.dim();
        let nnz = ckt.pattern.nnz();
        let profile_bsim =
            sansho_diag::profile_enabled() && std::env::var_os("SANSHO_PROFILE_BSIM4").is_some();
        Self {
            events: crate::digital::Events::new(&ckt.digital),
            timed: sansho_diag::profile_enabled() || sansho_diag::profile_devices(),
            detailed: sansho_diag::profile_devices(),
            assembly_time: Duration::ZERO,
            symbolic_time: Duration::ZERO,
            full_factors: 0,
            repivot_time: Duration::ZERO,
            refactor_time: Duration::ZERO,
            gathered: 0,
            stamp_sizes: {
                let count = |map: &Gather| map.contributions;
                let gm = &ckt.gather;
                let dc = count(&gm.g) + count(&gm.f) + count(&gm.b);
                [dc, dc + count(&gm.c) + count(&gm.q), count(&gm.q)]
            },
            refactors: 0,
            solve_time: Duration::ZERO,
            last_linear_jac: (ckt.linear && n >= 2048).then(Vec::new),
            identical_factors: 0,
            factor_time: std::time::Duration::ZERO,
            group_time: std::time::Duration::ZERO,
            assemblies: 0,
            newton_calls: 0,
            newton_iterations: 0,
            slots: ckt.new_slots(),
            core_cache: ckt
                .groups
                .iter()
                .map(|g| {
                    let mut c = sansho_devices::bsim4::CoreCache::default();
                    if profile_bsim && g.type_name() == "bsim4" {
                        c.enable_profile();
                    }
                    c
                })
                .collect(),
            asm: Assembled {
                g: vec![0.0; nnz],
                c: vec![0.0; nnz],
                f: vec![0.0; n],
                q: vec![0.0; n],
                b: vec![0.0; n],
            },
            jac: vec![0.0; nnz],
            rhs: vec![0.0; n],
            lu: None,
            state: ckt.state_len.iter().map(|&n| vec![0.0; n]).collect(),
            accepted: ckt.state_len.iter().map(|&n| vec![0.0; n]).collect(),
            noncon: false,
            clamp: Vec::new(),
        }
    }

    /// Accept the current device state.
    pub(crate) fn commit(&mut self) {
        for (a, s) in self.accepted.iter_mut().zip(&self.state) {
            a.copy_from_slice(s);
        }
    }

    /// Restart the working state from the accepted state.
    pub(crate) fn restore(&mut self) {
        for (s, a) in self.state.iter_mut().zip(&self.accepted) {
            s.copy_from_slice(a);
        }
    }

    /// Forget all device history (fresh analysis).
    pub(crate) fn clear_state(&mut self) {
        for v in self.state.iter_mut().chain(self.accepted.iter_mut()) {
            v.iter_mut().for_each(|x| *x = 0.0);
        }
    }
}

/// Build an evaluation context from the options.
pub(crate) fn make_ctx(opts: &SimOptions, mode: Mode, tran: TranParams) -> EvalCtx {
    EvalCtx {
        dc_sweep: false,
        initial_charge: false,
        mode,
        time: 0.0,
        tran,
        source_factor: 1.0,
        gmin: opts.gmin,
        temp: opts.temp + 273.15,
        limit: true,
        init_junctions: false,
        final_step: false,
    }
}

/// One device group with its slots and state, as zipped for evaluation.
type EvalItem<'a> = (
    (
        &'a std::sync::Arc<dyn sansho_devices::DeviceGroup>,
        &'a mut Slots,
    ),
    (
        (&'a mut Vec<f64>, &'a Vec<f64>),
        &'a mut sansho_devices::bsim4::CoreCache,
    ),
);

/// Evaluate all groups at `x` and sum contributions in a fixed order (architecture D4).
///
/// Device groups are evaluated in parallel (level L1) and every assembled value is gathered
/// from its sources in a fixed order (level L2), so the result is bit-identical for any
/// number of threads. `gshunt` is an extra conductance from every node to ground (gmin
/// stepping).
pub(crate) fn assemble(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &[f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    gshunt: f64,
) {
    assemble_for(ckt, ctx, x, ws, opts, gshunt, Assembly::Full);
}

/// Sample the exact converged charges and update device state without rebuilding the matrix.
pub(crate) fn assemble_charges(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &[f64],
    ws: &mut Workspace,
    opts: &SimOptions,
) -> Result<(), Diagnostic> {
    let fixed = fixed_voltage(ckt, ctx, x);
    assemble_for(
        ckt,
        ctx,
        x,
        ws,
        opts,
        0.,
        Assembly::Charges(fixed.map(|v| v.node)),
    );
    if let Some(d) = ws.slots.iter().find_map(|s| s.error.as_ref()) {
        return Err(d.clone());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Assembly {
    Full,
    Resistive,
    Charges(Option<usize>),
    Transient(Option<usize>),
}

fn fixed_voltage(ckt: &Circuit, ctx: &EvalCtx, x: &[f64]) -> Option<crate::circuit::FixedVoltage> {
    let fixed = ckt.fixed_voltage?;
    // Adding either zero sign to a +0 iterate yields +0. A negative-zero
    // iterate can instead expose the sign of a zero triangular update.
    (ctx.mode == Mode::Tran
        && ctx.time > 0.
        && ctx.source_factor == 1.
        && x[fixed.node].to_bits() == fixed.voltage.to_bits()
        && !x.iter().any(|v| v.to_bits() == (-0.0f64).to_bits()))
    .then_some(fixed)
}

fn assemble_for(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &[f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    gshunt: f64,
    request: Assembly,
) {
    ws.assemblies += 1;
    if ws.timed {
        ws.gathered += ws.stamp_sizes[match request {
            Assembly::Resistive => 0,
            Assembly::Charges(_) => 2,
            _ => 1,
        }];
    }
    let par = ckt.parallel && rayon::current_num_threads() > 1;
    let eval = |((group, s), ((current, accepted), cache)): EvalItem<'_>| {
        s.noncon = false;
        s.error = None;
        let mut state = StateRef { accepted, current };
        if matches!(request, Assembly::Charges(_))
            && s.initial_conditions.is_empty()
            && group.sample_charges(x, &mut state, s, cache)
        {
            return;
        }
        if let Assembly::Charges(Some(node)) | Assembly::Transient(Some(node)) = request {
            group.eval_fixed_voltage(ctx, x, state, s, node);
        } else if ckt.dim() >= 256 || opts.bypass || group.type_name() == "bsim4" {
            group.eval_cached(
                ctx,
                x,
                state,
                s,
                cache,
                if opts.bypass { opts.vntol } else { 0. },
            );
        } else {
            group.eval(ctx, x, state, s);
        }
    };
    let started = ws.timed.then(std::time::Instant::now);
    let states = ws
        .state
        .iter_mut()
        .zip(ws.accepted.iter())
        .zip(&mut ws.core_cache);
    if par {
        ckt.groups
            .par_iter()
            .zip(ws.slots.par_iter_mut())
            .zip(
                ws.state
                    .par_iter_mut()
                    .zip(ws.accepted.par_iter())
                    .zip(ws.core_cache.par_iter_mut()),
            )
            .for_each(eval);
    } else {
        ckt.groups
            .iter()
            .zip(ws.slots.iter_mut())
            .zip(states)
            .for_each(eval);
    }
    if let Some(started) = started {
        ws.group_time += started.elapsed();
    }
    let gather_start = ws.timed.then(Instant::now);
    ws.noncon = ws.slots.iter().any(|s| s.noncon);
    let slots = &ws.slots;
    let a = &mut ws.asm;
    let gm = &ckt.gather;
    if matches!(request, Assembly::Charges(_)) {
        gather(&mut a.q, &gm.q, slots, |s| &s.q, par);
        if let Some(t) = gather_start {
            ws.assembly_time += t.elapsed();
        }
        return;
    }
    let mut conductance = || {
        gather(&mut a.g, &gm.g, slots, |s| &s.g, par);
        gather(&mut a.f, &gm.f, slots, |s| &s.f, par);
    };
    let mut charge_and_sources = || {
        if request != Assembly::Resistive {
            gather(&mut a.c, &gm.c, slots, |s| &s.c, par);
            gather(&mut a.q, &gm.q, slots, |s| &s.q, par);
        }
        gather(&mut a.b, &gm.b, slots, |s| &s.b, par);
    };
    if par {
        rayon::join(conductance, charge_and_sources);
    } else {
        conductance();
        charge_and_sources();
    }
    ws.events
        .stamp(&ckt.digital, &mut a.b, ctx.time, ctx.source_factor);
    for &(i, v) in &ws.clamp {
        let d = ckt.node_diag[i];
        if d != DISCARD {
            a.g[d] += CLAMP_G;
            a.f[i] += CLAMP_G * (x[i] - v);
        }
    }
    let g = opts.rshunt.map_or(0.0, |r| 1.0 / r) + gshunt;
    if g > 0.0 {
        for (i, &d) in ckt.node_diag.iter().enumerate() {
            if d != DISCARD {
                a.g[d] += g;
                a.f[i] += g * x[i];
            }
        }
    }
    if let Some(t) = gather_start {
        ws.assembly_time += t.elapsed();
    }
}

/// Destinations per source-ordered native scatter tile.
pub(crate) const SCATTER_TILE: usize = 512;
/// Destinations per task in the CSR gather.
pub(crate) const GATHER_CHUNK: usize = 2048;

/// `dst[i] = Σ sources of i`, accumulated in the fixed order of `map`.
fn gather(dst: &mut [f64], map: &Gather, slots: &[Slots], pick: fn(&Slots) -> &[f64], par: bool) {
    if par && !map.tiles.is_empty() {
        dst.par_chunks_mut(SCATTER_TILE)
            .zip(&map.tiles)
            .for_each(|(out, tile)| {
                out.fill(0.);
                for (g, group) in tile {
                    group.sum(pick(&slots[*g]), out);
                }
            });
        return;
    }
    let Some((ptr, src)) = map.by_destination.as_ref().filter(|_| par) else {
        dst.fill(0.);
        for (slots, group) in slots.iter().zip(&map.by_group) {
            group.sum(pick(slots), dst);
        }
        return;
    };
    let fill = |base: usize, out: &mut [f64]| {
        for (j, d) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for &(g, k) in &src[ptr[base + j]..ptr[base + j + 1]] {
                sum += pick(&slots[g as usize])[k as usize];
            }
            *d = sum;
        }
    };
    dst.par_chunks_mut(GATHER_CHUNK)
        .enumerate()
        .for_each(|(c, out)| fill(c * GATHER_CHUNK, out));
}

/// An unknown that failed to converge.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Worst {
    pub index: usize,
    pub value: f64,
    pub ratio: f64,
}

/// Number of non-converged unknowns reported in diagnostics.
const REPORT_WORST: usize = 3;

/// Keep the `REPORT_WORST` largest ratios (ties keep the lower index: deterministic).
fn push_worst(list: &mut Vec<Worst>, w: Worst) {
    if w.ratio <= 1.0 {
        return;
    }
    let pos = list
        .iter()
        .position(|o| w.ratio > o.ratio)
        .unwrap_or(list.len());
    if pos < REPORT_WORST {
        list.insert(pos, w);
        list.truncate(REPORT_WORST);
    }
}

/// Newton failure modes.
#[derive(Debug)]
pub(crate) enum NewtonError {
    /// No convergence within the iteration limit; the unknowns with the largest update
    /// relative to their tolerance in the last iteration, largest first.
    NoConvergence { worst: Vec<Worst> },
    /// Singular Jacobian.
    Singular,
    /// Other solver failure.
    Solver(SolverError),
    /// Runtime device-model diagnostic.
    Device(Box<Diagnostic>),
}

/// Newton–Raphson iteration on `x` (updated in place). Returns the number of iterations.
///
/// Device state restarts from the accepted state. On the first iteration `ctx.init_junctions`
/// is honoured; later iterations always use `x`. The caller commits the state on success.
#[allow(clippy::too_many_arguments)]
pub(crate) fn newton(
    ckt: &Circuit,
    ctx: &EvalCtx,
    integ: &Integration,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    max_iter: usize,
    gshunt: f64,
) -> Result<usize, NewtonError> {
    ws.newton_calls += 1;
    if x.is_empty() {
        return Ok(0);
    }
    if ws.lu.is_none() {
        let start = ws.timed.then(Instant::now);
        ws.lu = Some(
            match &ckt.ordering {
                Some(o) => NewtonLu::from_ordering(o.clone()),
                None => NewtonLu::new(&ckt.pattern),
            }
            .map_err(NewtonError::Solver)?,
        );
        if let Some(t) = start {
            ws.symbolic_time += t.elapsed();
        }
    }
    ws.restore();
    let mut used = 0;
    let mut ctx = *ctx;
    let mut visited = Vec::new();
    loop {
        used += continuous_newton(ckt, &ctx, integ, x, ws, opts, max_iter - used, gshunt)?;
        let mut changed = false;
        for &i in &ckt.discrete {
            changed |= ckt.groups[i].settle(x, &mut ws.state[i]);
        }
        if !changed {
            return Ok(used);
        }
        let configuration: Vec<_> = ckt.discrete.iter().map(|&i| ws.state[i][0] > 0.).collect();
        if used >= max_iter || visited.contains(&configuration) {
            return Err(NewtonError::NoConvergence { worst: Vec::new() });
        }
        visited.push(configuration);
        ctx.init_junctions = false;
    }
}

#[allow(clippy::too_many_arguments)]
fn continuous_newton(
    ckt: &Circuit,
    ctx: &EvalCtx,
    integ: &Integration,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    max_iter: usize,
    gshunt: f64,
) -> Result<usize, NewtonError> {
    let n = ckt.dim();
    let kinds = ckt.unknowns.kinds();
    let mut ctx = *ctx;
    for iter in 1..=max_iter {
        ws.newton_iterations += 1;
        if iter > 1 {
            ctx.init_junctions = false;
        }
        let static_step = integ.a0 == 0.;
        let fixed = if static_step {
            None
        } else {
            fixed_voltage(ckt, &ctx, x).filter(|v| integ.hist[v.equation] == 0.)
        };
        let request = if static_step {
            Assembly::Resistive
        } else {
            Assembly::Transient(fixed.map(|v| v.node))
        };
        assemble_for(ckt, &ctx, x, ws, opts, gshunt, request);
        if let Some(d) = ws.slots.iter().find_map(|s| s.error.clone()) {
            return Err(NewtonError::Device(Box::new(d)));
        }
        let assemble_start = ws.timed.then(Instant::now);
        let a = &ws.asm;
        if static_step {
            ws.jac.copy_from_slice(&a.g);
            for i in 0..n {
                ws.rhs[i] = -(a.f[i] + integ.hist[i] - a.b[i]);
            }
        } else {
            for ((dst, &g), &c) in ws.jac.iter_mut().zip(&a.g).zip(&a.c) {
                *dst = g + integ.a0 * c;
            }
            for i in 0..n {
                ws.rhs[i] = -(a.f[i] + integ.a0 * a.q[i] + integ.hist[i] - a.b[i]);
            }
        }
        if let Some(fixed) = fixed {
            // The last pivot is the ideal source equation. Its Newton correction
            // is zero, so no earlier equation needs an upper entry in this column.
            let start = ckt.pattern.col_ptr()[fixed.node];
            let end = ckt.pattern.col_ptr()[fixed.node + 1];
            for (&r, a) in ckt.pattern.row_idx()[start..end]
                .iter()
                .zip(&mut ws.jac[start..end])
            {
                if r != fixed.equation && a.is_finite() {
                    *a = 0.;
                }
            }
        }
        if let Some(t) = assemble_start {
            ws.assembly_time += t.elapsed();
        }
        let dump = ws.newton_iterations == 1
            && std::env::var("SANSHO_PROFILE_MATRIX").as_deref() == Ok("entries");
        if dump {
            use std::io::Write;
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "sansho-first-rhs {:?}", ws.rhs);
            let _ = writeln!(
                stderr,
                "sansho-csc {:?} {:?} {:?}",
                ckt.pattern.col_ptr(),
                ckt.pattern.row_idx(),
                ws.jac
            );
        }
        let lu_start = ws.timed.then(std::time::Instant::now);
        let lu = ws
            .lu
            .as_mut()
            .ok_or(NewtonError::Solver(SolverError::NotReady))?;
        let identical = ws.last_linear_jac.as_ref().is_some_and(|old| {
            old.len() == ws.jac.len()
                && old
                    .iter()
                    .zip(&ws.jac)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
        });
        if identical {
            if ws.timed {
                ws.identical_factors += 1;
            }
        } else {
            let factor = lu.factor_with_status(&ws.jac);
            if ws.timed {
                if matches!(factor, Ok(true)) {
                    ws.refactors += 1;
                } else {
                    ws.full_factors += 1;
                }
            }
            if let Some(old) = &mut ws.last_linear_jac {
                old.clear();
                if factor.is_ok() {
                    old.extend_from_slice(&ws.jac);
                }
            }
            if let Some(t) = lu_start {
                let elapsed = t.elapsed();
                ws.factor_time += elapsed;
                if !matches!(factor, Ok(true)) {
                    ws.repivot_time += elapsed;
                } else {
                    ws.refactor_time += elapsed;
                }
            }
            match factor {
                Ok(_) => {}
                Err(SolverError::Singular { .. }) => return Err(NewtonError::Singular),
                Err(e) => return Err(NewtonError::Solver(e)),
            }
        }
        if identical && let Some(t) = lu_start {
            ws.factor_time += t.elapsed();
        }
        let solve_start = ws.timed.then(Instant::now);
        match lu.solve(&mut ws.rhs) {
            Ok(()) => {}
            Err(SolverError::Singular { .. }) => return Err(NewtonError::Singular),
            Err(e) => return Err(NewtonError::Solver(e)),
        }
        if let Some(t) = solve_start {
            ws.solve_time += t.elapsed();
        }
        if dump {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-first-solution {:?}",
                ws.rhs
            );
        }
        let mut converged = true;
        let mut worst = Vec::new();
        for i in 0..n {
            let old = x[i];
            let new = old + ws.rhs[i];
            x[i] = new;
            // Later elements still receive their Newton update. Only the last
            // allowed iteration needs the full list of tolerance violations.
            if iter != max_iter && (!converged || iter == 1 || ws.noncon) {
                continue;
            }
            let atol = match kinds[i] {
                UnknownKind::Voltage => opts.vntol,
                UnknownKind::Current => opts.abstol,
            };
            let tol = opts.reltol * old.abs().max(new.abs()) + atol;
            let ratio = ws.rhs[i].abs() / tol;
            if ratio > 1.0 {
                converged = false;
                if max_iter == iter {
                    push_worst(
                        &mut worst,
                        Worst {
                            index: i,
                            value: new,
                            ratio,
                        },
                    );
                }
            }
        }
        if x.iter().any(|v| !v.is_finite()) {
            return Err(NewtonError::Singular);
        }
        // A linear circuit is solved exactly by one Newton step.
        if ckt.linear || (converged && iter > 1 && !ws.noncon && !ctx.init_junctions) {
            if ws.detailed {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "sansho-factor {} {}",
                    ws.newton_iterations,
                    ws.factor_time.as_nanos()
                );
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "sansho-newton {} {} {}",
                    ctx.time,
                    iter,
                    (ws.solve_time + ws.factor_time).as_nanos()
                );
            }
            return Ok(iter);
        }
        if max_iter == iter {
            return Err(NewtonError::NoConvergence { worst });
        }
    }
    Err(NewtonError::NoConvergence { worst: Vec::new() })
}

/// Convert a Newton failure into a structured diagnostic.
///
/// `code` is the analysis-specific convergence code (`E-CONV-OP`, ...), used when Newton did
/// not converge; a singular matrix is always `E-SOLVE-SINGULAR`.
pub(crate) fn newton_diagnostic(
    ckt: &Circuit,
    err: &NewtonError,
    code: &'static str,
    analysis: &str,
) -> Diagnostic {
    match err {
        NewtonError::Device(d) => (**d).clone(),
        NewtonError::Singular => {
            let mut d = Diagnostic::error(
                "E-SOLVE-SINGULAR",
                format!("{analysis}: the circuit matrix is singular"),
            );
            if ckt.no_dc_path.is_empty() {
                d = d.with_suggestion(
                    "look for loops of voltage sources/inductors or cut sets of current sources",
                    Confidence::Medium,
                );
            } else {
                for n in &ckt.no_dc_path {
                    d = d.with_culprit(Culprit::new(
                        CulpritKind::Node,
                        n.clone(),
                        "no DC path to ground",
                    ));
                }
                d = d
                    .with_suggestion(
                        "add a resistor to ground at the culprit node(s)",
                        Confidence::High,
                    )
                    .with_suggestion("add `.options rshunt=1e12`", Confidence::High);
            }
            d
        }
        NewtonError::NoConvergence { worst } => {
            let mut d = Diagnostic::error(
                code,
                format!("{analysis}: Newton iteration did not converge"),
            );
            let names = ckt.unknowns.names();
            let node_name = |i: usize| {
                let n = names.get(i).map_or("?", String::as_str);
                n.strip_prefix("v(")
                    .and_then(|n| n.strip_suffix(')'))
                    .unwrap_or(n)
                    .to_string()
            };
            for w in worst {
                let kind = if w.index < ckt.dim()
                    && ckt.unknowns.kinds()[w.index] == UnknownKind::Voltage
                {
                    CulpritKind::Node
                } else {
                    CulpritKind::Branch
                };
                let name = match kind {
                    CulpritKind::Node => node_name(w.index),
                    _ => names.get(w.index).cloned().unwrap_or_default(),
                };
                d = d.with_culprit(Culprit {
                    kind,
                    name,
                    reason: Some("largest Newton update in the last iteration".into()),
                    value: Some(w.value),
                    update_ratio: Some(w.ratio),
                });
            }
            if let Some(w) = worst.first() {
                for dev in ckt.devices_at(w.index).iter().take(4) {
                    d = d.with_culprit(Culprit::new(
                        CulpritKind::Device,
                        dev.clone(),
                        format!("connected to node `{}`", node_name(w.index)),
                    ));
                }
            }
            d
        }
        NewtonError::Solver(e) => Diagnostic::error("E-SOLVE-BACKEND", format!("{analysis}: {e}")),
    }
}
