//! Predict invariant boolean branches in a pure circuit kernel for guarded specialization.
//! Provenance: original implementation from trace specialization and speculative constant propagation.

use super::*;

/// Training is independent of instance order. Each prediction is validated in
/// the generated code, including for inputs outside these sample distributions.
pub(super) fn predictions(p: &Program, s: &CircuitStamp) -> Vec<Option<bool>> {
    let mut observed = vec![0u8; p.values.len()];
    let mut predicate = vec![false; p.values.len()];
    for op in &p.ops {
        if let Op::Math(d, m, a, _) = *op {
            predicate[d] = matches!(
                m,
                Math::Lt
                    | Math::Le
                    | Math::Gt
                    | Math::Ge
                    | Math::Eq
                    | Math::Ne
                    | Math::Bool
                    | Math::Not
            ) || m == Math::Trunc && predicate[a]
        }
    }
    let mut values = p.values.clone();
    let mut bias = vec![0.; s.wires.len()];
    let mut seed = 1u64;
    let mut next_sample = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 11) as f64 / ((1u64 << 53) as f64)
    };
    for sample in 0..192 {
        for (i, v) in bias.iter_mut().enumerate() {
            let u = next_sample();
            *v = if let Some((known, range)) = &s.bias {
                let [lo, hi] = range.map(f64::from_bits);
                // Include overshoot and cluster some samples around either rail.
                let margin = (hi - lo) * 0.1;
                let (lo, hi) = (lo - margin, hi + margin);
                known[i].map(f64::from_bits).unwrap_or_else(|| {
                    let fraction = match sample % 4 {
                        0 => u,
                        1 => f64::from(u > 0.5),
                        2 => u.powi(4),
                        _ => 1. - u.powi(4),
                    };
                    lo + (hi - lo) * fraction
                })
            } else {
                let scale = [0., 0.01, 0.1, 1., 2., 5., 10., 100.][sample % 8];
                scale
                    * match sample / 8 % 4 {
                        0 => 2. * u - 1.,
                        1 => u,
                        2 => f64::from(i == sample / 8 % s.wires.len()),
                        _ => f64::from(u > 0.5),
                    }
            };
        }
        values.copy_from_slice(&p.values);
        let mut pc = 0;
        while let Some(op) = p.ops.get(pc) {
            pc += 1;
            match *op {
                Op::Math(d, m, a, b) => values[d] = m.apply(values[a], values[b]).0,
                Op::Unknown(d, i) => values[d] = s.wires[i].map_or(0., |j| bias[j]),
                Op::Phi(d, c, a, b) => {
                    observed[c] |= if values[c] == 0. { 1 } else { 2 };
                    values[d] = values[if values[c] != 0. { a } else { b }];
                }
                Op::IfFalse(c, j) => {
                    observed[c] |= if values[c] == 0. { 1 } else { 2 };
                    if values[c] == 0. {
                        pc = j
                    }
                }
                Op::Jump(j) => pc = j,
                Op::Simparam(d, ref key, fallback) => {
                    values[d] = match key.as_str() {
                        "gmin" => 1e-12,
                        "tnom" => BindOptions::default().tnom,
                        "scale" => 1.,
                        "shrink" => 0.,
                        "imax" | "imelt" if fallback.is_none() => 1.,
                        _ => fallback.map_or(0., |r| values[r]),
                    }
                }
                _ => {}
            }
        }
    }
    observed
        .into_iter()
        .enumerate()
        .map(|(i, seen)| (predicate[i] && (seen == 1 || seen == 2)).then_some(seen == 2))
        .collect()
}
