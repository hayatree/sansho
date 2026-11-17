//! Flat instruction dispatch and scratch-backed Jacobian rows.
//! Provenance: original work from the chain rule and forward-mode automatic differentiation.

use super::*;

impl Program {
    pub(super) fn run(
        &mut self,
        b: &Bound,
        input: &EvalInput<'_>,
        slots: &mut [Slot],
        out: &mut EvalOutput,
        state: &mut StateIo<'_>,
        mut query: impl FnMut(&str) -> Option<f64>,
    ) -> bool {
        let mut valid = true;
        let n = input.x.len();
        let w = n + 1;
        let Self {
            ops,
            values,
            rows,
            gradient_count,
        } = self;
        let gradient_count = *gradient_count;
        let mut pc = 0;
        while let Some(op) = ops.get(pc) {
            pc += 1;
            match *op {
                Op::Unknown(d, i) => {
                    values[d] = input.x[i];
                    rows[d * w + i] = 1.;
                }
                Op::Nop => {}
                Op::Charge(d, m, a, b) | Op::Math(d, m, a, b) => {
                    if d >= gradient_count {
                        values[d] = m.apply(values[a], values[b]).0;
                        continue;
                    }
                    let (v, sa, sb) = m.apply(values[a], values[b]);
                    values[d] = v;
                    // SSA operands precede their destination. A zero incoming partial
                    // contributes exactly zero even for an infinite local slope.
                    let (src, dst) = rows.split_at_mut(d * w);
                    use crate::dual::scaled;
                    match (a < gradient_count, b < gradient_count) {
                        (true, true) => {
                            for ((v, x), y) in
                                dst[..w].iter_mut().zip(&src[a * w..]).zip(&src[b * w..])
                            {
                                *v = scaled(*x, sa) + scaled(*y, sb)
                            }
                        }
                        (true, false) => {
                            for (v, x) in dst[..w].iter_mut().zip(&src[a * w..]) {
                                *v = scaled(*x, sa)
                            }
                        }
                        (false, true) => {
                            for (v, y) in dst[..w].iter_mut().zip(&src[b * w..]) {
                                *v = scaled(*y, sb)
                            }
                        }
                        _ => {}
                    }
                }
                Op::Phi(d, c, a, b) => {
                    let s = if values[c] != 0. { a } else { b };
                    values[d] = values[s];
                    if s < gradient_count {
                        rows.copy_within(s * w..(s + 1) * w, d * w)
                    } else if d < gradient_count {
                        rows[d * w..(d + 1) * w].fill(0.);
                    }
                }
                Op::Ddx(d, a, i) => {
                    values[d] = if a < gradient_count {
                        rows[a * w + i]
                    } else {
                        0.
                    }
                }
                Op::IntegralInitial(d) => {
                    values[d] = f64::from(
                        input.analysis == Analysis::Dc
                            || (input.analysis == Analysis::Tran && input.abstime == 0.),
                    )
                }
                Op::Event(d, i, l) => {
                    values[d] = f64::from((i && input.initial_step) || (l && input.final_step))
                }
                Op::LimitEnabled(d) => values[d] = f64::from(input.limit),
                Op::Previous(d, i) => {
                    let v = state.current[i];
                    values[d] = if v.is_finite() { v } else { state.accepted[i] };
                }
                Op::LimitValue {
                    dst,
                    junction,
                    new,
                    old,
                    arg,
                    critical,
                } => {
                    values[dst] = if junction {
                        limit::pnjlim(values[new], values[old], values[arg], values[critical])
                    } else {
                        limit::fetlim(values[new], values[old], values[arg])
                    }
                }
                Op::Limited {
                    dst,
                    slot,
                    new,
                    limited,
                } => {
                    let v = values[limited];
                    let delta = values[new] - v;
                    state.current[slot] = v;
                    out.noncon |= delta.abs() > 1e-14 * (1. + values[new].abs());
                    values[dst] = v;
                    if new < gradient_count {
                        rows.copy_within(new * w..(new + 1) * w, dst * w)
                    } else {
                        rows[dst * w..(dst + 1) * w].fill(0.)
                    }
                    rows[dst * w + n] += delta;
                }
                Op::CheckZeroProduct(a, division) => {
                    let x = values[a];
                    valid &= x.is_finite()
                        && (!division || x * x != 0.)
                        && (a >= gradient_count
                            || rows[a * w..(a + 1) * w].iter().all(|x| x.is_finite()));
                }
                Op::Noise(d) => values[d] = 0.,
                Op::Time(d) => values[d] = input.abstime,
                Op::History(d, i, q) => {
                    values[d] = slots[i]
                        .values
                        .first()
                        .map_or(0., |v| if q { v.q.v } else { v.f.v });
                }
                Op::Simparam(d, ref key, default) => {
                    values[d] = query(key)
                        .or(match key.as_str() {
                            "gmin" => Some(input.gmin),
                            "tnom" => Some(b.sim.tnom),
                            "scale" => Some(1.),
                            "shrink" => Some(0.),
                            "imelt" | "imax" if default.is_none() => Some(1.),
                            _ => None,
                        })
                        .unwrap_or_else(|| default.map_or(0., |r| values[r]));
                }
                Op::Analysis(d, ref k) => {
                    values[d] = f64::from(match k.as_str() {
                        "dc" | "static" => input.analysis == Analysis::Dc,
                        "ac" => input.analysis == Analysis::Ac,
                        "tran" => input.analysis == Analysis::Tran,
                        "noise" => input.analysis == Analysis::Noise,
                        "ic" => input.initial_step,
                        _ => false,
                    })
                }
                Op::IfFalse(c, j) => {
                    if values[c] == 0. {
                        pc = j
                    }
                }
                Op::Jump(j) => pc = j,
                Op::Stamp(i, v, sign) => {
                    for (r, res, jac) in [
                        (v.f, &mut out.f, &mut out.df),
                        (v.q, &mut out.q, &mut out.dq),
                    ] {
                        res[i] += sign
                            * (values[r]
                                + if r < gradient_count {
                                    rows[r * w + n]
                                } else {
                                    0.
                                });
                        if r < gradient_count {
                            for (j, g) in jac[i * n..(i + 1) * n].iter_mut().zip(&rows[r * w..]) {
                                *j += sign * g
                            }
                        }
                    }
                }
                Op::Remember(..) if !valid => {}
                Op::Remember(i, v, ty) => {
                    let slot = &mut slots[i];
                    if slot.values.is_empty() {
                        slot.ty = ty;
                        slot.values.push(Value::num(0.))
                    }
                    slot.values[0].f.v = values[v.f];
                    slot.values[0].q.v = values[v.q];
                }
            }
        }
        valid
    }
}
