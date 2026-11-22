//! Single shooting for periodically driven networks and free-running oscillators.
//! Provenance: Aprille and Trick (1972), the two driven and autonomous shooting papers;
//! Kundert, White and Sangiovanni-Vincentelli (1990); ngspice 44 manual §§1.2.8, 11.3.12.

use crate::analysis::{tran, transient_start, variables};
use crate::small_signal::{error, var};
use crate::solve::{Integration, Workspace, assemble, make_ctx, newton, newton_diagnostic};
use crate::{Circuit, SimOptions};
use sansho_devices::{EvalCtx, Mode, TranParams, UnknownKind};
use sansho_diag::Diagnostic;
use sansho_netlist::{ElementKind, Netlist, PssSpec};
use sansho_output::{AnalysisKind, AnalysisResult, Data, PrintTable, Quantity};
use sansho_sparse::{CscPattern, NewtonLu, PatternBuilder, SerialLu, SolverError};
use std::f64::consts::{FRAC_1_SQRT_2, TAU};

const GAMMA: f64 = 1. - FRAC_1_SQRT_2;
const MIN_STEPS: usize = 512;

fn convergence(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-CONV-PSS", message)
}

fn linear_failure(e: SolverError) -> Diagnostic {
    convergence(format!("PSS sensitivity solve failed: {e}"))
}

fn no_oscillation() -> Diagnostic {
    Diagnostic::error(
        "E-PSS-NOOSC",
        "cannot resolve a periodic oscillation at oscnob",
    )
    .with_note("Check the observation node or branch, startup excitation and stabilization time.")
}

struct Seed {
    x: Vec<f64>,
    time: f64,
    period: f64,
    phase: Option<f64>,
}

fn seed(
    ckt: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    p: &PssSpec,
    probe: usize,
    tp: TranParams,
) -> Result<Seed, Diagnostic> {
    let period = p.gfreq.recip();
    let stop = tp.tstop;
    if p.driven && p.tstab == 0. {
        let ctx = make_ctx(opts, Mode::Tran, tp);
        let (_, x) = transient_start(ckt, opts, &ctx, p.uic, nl)?;
        return Ok(Seed {
            x,
            time: 0.,
            period,
            phase: None,
        });
    }
    // Bound retained history independently of the stabilization duration.
    let start = if p.driven {
        (stop - tp.tstep).max(0.)
    } else {
        (stop - 64. * period).max(0.)
    };
    let (plot, _) = tran(
        ckt,
        opts,
        tp,
        start,
        Some(tp.tstep.min(stop / 64.)),
        p.uic,
        nl,
    )?;
    let times = &plot.scale_values;
    let Data::Real { columns } = plot.data else {
        return Err(error("missing PSS stabilization trace"));
    };
    let values = &columns[..ckt.dim()];
    if p.driven {
        let x = values
            .iter()
            .map(|v| {
                v.last()
                    .copied()
                    .ok_or_else(|| error("empty stabilization trace"))
            })
            .collect::<Result<_, _>>()?;
        return Ok(Seed {
            x,
            time: stop,
            period,
            phase: None,
        });
    }
    let wave = &values[probe];
    let tail = times.partition_point(|&t| t < start + (stop - start) * 0.5);
    let (low, high) = extrema(&wave[tail..]);
    let level = 0.5 * (low + high);
    let tol = abs_tol(ckt, opts, probe);
    if high - low <= 10. * tol {
        return Err(no_oscillation());
    }
    let mut crossings = wave
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < level && w[1] >= level)
        .map(|(i, w)| {
            let fraction = (level - w[0]) / (w[1] - w[0]);
            (i, fraction, times[i] + fraction * (times[i + 1] - times[i]))
        })
        .rev();
    let (i, fraction, time) = crossings.next().ok_or_else(no_oscillation)?;
    let (_, _, previous) = crossings.next().ok_or_else(no_oscillation)?;
    let observed = time - previous;
    let period = if (0.9..=1.1).contains(&(p.gfreq * observed)) {
        period
    } else {
        observed
    };
    let x = values
        .iter()
        .map(|v| v[i] + fraction * (v[i + 1] - v[i]))
        .collect();
    Ok(Seed {
        x,
        time: time + ((stop - time) / observed).ceil() * observed,
        period,
        phase: Some(level),
    })
}

fn abs_tol(ckt: &Circuit, opts: &SimOptions, i: usize) -> f64 {
    match ckt.unknowns.kinds()[i] {
        UnknownKind::Voltage => opts.vntol,
        UnknownKind::Current => opts.abstol,
    }
}

fn extrema(v: &[f64]) -> (f64, f64) {
    v.iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
            (lo.min(x), hi.max(x))
        })
}

// One column per initial MNA coordinate, plus d/d(log T) in autonomous mode.
type Columns = Vec<Vec<f64>>;

fn charge_product(pattern: &CscPattern, c: &[f64], dx: &[f64], dq: &mut [f64]) {
    dq.fill(0.);
    for (col, &v) in dx.iter().enumerate() {
        let range = pattern.col_ptr()[col]..pattern.col_ptr()[col + 1];
        for (&row, &cap) in pattern.row_idx()[range.clone()].iter().zip(&c[range]) {
            dq[row] += cap * v;
        }
    }
}

struct Stepper<'a> {
    ckt: &'a Circuit,
    opts: &'a SimOptions,
    ctx: EvalCtx,
    ws: Workspace,
    integ: Integration,
    lu: NewtonLu,
    jac: Vec<f64>,
    dx: Columns,
}

impl Stepper<'_> {
    fn sample(&mut self, x: &[f64]) -> Result<(), Diagnostic> {
        let ctx = EvalCtx {
            limit: false,
            ..self.ctx
        };
        assemble(self.ckt, &ctx, x, &mut self.ws, self.opts, 0.);
        if let Some(e) = self.ws.slots.iter().find_map(|s| s.error.as_ref()) {
            return Err(e.clone());
        }
        self.ws.commit();
        Ok(())
    }

    // q(x) + gamma*h*(f(x)-b(t)) = base. Differentiate the same stage
    // equation on the same grid. The extra period column contains h' = h.
    fn stage(
        &mut self,
        time: f64,
        x: &mut [f64],
        base: &[f64],
        dq: &mut Columns,
    ) -> Result<(), Diagnostic> {
        self.ctx.time = time;
        let a = self.integ.a0;
        for (hist, &q) in self.integ.hist.iter_mut().zip(base) {
            *hist = -a * q;
        }
        newton(
            self.ckt,
            &self.ctx,
            &self.integ,
            x,
            &mut self.ws,
            self.opts,
            self.opts.itl4,
            0.,
        )
        .map_err(|e| {
            newton_diagnostic(self.ckt, &e, "E-CONV-PSS", "PSS transient stage")
                .with_note(format!("time {time:e} s"))
        })?;
        self.sample(x)?;
        let asm = &self.ws.asm;
        for ((j, &g), &c) in self.jac.iter_mut().zip(&asm.g).zip(&asm.c) {
            *j = g + a * c;
        }
        self.lu.factor(&self.jac).map_err(linear_failure)?;
        for (col, (s, q)) in self.dx.iter_mut().zip(dq).enumerate() {
            for (i, (v, &b)) in s.iter_mut().zip(q.iter()).enumerate() {
                *v = a * b;
                if col == x.len() {
                    *v -= asm.f[i] - asm.b[i];
                }
            }
            self.lu.solve(s).map_err(linear_failure)?;
            charge_product(&self.ckt.pattern, &asm.c, s, q);
        }
        Ok(())
    }
}

struct Shot {
    end: Vec<f64>,
    derivative: Columns,
    traces: Columns,
}

// The stiffly accurate, L-stable, second-order two-stage SDIRK method. Both
// stages use the engine's transient Newton residual and device limiting.
fn shoot(
    ckt: &Circuit,
    opts: &SimOptions,
    seed: &Seed,
    steps: usize,
    tp: TranParams,
) -> Result<Shot, Diagnostic> {
    // Midpoint preserves quadratic invariants of a linear autonomous tank.
    let midpoint = ckt.is_linear() && seed.phase.is_some();
    let gamma = if midpoint { 0.5 } else { GAMMA };
    let h = seed.period / steps as f64;
    if !h.is_finite() || !(gamma * h).recip().is_finite() || seed.time + gamma * h <= seed.time {
        return Err(error("PSS timestep outside representable time range"));
    }
    let mut x = seed.x.clone();
    let n = x.len();
    let count = n + usize::from(seed.phase.is_some());
    let mut stepper = Stepper {
        ckt,
        opts,
        ctx: make_ctx(opts, Mode::Tran, tp),
        ws: Workspace::new(ckt),
        integ: Integration {
            a0: 1. / (gamma * h),
            hist: vec![0.; n],
        },
        lu: NewtonLu::new(&ckt.pattern).map_err(linear_failure)?,
        jac: vec![0.; ckt.pattern.nnz()],
        dx: vec![vec![0.; n]; count],
    };
    stepper.ctx.time = seed.time;
    stepper.sample(&x)?;
    for (g, state) in ckt.groups.iter().zip(&stepper.ws.accepted) {
        let mut delayed = false;
        g.transport_delays(state, &mut |tau, _, _, _| delayed |= tau > 0.);
        if delayed {
            return Err(error(
                "PSS cannot reconstruct device transport-delay history",
            ));
        }
    }
    let mut dq = stepper.dx.clone();
    for (i, col) in dq.iter_mut().enumerate().take(n) {
        stepper.dx[i][i] = 1.;
        let slots = ckt.pattern.col_ptr()[i]..ckt.pattern.col_ptr()[i + 1];
        for (&row, &cap) in ckt.pattern.row_idx()[slots.clone()]
            .iter()
            .zip(&stepper.ws.asm.c[slots])
        {
            col[row] = cap;
        }
    }
    let mut traces: Columns = (0..n).map(|_| Vec::with_capacity(steps)).collect();
    let mut base = vec![0.; n];
    let mut previous = dq.clone();

    let beta = (1. - GAMMA) / GAMMA;
    for k in 0..steps {
        for (v, &sample) in traces.iter_mut().zip(&x) {
            v.push(sample);
        }
        base.copy_from_slice(&stepper.ws.asm.q);
        if midpoint {
            previous.clone_from(&stepper.dx);
        } else {
            previous.clone_from(&dq);
        }
        stepper.stage(seed.time + (k as f64 + gamma) * h, &mut x, &base, &mut dq)?;
        if midpoint {
            // Reflect the starting state and tangent across their midpoint values.
            for (v, trace) in x.iter_mut().zip(&traces) {
                *v = 2. * *v - trace[k];
            }
            stepper.ctx.time = seed.time + (k + 1) as f64 * h;
            stepper.sample(&x)?;
            for ((col, old), q) in stepper.dx.iter_mut().zip(&previous).zip(&mut dq) {
                for (d, s) in col.iter_mut().zip(old) {
                    *d = 2. * *d - s;
                }
                charge_product(&ckt.pattern, &stepper.ws.asm.c, col, q);
            }
            continue;
        }
        // Eliminate the first-stage residual using its stage equation.
        for (q, &stage) in base.iter_mut().zip(&stepper.ws.asm.q) {
            *q += beta * (stage - *q);
        }
        for (col, old) in dq.iter_mut().zip(&previous) {
            for (stage, &start) in col.iter_mut().zip(old) {
                *stage = start + beta * (*stage - start);
            }
        }
        stepper.stage(seed.time + (k + 1) as f64 * h, &mut x, &base, &mut dq)?;
    }
    Ok(Shot {
        end: x,
        derivative: stepper.dx,
        traces,
    })
}

fn correct(
    seed: &mut Seed,
    shot: &Shot,
    probe: usize,
    amplitude: Option<usize>,
) -> Result<(), Diagnostic> {
    let n = seed.x.len();
    let dim = shot.derivative.len();
    let mut rhs: Vec<_> = seed
        .x
        .iter()
        .zip(&shot.end)
        .map(|(start, end)| start - end)
        .collect();
    if let Some(level) = seed.phase {
        rhs.push(level - seed.x[probe]);
    }
    // The amplitude of a linear free oscillation comes from its initial conditions.
    // Hold one independent coordinate. Acceptance still tests every closure equation.
    if let Some(row) = amplitude {
        rhs[row] = 0.;
    }
    let mut pattern = PatternBuilder::new(dim);
    let mut values = Vec::with_capacity(dim * dim);
    for (j, col) in shot.derivative.iter().enumerate() {
        for (i, &s) in col.iter().enumerate() {
            pattern.add(i, j);
            values.push(if Some(i) == amplitude {
                f64::from(i == j)
            } else {
                s - f64::from(i == j)
            });
        }
        if seed.phase.is_some() {
            pattern.add(n, j);
            values.push(f64::from(j == probe));
        }
    }
    let mut lu = SerialLu::new(&pattern.build().0).map_err(linear_failure)?;
    lu.factor(&values).map_err(linear_failure)?;
    lu.solve(&mut rhs).map_err(linear_failure)?;
    let damping = rhs.get(n).map_or(1., |dt| (0.25 / dt.abs()).min(1.));
    for (x, &d) in seed.x.iter_mut().zip(&rhs) {
        *x += damping * d;
    }
    if let Some(&d) = rhs.get(n) {
        seed.period *= (damping * d).exp();
    }
    Ok(())
}

fn plots(
    ckt: &Circuit,
    shot: Shot,
    points: usize,
    spec: &PssSpec,
    seed: &Seed,
    probe: usize,
) -> (Vec<AnalysisResult>, PrintTable) {
    let period = seed.period;
    let autonomous = !spec.driven;
    let steps = shot.traces[0].len();
    let samples: Columns = shot
        .traces
        .iter()
        .zip(&shot.end)
        .map(|(v, &end)| {
            let mut samples: Vec<_> = (0..points)
                .map(|k| {
                    let position = k as u64 * steps as u64;
                    let index = (position / points as u64) as usize;
                    let weight = (position % points as u64) as f64 / points as f64;
                    v[index] + weight * (v.get(index + 1).copied().unwrap_or(end) - v[index])
                })
                .collect();
            if autonomous {
                samples.push(end);
            }
            samples
        })
        .collect();

    // The oracle transforms the printed uniform grid, excluding its endpoint.
    // Driven peak phasors retain the original internal-mesh quadrature.
    let (signals, count) = if autonomous {
        (&samples, points)
    } else {
        (&shot.traces, steps)
    };
    let mut spectrum = vec![Vec::with_capacity(spec.harmonics); signals.len()];
    let first = usize::from(spec.driven);
    let harmonics = first..first + spec.harmonics;
    for harmonic in harmonics.clone() {
        let basis: Vec<_> = (0..count)
            .map(|i| (-TAU * harmonic as f64 * i as f64 / count as f64).sin_cos())
            .collect();
        let scale = if harmonic == 0 { 1. } else { 2. };
        for (signal, col) in signals.iter().zip(&mut spectrum) {
            let (im, re) = signal
                .iter()
                .zip(&basis)
                .fold((0., 0.), |(a, b), (&v, &(s, c))| (a + v * s, b + v * c));
            col.push((scale * re / count as f64, scale * im / count as f64));
        }
    }
    let frequencies: Vec<_> = harmonics.map(|i| i as f64 / period).collect();
    let name = &ckt.unknowns.names()[probe];
    let report = PrintTable {
        analysis: AnalysisKind::PssFreq,
        headings: vec![
            "frequency".into(),
            format!("magnitude({name})"),
            format!("phase({name}) [deg]"),
        ],
        columns: vec![
            frequencies.clone(),
            spectrum[probe].iter().map(|(r, i)| r.hypot(*i)).collect(),
            spectrum[probe]
                .iter()
                .map(|(r, i)| i.atan2(*r).to_degrees())
                .collect(),
        ],
    };
    let frequency_data = if autonomous {
        Data::Real {
            columns: spectrum
                .into_iter()
                .map(|col| {
                    col.into_iter()
                        .enumerate()
                        .map(|(h, (re, im))| if h == 0 { re } else { re.hypot(im) })
                        .collect()
                })
                .collect(),
        }
    } else {
        Data::Complex { columns: spectrum }
    };
    let start = if autonomous { seed.time } else { 0. };
    (
        vec![
            AnalysisResult {
                kind: AnalysisKind::Pss,
                events: Default::default(),
                scale: Some(var("time", Quantity::Time)),
                scale_values: (0..points + usize::from(autonomous))
                    .map(|i| start + period * (i as f64 / points as f64))
                    .collect(),
                variables: variables(ckt),
                data: Data::Real { columns: samples },
            },
            AnalysisResult {
                kind: AnalysisKind::PssFreq,
                events: Default::default(),
                scale: Some(var("frequency", Quantity::Frequency)),
                scale_values: frequencies,
                variables: variables(ckt),
                data: frequency_data,
            },
        ],
        report,
    )
}

pub(crate) struct Output {
    pub plots: Vec<AnalysisResult>,
    pub report: PrintTable,
    pub warning: Option<Diagnostic>,
}

pub(crate) fn analyze(
    ckt: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    p: &PssSpec,
) -> Result<Output, Diagnostic> {
    if !(p.gfreq.is_finite()
        && p.gfreq > 0.
        && p.tstab.is_finite()
        && p.tstab >= 0.
        && p.steadycoeff.is_finite()
        && p.steadycoeff > 0.)
        || p.points == 0
        || p.harmonics == 0
        || p.iterations == 0
    {
        return Err(error(
            "invalid PSS frequency, stabilization time, count or convergence coefficient",
        ));
    }
    let points = p
        .points
        .max(p.harmonics.saturating_mul(2).saturating_add(1));
    // For a second-order integrator the leading relative dispersion error is
    // proportional to (omega*h)^2. Resolve the highest exported oscillator
    // harmonic to the same angular-step budget as the driven fundamental.
    let highest = if p.driven {
        1
    } else {
        p.harmonics.saturating_sub(1).max(1)
    };
    let fundamental_steps = TAU / opts.reltol.sqrt();
    let steps = MIN_STEPS
        .max(p.harmonics.saturating_mul(8))
        .max(fundamental_steps.ceil() as usize);
    // An odd number of midpoint steps avoids a unit multiplier for algebraic
    // perturbations that reverse sign on each step.
    let linear_oscillator = ckt.is_linear() && !p.driven;
    let mesh_limit = 65536 - usize::from(linear_oscillator);
    let refined = (fundamental_steps * highest as f64).ceil() as usize;
    let steps = steps.max(refined.min(mesh_limit));
    let steps = if linear_oscillator { steps | 1 } else { steps };
    if ckt.dim() > 256
        || points > 65536
        || steps > 65536
        || p.harmonics
            .saturating_mul(if p.driven { steps } else { points })
            > 16_777_216
    {
        return Err(error(
            "PSS exceeds the dense shooting or harmonic grid limit",
        ));
    }
    if !p.gfreq.recip().is_finite() || !(3. / p.gfreq).is_finite() || p.tstab * p.gfreq > 1e6 {
        return Err(error("PSS stabilization interval outside supported range"));
    }
    if nl.elements.iter().any(|e| {
        matches!(
            e.kind,
            ElementKind::CodeModel { .. }
                | ElementKind::Va { .. }
                | ElementKind::Kspice { .. }
                | ElementKind::Ltra { .. }
                | ElementKind::Line { .. }
                | ElementKind::Switch { .. }
        )
    }) || ckt.groups.iter().any(|g| g.has_random_waveform())
    {
        return Err(error(
            "PSS requires deterministic lumped devices with continuous charge/flux states",
        ));
    }
    let node = format!("v({})", p.oscnob);
    let probe = ckt
        .unknowns
        .names()
        .iter()
        .position(|s| s == &p.oscnob || s == &node)
        .or_else(|| {
            ckt.unknowns
                .branch(p.oscnob.strip_suffix("#branch").unwrap_or(&p.oscnob))
        })
        .or_else(|| {
            ckt.aliases
                .iter()
                .find(|(s, _)| s == &node || s == &p.oscnob)
                .and_then(|a| a.1)
        })
        .ok_or_else(|| {
            error(format!(
                "unknown PSS observation node or branch {}",
                p.oscnob
            ))
        })?;
    let opts = SimOptions {
        bypass: false,
        ..opts.clone()
    };
    let tp = TranParams {
        tstep: 1. / p.gfreq / 64.,
        tstop: if p.driven {
            if p.tstab == 0. { 1. / p.gfreq } else { p.tstab }
        } else {
            p.tstab.max(3. / p.gfreq)
        },
    };
    if !(0.01 * tp.tstep.min(tp.tstop / 64.)).recip().is_finite() {
        return Err(error(
            "PSS stabilization timestep outside representable range",
        ));
    }
    let mut seed = seed(ckt, nl, &opts, p, probe, tp)?;
    for e in &nl.elements {
        if let ElementKind::VoltageSource { source, .. } | ElementKind::CurrentSource { source, .. } =
            &e.kind
            && let Some(wave) = &source.tran
            && !sansho_devices::waveform::periodic_after(
                wave,
                &tp,
                seed.time,
                p.driven.then_some(seed.period),
            )
        {
            return Err(error(format!(
                "{}: excitation must be constant for autonomous PSS or repeat at the specified driven period",
                e.name
            )));
        }
    }
    let mut residual = f64::INFINITY;
    let mut amplitude = None;
    let mut best: Option<(f64, usize, Seed, Shot)> = None;
    // When budget permits, resolve closure down to the second-order mesh
    // error so that premature Newton termination does not dominate dispersion.
    let target = if p.driven {
        p.steadycoeff
    } else {
        p.steadycoeff.min((TAU / steps as f64).powi(2))
    };
    for iteration in 1..=p.iterations {
        let shot = shoot(ckt, &opts, &seed, steps, tp)?;
        let mut sum = 0.;
        for (i, trace) in shot.traces.iter().enumerate() {
            let (lo, hi) = extrema(trace);
            let range = hi - lo;
            if i == probe && seed.phase.is_some() && range <= 10. * abs_tol(ckt, &opts, i) {
                return Err(no_oscillation());
            }
            let scale = range.max(abs_tol(ckt, &opts, i));
            sum += (shot.end[i] - seed.x[i]).abs() / scale;
        }
        residual = sum / ckt.dim() as f64;
        if !residual.is_finite() {
            return Err(convergence("nonfinite PSS closure residual"));
        }
        if residual <= target {
            let (plots, report) = plots(ckt, shot, points, p, &seed, probe);
            return Ok(Output {
                plots,
                report,
                warning: None,
            });
        }
        let mut next = Seed {
            x: seed.x.clone(),
            ..seed
        };

        if iteration < p.iterations {
            if linear_oscillator && amplitude.is_none() {
                amplitude = shot
                    .traces
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        let (lo, hi) = extrema(v);
                        (i != probe && hi - lo > 10. * abs_tol(ckt, &opts, i))
                            .then(|| (i, (seed.x[i] - 0.5 * (lo + hi)).abs() / (hi - lo)))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
            }
            correct(&mut next, &shot, probe, amplitude)?;
            if !p.driven {
                next.time += seed.period;
            }
        }
        if !p.driven && best.as_ref().is_none_or(|(score, ..)| residual < *score) {
            best = Some((residual, iteration, seed, shot));
        }
        seed = next;
    }
    if let Some((residual, iteration, mut best_seed, shot)) = best {
        // The 44.2 tight LC capture keeps shot 21's frequency and starts
        // the output at shot 25's clock. Autonomous orbits are time invariant.
        best_seed.time = seed.time;
        let warning = (residual > p.steadycoeff).then(|| Diagnostic::warning("W-CONV-PSS", format!(
            "Convergence not reached after {} shots. Returning shot {iteration} at {:.9e} Hz (mean normalized error {residual:e}, threshold {:e})",
            p.iterations, best_seed.period.recip(), p.steadycoeff
        )));
        let (plots, report) = plots(ckt, shot, points, p, &best_seed, probe);
        return Ok(Output {
            plots,
            report,
            warning,
        });
    }
    Err(convergence(format!(
        "PSS did not close after {} shots (mean normalized error {residual:e}, threshold {:e})",
        p.iterations, p.steadycoeff
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{RunConfig, check, run, run_with};
    use sansho_netlist::parse;

    #[test]
    fn transient_tangents_include_initial_state_and_period() {
        let nl =
            parse("nonlinear RC\nC1 x 0 1u\nB1 x 0 I={1e-3*(v(x)+v(x)*v(x)*v(x))}\n.op").unwrap();
        let checked = check(&nl).unwrap();
        let period = 1e-3;
        let tp = TranParams {
            tstep: period / 64.,
            tstop: period,
        };
        let shot = |x, t| {
            shoot(
                &checked.circuit,
                &checked.options,
                &Seed {
                    x: vec![x],
                    time: 0.,
                    period: t,
                    phase: Some(0.),
                },
                9,
                tp,
            )
            .unwrap()
        };
        let delta: f64 = 1e-5;
        let center = shot(0.8, period);
        let dx =
            (shot(0.8 + delta, period).end[0] - shot(0.8 - delta, period).end[0]) / (2. * delta);
        let dt = (shot(0.8, period * delta.exp()).end[0]
            - shot(0.8, period * (-delta).exp()).end[0])
            / (2. * delta);
        assert!((center.derivative[0][0] - dx).abs() < 1e-7);
        assert!((center.derivative[1][0] - dt).abs() < 1e-7);
    }

    #[test]
    fn resolved_plots_round_trip_and_are_bit_deterministic() {
        let nl=parse("RC\nV1 in 0 sin(0 1 10)\nR1 in out 1k\nC1 out 0 10u\n.save v(out)\n.op\n.pss 10 0 out 3 3 driven\n").unwrap();
        let run = |threads| run_with(&nl, &RunConfig { threads }).unwrap().results;
        let result = run(1);
        let table = &result.prints[0];
        assert_eq!(
            table.headings,
            ["frequency", "magnitude(v(out))", "phase(v(out)) [deg]"]
        );
        assert_eq!(table.columns[0], [10., 20., 30.]);
        let mut binary = vec![];
        sansho_output::write_raw_binary(&result, &mut binary).unwrap();
        let mut parallel = vec![];
        sansho_output::write_raw_binary(&run(2), &mut parallel).unwrap();
        assert_eq!(binary, parallel);
        let mut ascii = vec![];
        sansho_output::write_raw_ascii(&result, &mut ascii).unwrap();
        let read = sansho_output::read_raw_ascii(&String::from_utf8(ascii).unwrap()).unwrap();
        assert_eq!(read.analyses[1].kind, AnalysisKind::Pss);
        assert_eq!(read.analyses[1].real("v(out)").unwrap().len(), 7);
        assert_eq!(read.analyses[2].kind, AnalysisKind::PssFreq);
        assert_eq!(read.analyses[2].complex("v(out)").unwrap().len(), 3);
        assert_eq!(read.analyses[2].scale_values, [10., 20., 30.]);
        assert_eq!(read.analyses[2].variables.len(), 1);
        assert_eq!(
            sansho_netlist::write_analysis(&nl.analyses[1]),
            ".pss 1e1 0 out 3 3 50 1e-3 driven"
        );
    }

    #[test]
    fn lossless_tank_retains_initial_amplitude_while_correcting_frequency() {
        let nl = parse("lossless LC\nL1 x 0 1m\nC1 x 0 1u ic=3\n.pss 5.2k 1m x 128 3 10 1e-9 uic")
            .unwrap();
        let r = run(&nl).unwrap().results;
        let f = r.analyses[1].scale_values[1];
        let peak = r.analyses[1].real("v(x)").unwrap()[1];
        assert!((f * TAU * 1e-9f64.sqrt() - 1.).abs() < 2e-5);
        assert!((peak - 3.).abs() < 0.01);
    }

    #[test]
    fn unfinished_autonomous_shot_keeps_its_time_grid_and_signed_dc() {
        let nl = parse("tank\nVbias bias 0 -2\nL1 x 0 1m\nC1 x 0 1u ic=3\n.pss 5.3k 1m l1#branch 15 4 1 1e-14 uic").unwrap();
        let output = run(&nl).unwrap();
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.code == "W-CONV-PSS" && w.span.is_some())
        );
        let time = &output.results.analyses[0];
        let freq = &output.results.analyses[1];
        assert_eq!(time.len(), 16);
        assert!(time.scale_values[0] >= 1e-3);
        assert_eq!(freq.scale_values, [0., 5300., 10600., 15900.]);
        assert!((freq.real("v(bias)").unwrap()[0] + 2.).abs() < 1e-12);
        let branch = freq.real("i(l1)").unwrap();
        assert!(branch[1] > 0.01);
        assert_eq!(
            freq.variables.iter().filter(|v| v.name == "i(l1)").count(),
            2
        );
        let trace = time.real("i(l1)").unwrap();
        let expected = trace[..15].iter().sum::<f64>() / 15.;
        assert!((branch[0] - expected).abs() < 1e-12);
        let mut raw = Vec::new();
        sansho_output::write_raw_ascii(&output.results, &mut raw).unwrap();
        let raw = String::from_utf8(raw).unwrap();
        assert!(raw.contains("Plotname: Time Domain Periodic Steady State Analysis\nFlags: real"));
        assert!(
            raw.contains("Plotname: Frequency Domain Periodic Steady State Analysis\nFlags: real")
        );

        // 5.3 kHz was inside the ten-percent band, so the first shot kept
        // the guess. An out-of-band guess uses the measured startup period.
        let nl = parse("tank\nL1 x 0 1m\nC1 x 0 1u ic=3\n.pss 4k 1m x 15 4 1 1e-14 uic").unwrap();
        let output = run(&nl).unwrap();
        let f = output.results.analyses[1].scale_values[1];
        assert!((f * TAU * 1e-9_f64.sqrt() - 1.).abs() < 0.01);
    }

    #[test]
    fn equilibrium_and_iteration_exhaustion_report_different_failures() {
        for (source, card, expected) in [
            ("0", ".pss 10 0 out 32 4", "E-PSS-NOOSC"),
            ("sin(0 1 15)", ".pss 10 0 out 32 4 driven", "E-ANALYSIS"),
            (
                "sin(0 1 10)",
                ".pss 10 0 out 32 4 1 1e-8 driven",
                "E-CONV-PSS",
            ),
        ] {
            let nl = parse(&format!(
                "RC\nV1 in 0 {source}\nR1 in out 1k\nC1 out 0 10u\n{card}"
            ))
            .unwrap();
            let d = &run(&nl).unwrap_err().diagnostics[0];
            assert_eq!(d.code, expected);
            assert!(d.span.is_some());
        }
    }
}
