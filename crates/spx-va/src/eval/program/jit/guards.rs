//! Backward propagation of finiteness checks through the scalar Jacobian.
//! Provenance: original data-flow analysis using IEEE 754 arithmetic and SSA dominance.

use super::*;

/// A finite sum/product requires finite operands. In the stateless kernel this
/// holds for every emitted chain-rule term (including zero times infinity).
/// The final dense stamps are checked by the caller. A future check on both
/// successors is guaranteed to run, so it can discharge an earlier obligation.
pub(super) fn required(p: &Program, joins: &joins::Joins) -> Vec<(bool, bool)> {
    const PRIMAL: u8 = 1;
    const GRAD: u8 = 2;
    let mut checks = vec![(true, true); p.ops.len()];
    let mut targets = HashMap::new();
    for op in &p.ops {
        if let Op::IfFalse(_, j) | Op::Jump(j) = *op {
            targets.entry(j).or_insert((0usize, Vec::new())).0 += 1;
        }
    }
    let mut finite = vec![0; p.values.len()];
    if let Some((_, end)) = targets.get_mut(&p.ops.len()) {
        end.clone_from(&finite);
    }
    for (pc, op) in p.ops.iter().enumerate().rev() {
        match *op {
            Op::Jump(j) | Op::IfFalse(_, j) => {
                if let Some((uses, facts)) = targets.get_mut(&j) {
                    // The transformed join has exactly one explicit incoming
                    // edge. Apply its phi transfer before merging with fallthrough.
                    for copy in &joins.before_jump[pc] {
                        facts[copy.src] |= facts[copy.dst];
                    }
                    if matches!(op, Op::Jump(_)) {
                        finite.clone_from(facts)
                    } else {
                        for (v, other) in finite.iter_mut().zip(facts.iter()) {
                            *v &= other;
                        }
                    }
                    *uses -= 1;
                    if *uses == 0 {
                        targets.remove(&j);
                    }
                }
            }
            Op::Stamp(_, pair, _) => {
                finite[pair.f] |= PRIMAL | GRAD;
                finite[pair.q] |= PRIMAL | GRAD;
            }
            Op::CheckZeroProduct(r, _) => {
                checks[pc] = (finite[r] & PRIMAL == 0, finite[r] & GRAD == 0);
                finite[r] |= PRIMAL | GRAD;
            }
            Op::Math(d, m, a, b) => {
                if d < p.gradient_count && finite[d] & GRAD != 0 {
                    finite[a] |= GRAD;
                    finite[b] |= GRAD;
                }
                if finite[d] & PRIMAL != 0 {
                    match m {
                        Math::Add | Math::Sub | Math::Mul => {
                            finite[a] |= PRIMAL;
                            finite[b] |= PRIMAL;
                        }
                        Math::Div
                        | Math::Neg
                        | Math::Abs
                        | Math::Trunc
                        | Math::Floor
                        | Math::Ceil => finite[a] |= PRIMAL,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if let Some((_, target)) = targets.get_mut(&pc) {
            target.clone_from(&finite);
        }
        for copy in &joins.fallthrough[pc] {
            finite[copy.src] |= finite[copy.dst];
        }
    }
    checks
}

/// Backward slice per derivative column. Phi edges carry only the columns
/// requested at the join. A ddx reads exactly one column even when used as
/// a primal. A retained numeric variable carries no tangents between calls.
pub(super) fn tangents(p: &Program, n: usize, checks: &[(bool, bool)]) -> Vec<Vec<bool>> {
    let mut live = vec![vec![false; n + 1]; p.gradient_count];
    for (pc, op) in p.ops.iter().enumerate().rev() {
        match *op {
            Op::Stamp(_, v, _) => {
                for r in [v.f, v.q] {
                    if let Some(row) = live.get_mut(r) {
                        row.fill(true);
                    }
                }
            }
            Op::Ddx(_, a, i) if a < live.len() => live[a][i] = true,
            Op::CheckZeroProduct(a, _) if checks[pc].1 && a < live.len() => live[a].fill(true),
            Op::Math(d, _, a, b) | Op::Charge(d, _, a, b) | Op::Phi(d, _, a, b)
                if d < live.len() =>
            {
                for i in 0..=n {
                    if live[d][i] {
                        for src in [a, b] {
                            if src < live.len() {
                                live[src][i] = true;
                            }
                        }
                    }
                }
            }
            Op::Limited { dst, new, .. } if new < live.len() => {
                for i in 0..=n {
                    live[new][i] |= live[dst][i];
                }
            }
            _ => {}
        }
    }
    live
}
