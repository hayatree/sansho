//! Split long straight-line kernels at a register-program instruction boundary.
//! Provenance: original implementation from SSA liveness and register spilling.

use super::{abi::Frame, lower::Lower, *};
use cranelift_codegen::ir::InstBuilder;
use std::mem::offset_of;

/// Below this size, the extra call and handoff buy little compile time.
const MIN_OPS: usize = 8192;

/// A cut is eligible only when both halves execute exactly once, with no
/// callbacks, retained stores, limiter side effects or nonconvergence flag.
pub(super) fn cut(p: &Program, regular: bool) -> Option<usize> {
    (regular
        && p.ops.len() >= MIN_OPS
        && p.ops.iter().all(|op| {
            matches!(
                op,
                Op::Unknown(..)
                    | Op::Math(..)
                    | Op::Charge(..)
                    | Op::Stamp(..)
                    | Op::CheckZeroProduct(..)
                    | Op::Nop
            )
        }))
    .then_some(p.ops.len() / 2)
}

struct LiveValue {
    register: Reg,
    column: Option<usize>,
    location: Location,
}

enum Location {
    Literal(f64),
    Slot(usize),
}

#[derive(Default)]
pub(super) struct Handoff {
    values: Vec<LiveValue>,
    pub slots: usize,
}

impl Handoff {
    pub fn save(&mut self, lower: &mut Lower<'_, '_>, end: usize) -> Result<(), String> {
        let mut live = vec![false; lower.program.values.len()];
        for op in &lower.program.ops[end..] {
            match *op {
                Op::Math(_, _, a, b) | Op::Charge(_, _, a, b) => {
                    live[a] = true;
                    live[b] = true;
                }
                Op::Stamp(_, v, _) => {
                    live[v.f] = true;
                    live[v.q] = true;
                }
                Op::CheckZeroProduct(r, _) => live[r] = true,
                _ => {}
            }
        }
        let buffer = offset_of!(Frame, bridge);
        let sum = lower.fb.use_var(lower.guard_sum);
        lower.store(buffer, 0, sum);
        let valid = lower.fb.use_var(lower.valid);
        let valid = lower.number_bool(valid);
        lower.store(buffer, 1, valid);
        self.slots = 2;
        for (r, used) in live.into_iter().enumerate() {
            if !used {
                continue;
            }
            let Some(var) = lower.primal[r] else { continue };
            let primal = lower.fb.use_var(var);
            // Absent partials remain structural zeroes. Rematerialize literals
            // so the second half retains signed zeros and the same slope shortcuts.
            for column in std::iter::once(None).chain((0..=lower.n).map(Some)) {
                let value = match column {
                    None => primal,
                    Some(c) => match lower.grad(r, c) {
                        Some(v) => v,
                        None => continue,
                    },
                };
                let location = if let Some(v) = lower.constant(value) {
                    Location::Literal(v)
                } else {
                    if self.slots >= i32::MAX as usize / 8 {
                        return Err("split frame exceeds i32 offsets".into());
                    }
                    let slot = self.slots;
                    lower.store(buffer, slot, value);
                    self.slots += 1;
                    Location::Slot(slot)
                };
                self.values.push(LiveValue {
                    register: r,
                    column,
                    location,
                });
            }
        }
        let ok = lower.fb.ins().iconst(cranelift_codegen::ir::types::I32, 1);
        lower.fb.ins().return_(&[ok]);
        Ok(())
    }

    pub fn restore(&self, lower: &mut Lower<'_, '_>) {
        let buffer = offset_of!(Frame, bridge);
        let sum = lower.load(buffer, 0);
        lower.fb.def_var(lower.guard_sum, sum);
        let valid = lower.load(buffer, 1);
        let valid = lower.truth(valid);
        lower.fb.def_var(lower.valid, valid);
        for value in &self.values {
            let v = match value.location {
                Location::Literal(x) => lower.num(x),
                Location::Slot(i) => lower.load(buffer, i),
            };
            if let Some(c) = value.column {
                lower.set_grad(value.register, c, v);
            } else {
                lower.set(value.register, v);
            }
        }
    }
}
