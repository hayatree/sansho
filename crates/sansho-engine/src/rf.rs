//! Multiport power waves and two-port noise parameters for real reference impedances.
//! Provenance: Kurokawa, IEEE Trans. Microwave Theory and Techniques 13 (1965),
//! 194–202; Pozar, Microwave Engineering, two-port noise figure.

use crate::{
    Circuit, SimOptions,
    analysis::{DelayedStamp, ac_frequencies, ac_matrix, linearize, variables},
    small_signal::{error, node, read, solver, var},
};
use sansho_devices::{Mode, Node, NoiseSource};
use sansho_diag::Diagnostic;
use sansho_netlist::{ElementKind, FrequencySweep, Netlist};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity};
use sansho_sparse::{PatternBuilder, SerialLu, c64};
use std::f64::consts::TAU;

struct Port {
    number: usize,
    source_node: String,
    pins: [Node; 2],
    branch: usize,
    root_z: f64,
}

// Solve X B = A row by row using the factorization of B transposed.
fn port_ratio(
    a: &[Vec<c64>],
    b: &[Vec<c64>],
    name: &str,
    f: f64,
) -> Result<Vec<Vec<c64>>, Diagnostic> {
    let n = b.len();
    let mut pattern = PatternBuilder::new(n);
    let mut values = Vec::with_capacity(n * n);
    for (col, row) in b.iter().enumerate() {
        for (r, &v) in row.iter().enumerate() {
            pattern.add(r, col);
            values.push(v);
        }
    }
    let mut lu = SerialLu::new(&pattern.build().0).map_err(solver)?;
    let mut result = a.to_vec();
    lu.factor(&values)
        .map_err(|e| error(format!("undefined {name} matrix at {f} Hz: {e}")))?;
    for row in &mut result {
        lu.solve(row).map_err(solver)?;
    }
    Ok(result)
}
fn conj(z: c64) -> c64 {
    c64::new(z.re, -z.im)
}

// The internally generated outgoing waves are measured with both ports
// matched. Accumulate their Hermitian covariance before optimizing over Gamma.
fn noise_parameters(
    adj: &[Vec<c64>],
    ports: &[Port],
    generators: &[NoiseSource],
    f: f64,
    s: &[Vec<c64>],
    admittance: &[Vec<c64>],
    temp: f64,
) -> Result<[c64; 8], Diagnostic> {
    let (mut c11, mut c22, mut c12) = (0., 0., c64::default());
    let mut cy = [c64::default(); 4];
    for generator in generators {
        let transfer = |rhs: &[c64]| -> c64 {
            generator
                .rows
                .iter()
                .map(|&(k, a, b)| rhs[k] * c64::new(a, b * TAU * f))
                .sum()
        };
        let (x, y) = (transfer(&adj[0]), transfer(&adj[1]));
        let power = generator.spectral_power(f);
        // Matched voltages are sqrt(Z0) times the outgoing noise waves.
        // Removing the loads gives the Norton currents (Y + diag(1/Z0)) V.
        let v = [x * ports[0].root_z, y * ports[1].root_z];
        let currents = [0, 1].map(|r| {
            admittance[r][0] * v[0] + admittance[r][1] * v[1] + v[r] / ports[r].root_z.powi(2)
        });
        for (r, row) in cy.chunks_mut(2).enumerate() {
            for (entry, i) in row.iter_mut().zip(currents) {
                *entry += currents[r] * conj(i) * power;
            }
        }
        let p = power / (sansho_devices::BOLTZMANN * temp);
        c11 += p * x.norm_sqr();
        c22 += p * y.norm_sqr();
        c12 += x * conj(y) * p;
    }
    let (s11, s21) = (s[0][0], s[1][0]);
    let gain = s21.norm_sqr();
    if gain == 0. {
        return Err(error(format!(
            "undefined two-port noise figure at {f} Hz: zero forward gain"
        )));
    }
    let a = c22 / gain;
    let b = (s21 * c12 - s11 * c22) / gain;
    let c = c11 + a * s11.norm_sqr() - 2. * (s21 * c12 * conj(s11)).re / gain;
    let sum = a + c;
    let root = ((sum - 2. * b.norm()).max(0.) * (sum + 2. * b.norm())).sqrt();
    let opt = if sum + root > 0. {
        conj(b) * (-2. / (sum + root))
    } else {
        c64::default()
    };
    let min = (a - c + root) * 0.5;
    let rn = ports[0].root_z.powi(2) * (c + min) * (1. + opt).norm_sqr() / 4.;
    Ok([
        cy[0],
        cy[1],
        cy[2],
        cy[3],
        (10. * a.ln_1p() / std::f64::consts::LN_10).into(),
        opt,
        (10. * min.ln_1p() / std::f64::consts::LN_10).into(),
        rn.into(),
    ])
}
pub(super) fn analyze(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    grid: FrequencySweep,
    noise: bool,
) -> Result<AnalysisResult, Diagnostic> {
    let mut ports = Vec::new();
    for e in &nl.elements {
        if let ElementKind::VoltageSource { source, p, n } = &e.kind
            && let Some((number, z)) = source.port
        {
            ports.push(Port {
                number,
                source_node: format!("v({}#res)", e.name),
                root_z: z.sqrt(),
                pins: [node(c, p)?, node(c, n)?],
                branch: c
                    .unknowns
                    .branch(&e.name)
                    .ok_or_else(|| error(format!("missing port branch {}", e.name)))?,
            });
        }
    }
    ports.sort_by_key(|p| p.number);
    let n = ports.len();
    if n == 0 || ports.iter().enumerate().any(|(i, p)| p.number != i + 1) {
        return Err(error("SP requires unique consecutive ports starting at 1"));
    }
    if noise && n != 2 {
        return Err(error("SP noise parameters require two ports"));
    }
    let freq = ac_frequencies(grid.sweep, grid.points, grid.fstart, grid.fstop)?;
    let (bias, ws, ctx) = linearize(c, opts, if noise { Mode::Noise } else { Mode::Ac })?;
    let mut generators = vec![];
    if noise {
        for (group, slots) in c.groups.iter().zip(&ws.slots) {
            group.noise(&ctx, &bias, slots, &mut generators)?;
        }
    }
    let mut vars = variables(c);
    vars.extend(
        c.aliases
            .iter()
            .map(|(name, _)| var(name, Quantity::Voltage)),
    );
    vars.extend(ports.iter().map(|p| var(&p.source_node, Quantity::Voltage)));
    let parameter_start = vars.len();
    for (name, q) in [
        ("S", Quantity::Scalar),
        ("Y", Quantity::Admittance),
        ("Z", Quantity::Resistance),
    ] {
        for r in 1..=n {
            for k in 1..=n {
                vars.push(var(format!("{name}_{r}_{k}"), q));
            }
        }
    }
    if noise {
        for r in 1..=2 {
            for k in 1..=2 {
                vars.push(var(format!("Cy_{r}_{k}"), Quantity::CurrentNoisePower));
            }
        }
        for (name, q) in [
            ("NF", Quantity::Decibels),
            ("SOpt", Quantity::Scalar),
            ("NFmin", Quantity::Decibels),
            ("Rn", Quantity::Resistance),
        ] {
            vars.push(var(name, q));
        }
    }
    let mut columns = vec![Vec::with_capacity(freq.len()); vars.len()];
    let mut lu = SerialLu::new(&c.pattern).map_err(solver)?;
    let delayed = DelayedStamp::collect(c, &ws);
    let mut matrix = vec![c64::default(); c.nnz()];
    for &f in &freq {
        ac_matrix(&ws.asm.g, &ws.asm.c, &delayed, TAU * f, &mut matrix);
        lu.factor(&matrix).map_err(solver)?;
        let mut s = vec![vec![c64::default(); n]; n];
        let mut volts = s.clone();
        let mut amps = s.clone();
        for (k, drive) in ports.iter().enumerate() {
            let mut x = vec![c64::default(); c.dim()];
            x[drive.branch] = 1.0.into();
            lu.solve(&mut x).map_err(solver)?;
            // The circuit traces correspond to a 1 V drive at the final port.
            if k + 1 == n {
                let aliases = c
                    .aliases
                    .iter()
                    .map(|(_, idx)| idx.map_or(c64::default(), |i| x[i]));
                let source_nodes = ports.iter().map(|p| {
                    p.pins[1].map_or(c64::default(), |i| x[i]) + c64::from(f64::from(p.number == n))
                });
                for (col, v) in columns
                    .iter_mut()
                    .zip(x.iter().copied().chain(aliases).chain(source_nodes))
                {
                    col.push((v.re, v.im));
                }
            }
            for (r, p) in ports.iter().enumerate() {
                volts[r][k] = read(&x, p.pins);
                amps[r][k] = -x[p.branch];
                s[r][k] = (volts[r][k] / p.root_z - amps[r][k] * p.root_z) * drive.root_z;
            }
        }
        let y = port_ratio(&amps, &volts, "Y", f)?;
        let z = port_ratio(&volts, &amps, "Z", f)?;
        let parameters = if noise {
            let mut adj = vec![vec![c64::default(); c.dim()]; 2];
            for (rhs, p) in adj.iter_mut().zip(&ports) {
                rhs[p.branch] = p.root_z.into();
                lu.solve_transpose(rhs).map_err(solver)?;
            }
            Some(noise_parameters(
                &adj,
                &ports,
                &generators,
                f,
                &s,
                &y,
                ctx.temp,
            )?)
        } else {
            None
        };
        for (col, v) in columns[parameter_start..].iter_mut().zip(
            [s, y, z]
                .into_iter()
                .flatten()
                .flatten()
                .chain(parameters.into_iter().flatten()),
        ) {
            if !v.re.is_finite() || !v.im.is_finite() {
                return Err(error(format!("nonfinite SP parameter at {f} Hz")));
            }
            col.push((v.re, v.im));
        }
    }
    Ok(AnalysisResult {
        kind: AnalysisKind::Sp,
        scale: Some(var("frequency", Quantity::Frequency)),
        scale_values: freq,
        variables: vars,
        data: Data::Complex { columns },
    })
}
