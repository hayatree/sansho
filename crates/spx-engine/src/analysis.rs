//! Analyses: DC operating point, DC sweep, AC sweep, transient.
//!
//! Provenance: original work. The transient algorithm (backward Euler at start and after
//! breakpoints, trapezoidal rule otherwise, breakpoint-aligned steps) follows the classic SPICE
//! approach described in Nagel (1975). See `docs/PROVENANCE.md`.

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
    if ckt.linear
        || matches!(
            first,
            crate::solve::NewtonError::Singular | crate::solve::NewtonError::Device(_)
        )
    {
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
    source: &str,
    start: f64,
    stop: f64,
    step: f64,
) -> Result<(AnalysisResult, crate::DcStats), Diagnostic> {
    let _phase = spx_diag::Phase::start("analysis");
    let Some(original) = ckt.param(source, "dc") else {
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
        if !ckt.set_param(source, "dc", v) {
            result = Err(Diagnostic::error(
                "E-DC-SOURCE",
                format!("cannot apply dc={v} to {source} in the current circuit layout"),
            ));
            break;
        }
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
    ckt.set_param(source, "dc", original);
    result?;
    let stats = crate::DcStats {
        source: source.to_owned(),
        points: count,
        iterations: ws.newton_iterations,
        extra_solves: ws.newton_calls.saturating_sub(count),
    };
    Ok((
        AnalysisResult {
            kind: AnalysisKind::Dc,
            scale: Some(Variable {
                name: if is_current { "i-sweep" } else { "v-sweep" }.into(),
                quantity: if is_current {
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

fn ac_frequencies(sweep: AcSweep, points: u32, fstart: f64, fstop: f64) -> Vec<f64> {
    let np = points.max(1) as f64;
    match sweep {
        AcSweep::Lin => {
            if points <= 1 {
                return vec![fstart];
            }
            let df = (fstop - fstart) / (np - 1.0);
            (0..points).map(|k| fstart + k as f64 * df).collect()
        }
        AcSweep::Dec | AcSweep::Oct => {
            let base: f64 = if sweep == AcSweep::Dec { 10.0 } else { 2.0 };
            let count = ((fstop / fstart).ln() / base.ln() * np + 1e-9).floor() as usize + 1;
            (0..count)
                .map(|k| fstart * base.powf(k as f64 / np))
                .collect()
        }
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
        "AC operating point",
    )?;
    // Linearize at the operating point (no limiting, state = accepted operating point).
    ctx.mode = Mode::Ac;
    ctx.final_step = true;
    ctx.limit = false;
    ws.restore();
    assemble(ckt, &ctx, &x, &mut ws, opts, 0.0);
    if let Some(d) = ws.slots.iter().find_map(|s| s.error.clone()) {
        return Err(d);
    }
    let mut b = vec![c64::new(0.0, 0.0); n];
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
    let mut lu = FaerLu::<c64>::new();
    lu.analyze(&ckt.pattern)
        .map_err(|e| Diagnostic::error("E-SOLVE-BACKEND", format!("AC: {e}")))?;
    let freqs = ac_frequencies(sweep, points, fstart, fstop);
    let mut cols = vec![Vec::with_capacity(freqs.len()); n];
    let mut vals = vec![c64::new(0.0, 0.0); ckt.pattern.nnz()];
    for &f in &freqs {
        let w = 2.0 * std::f64::consts::PI * f;
        for (k, v) in vals.iter_mut().enumerate() {
            *v = c64::new(ws.asm.g[k], w * ws.asm.c[k]);
        }
        let mut sol = b.clone();
        lu.factor(&vals)
            .and_then(|()| lu.solve(&mut sol))
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
    fn push(&mut self, t: f64, q: &[f64]) {
        if self.t.len() == HISTORY {
            self.t.pop();
            if let Some(mut old) = self.q.pop() {
                old.copy_from_slice(q);
                self.q.insert(0, old);
            }
        } else {
            self.q.insert(0, q.to_vec());
        }
        self.t.insert(0, t);
    }
    fn len(&self) -> usize {
        self.t.len()
    }
}

/// Integration coefficients for a step of size `h` from the newest history point.
fn integration(
    method: Method,
    order: usize,
    h: f64,
    hist: &History,
    qdot: &[f64],
    integ: &mut Integration,
) {
    let q0 = &hist.q[0];
    if order == 1 {
        integ.a0 = 1.0 / h;
        for (i, v) in integ.hist.iter_mut().enumerate() {
            *v = -q0[i] / h;
        }
        return;
    }
    match method {
        Method::Trap => {
            integ.a0 = 2.0 / h;
            for (i, v) in integ.hist.iter_mut().enumerate() {
                *v = -integ.a0 * q0[i] - qdot[i];
            }
        }
        Method::Gear => {
            // Variable-step BDF2 with step ratio w = h / h_prev.
            let q1 = &hist.q[1];
            let w = h / (hist.t[0] - hist.t[1]);
            integ.a0 = (1.0 + 2.0 * w) / ((1.0 + w) * h);
            let c0 = -(1.0 + w) / h;
            let c1 = w * w / ((1.0 + w) * h);
            for (i, v) in integ.hist.iter_mut().enumerate() {
                *v = c0 * q0[i] + c1 * q1[i];
            }
        }
    }
}

/// Charge history starts with the MNA rows. Device-local samples follow in
/// stamp order, including slots connected to ground or cancelling at a node.
fn charge_sample<'a>(
    ws: &'a Workspace,
    slots: &[(usize, usize)],
    buffer: &'a mut Vec<f64>,
) -> &'a [f64] {
    if slots.is_empty() {
        return &ws.asm.q;
    }
    buffer.clear();
    buffer.extend_from_slice(&ws.asm.q);
    buffer.extend(slots.iter().map(|&(group, slot)| ws.slots[group].q[slot]));
    buffer
}

/// Ratio of the estimated local truncation error to its tolerance (≤ 1 means acceptable),
/// computed from divided differences of the charges over the new point and the history.
#[allow(clippy::too_many_arguments)]
fn lte_ratio(
    method: Method,
    order: usize,
    h: f64,
    t_new: f64,
    q_new: &[f64],
    qdot_new: &[f64],
    qdot_old: &[f64],
    hist: &History,
    opts: &SimOptions,
) -> f64 {
    let ts = [
        t_new,
        hist.t[0],
        hist.t[1],
        hist.t.get(2).copied().unwrap_or(0.0),
    ];
    // LTE = C · h^(k+1) · (k+1)! · DD_(k+1).
    let coeff = match (order, method) {
        (1, _) => h * h,
        (_, Method::Trap) => 0.5 * h * h * h,
        (_, Method::Gear) => 4.0 / 3.0 * h * h * h,
    };
    let mut worst = 0.0f64;
    for i in 0..q_new.len() {
        let qs = [
            q_new[i],
            hist.q[0][i],
            hist.q[1][i],
            hist.q.get(2).map_or(0.0, |q| q[i]),
        ];
        if qs[0] == qs[1] && qs[1] == qs[2] && (order == 1 || qs[2] == qs[3]) {
            continue;
        }
        let d1 = [
            (qs[0] - qs[1]) / (ts[0] - ts[1]),
            (qs[1] - qs[2]) / (ts[1] - ts[2]),
            (qs[2] - qs[3]) / (ts[2] - ts[3]),
        ];
        let d2 = [
            (d1[0] - d1[1]) / (ts[0] - ts[2]),
            (d1[1] - d1[2]) / (ts[1] - ts[3]),
        ];
        let dd = if order == 1 {
            d2[0]
        } else {
            (d2[0] - d2[1]) / (ts[0] - ts[3])
        };
        let lte = (coeff * dd).abs();
        if lte == 0.0 {
            continue;
        }
        let qtol = (opts.reltol * qs[0].abs().max(qs[1].abs())).max(opts.chgtol);
        let itol = (opts.abstol + opts.reltol * qdot_new[i].abs().max(qdot_old[i].abs())) * h;
        let r = lte / (opts.trtol * qtol.max(itol));
        if r.is_finite() {
            worst = worst.max(r);
        }
    }
    worst
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
        .unwrap_or(f64::INFINITY)
        .min(tstep)
        .min((tstop - tstart) / 50.0);
    let hmin = 1e-9 * hmax;
    let eps = 1e-9 * hmax;

    let mut bps: Vec<f64> = Vec::new();
    for g in &ckt.groups {
        g.breakpoints(&tp, &mut bps);
    }
    bps.push(tstop);
    bps.retain(|&t| t > eps && t <= tstop);
    bps.sort_by(f64::total_cmp);
    bps.dedup_by(|a, b| (*a - *b).abs() <= eps);

    // Charges at the operating point.
    let mut eval_ctx = ctx;
    eval_ctx.limit = false;
    ws.restore();
    crate::solve::assemble_charges(ckt, &eval_ctx, &x, &mut ws, opts)?;
    if uic {
        ws.commit();
    }
    let mut hist = History::default();
    // The leading entries are the MNA equations. Track individual device charges
    // after them so that cancellation at a shared node cannot mask local error.
    let local_slots: Vec<_> = ckt
        .layouts
        .iter()
        .enumerate()
        .flat_map(|(group, layout)| layout.lte_q.iter().map(move |&slot| (group, slot)))
        .collect();
    let tracks_device_charges = !local_slots.is_empty();
    let mut buffer = Vec::new();
    let charges = charge_sample(&ws, &local_slots, &mut buffer);
    hist.push(0.0, charges);
    let mut qdot = vec![0.0; charges.len()];
    let mut qdot_new = vec![0.0; charges.len()];

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
    let mut integ = Integration::dc(charges.len());
    let mut x_new = x.clone();
    while t < tstop - eps {
        attempted_steps += 1;
        let next_bp = bps.get(bp_idx).copied().unwrap_or(tstop);
        h = h.min(hmax);
        if t + h >= next_bp - eps {
            h = next_bp - t;
        } else if !tracks_device_charges && t + 2.0 * h > next_bp {
            // Nodal-only control balances the final two intervals. With local
            // charge estimates, retain the proposed step until the corner clips it.
            h = 0.5 * (next_bp - t);
        }
        // After a source corner one backward-Euler step gives a right-sided
        // current for every tracked charge. Trap only needs that one derivative.
        let restart_trap = tracks_device_charges && bp_idx > 0 && opts.method == Method::Trap;
        let order = if hist.len() >= 3 || (restart_trap && hist.len() >= 2) {
            2
        } else {
            1
        };
        integration(opts.method, order, h, &hist, &qdot, &mut integ);
        ctx.time = t + h;
        ctx.final_step = ctx.time >= tstop - eps;
        x_new.copy_from_slice(&x);
        if let Err(e) = newton(ckt, &ctx, &integ, &mut x_new, &mut ws, opts, opts.itl4, 0.0) {
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
        // Charges at the converged point (state is updated consistently for commit).
        eval_ctx.time = ctx.time;
        eval_ctx.final_step = ctx.final_step;
        crate::solve::assemble_charges(ckt, &eval_ctx, &x_new, &mut ws, opts)?;
        let charges = charge_sample(&ws, &local_slots, &mut buffer);
        for i in 0..charges.len() {
            qdot_new[i] = integ.a0 * charges[i] + integ.hist[i];
        }
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
        } else if tracks_device_charges && accepted_steps == 0 {
            // No curvature estimate yet. Acquire a second sample at the initial step.
            1.0
        } else {
            2.0
        };
        if hist.len() >= 2 {
            // Use the conservative first-order estimate while the third divided
            // difference is unavailable at a trapezoidal restart.
            let error_order = order.min(hist.len() - 1);
            let r = lte_ratio(
                opts.method,
                error_order,
                h,
                ctx.time,
                charges,
                &qdot_new,
                &qdot,
                &hist,
                opts,
            );
            let factor = 0.9 * r.max(1e-12).powf(-1.0 / (error_order as f64 + 1.0));
            if r > 1.0 {
                let h_new = h * factor.clamp(0.125, 0.9);
                if h_new >= hmin {
                    h = h_new;
                    continue;
                }
                // Cannot reduce further; accept with a diagnostic-free best effort.
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
        hist.push(t, charges);
        ws.commit();
        std::mem::swap(&mut qdot, &mut qdot_new);
        std::mem::swap(&mut x, &mut x_new);
        record(t, &x, &mut times, &mut cols);
        h *= grow;
        if hit_bp {
            let gap = bps.get(bp_idx).copied().unwrap_or(tstop) - t;
            h = h.min(0.1 * hmax.min(gap));
        }
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
    fn ac_frequency_grids() {
        let f = ac_frequencies(AcSweep::Dec, 10, 1.0, 1e3);
        assert_eq!(f.len(), 31);
        assert!((f[30] - 1e3).abs() < 1e-9);
        let f = ac_frequencies(AcSweep::Oct, 1, 1.0, 8.0);
        assert_eq!(f, vec![1.0, 2.0, 4.0, 8.0]);
        let f = ac_frequencies(AcSweep::Lin, 5, 0.0 + 1.0, 5.0);
        assert_eq!(f, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }
}
