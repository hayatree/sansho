//! Pair frequently co-occurring tangent columns in native vector registers.
//! Provenance: original work from forward automatic differentiation and SLP vectorization.

use super::{
    lower::{Lower, Partial},
    *,
};
use cranelift_codegen::ir::{InstBuilder, Value, types::F64X2};

/// Match columns by how many gradient operations use both, after grounding.
/// Scalar lowering handles large layouts and programs with retained state.
pub(super) fn pairs(p: &Program, circuit: Option<&CircuitStamp>) -> Vec<(usize, usize)> {
    let Some(s) = circuit else { return Vec::new() };
    let n = s.wires.len();
    if n > 16 {
        return Vec::new();
    }
    let mut columns = vec![0u16; p.values.len()];
    let mut counts = vec![vec![0; n]; n];
    for op in &p.ops {
        let (dst, mask) = match *op {
            Op::Unknown(d, i) if s.wires[i].is_some() => (d, 1 << i),
            Op::Math(d, _, a, b) | Op::Phi(d, _, a, b) if d < p.gradient_count => {
                (d, columns[a] | columns[b])
            }
            _ => continue,
        };
        columns[dst] = mask;
        for (i, row) in counts.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                for (j, count) in row.iter_mut().enumerate().skip(i + 1) {
                    *count += usize::from(mask & (1 << j) != 0);
                }
            }
        }
    }
    let mut candidates: Vec<_> = counts
        .into_iter()
        .enumerate()
        .flat_map(|(i, row)| {
            row.into_iter()
                .enumerate()
                .filter(|&(_, count)| count > 0)
                .map(move |(j, count)| (count, i, j))
        })
        .collect();
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    let mut used = 0u16;
    candidates
        .into_iter()
        .filter_map(|(_, i, j)| {
            let mask = (1 << i) | (1 << j);
            if used & mask != 0 {
                return None;
            }
            used |= mask;
            Some((i, j))
        })
        .collect()
}

impl Lower<'_, '_> {
    /// Keep sparse or constant inputs scalar until both lanes do real work.
    pub(super) fn packed_derivative(
        &mut self,
        d: Reg,
        (i, j): (usize, usize),
        inputs: [(Reg, Value); 2],
    ) {
        let mut result = None;
        for (src, slope) in inputs {
            let Some(row) = self.derivatives.get(src) else {
                continue;
            };
            let v = match (row[i], row[j]) {
                (None, None) => continue,
                (Some(Partial::Lane(x, 0)), Some(Partial::Lane(y, 1))) if x == y => {
                    self.fb.use_var(x)
                }
                (Some(_), Some(_)) => {
                    let Some(x) = self.grad(src, i) else { return };
                    let Some(y) = self.grad(src, j) else { return };
                    let v = self.fb.ins().splat(F64X2, x);
                    self.fb.ins().insertlane(v, y, 1)
                }
                _ => return,
            };
            let v = match self.constant(slope) {
                Some(1.) => v,
                Some(-1.) => self.fb.ins().fneg(v),
                _ => {
                    let scale = self.fb.ins().splat(F64X2, slope);
                    self.fb.ins().fmul(v, scale)
                }
            };
            result = Some(match result {
                Some(left) => self.fb.ins().fadd(left, v),
                None => v,
            });
        }
        if let Some(v) = result {
            let var = self.fb.declare_var(F64X2);
            self.fb.def_var(var, v);
            self.derivatives[d][i] = Some(Partial::Lane(var, 0));
            self.derivatives[d][j] = Some(Partial::Lane(var, 1));
        }
    }
}
