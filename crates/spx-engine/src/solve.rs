//! Deterministic assembly and the Newton–Raphson solver.
//!
//! The circuit equations are `F(x) = f(x) + a0·q(x) + h − b = 0` with Jacobian
//! `J = G + a0·C`, where `a0` and the history vector `h` come from the integration method
//! (`a0 = 0`, `h = 0` for DC). Convergence uses SPICE-style per-unknown tolerances.
//!
//! Provenance: original work. Newton iteration and convergence criteria follow Nagel (1975).
//! See `docs/PROVENANCE.md`.

use spx_devices::{EvalCtx, Mode, Slots, StateRef, TranParams, UnknownKind};
use spx_diag::Diagnostic;
use spx_sparse::{FaerLu, LinearSolver, SolverError};

use crate::circuit::{Circuit, DISCARD};
use crate::options::SimOptions;

/// Integration terms for one timepoint.
#[derive(Debug, Clone)]
pub(crate) struct Integration {
    /// Coefficient of `q` (0 for DC).
    pub a0: f64,
    /// History term (one entry per unknown).
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
    pub slots: Vec<Slots>,
    pub asm: Assembled,
    jac: Vec<f64>,
    rhs: Vec<f64>,
    lu: FaerLu<f64>,
    analyzed: bool,
    /// Per-group device state of the current iteration.
    pub state: Vec<Vec<f64>>,
    /// Per-group device state at the last accepted solution.
    pub accepted: Vec<Vec<f64>>,
    /// Whether any device limited its controlling voltages in the last assembly.
    pub noncon: bool,
}

impl Workspace {
    pub(crate) fn new(ckt: &Circuit) -> Self {
        let n = ckt.dim();
        let nnz = ckt.pattern.nnz();
        Self {
            slots: ckt.new_slots(),
            asm: Assembled {
                g: vec![0.0; nnz],
                c: vec![0.0; nnz],
                f: vec![0.0; n],
                q: vec![0.0; n],
                b: vec![0.0; n],
            },
            jac: vec![0.0; nnz],
            rhs: vec![0.0; n],
            lu: FaerLu::new(),
            analyzed: false,
            state: ckt.state_len.iter().map(|&n| vec![0.0; n]).collect(),
            accepted: ckt.state_len.iter().map(|&n| vec![0.0; n]).collect(),
            noncon: false,
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
        mode,
        time: 0.0,
        tran,
        source_factor: 1.0,
        gmin: opts.gmin,
        temp: opts.temp + 273.15,
        limit: true,
        init_junctions: false,
    }
}

/// Evaluate all groups at `x` and sum contributions in a fixed order (architecture D4).
///
/// `gshunt` is an extra conductance from every node to ground (gmin stepping).
pub(crate) fn assemble(
    ckt: &Circuit,
    ctx: &EvalCtx,
    x: &[f64],
    ws: &mut Workspace,
    opts: &SimOptions,
    gshunt: f64,
) {
    ws.noncon = false;
    let a = &mut ws.asm;
    a.g.iter_mut().for_each(|v| *v = 0.0);
    a.c.iter_mut().for_each(|v| *v = 0.0);
    a.f.iter_mut().for_each(|v| *v = 0.0);
    a.q.iter_mut().for_each(|v| *v = 0.0);
    a.b.iter_mut().for_each(|v| *v = 0.0);
    for (gi, group) in ckt.groups.iter().enumerate() {
        let s = &mut ws.slots[gi];
        s.noncon = false;
        group.eval(
            ctx,
            x,
            StateRef {
                accepted: &ws.accepted[gi],
                current: &mut ws.state[gi],
            },
            s,
        );
        ws.noncon |= s.noncon;
        let l = &ckt.layouts[gi];
        for (k, &idx) in ckt.gmap[gi].iter().enumerate() {
            if idx != DISCARD {
                a.g[idx] += s.g[k];
            }
        }
        for (k, &idx) in ckt.cmap[gi].iter().enumerate() {
            if idx != DISCARD {
                a.c[idx] += s.c[k];
            }
        }
        for (k, row) in l.f.iter().enumerate() {
            if let Some(r) = *row {
                a.f[r] += s.f[k];
            }
        }
        for (k, row) in l.q.iter().enumerate() {
            if let Some(r) = *row {
                a.q[r] += s.q[k];
            }
        }
        for (k, row) in l.b.iter().enumerate() {
            if let Some(r) = *row {
                a.b[r] += s.b[k];
            }
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
}

/// Newton failure modes.
#[derive(Debug)]
pub(crate) enum NewtonError {
    /// No convergence within the iteration limit.
    NoConvergence { worst: usize },
    /// Singular Jacobian.
    Singular,
    /// Other solver failure.
    Solver(SolverError),
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
    let n = ckt.dim();
    if !ws.analyzed {
        ws.lu.analyze(&ckt.pattern).map_err(NewtonError::Solver)?;
        ws.analyzed = true;
    }
    ws.restore();
    let kinds = ckt.unknowns.kinds();
    let mut ctx = *ctx;
    for iter in 1..=max_iter {
        if iter > 1 {
            ctx.init_junctions = false;
        }
        assemble(ckt, &ctx, x, ws, opts, gshunt);
        let a = &ws.asm;
        for k in 0..ws.jac.len() {
            ws.jac[k] = a.g[k] + integ.a0 * a.c[k];
        }
        for i in 0..n {
            ws.rhs[i] = -(a.f[i] + integ.a0 * a.q[i] + integ.hist[i] - a.b[i]);
        }
        match ws.lu.factor(&ws.jac) {
            Ok(()) => {}
            Err(SolverError::Singular { .. }) => return Err(NewtonError::Singular),
            Err(e) => return Err(NewtonError::Solver(e)),
        }
        match ws.lu.solve(&mut ws.rhs) {
            Ok(()) => {}
            Err(SolverError::Singular { .. }) => return Err(NewtonError::Singular),
            Err(e) => return Err(NewtonError::Solver(e)),
        }
        let mut converged = true;
        let mut worst = 0usize;
        let mut worst_ratio = 0.0f64;
        for i in 0..n {
            let old = x[i];
            let new = old + ws.rhs[i];
            let atol = match kinds[i] {
                UnknownKind::Voltage => opts.vntol,
                UnknownKind::Current => opts.abstol,
            };
            let tol = opts.reltol * old.abs().max(new.abs()) + atol;
            let ratio = ws.rhs[i].abs() / tol;
            if ratio > worst_ratio {
                worst_ratio = ratio;
                worst = i;
            }
            if ratio > 1.0 {
                converged = false;
            }
            x[i] = new;
        }
        if x.iter().any(|v| !v.is_finite()) {
            return Err(NewtonError::Singular);
        }
        // A linear circuit is solved exactly by one Newton step.
        if ckt.linear || (converged && iter > 1 && !ws.noncon && !ctx.init_junctions) {
            return Ok(iter);
        }
        if max_iter == iter {
            return Err(NewtonError::NoConvergence { worst });
        }
    }
    Err(NewtonError::NoConvergence { worst: 0 })
}

/// Convert a Newton failure into a structured diagnostic.
pub(crate) fn newton_diagnostic(ckt: &Circuit, err: &NewtonError, analysis: &str) -> Diagnostic {
    match err {
        NewtonError::Singular => {
            let mut d = Diagnostic::error(
                "E-SOLVE-SINGULAR",
                format!("{analysis}: the circuit matrix is singular"),
            );
            if !ckt.no_dc_path.is_empty() {
                d = d.with_note(format!(
                    "node(s) without a DC path to ground: {}",
                    ckt.no_dc_path.join(", ")
                ));
                d = d.with_note("add a resistor to ground or set `.options rshunt=1e12`");
            } else {
                d = d.with_note(
                    "check for loops of voltage sources/inductors or cut sets of current sources",
                );
            }
            d
        }
        NewtonError::NoConvergence { worst } => Diagnostic::error(
            "E-CONV-NEWTON",
            format!("{analysis}: Newton iteration did not converge"),
        )
        .with_note(format!(
            "largest update at {}",
            ckt.unknowns.names().get(*worst).map_or("?", String::as_str)
        )),
        NewtonError::Solver(e) => Diagnostic::error("E-SOLVE-BACKEND", format!("{analysis}: {e}")),
    }
}
