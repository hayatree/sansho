//! Second- and third-order small-signal distortion using nonlinear current generators.
//! Provenance: Wambacq & Sansen, Distortion Analysis of Analog Integrated Circuits
//! (1998), Volterra kernels and the nonlinear-current method.

use crate::{
    Circuit, SimOptions,
    analysis::{DelayedStamp, ac_frequencies, ac_matrix, linearize, variables},
    small_signal::{error, node, solver, var},
};
use sansho_devices::{Mode, distortion::Term};
use sansho_diag::Diagnostic;
use sansho_netlist::{ElementKind, FrequencySweep, Netlist};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity};
use sansho_sparse::{SerialLu, c64};
use std::f64::consts::TAU;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Mixing(i8, i8);
impl Mixing {
    fn omega(self, f1: f64, f2: f64) -> f64 {
        TAU * (f1 * f64::from(self.0) + f2 * f64::from(self.1))
    }
    fn plus(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
struct Component {
    mixing: Mixing,
    order: usize,
    x: Vec<c64>,
}

// Each product is an ordered convolution. The local coefficients already
// contain the factorial divisors for repeated differentiation axes.
fn generate(
    terms: &[Term],
    components: &[Component],
    target: Mixing,
    order: usize,
    f: f64,
    f2: f64,
    rhs: &mut [c64],
) {
    let w = target.omega(f, f2);
    for a in components {
        for b in components {
            let pair = a.mixing.plus(b.mixing);
            let degree = a.order + b.order;
            let mut accumulate = |last: &Component, third: bool| {
                let wd = last.mixing.omega(f, f2);
                for term in terms
                    .iter()
                    .filter(|t| t.order == if third { 3 } else { 2 })
                {
                    let [i, j, k] = term.axes;
                    let mut product = a.x[i] * b.x[j];
                    if third {
                        product = product * last.x[k];
                    }
                    rhs[term.row] -= product
                        * c64::new(
                            term.current,
                            term.charge * if term.differential { wd } else { w },
                        );
                }
            };
            if degree == order && pair == target {
                accumulate(b, false);
            }
            if degree < order {
                for c in components {
                    if degree + c.order == order && pair.plus(c.mixing) == target {
                        accumulate(c, true);
                    }
                }
            }
        }
    }
}

pub(super) fn analyze(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    grid: FrequencySweep,
    ratio: Option<f64>,
) -> Result<Vec<AnalysisResult>, Diagnostic> {
    // ngspice 44.2 places `np` interior points on a linear distortion grid:
    // `.disto lin np f1 f2` gives np + 2 evenly spaced frequencies including
    // both ends (R22g probes, np = 1..7). Decade and octave grids follow AC.
    let points = if grid.sweep == sansho_netlist::AcSweep::Lin {
        grid.points.saturating_add(2)
    } else {
        grid.points
    };
    let freq = ac_frequencies(grid.sweep, points, grid.fstart, grid.fstop)?;
    let (bias, ws, ctx) = linearize(c, opts, Mode::Ac)?;
    let mut terms = Vec::new();
    for g in &c.groups {
        g.distortion(&ctx, &bias, &mut terms)?;
    }
    let mut drives = [vec![c64::default(); c.dim()], vec![c64::default(); c.dim()]];
    for e in &nl.elements {
        let (source, plus, minus) = match &e.kind {
            ElementKind::VoltageSource { source, .. } => (
                source,
                Some(
                    c.unknowns
                        .branch(&e.name)
                        .ok_or_else(|| error(format!("missing branch {}", e.name)))?,
                ),
                None,
            ),
            ElementKind::CurrentSource { source, p, n } => (source, node(c, n)?, node(c, p)?),
            _ => continue,
        };
        for (dst, &tone) in drives.iter_mut().zip(&source.disto) {
            if let Some((mag, phase)) = tone {
                let (sin, cos) = phase.to_radians().sin_cos();
                let amplitude = c64::new(cos, sin) * (mag * 0.5);
                if let Some(i) = plus {
                    dst[i] += amplitude;
                }
                if let Some(i) = minus {
                    dst[i] -= amplitude;
                }
            }
        }
    }
    let two = ratio.is_some();
    let f2 = grid.fstart * ratio.unwrap_or(0.);
    let mut requests = vec![(Mixing(2, 0), 2, (!two).then_some(AnalysisKind::Harmonic2))];
    if two {
        requests.extend([
            (Mixing(1, 1), 2, Some(AnalysisKind::Sum)),
            (Mixing(1, -1), 2, Some(AnalysisKind::Difference)),
            (Mixing(2, -1), 3, Some(AnalysisKind::Intermod3)),
        ]);
    } else {
        requests.push((Mixing(3, 0), 3, Some(AnalysisKind::Harmonic3)));
    }
    let mut plots: Vec<_> = requests
        .iter()
        .filter_map(|(_, _, kind)| {
            Some(AnalysisResult {
                events: Default::default(),
                kind: (*kind)?,
                variables: variables(c),
                scale: Some(var("frequency", Quantity::Frequency)),
                scale_values: freq.clone(),
                data: Data::Complex {
                    columns: vec![Vec::with_capacity(freq.len()); c.dim()],
                },
            })
        })
        .collect();
    let mut lu = SerialLu::new(&c.pattern).map_err(solver)?;
    let mut matrix = vec![c64::default(); c.nnz()];
    let delayed = DelayedStamp::collect(c, &ws);
    for &f in &freq {
        let mut solve = |mix: Mixing, x: &mut [c64]| {
            ac_matrix(
                &ws.asm.g,
                &ws.asm.c,
                &delayed,
                mix.omega(f, f2),
                &mut matrix,
            );
            lu.factor(&matrix).and_then(|_| lu.solve(x)).map_err(solver)
        };
        let mut components = Vec::new();
        for (i, &mix) in [Mixing(1, 0), Mixing(0, 1)]
            .iter()
            .take(if two { 2 } else { 1 })
            .enumerate()
        {
            let mut x = drives[i].clone();
            solve(mix, &mut x)?;
            if i == 1 {
                components.push(Component {
                    mixing: Mixing(0, -1),
                    order: 1,
                    x: x.iter().map(|z| c64::new(z.re, -z.im)).collect(),
                });
            }
            components.push(Component {
                mixing: mix,
                order: 1,
                x,
            });
        }
        let mut outputs = plots.iter_mut();
        for &(mix, order, kind) in &requests {
            let mut x = vec![c64::default(); c.dim()];
            generate(&terms, &components, mix, order, f, f2, &mut x);
            solve(mix, &mut x)?;
            if x.iter().any(|z| !z.re.is_finite() || !z.im.is_finite()) {
                return Err(error(format!(
                    "nonfinite distortion product at fundamental {f} Hz"
                )));
            }
            if kind.is_some()
                && let Some(AnalysisResult {
                    data: Data::Complex { columns },
                    ..
                }) = outputs.next()
            {
                for (col, z) in columns.iter_mut().zip(&x) {
                    col.push((2. * z.re, 2. * z.im));
                }
            }
            components.push(Component {
                mixing: mix,
                order,
                x,
            });
        }
    }
    for plot in &mut plots {
        crate::add_aliases(c, plot);
    }
    Ok(plots)
}
