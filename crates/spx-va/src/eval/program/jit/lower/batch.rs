//! Masked SIMD evaluation of two pure circuit instances.
//! Provenance: original implementation from SSA, postdominators and SIMD if-conversion.

use super::*;

// The register program is acyclic. Trace both edges of a divergent conditional
// and select at its phi. Only active lanes may affect a guard or a stamp.
impl Lower<'_, '_> {
    pub(super) fn batch_body(&mut self, checks: &[(bool, bool)]) -> Result<(), String> {
        let p = self.program;
        let yes = self.fb.ins().iconst(I64, -1);
        let yes = self.fb.ins().splat(I64X2, yes);
        let mut edges = vec![None; p.ops.len() + 1];
        edges[0] = Some(yes);
        let valid_var = self.fb.declare_var(I64X2);
        self.fb.def_var(valid_var, yes);
        // Immediate postdominators in the forward-only register program.
        let end = p.ops.len();
        let mut postdom = vec![end; end + 1];
        let mut boundaries = vec![false; end + 1];
        boundaries[end] = true;
        for (pc, op) in p.ops.iter().enumerate().rev() {
            postdom[pc] = match *op {
                Op::Jump(j) => {
                    boundaries[pc] = true;
                    boundaries[j] = true;
                    j
                }
                Op::IfFalse(_, j) => {
                    boundaries[pc] = true;
                    boundaries[pc + 1] = true;
                    boundaries[j] = true;
                    let (mut a, mut b) = (pc + 1, j);
                    while a != b {
                        if a < b {
                            a = postdom[a]
                        } else {
                            b = postdom[b]
                        }
                    }
                    a
                }
                _ => pc + 1,
            };
        }
        let mut join_masks = vec![None; end + 1];
        let mut skip = None;
        for (pc, op) in p.ops.iter().enumerate() {
            let Some(mask) = join_masks[pc].or(edges[pc]) else {
                continue;
            };
            if skip.is_none()
                && mask != yes
                && boundaries[pc]
                && !matches!(op, Op::IfFalse(..) | Op::Jump(..))
            {
                let at = boundaries[pc + 1..]
                    .iter()
                    .position(|&x| x)
                    .map_or(end, |i| pc + 1 + i);
                if at < end && at - pc >= 3 {
                    let run = self.fb.create_block();
                    let join = self.fb.create_block();
                    let any = self.fb.ins().vany_true(mask);
                    self.fb.ins().brif(any, run, &[], join, &[]);
                    self.fb.switch_to_block(run);
                    self.fb.seal_block(run);
                    self.clear_block();
                    skip = Some((at, join));
                }
            }
            let mut target = pc + 1;
            let mut active = mask;
            match *op {
                Op::IfFalse(c, j) => {
                    if mask == yes {
                        join_masks[postdom[pc]] = Some(yes);
                    }
                    if let Some(value) = self.fixed[c] {
                        if value == 0. {
                            target = j;
                        }
                    } else {
                        let x = self.get(c);
                        let pred = self.truth(x);
                        let other = self.fb.ins().band_not(mask, pred);
                        Self::mask_edge(&mut self.fb, &mut edges[j], other);
                        active = self.fb.ins().band(mask, pred);
                    }
                }
                Op::Jump(j) => target = j,
                Op::Unknown(d, i) => self.unknown(d, i),
                Op::Math(d, m, a, b) | Op::Charge(d, m, a, b) => self.arithmetic(d, m, a, b),
                Op::Phi(d, c, a, b) => self.join(d, c, a, b),
                Op::Simparam(d, ref key, default) => {
                    let v = match key.as_str() {
                        "gmin" | "tnom" => {
                            let field = if key == "gmin" {
                                offset_of!(Frame, gmin)
                            } else {
                                offset_of!(Frame, tnom)
                            };
                            let scalar = self.field(F64, field);
                            self.fb.ins().splat(self.ty, scalar)
                        }
                        "scale" => self.num(1.),
                        "shrink" => self.num(0.),
                        "imelt" | "imax" if default.is_none() => self.num(1.),
                        _ => default.map(|r| self.get(r)).unwrap_or_else(|| self.num(0.)),
                    };
                    self.set(d, v);
                }
                Op::CheckZeroProduct(_, false) if checks[pc] == (false, false) => {}
                Op::CheckZeroProduct(r, div) => {
                    let v = self.get(r);
                    let (primal, grad) = checks[pc];
                    let mut ok = yes;
                    if div {
                        let sq = self.fb.ins().fmul(v, v);
                        ok = self.truth(sq);
                    }
                    if primal || grad {
                        let mut sum = if primal { v } else { self.num(0.) };
                        if grad {
                            for i in 0..self.n {
                                if let Some(g) = self.grad(r, i) {
                                    sum = self.fb.ins().fadd(sum, g);
                                }
                            }
                        }
                        if mask != yes {
                            let z = self.num(0.);
                            sum = self.select(mask, sum, z);
                        }
                        let prev = self.fb.use_var(self.guard_sum);
                        let sum = self.fb.ins().fadd(prev, sum);
                        self.fb.def_var(self.guard_sum, sum);
                    }
                    let bad = self.fb.ins().band_not(mask, ok);
                    let valid = self.fb.use_var(valid_var);
                    let valid = self.fb.ins().band_not(valid, bad);
                    self.fb.def_var(valid_var, valid);
                }
                Op::Stamp(row, pair, sign) => {
                    self.stamp_mask = (mask != yes).then_some(mask);
                    self.stamp(row, pair, sign);
                }
                Op::Nop => {}
                _ => return Err("instance SIMD requires a stateless bias kernel".into()),
            }
            // Merge outgoing masks at the join so the value also dominates
            // the successor when both lanes bypass this block.
            if let Some((at, join)) = skip
                && at == pc + 1
            {
                self.fb.ins().jump(join, &[]);
                self.fb.switch_to_block(join);
                self.fb.seal_block(join);
                self.clear_block();
                skip = None;
            }
            Self::mask_edge(&mut self.fb, &mut edges[target], active);
        }
        let valid = self.fb.use_var(valid_var);
        let sum = self.fb.use_var(self.guard_sum);
        let finite = self.finite(sum);
        let valid = self.fb.ins().band(valid, finite);
        let valid = self.fb.ins().vall_true(valid);
        let status = self.fb.ins().uextend(I32, valid);
        self.fb.ins().return_(&[status]);
        Ok(())
    }
    fn mask_edge(fb: &mut FunctionBuilder<'_>, dest: &mut Option<V>, incoming: V) {
        *dest = Some(dest.map_or(incoming, |prev| fb.ins().bor(prev, incoming)));
    }
}
