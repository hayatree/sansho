//! Analyses: DC operating point, DC sweep, AC sweep, transient.
//!
//! Provenance: original work. The transient algorithm (backward Euler at start and after
//! breakpoints, trapezoidal rule otherwise, breakpoint-aligned steps) follows the classic SPICE
//! approach described in Nagel (1975). See `docs/PROVENANCE.md`.

use rayon::prelude::*;
use spx_devices::{AcSlots, EvalCtx, Mode, TranParams, UnknownKind};
use spx_diag::{Confidence, Diagnostic};
use spx_netlist::AcSweep;
use spx_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};
use spx_sparse::{FaerLu, LinearSolver, c64};

use crate::circuit::Circuit;
use crate::options::Method;
use crate::options::SimOptions;
use crate::solve::{Integration, Workspace, assemble, make_ctx, newton, newton_diagnostic};

const NO_TRAN: TranParams = TranParams {
    tstep: 0.0,
    tstop: 0.0,
};

fn variables(ckt: &Circuit) -> Vec<Variable> {
    ckt.unknowns
        .names()
        .iter()
        .zip(ckt.unknowns.kinds())
        .map(|(name, k)| Variable {
            name: name.clone(),
            quantity: match k {
                UnknownKind::Voltage => Quantity::Voltage,
                UnknownKind::Current => Quantity::Current,
            },
        })
        .collect()
}

/// Solve the DC operating point with the given source mode.
///
/// Strategy: plain Newton from the device initial guesses; if that fails, gmin stepping (a
/// shunt conductance on every node, reduced geometrically to zero); if that fails too, source
/// stepping (all independent sources ramped from 0 to their full value). On success the device
/// state is committed.
pub(crate) fn solve_op(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    code: &'static str,
    analysis: &str,
) -> Result<(), Diagnostic> {
    // `.nodeset`: solve with the nodes held at their values, then release them.
    if !ckt.nodesets.is_empty() && ws.clamp.is_empty() {
        ws.clamp = ckt.nodesets.clone();
        let held = solve_op_chain(ckt, ctx, x, ws, opts, code, analysis);
        ws.clamp.clear();
        if held.is_ok() {
            let integ = Integration::dc(ckt.dim());
            if newton(ckt, ctx, &integ, x, ws, opts, opts.itl1, 0.0).is_ok() {
                ws.commit();
                return Ok(());
            }
        }
        x.iter_mut().for_each(|v| *v = 0.0);
    }
    solve_op_chain(ckt, ctx, x, ws, opts, code, analysis)
}

/// The fallback chain of [`solve_op`] (Newton, gmin stepping, source stepping).
fn solve_op_chain(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    code: &'static str,
    analysis: &str,
) -> Result<(), Diagnostic> {
    let integ = Integration::dc(ckt.dim());
    let mut init = *ctx;
    init.init_junctions = true;
    let x0 = x.to_vec();
    ws.clear_state();
    let first = match newton(ckt, &init, &integ, x, ws, opts, opts.itl1, 0.0) {
        Ok(_) => {
            ws.commit();
            return Ok(());
        }
        Err(e) => e,
    };
    // A nonlinear Jacobian can lose rank at a trial bias. Continuation
    // can still reach a regular operating point from the saved initial guess.
    if ckt.linear || matches!(first, crate::solve::NewtonError::Device(_)) {
        return Err(newton_diagnostic(ckt, &first, code, analysis).with_attempts(["newton"]));
    }
    let mut attempts = vec!["newton"];
    if opts.gmin_steps > 0 {
        attempts.push("gmin_stepping");
        if gmin_stepping(ckt, &init, &integ, x, &x0, ws, opts) {
            return Ok(());
        }
    }
    if opts.src_steps > 0 {
        attempts.push("source_stepping");
        if source_stepping(ckt, &init, &integ, x, ws, opts) {
            return Ok(());
        }
    }
    let mut d = newton_diagnostic(ckt, &first, code, analysis).with_attempts(attempts);
    if opts.itl1 < 500 {
        d = d.with_suggestion(
            format!(
                "raise the iteration limit: `.options itl1=500` (now {})",
                opts.itl1
            ),
            Confidence::Low,
        );
    }
    if opts.gmin_steps == 0 || opts.src_steps == 0 {
        d = d.with_suggestion(
            "re-enable continuation: `.options gminsteps=100 srcsteps=100`",
            Confidence::Medium,
        );
    }
    if !ckt.no_dc_path.is_empty() {
        d = d.with_suggestion("add `.options rshunt=1e12`", Confidence::Medium);
    }
    Err(d.with_suggestion(
        "check the culprit devices for floating gates, missing bias or swapped terminals",
        Confidence::Medium,
    ))
}

/// Gmin stepping. Returns `true` on success with `x` and the committed state set.
fn gmin_stepping(
    ckt: &Circuit,
    init: &EvalCtx,
    integ: &Integration,
    x: &mut [f64],
    x0: &[f64],
    ws: &mut Workspace,
    opts: &SimOptions,
) -> bool {
    let mut ctx = *init;
    x.copy_from_slice(x0);
    ws.clear_state();
    let mut g = 1e-2;
    let mut factor = 10.0f64;
    let mut saved = x.to_vec();
    let mut solves = 0;
    // Find a starting shunt that converges.
    loop {
        solves += 1;
        if newton(ckt, &ctx, integ, x, ws, opts, opts.itl1, g).is_ok() {
            ws.commit();
            break;
        }
        if solves >= opts.gmin_steps || g >= 1.0 {
            return false;
        }
        x.copy_from_slice(x0);
        ws.clear_state();
        g *= 10.0;
    }
    ctx.init_junctions = false;
    let floor = opts.gmin.max(1e-15);
    while solves < opts.gmin_steps {
        let next = if g <= floor {
            0.0
        } else {
            (g / factor).max(floor * 0.999)
        };
        saved.copy_from_slice(x);
        solves += 1;
        match newton(ckt, &ctx, integ, x, ws, opts, opts.itl1, next) {
            Ok(_) => {
                ws.commit();
                if next == 0.0 {
                    return true;
                }
                g = next;
                factor = (factor * 2.0).min(10.0);
            }
            Err(_) => {
                x.copy_from_slice(&saved);
                factor = factor.sqrt();
                if factor < 1.0001 {
                    return false;
                }
            }
        }
    }
    false
}

/// Source stepping. Returns `true` on success with `x` and the committed state set.
fn source_stepping(
    ckt: &Circuit,
    init: &EvalCtx,
    integ: &Integration,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
) -> bool {
    let mut ctx = *init;
    x.iter_mut().for_each(|v| *v = 0.0);
    ws.clear_state();
    ctx.source_factor = 0.0;
    ctx.init_junctions = false;
    if newton(ckt, &ctx, integ, x, ws, opts, opts.itl1, 0.0).is_err() {
        return false;
    }
    ws.commit();
    let mut s = 0.0f64;
    let mut ds = 0.1f64;
    let mut saved = x.to_vec();
    for _ in 0..opts.src_steps {
        let next = (s + ds).min(1.0);
        ctx.source_factor = next;
        saved.copy_from_slice(x);
        match newton(ckt, &ctx, integ, x, ws, opts, opts.itl1, 0.0) {
            Ok(_) => {
                ws.commit();
                if next >= 1.0 {
                    return true;
                }
                s = next;
                ds = (ds * 1.5).min(0.5);
            }
            Err(_) => {
                x.copy_from_slice(&saved);
                ds /= 4.0;
                if ds < 1e-7 {
                    return false;
                }
            }
        }
    }
    false
}

/// `.op`
pub(crate) fn op(ckt: &Circuit, opts: &SimOptions) -> Result<AnalysisResult, Diagnostic> {
    let _phase = spx_diag::Phase::start("analysis");
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; ckt.dim()];
    solve_op(
        ckt,
        &EvalCtx {
            final_step: true,
            ..make_ctx(opts, Mode::Dc, NO_TRAN)
        },
        &mut x,
        &mut ws,
        opts,
        "E-CONV-OP",
        "operating point",
    )?;
    Ok(AnalysisResult {
        kind: AnalysisKind::Op,
        scale: None,
        scale_values: Vec::new(),
        variables: variables(ckt),
        data: Data::Real {
            columns: x.into_iter().map(|v| vec![v]).collect(),
        },
    })
}

/// `.dc source start stop step`
pub(crate) fn dc_sweep(
    ckt: &mut Circuit,
    opts: &SimOptions,
    nl: &spx_netlist::Netlist,
    source: &str,
    start: f64,
    stop: f64,
    step: f64,
) -> Result<(AnalysisResult, crate::DcStats), Diagnostic> {
    let _phase = spx_diag::Phase::start("analysis");
    let temperature = source == "temp";
    let mut previous_iterations = 0;
    let mut previous_calls = 0;
    let Some(original) = (if temperature {
        Some(opts.temp)
    } else {
        ckt.param(source, "dc")
    }) else {
        return Err(Diagnostic::error(
            "E-DC-SOURCE",
            format!("DC sweep source `{source}` is not an independent voltage or current source"),
        ));
    };
    let is_current = source.starts_with('i');
    let count = ((stop - start) / step + 1e-9).floor() as usize + 1;
    let mut ws = Workspace::new(ckt);
    let n = ckt.dim();
    let mut x = vec![0.0; n];
    let mut scale = Vec::with_capacity(count);
    let mut cols = vec![Vec::with_capacity(count); n];
    let mut result = Ok(());
    let mut ctx = make_ctx(opts, Mode::Dc, NO_TRAN);
    let integ = Integration::dc(n);
    for k in 0..count {
        let v = start + k as f64 * step;
        ctx.final_step = k + 1 == count;
        let point;
        let ckt = if temperature {
            if v <= -273.15 {
                return Err(Diagnostic::error(
                    "E-PARSE-VALUE",
                    "temperature must exceed absolute zero",
                ));
            }
            let mut options = opts.clone();
            options.temp = v;
            point = crate::circuit::compile_cached(nl, &options, &ckt.va_modules, true)
                .map_err(|ds| {
                    ds.into_iter().next().unwrap_or_else(|| {
                        Diagnostic::error("E-ELAB-PARAM", "temperature elaboration failed")
                    })
                })?
                .0;
            if point.unknowns.names() != ckt.unknowns.names() {
                return Err(Diagnostic::error(
                    "E-ELAB-PARAM",
                    "temperature sweep changed the circuit unknowns",
                ));
            }
            previous_iterations += ws.newton_iterations;
            previous_calls += ws.newton_calls;
            let mut fresh = Workspace::new(&point);
            if fresh
                .accepted
                .iter()
                .map(Vec::len)
                .eq(ws.accepted.iter().map(Vec::len))
            {
                std::mem::swap(&mut fresh.accepted, &mut ws.accepted);
            }
            ws = fresh;
            ctx.temp = v + 273.15;
            &point
        } else if !ckt.set_param(source, "dc", v) {
            result = Err(Diagnostic::error(
                "E-DC-SOURCE",
                format!("cannot apply dc={v} to {source} in the current circuit layout"),
            ));
            break;
        } else {
            &*ckt
        };
        // Continue from the previous point; fall back to a full operating-point solve.
        let continued = k > 0
            && newton(ckt, &ctx, &integ, &mut x, &mut ws, opts, opts.itl1, 0.0)
                .map(|_| ws.commit())
                .is_ok();
        if !continued
            && let Err(d) = solve_op(ckt, &ctx, &mut x, &mut ws, opts, "E-CONV-DC", "DC sweep")
        {
            result = Err(d.with_note(format!("at {source} = {v:e}")));
            break;
        }
        scale.push(v);
        for (c, &xv) in cols.iter_mut().zip(&x) {
            c.push(xv);
        }
    }
    if !temperature {
        ckt.set_param(source, "dc", original);
    }
    result?;
    let stats = crate::DcStats {
        source: source.to_owned(),
        points: count,
        iterations: previous_iterations + ws.newton_iterations,
        extra_solves: (previous_calls + ws.newton_calls).saturating_sub(count),
    };
    Ok((
        AnalysisResult {
            kind: AnalysisKind::Dc,
            scale: Some(Variable {
                name: if temperature {
                    "temp-sweep"
                } else if is_current {
                    "i-sweep"
                } else {
                    "v-sweep"
                }
                .into(),
                quantity: if temperature {
                    Quantity::Temperature
                } else if is_current {
                    Quantity::Current
                } else {
                    Quantity::Voltage
                },
            }),
            scale_values: scale,
            variables: variables(ckt),
            data: Data::Real { columns: cols },
        },
        stats,
    ))
}

pub(crate) fn ac_frequencies(
    sweep: AcSweep,
    points: u32,
    fstart: f64,
    fstop: f64,
) -> Result<Vec<f64>, Diagnostic> {
    if points == 0 || fstart <= 0. || fstop < fstart || !fstart.is_finite() || !fstop.is_finite() {
        return Err(crate::small_signal::error("invalid frequency grid"));
    }
    let np = f64::from(points);
    let ratio = fstop / fstart;
    let log_span = if ratio.is_finite() {
        ratio.ln()
    } else {
        fstop.ln() - fstart.ln()
    };
    let base: f64 = if sweep == AcSweep::Oct { 2. } else { 10. };
    let count = if sweep == AcSweep::Lin {
        points as usize
    } else {
        ((log_span / base.ln() * np + 1e-9).floor() as usize)
            .checked_add(1)
            .ok_or_else(|| crate::small_signal::error("frequency grid is too large"))?
    };
    let mut freq = Vec::new();
    freq.try_reserve_exact(count)
        .map_err(|_| crate::small_signal::error("cannot allocate frequency grid"))?;
    if sweep == AcSweep::Lin {
        let step = (fstop - fstart) / (np - 1.).max(1.);
        freq.extend((0..count).map(|k| fstart + k as f64 * step));
    } else if ratio.is_finite() {
        freq.extend((0..count).map(|k| fstart * base.powf(k as f64 / np)));
    } else {
        freq.extend((0..count).map(|k| (fstart.ln() + base.ln() * (k as f64 / np)).exp()));
    }
    Ok(freq)
}

pub(crate) fn linearize(
    ckt: &Circuit,
    opts: &SimOptions,
    mode: Mode,
) -> Result<(Vec<f64>, Workspace, EvalCtx), Diagnostic> {
    let n = ckt.dim();
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; n];
    let mut ctx = make_ctx(opts, Mode::Dc, NO_TRAN);
    solve_op(
        ckt,
        &ctx,
        &mut x,
        &mut ws,
        opts,
        "E-CONV-OP",
        "small-signal operating point",
    )?;
    // Linearize at the operating point (no limiting, state = accepted operating point).
    ctx.mode = mode;
    ctx.final_step = true;
    ctx.limit = false;
    ws.restore();
    assemble(ckt, &ctx, &x, &mut ws, opts, 0.0);
    if let Some(d) = ws.slots.iter().find_map(|s| s.error.clone()) {
        return Err(d);
    }
    Ok((x, ws, ctx))
}

pub(crate) fn ac_rhs(ckt: &Circuit) -> Vec<c64> {
    let mut b = vec![c64::new(0.0, 0.0); ckt.dim()];
    for (gi, group) in ckt.groups.iter().enumerate() {
        let l = &ckt.layouts[gi];
        let mut s = AcSlots {
            re: vec![0.0; l.b.len()],
            im: vec![0.0; l.b.len()],
        };
        group.eval_ac(&mut s);
        for (k, row) in l.b.iter().enumerate() {
            if let Some(r) = *row {
                b[r] += c64::new(s.re[k], s.im[k]);
            }
        }
    }
    b
}

/// Coefficients of the nonlocal terms in each line's two wave equations.
pub(crate) struct DelayedStamp {
    delay: f64,
    entries: Vec<(usize, f64)>,
}
impl DelayedStamp {
    pub(crate) fn collect(c: &Circuit) -> Vec<Self> {
        c.groups
            .iter()
            .filter_map(|g| g.line())
            .map(|line| Self {
                delay: line.delay,
                entries: line
                    .delayed_entries()
                    .filter_map(|(r, col, v)| Some((c.pattern.find(r?, col?)?, v)))
                    .collect(),
            })
            .collect()
    }
    pub(crate) fn apply(&self, omega: f64, sign: f64, values: &mut [c64]) {
        let phase = omega * self.delay;
        // DC assembly already includes a propagation factor of one.
        let change = c64::new(-2. * (phase * 0.5).sin().powi(2), -phase.sin());
        for &(slot, coefficient) in &self.entries {
            values[slot] += change * (sign * coefficient);
        }
    }
}

pub(crate) fn ac_matrix(
    g: &[f64],
    c: &[f64],
    delayed: &[DelayedStamp],
    omega: f64,
    values: &mut [c64],
) {
    for ((z, &g), &c) in values.iter_mut().zip(g).zip(c) {
        *z = c64::new(g, omega * c);
    }
    for stamp in delayed {
        stamp.apply(omega, 1., values);
    }
}

/// `.ac`
pub(crate) fn ac(
    ckt: &Circuit,
    opts: &SimOptions,
    sweep: AcSweep,
    points: u32,
    fstart: f64,
    fstop: f64,
) -> Result<AnalysisResult, Diagnostic> {
    let _phase = spx_diag::Phase::start("analysis");
    let n = ckt.dim();
    let (_, ws, _) = linearize(ckt, opts, Mode::Ac)?;
    let b = ac_rhs(ckt);
    let mut lu = FaerLu::<c64>::new();
    let mut batch_lu = ckt
        .ordering
        .clone()
        .map(spx_sparse::SerialLu::<c64>::from_ordering)
        .transpose()
        .map_err(|e| Diagnostic::error("E-SOLVE-BACKEND", format!("AC: {e}")))?;
    if batch_lu.is_none() {
        lu.analyze(&ckt.pattern)
            .map_err(|e| Diagnostic::error("E-SOLVE-BACKEND", format!("AC: {e}")))?;
    }
    let freqs = ac_frequencies(sweep, points, fstart, fstop)?;
    let mut cols = vec![Vec::with_capacity(freqs.len()); n];
    let mut vals = vec![c64::new(0.0, 0.0); ckt.pattern.nnz()];
    let delayed = DelayedStamp::collect(ckt);
    for &f in &freqs {
        let w = 2.0 * std::f64::consts::PI * f;
        ac_matrix(&ws.asm.g, &ws.asm.c, &delayed, w, &mut vals);
        let mut sol = b.clone();
        if let Some(lu) = &mut batch_lu {
            lu.factor(&vals).and_then(|_| lu.solve(&mut sol))
        } else {
            lu.factor(&vals).and_then(|()| lu.solve(&mut sol))
        }
        .map_err(|e| Diagnostic::error("E-SOLVE-SINGULAR", format!("AC at {f:e} Hz: {e}")))?;
        for (c, v) in cols.iter_mut().zip(&sol) {
            c.push((v.re, v.im));
        }
    }
    Ok(AnalysisResult {
        kind: AnalysisKind::Ac,
        scale: Some(Variable {
            name: "frequency".into(),
            quantity: Quantity::Frequency,
        }),
        scale_values: freqs,
        variables: variables(ckt),
        data: Data::Complex { columns: cols },
    })
}

/// Number of accepted points kept for truncation-error estimates.
const HISTORY: usize = 3;

/// Accepted solutions used for divided differences (newest first).
#[derive(Debug, Default)]
struct History {
    t: Vec<f64>,
    q: Vec<Vec<f64>>,
}

impl History {
    fn clear(&mut self) {
        self.t.clear();
        self.q.clear();
    }
    fn push(&mut self, t: f64, buffer: &mut Vec<f64>) {
        let spare = if self.t.len() == HISTORY {
            self.t.pop();
            self.q.pop().unwrap_or_default()
        } else {
            Vec::new()
        };
        self.q.insert(0, std::mem::replace(buffer, spare));
        self.t.insert(0, t);
    }
    fn len(&self) -> usize {
        self.t.len()
    }
}

/// Coefficients of the accepted charge and current samples. Only the MNA
/// rows need their history term before Newton. Local charges use it at LTE.
#[derive(Clone, Copy)]
enum HistoryTerm {
    Euler(f64),
    Trap(f64),
    Gear(f64, f64),
}

impl HistoryTerm {
    fn step(method: Method, order: usize, h: f64, times: &[f64]) -> (f64, Self) {
        if order == 1 {
            return (1. / h, Self::Euler(h));
        }
        match method {
            Method::Trap => (2. / h, Self::Trap(-2. / h)),
            Method::Gear => {
                let w = h / (times[0] - times[1]);
                (
                    (1. + 2. * w) / ((1. + w) * h),
                    Self::Gear(-(1. + w) / h, w * w / ((1. + w) * h)),
                )
            }
        }
    }

    fn write(self, out: &mut [f64], start: usize, history: &History, old: &[f64]) {
        let range = start..start + out.len();
        let recent = &history.q[0][range.clone()];
        match self {
            Self::Euler(h) => {
                for (v, &q) in out.iter_mut().zip(recent) {
                    *v = -q / h;
                }
            }
            Self::Trap(c) => {
                for ((v, &q), &d) in out.iter_mut().zip(recent).zip(&old[range]) {
                    *v = c * q - d;
                }
            }
            Self::Gear(c0, c1) => {
                for ((v, &q0), &q1) in out.iter_mut().zip(recent).zip(&history.q[1][range]) {
                    *v = c0 * q0 + c1 * q1;
                }
            }
        }
    }
}

/// Charge history starts with the MNA rows. Device-local samples follow in
/// stamp order, including slots connected to ground or cancelling at a node.
fn charge_sample(ws: &Workspace, slots: &[(usize, std::ops::Range<usize>)], buffer: &mut Vec<f64>) {
    buffer.clear();
    buffer.extend_from_slice(&ws.asm.q);
    for (group, range) in slots {
        buffer.extend_from_slice(&ws.slots[*group].q[range.clone()]);
    }
}

/// Variable-step charge truncation estimate shared by all charge chunks.
struct Truncation {
    ts: [f64; 4],
    coeff: f64,
    h: f64,
    order: usize,
}
impl Truncation {
    fn new(method: Method, order: usize, h: f64, t_new: f64, hist: &History) -> Self {
        let ts = [
            t_new,
            hist.t[0],
            hist.t[1],
            hist.t.get(2).copied().unwrap_or(0.0),
        ];
        // Convert the derivative defect to a charge error by dividing by a0.
        // Differentiating the BDF2 interpolation remainder at the new point
        // gives h * (h + h_prev) * DD3; 1/a0 = h * span / (span + h).
        let coeff = match (order, method) {
            (1, _) => h * h,
            (_, Method::Trap) => 0.5 * h * h * h,
            (_, Method::Gear) => {
                let span = h + (ts[1] - ts[2]);
                h * h * span * (span / (span + h))
            }
        };
        Self {
            ts,
            coeff,
            h,
            order,
        }
    }

    fn ratio(
        &self,
        range: std::ops::Range<usize>,
        values: &[f64],
        dnew: &[f64],
        dold: &[f64],
        hist: &History,
        opts: &SimOptions,
    ) -> f64 {
        let Self {
            ts,
            coeff,
            h,
            order,
        } = *self;
        let recent = &hist.q[0][range.clone()];
        let previous = &hist.q[1][range.clone()];
        let older = hist.q.get(2).map_or(previous, |q| &q[range]);
        let mut worst = 0.0f64;
        for (((((&new, &q0), &q1), &q2), &dnew), &dold) in values
            .iter()
            .zip(recent)
            .zip(previous)
            .zip(older)
            .zip(dnew)
            .zip(dold)
        {
            let qs = [new, q0, q1, q2];
            if qs[0] == qs[1] && qs[1] == qs[2] && (order == 1 || qs[2] == qs[3]) {
                continue;
            }
            let slope = (qs[0] - qs[1]) / (ts[0] - ts[1]);
            let previous = (qs[1] - qs[2]) / (ts[1] - ts[2]);
            let curvature = (slope - previous) / (ts[0] - ts[2]);
            let dd = if order == 1 {
                curvature
            } else {
                let older = (qs[2] - qs[3]) / (ts[2] - ts[3]);
                let previous_curvature = (previous - older) / (ts[1] - ts[3]);
                (curvature - previous_curvature) / (ts[0] - ts[3])
            };
            let lte = (coeff * dd).abs();
            if lte == 0.0 {
                continue;
            }
            let qtol = opts.reltol * qs[0].abs().max(qs[1].abs()).max(opts.chgtol);
            let itol = (opts.abstol + opts.reltol * dnew.abs().max(dold.abs())) * h;
            let r = lte / (opts.trtol * qtol.max(itol));
            if !r.is_finite() {
                return f64::INFINITY;
            }
            worst = worst.max(r);
        }
        worst
    }
}

#[allow(clippy::too_many_arguments)]
fn charge_derivatives(
    term: HistoryTerm,
    a0: f64,
    hist: &History,
    charges: &[f64],
    old: &[f64],
    new: &mut [f64],
    estimate: Option<&Truncation>,
    opts: &SimOptions,
) -> f64 {
    const CHUNK: usize = 2048;
    let evaluate = |(chunk, dnew): (usize, &mut [f64])| {
        let start = chunk * CHUNK;
        let range = start..start + dnew.len();
        let q = &charges[range.clone()];
        term.write(dnew, start, hist, old);
        for (d, &v) in dnew.iter_mut().zip(q) {
            let history_term = *d;
            *d = a0 * v + history_term;
        }
        estimate.map_or(0., |lte| {
            lte.ratio(range.clone(), q, dnew, &old[range], hist, opts)
        })
    };
    if new.len() >= 16 * CHUNK && rayon::current_num_threads() > 1 {
        new.par_chunks_mut(CHUNK)
            .enumerate()
            .map(evaluate)
            .reduce(|| 0., f64::max)
    } else {
        new.chunks_mut(CHUNK)
            .enumerate()
            .map(evaluate)
            .fold(0., f64::max)
    }
}

/// Accepted outgoing waves at both ends. Keep the interval bracketing the
/// oldest time a new trial can request, including after a rejected step.
struct LineHistory<'a> {
    group: usize,
    line: &'a spx_devices::line::Line,
    samples: std::collections::VecDeque<(f64, [f64; 2])>,
}
impl LineHistory<'_> {
    fn accept(&mut self, t: f64, x: &[f64]) {
        self.samples.push_back((t, self.line.outgoing(x)));
        while self.samples.len() > 1 && self.samples[1].0 <= t - self.line.delay {
            self.samples.pop_front();
        }
    }
    fn delayed_outgoing(&self, t: f64) -> [f64; 2] {
        let retarded = t - self.line.delay;
        let i = self.samples.partition_point(|&(time, _)| time <= retarded);
        let (t0, left) = self.samples[i.saturating_sub(1)];
        let Some(&(t1, right)) = self.samples.get(i) else {
            return left;
        };
        if t1 == t0 {
            return left;
        }
        let fraction = (retarded - t0) / (t1 - t0);
        [0, 1].map(|k| left[k] + fraction * (right[k] - left[k]))
    }
}

fn schedule_arrival(bps: &mut Vec<f64>, time: f64, eps: f64, tstop: f64) {
    if time > tstop || time <= 0. {
        return;
    }
    let i = bps.partition_point(|&t| t < time - eps);
    if bps.get(i).is_none_or(|&t| t > time + eps) {
        bps.insert(i, time);
    }
}

/// `.tran`
pub(crate) fn tran(
    ckt: &Circuit,
    opts: &SimOptions,
    tstep: f64,
    tstop: f64,
    tstart: f64,
    tmax: Option<f64>,
    uic: bool,
) -> Result<(AnalysisResult, crate::TranStats), Diagnostic> {
    let _phase = spx_diag::Phase::start("analysis");
    let n = ckt.dim();
    let tp = TranParams { tstep, tstop };
    let mut ctx = make_ctx(opts, Mode::Tran, tp);
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; n];
    if uic {
        // Skip the operating point: `.ic` nodes start at their value, all others at zero.
        for &(i, v) in &ckt.ics {
            x[i] = v;
        }
        // Propagate the capacitor voltage differences for the reported t=0
        // sample. Independent capacitor charge histories also handle conflicting
        // constraints (e.g. parallel capacitors initially at different voltages).
        let mut voltages = vec![];
        for g in &ckt.groups {
            g.initial_values(&mut x, &mut voltages);
        }
        if !voltages.is_empty() {
            let ground = n;
            let mut edges = vec![vec![]; n + 1];
            for (p, q, v) in voltages {
                let (p, q) = (p.unwrap_or(ground), q.unwrap_or(ground));
                edges[q].push((p, v));
                edges[p].push((q, -v));
            }
            let mut seen = vec![false; n + 1];
            let mut queue = std::collections::VecDeque::from([(ground, 0.)]);
            seen[ground] = true;
            for &(i, v) in &ckt.ics {
                queue.push_back((i, v));
                seen[i] = true;
            }
            for root in 0..=n {
                while let Some((i, v)) = queue.pop_front() {
                    for &(j, d) in &edges[i] {
                        if !seen[j] {
                            seen[j] = true;
                            x[j] = v + d;
                            queue.push_back((j, x[j]));
                        }
                    }
                }
                if !seen[root] && !edges[root].is_empty() {
                    seen[root] = true;
                    queue.push_back((root, x[root]));
                }
            }
        }
        ws.clear_state();
    } else {
        // `.ic` holds nodes at their value while the operating point is solved.
        ws.clamp = ckt.ics.clone();
        let r = solve_op(
            ckt,
            &ctx,
            &mut x,
            &mut ws,
            opts,
            "E-CONV-OP",
            "transient initial operating point",
        );
        ws.clamp.clear();
        r?;
    }

    // Default maximum step as in SPICE: min(tstep, (tstop - tstart) / 50).
    let hmax = tmax
        .filter(|&t| t > 0.0)
        .unwrap_or_else(|| tstep.min((tstop - tstart) / 50.0));
    let mut lines: Vec<_> = ckt
        .groups
        .iter()
        .enumerate()
        .filter_map(|(group, g)| {
            Some(LineHistory {
                group,
                line: g.line()?,
                samples: Default::default(),
            })
        })
        .collect();
    let hmax = lines.iter().fold(hmax, |h, l| h.min(l.line.delay));
    let hmin = 1e-9 * hmax.min(tstep).min(tstop - tstart);
    let eps = hmin;

    let mut bps: Vec<f64> = Vec::new();
    for g in &ckt.groups {
        g.breakpoints(&tp, &mut bps);
    }
    for history in &mut lines {
        history.accept(0., &x);
        bps.push(history.line.delay);
    }
    bps.push(tstop);
    bps.retain(|&t| t > eps && t <= tstop);
    bps.sort_by(f64::total_cmp);
    bps.dedup_by(|a, b| (*a - *b).abs() <= eps);

    // Charges at the operating point.
    let mut eval_ctx = ctx;
    eval_ctx.initial_charge = uic;
    eval_ctx.limit = false;
    ws.restore();
    if uic {
        for &i in &ckt.discrete {
            ckt.groups[i].settle(&x, &mut ws.state[i]);
        }
    }
    crate::solve::assemble_charges(ckt, &eval_ctx, &x, &mut ws, opts)?;
    eval_ctx.initial_charge = false;
    if uic {
        ws.commit();
    }
    let mut hist = History::default();
    // The leading entries are the MNA equations. Track individual device charges
    // after them so that cancellation at a shared node cannot mask local error.
    let mut local_slots = Vec::new();
    for (group, layout) in ckt.layouts.iter().enumerate() {
        for run in layout.lte_q.chunk_by(|&a, &b| a.checked_add(1) == Some(b)) {
            if let (Some(first), Some(last)) = (run.first(), run.last()) {
                local_slots.push((group, *first..last + 1));
            }
        }
    }
    // An assembled C stamp also implies a potential dynamic state (e.g. a
    // direct Verilog-A ddt contribution with no separate history variable).
    let has_storage = ckt
        .layouts
        .iter()
        .any(|l| l.has_independent_charges || !l.c.is_empty());
    let mut buffer = Vec::new();
    charge_sample(&ws, &local_slots, &mut buffer);
    let mut qdot = vec![0.; buffer.len()];
    let mut qdot_new = qdot.clone();
    hist.push(0., &mut buffer);

    let mut times = Vec::new();
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); n];
    let record = |t: f64, x: &[f64], times: &mut Vec<f64>, cols: &mut Vec<Vec<f64>>| {
        if t >= tstart - eps {
            times.push(t);
            for (c, &v) in cols.iter_mut().zip(x) {
                c.push(v);
            }
        }
    };
    record(0.0, &x, &mut times, &mut cols);

    let mut accepted_steps = 0;
    let mut attempted_steps = 0;
    let mut t = 0.0;
    let mut bp_idx = 0;
    // The first step is taken without error control, so start small.
    let mut h = 0.01 * hmax.min(bps.first().copied().unwrap_or(tstop));
    let mut integ = Integration::dc(n);
    let mut x_new = x.clone();
    let predict = ckt.groups.iter().any(|g| g.type_name() == "bsim4");
    let mut previous_x = if predict { x.clone() } else { Vec::new() };
    let mut older_x = previous_x.clone();
    let profile = spx_diag::profile_enabled();
    let mut control = std::time::Duration::ZERO;
    while t < tstop - eps {
        let begin = profile.then(std::time::Instant::now);
        attempted_steps += 1;
        let next_bp = bps.get(bp_idx).copied().unwrap_or(tstop);
        h = h.min(hmax);
        // Corner snapping can stretch a minimum-size proposal by eps. Keep
        // its original size so an LTE failure there cannot retry it forever.
        let proposed_h = h;
        if t + h >= next_bp - eps {
            h = next_bp - t;
        } else if !has_storage && t + 2.0 * h > next_bp {
            // Balance the last two intervals on purely algebraic circuits.
            h = 0.5 * (next_bp - t);
        }
        // After a source corner one backward-Euler step gives a right-sided
        // current for every tracked charge. Trap only needs that one derivative.
        let restart_trap = has_storage && bp_idx > 0 && opts.method == Method::Trap;
        let order = if hist.len() >= 3 || (restart_trap && hist.len() >= 2) {
            2
        } else {
            1
        };
        let (a0, term) = HistoryTerm::step(opts.method, order, h, &hist.t);
        integ.a0 = a0;
        term.write(&mut integ.hist, 0, &hist, &qdot);
        ctx.time = t + h;
        if ctx.time <= t {
            return Err(Diagnostic::error(
                "E-CONV-TRAN",
                "timestep below floating-point time resolution",
            ));
        }
        for history in &lines {
            let [from_first, from_second] = history.delayed_outgoing(ctx.time);
            ws.slots[history.group]
                .b
                .copy_from_slice(&[from_second, from_first]);
        }
        ctx.final_step = ctx.time >= tstop - eps;
        x_new.copy_from_slice(&x);
        let extrapolated = predict && hist.len() >= 2;
        if extrapolated {
            // Newton divided differences on the accepted grid. Clearing the
            // charge history at a source corner also restarts this polynomial.
            let previous_step = hist.t[0] - hist.t[1];
            let ratio = h / previous_step;
            let curvature = hist.t.get(2).map(|&t2| {
                (
                    previous_step / (hist.t[1] - t2),
                    h * (h + previous_step) / (previous_step * (hist.t[0] - t2)),
                )
            });
            for (i, next) in x_new.iter_mut().enumerate() {
                if ckt.unknowns.kinds()[i] == spx_devices::UnknownKind::Voltage {
                    let diff = x[i] - previous_x[i];
                    *next += ratio * diff;
                    if let Some((ratio_old, weight)) = curvature {
                        *next += weight * (diff - ratio_old * (previous_x[i] - older_x[i]));
                    }
                    if !next.is_finite() {
                        *next = x[i];
                    }
                }
            }
        }
        if let Some(t) = begin {
            control += t.elapsed();
        }
        let solve = |seed: &mut [f64], ws: &mut Workspace| {
            newton(ckt, &ctx, &integ, seed, ws, opts, opts.itl4, 0.)
        };
        let mut result = solve(&mut x_new, &mut ws);
        if extrapolated
            && matches!(
                result,
                Err(crate::solve::NewtonError::NoConvergence { .. }
                    | crate::solve::NewtonError::Singular)
            )
        {
            x_new.copy_from_slice(&x);
            result = solve(&mut x_new, &mut ws);
        }
        if let Err(e) = result {
            h /= 8.0;
            if h < hmin {
                let mut d = newton_diagnostic(ckt, &e, "E-CONV-TRAN", "transient")
                    .with_note(format!("timestep too small at t = {:e} s", ctx.time))
                    .with_attempts(["newton", "timestep_reduction"]);
                if opts.itl4 < 50 {
                    d = d.with_suggestion(
                        format!("raise `.options itl4=50` (now {})", opts.itl4),
                        Confidence::Low,
                    );
                }
                return Err(d.with_suggestion(
                    "give pulse/pwl sources finite rise and fall times",
                    Confidence::Medium,
                ));
            }
            continue;
        }
        let crossing = ckt
            .discrete
            .iter()
            .filter_map(|&i| ckt.groups[i].crossing(&x, &x_new, &ws.accepted[i], &ws.state[i]))
            .reduce(f64::min);
        if let Some(fraction) = crossing
            && fraction < 0.999
            && h > 2. * hmin
        {
            h = (h * fraction.clamp(0.05, 0.999)).max(hmin);
            continue;
        }
        // Charges at the converged point (state is updated consistently for commit).
        eval_ctx.time = ctx.time;
        eval_ctx.final_step = ctx.final_step;
        crate::solve::assemble_charges(ckt, &eval_ctx, &x_new, &mut ws, opts)?;
        let begin = profile.then(std::time::Instant::now);
        charge_sample(&ws, &local_slots, &mut buffer);
        let error_order = order.min(hist.len() - 1);
        let estimate = (error_order > 0)
            .then(|| Truncation::new(opts.method, error_order, h, ctx.time, &hist));
        let r = charge_derivatives(
            term,
            a0,
            &hist,
            &buffer,
            &qdot,
            &mut qdot_new,
            estimate.as_ref(),
            opts,
        );
        // Truncation-error control (needs order + 1 history points).
        // A UIC voltage jump may place a displacement-current impulse in the first
        // backward-Euler sample. Resolve the right-hand limit before growing the step.
        let voltage_jump = uic
            && accepted_steps == 0
            && x_new
                .iter()
                .zip(&x)
                .zip(ckt.unknowns.kinds())
                .any(|((&new, &old), kind)| {
                    *kind == spx_devices::UnknownKind::Voltage
                        && (new - old).abs() > 0.1 + opts.vntol
                });
        let mut grow = if voltage_jump {
            // A UIC charge jump invalidates extrapolation from the initial
            // derivative. Where the device has internal RC storage, limit the
            // next step by its local linearized relaxation time.
            let relaxation = ckt
                .layouts
                .iter()
                .flat_map(|l| &l.relaxation_nodes)
                .filter(|&&i| (x_new[i] - x[i]).abs() > opts.vntol)
                .filter_map(|&i| {
                    let diag = ckt.node_diag[i];
                    let c = ws.asm.c.get(diag)?;
                    let g = ws.asm.g.get(diag)?;
                    (*g > 0. && *c > 0.).then(|| c / g)
                })
                .reduce(f64::min);
            relaxation.map_or(0.0625, |tau| tau.max(hmin).min(h) / h)
        } else if has_storage && accepted_steps == 0 {
            // No curvature estimate yet. Acquire a second sample at the initial step.
            1.0
        } else {
            2.0
        };
        if hist.len() >= 2 {
            // Use the conservative first-order estimate while the third divided
            // difference is unavailable at a trapezoidal restart.
            let factor = 0.9 * r.max(1e-12).powf(-1.0 / (error_order as f64 + 1.0));
            if r > 1.0 {
                let h_new = h * factor.clamp(0.125, 0.9);
                if h > hmin && proposed_h > hmin {
                    if let Some(t) = begin {
                        control += t.elapsed();
                    }
                    h = h_new.max(hmin);
                    continue;
                }
                return Err(Diagnostic::error(
                    "E-CONV-TRAN",
                    format!(
                        "charge truncation error exceeds tolerance at t = {:e} s (ratio {r:e}, step {h:e} s)",
                        ctx.time
                    ),
                ));
            }
            grow = factor.clamp(0.5, 2.0);
        }
        accepted_steps += 1;
        // Accept.
        t += h;
        let hit_bp = (t - next_bp).abs() <= eps;
        if hit_bp {
            t = next_bp;
            bp_idx += 1;
            hist.clear();
        }
        if crossing.is_some() {
            hist.clear();
            grow = grow.min(1.);
        }
        for history in &mut lines {
            history.accept(t, &x_new);
            if hit_bp || crossing.is_some() {
                schedule_arrival(&mut bps, t + history.line.delay, eps, tstop);
            }
        }
        hist.push(t, &mut buffer);
        ws.commit();
        std::mem::swap(&mut qdot, &mut qdot_new);
        if predict {
            std::mem::swap(&mut previous_x, &mut older_x);
            std::mem::swap(&mut previous_x, &mut x);
        }
        std::mem::swap(&mut x, &mut x_new);
        if let Some(t) = begin {
            control += t.elapsed();
        }
        record(t, &x, &mut times, &mut cols);
        h *= grow;
        if hit_bp {
            let gap = bps.get(bp_idx).copied().unwrap_or(tstop) - t;
            h = h.min(0.1 * hmax.min(gap));
        }
    }

    if profile {
        use std::io::Write;
        if accepted_steps > 0 {
            let _ = writeln!(
                std::io::stderr().lock(),
                "spx-newton-per-step {:.4}",
                ws.newton_iterations as f64 / accepted_steps as f64
            );
        }
        let _ = writeln!(
            std::io::stderr().lock(),
            "spx-phase control {:.6}",
            control.as_secs_f64() * 1000.
        );
    }
    let stats = crate::TranStats {
        accepted_steps,
        rejected_steps: attempted_steps - accepted_steps,
        iterations: ws.newton_iterations,
        assemblies: ws.assemblies,
    };
    Ok((
        AnalysisResult {
            kind: AnalysisKind::Tran,
            scale: Some(Variable {
                name: "time".into(),
                quantity: Quantity::Time,
            }),
            scale_values: times,
            variables: variables(ckt),
            data: Data::Real { columns: cols },
        },
        stats,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::compile;

    /// Gmin stepping and source stepping must reach the same operating point as plain Newton.
    #[test]
    fn continuation_methods_reach_the_operating_point() {
        let src = "t\nV1 a 0 3\nR1 a b 100\nD1 b c dm\nD2 c 0 dm\nQ1 a c 0 qm\n\
                   .model dm d is=1e-12 rs=5\n.model qm npn bf=50 rb=10\n.op\n";
        let nl = spx_netlist::parse(src).unwrap_or_else(|d| panic!("{d:?}"));
        let opts = SimOptions::default();
        let (ckt, _) = compile(&nl, &opts).unwrap_or_else(|d| panic!("{d:?}"));
        let ctx = make_ctx(&opts, Mode::Dc, NO_TRAN);
        let n = ckt.dim();
        let mut ws = Workspace::new(&ckt);
        let mut direct = vec![0.0; n];
        solve_op(&ckt, &ctx, &mut direct, &mut ws, &opts, "E-CONV-OP", "op")
            .unwrap_or_else(|d| panic!("{d}"));
        let mut init = ctx;
        init.init_junctions = true;
        let integ = Integration::dc(n);
        let mut x = vec![0.0; n];
        assert!(gmin_stepping(
            &ckt,
            &init,
            &integ,
            &mut x,
            &vec![0.0; n],
            &mut ws,
            &opts
        ));
        let close = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(p, q)| (p - q).abs() < 1e-5);
        assert!(close(&x, &direct), "{x:?} vs {direct:?}");
        let mut x = vec![0.0; n];
        assert!(source_stepping(&ckt, &init, &integ, &mut x, &mut ws, &opts));
        assert!(close(&x, &direct), "{x:?} vs {direct:?}");
    }

    #[test]
    fn bdf2_charge_error_on_unequal_steps() {
        let opts = SimOptions::default();
        let mut history = History::default();
        let charge = |t: f64| 1e-17 * t.powi(3);
        for t in [0., 1., 3.] {
            history.push(t, &mut vec![charge(t)]);
        }
        // In picoseconds the last step is 2 and the new step is 1.
        for t in &mut history.t {
            *t *= 1e-12;
        }
        let ratio = Truncation::new(Method::Gear, 2, 1e-12, 4e-12, &history).ratio(
            0..1,
            &[charge(4.)],
            &[0.],
            &[0.],
            &history,
            &opts,
        );
        let expected = (9. / 4.) * 1e-17 / (opts.trtol * opts.reltol * opts.chgtol);
        assert!((ratio / expected - 1.).abs() < 1e-12);
    }

    #[test]
    fn ac_frequency_grids() {
        let f = ac_frequencies(AcSweep::Dec, 10, 1.0, 1e3).unwrap();
        assert_eq!(f.len(), 31);
        assert!((f[30] - 1e3).abs() < 1e-9);
        let f = ac_frequencies(AcSweep::Oct, 1, 1.0, 8.0).unwrap();
        assert_eq!(f, vec![1.0, 2.0, 4.0, 8.0]);
        let f = ac_frequencies(AcSweep::Lin, 5, 0.0 + 1.0, 5.0).unwrap();
        assert_eq!(f, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }
}
