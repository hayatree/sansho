//! Analyses: DC operating point, DC sweep, AC sweep, transient.
//!
//! Provenance: original work. The transient algorithm (backward Euler at start and after
//! breakpoints, trapezoidal rule otherwise, breakpoint-aligned steps) follows the classic SPICE
//! approach described in Nagel (1975). See `docs/PROVENANCE.md`.

use spx_devices::{AcSlots, EvalCtx, Mode, TranParams, UnknownKind};
use spx_diag::Diagnostic;
use spx_netlist::AcSweep;
use spx_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};
use spx_sparse::{FaerLu, LinearSolver, c64};

use crate::circuit::Circuit;
use crate::options::SimOptions;
use crate::solve::{Integration, Workspace, assemble, newton, newton_diagnostic};

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
pub(crate) fn solve_op(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &mut [f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    analysis: &str,
) -> Result<(), Diagnostic> {
    let integ = Integration::dc(ckt.dim());
    newton(ckt, ctx, &integ, x, ws, opts, opts.itl1)
        .map(|_| ())
        .map_err(|e| newton_diagnostic(ckt, &e, analysis))
}

/// `.op`
pub(crate) fn op(ckt: &Circuit, opts: &SimOptions) -> Result<AnalysisResult, Diagnostic> {
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; ckt.dim()];
    solve_op(
        ckt,
        &EvalCtx::dc(),
        &mut x,
        &mut ws,
        opts,
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
) -> Result<AnalysisResult, Diagnostic> {
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
    for k in 0..count {
        let v = start + k as f64 * step;
        ckt.set_param(source, "dc", v);
        if let Err(d) = solve_op(ckt, &EvalCtx::dc(), &mut x, &mut ws, opts, "DC sweep") {
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
    Ok(AnalysisResult {
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
    })
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
    let n = ckt.dim();
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; n];
    let ctx = EvalCtx::dc();
    solve_op(ckt, &ctx, &mut x, &mut ws, opts, "AC operating point")?;
    // Linearize at the operating point.
    assemble(ckt, &ctx, &x, &mut ws, opts);
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

/// `.tran`
pub(crate) fn tran(
    ckt: &Circuit,
    opts: &SimOptions,
    tstep: f64,
    tstop: f64,
    tstart: f64,
    tmax: Option<f64>,
) -> Result<AnalysisResult, Diagnostic> {
    let n = ckt.dim();
    let tp = TranParams { tstep, tstop };
    let mut ctx = EvalCtx {
        mode: Mode::Tran,
        time: 0.0,
        tran: tp,
        source_factor: 1.0,
    };
    let mut ws = Workspace::new(ckt);
    let mut x = vec![0.0; n];
    solve_op(
        ckt,
        &ctx,
        &mut x,
        &mut ws,
        opts,
        "transient initial operating point",
    )?;

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

    assemble(ckt, &ctx, &x, &mut ws, opts);
    let mut q_prev = ws.asm.q.clone();
    let mut qdot_prev = vec![0.0; n];

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

    let mut t = 0.0;
    let mut bp_idx = 0;
    let mut after_bp = true;
    let mut integ = Integration::dc(n);
    let mut x_new = x.clone();
    while t < tstop - eps {
        let next_bp = bps.get(bp_idx).copied().unwrap_or(tstop);
        let mut h = if after_bp { 0.1 * hmax } else { hmax };
        if next_bp - t <= h * 1.1 {
            h = next_bp - t;
        }
        let mut accepted = false;
        while !accepted {
            let order1 = after_bp;
            integ.a0 = if order1 { 1.0 / h } else { 2.0 / h };
            for i in 0..n {
                integ.hist[i] = -integ.a0 * q_prev[i] - if order1 { 0.0 } else { qdot_prev[i] };
            }
            ctx.time = t + h;
            x_new.copy_from_slice(&x);
            match newton(ckt, &ctx, &integ, &mut x_new, &mut ws, opts, opts.itl4) {
                Ok(_) => accepted = true,
                Err(e) => {
                    h /= 8.0;
                    if h < hmin {
                        return Err(newton_diagnostic(ckt, &e, "transient")
                            .with_note(format!("timestep too small at t = {:e} s", ctx.time)));
                    }
                }
            }
        }
        // Update charge history.
        assemble(ckt, &ctx, &x_new, &mut ws, opts);
        for i in 0..n {
            let q = ws.asm.q[i];
            qdot_prev[i] = integ.a0 * q + integ.hist[i];
            q_prev[i] = q;
        }
        t += h;
        if (t - next_bp).abs() <= eps {
            t = next_bp;
            bp_idx += 1;
            after_bp = true;
        } else {
            after_bp = false;
        }
        x.copy_from_slice(&x_new);
        record(t, &x, &mut times, &mut cols);
    }

    Ok(AnalysisResult {
        kind: AnalysisKind::Tran,
        scale: Some(Variable {
            name: "time".into(),
            quantity: Quantity::Time,
        }),
        scale_values: times,
        variables: variables(ckt),
        data: Data::Real { columns: cols },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
