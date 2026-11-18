//! Dead-store elimination, guarded charge simplification and register packing.
//! Provenance: original implementation from SSA liveness and constant propagation.

use super::*;

impl Program {
    // Walk SSA use-def chains backwards, then pack live rows and retarget jumps.
    fn liveness(b: &Builder<'_>) -> (Vec<bool>, Vec<bool>) {
        let mut after = vec![0; b.ops.len() + 1];
        let mut live = vec![false; b.known.len()];
        let mut keep = vec![true; b.ops.len()];
        for (i, op) in b.ops.iter().enumerate().rev() {
            after[i] = after[i + 1];
            if matches!(*op,Op::Jump(j)|Op::IfFalse(_,j) if after[j]==after[i+1])
                || matches!(op, Op::Nop)
            {
                keep[i] = false;
                continue;
            }
            let dst = match op {
                Op::Charge(d, ..)
                | Op::Math(d, ..)
                | Op::Unknown(d, _)
                | Op::History(d, ..)
                | Op::Ddx(d, ..)
                | Op::Phi(d, ..)
                | Op::Event(d, ..)
                | Op::LimitEnabled(d)
                | Op::IntegralInitial(d)
                | Op::Previous(d, _)
                | Op::LimitValue { dst: d, .. }
                | Op::Noise(d)
                | Op::Time(d)
                | Op::Analysis(d, _) => Some(*d),
                _ => None,
            };
            if dst.is_some_and(|d| !live[d]) {
                keep[i] = false;
                continue;
            }
            after[i] += 1;
            match *op {
                Op::Charge(_, _, a, b) | Op::Math(_, _, a, b) => {
                    live[a] = true;
                    live[b] = true;
                }
                Op::LimitValue {
                    new,
                    old,
                    arg,
                    critical,
                    ..
                } => {
                    for i in [new, old, arg, critical] {
                        live[i] = true
                    }
                }
                Op::Limited {
                    dst, new, limited, ..
                } => {
                    for i in [dst, new, limited] {
                        live[i] = true
                    }
                }
                Op::Ddx(_, a, _) | Op::IfFalse(a, _) | Op::CheckZeroProduct(a, _) => live[a] = true,
                Op::Phi(_, c, a, b) => {
                    live[c] = true;
                    live[a] = true;
                    live[b] = true
                }
                Op::Stamp(_, v, _) | Op::Remember(_, v, _) => {
                    live[v.f] = true;
                    live[v.q] = true
                }
                Op::Simparam(d, _, a) => {
                    live[d] = true;
                    if let Some(a) = a {
                        live[a] = true
                    }
                }
                _ => {}
            }
        }
        (live, keep)
    }
    pub(super) fn finish(mut b: Builder<'_>, width: usize) -> Self {
        let (_, keep) = Self::liveness(&b);
        let mut checked = HashSet::new();
        let mut entry = vec![false; b.ops.len() + 1];
        for op in &b.ops {
            if let Op::Jump(j) | Op::IfFalse(_, j) = *op {
                entry[j] = true
            }
        }
        for ((instruction, keep), entry) in b.ops.iter_mut().zip(keep).zip(entry) {
            if entry {
                checked.clear()
            }

            if !keep {
                *instruction = Op::Nop;
                continue;
            }
            match *instruction {
                Op::Jump(_) | Op::IfFalse(_, _) => checked.clear(),
                Op::Charge(d, m, a, c) | Op::Math(d, m, a, c) => {
                    let (x, y) = (b.known[a], b.known[c]);
                    if let (Some(x), Some(y)) = (x, y) {
                        b.known[d] = Some(m.apply(x, y).0);
                        *instruction = Op::Nop;
                    } else if matches!(instruction, Op::Charge(..)) {
                        let guard = if x == Some(0.) {
                            Some(c)
                        } else if m == Math::Mul && y == Some(0.) {
                            Some(a)
                        } else {
                            None
                        };
                        if let Some(r) = guard {
                            b.known[d] = Some(0.);
                            *instruction = if checked.insert((r, m == Math::Div)) {
                                Op::CheckZeroProduct(r, m == Math::Div)
                            } else {
                                Op::Nop
                            };
                        } else {
                            *instruction = Op::Math(d, m, a, c)
                        }
                    }
                }
                // In a stateless native kernel the charge channel only reaches
                // additive stamps. Both zero signs have the same stamped bits.
                Op::Phi(d, _, a, c)
                    if b.charge_joins
                        .as_ref()
                        .is_some_and(|joins| joins.contains(&d))
                        && b.known[a] == Some(0.)
                        && b.known[c] == Some(0.) =>
                {
                    b.known[d] = Some(0.);
                    *instruction = Op::Nop;
                }
                Op::Phi(d, _, a, c)
                    if b.known[a].is_some()
                        && b.known[a].map(f64::to_bits) == b.known[c].map(f64::to_bits) =>
                {
                    b.known[d] = b.known[a];
                    *instruction = Op::Nop;
                }
                _ => {}
            }
        }
        let (live, keep) = Self::liveness(&b);
        // Stable partition leaves every gradient operand before its destination.
        // Registers after the partition only need a primal value.
        let mut gradient = vec![false; live.len()];
        for (op, &keep) in b.ops.iter().zip(&keep) {
            if !keep {
                continue;
            }
            match *op {
                Op::Unknown(d, _) | Op::Limited { dst: d, .. } => gradient[d] = true,
                Op::Math(d, m, a, c) => {
                    gradient[d] = m.differentiable() && (gradient[a] || gradient[c])
                }
                Op::Phi(d, _, a, c) => gradient[d] = gradient[a] || gradient[c],
                _ => {}
            }
        }
        let gradient_count = gradient.iter().filter(|x| **x).count();
        let mut values = Vec::new();
        let mut map = vec![0; live.len()];
        for has_gradient in [true, false] {
            for (i, &used) in live.iter().enumerate() {
                if used && gradient[i] == has_gradient {
                    map[i] = values.len();
                    values.push(b.known[i].unwrap_or(0.));
                }
            }
        }
        let mut pc = 0;
        let mut targets: Vec<_> = keep
            .iter()
            .map(|k| {
                let x = pc;
                pc += usize::from(*k);
                x
            })
            .collect();
        targets.push(pc);
        let pair = |v: Pair| Pair {
            f: map[v.f],
            q: map[v.q],
        };
        let ops = b
            .ops
            .into_iter()
            .zip(keep)
            .filter_map(|(op, k)| {
                k.then(|| match op {
                    Op::LimitValue {
                        dst,
                        junction,
                        new,
                        old,
                        arg,
                        critical,
                    } => Op::LimitValue {
                        dst: map[dst],
                        junction,
                        new: map[new],
                        old: map[old],
                        arg: map[arg],
                        critical: map[critical],
                    },
                    Op::Limited {
                        dst,
                        slot,
                        new,
                        limited,
                    } => Op::Limited {
                        dst: map[dst],
                        slot,
                        new: map[new],
                        limited: map[limited],
                    },
                    Op::LimitEnabled(d) => Op::LimitEnabled(map[d]),
                    Op::Event(d, i, l) => Op::Event(map[d], i, l),
                    Op::IntegralInitial(d) => Op::IntegralInitial(map[d]),
                    Op::Previous(d, i) => Op::Previous(map[d], i),
                    Op::Nop => Op::Nop,
                    Op::Math(d, m, a, b) | Op::Charge(d, m, a, b) => {
                        Op::Math(map[d], m, map[a], map[b])
                    }
                    Op::Unknown(d, i) => Op::Unknown(map[d], i),
                    Op::History(d, i, q) => Op::History(map[d], i, q),
                    Op::Ddx(d, a, i) => Op::Ddx(map[d], map[a], i),
                    Op::Phi(d, c, a, b) => Op::Phi(map[d], map[c], map[a], map[b]),
                    Op::CheckZeroProduct(a, div) => Op::CheckZeroProduct(map[a], div),
                    Op::Noise(d) => Op::Noise(map[d]),
                    Op::Time(d) => Op::Time(map[d]),
                    Op::Simparam(d, k, a) => Op::Simparam(map[d], k, a.map(|r| map[r])),
                    Op::Analysis(d, k) => Op::Analysis(map[d], k),
                    Op::IfFalse(c, i) => Op::IfFalse(map[c], targets[i]),
                    Op::Jump(i) => Op::Jump(targets[i]),
                    Op::Stamp(i, v, s) => Op::Stamp(i, pair(v), s),
                    Op::Remember(i, v, ty) => Op::Remember(i, pair(v), ty),
                })
            })
            .collect();
        Self {
            ops,
            gradient_count,
            rows: vec![0.; width * gradient_count],
            values,
        }
    }
}
