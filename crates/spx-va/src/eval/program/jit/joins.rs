//! Feed selected SSA values to control-flow joins on their incoming edges.
//! Provenance: original implementation from SSA construction and dominance.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Copy {
    pub dst: Reg,
    pub src: Reg,
    pub condition: Reg,
}

pub(super) struct Joins {
    pub before_jump: Vec<Vec<Copy>>,
    pub fallthrough: Vec<Vec<Copy>>,
    pub replaced: Vec<bool>,
}

impl Joins {
    pub fn new(p: &Program) -> Self {
        let n = p.ops.len();
        let mut result = Self {
            before_jump: vec![Vec::new(); n],
            fallthrough: vec![Vec::new(); n + 1],
            replaced: vec![false; n],
        };
        let mut incoming = vec![0; n + 1];
        for op in &p.ops {
            if let Op::Jump(j) | Op::IfFalse(_, j) = *op {
                incoming[j] += 1;
            }
        }
        for (pc, op) in p.ops.iter().enumerate() {
            let Op::IfFalse(c, other) = *op else { continue };
            let (jump, end, on_true) = match p.ops[other - 1] {
                Op::Jump(end) if other > pc + 1 => (other - 1, end, true),
                _ => (pc, other, false),
            };
            if incoming[end] != 1 || matches!(p.ops[end - 1], Op::Jump(_) | Op::IfFalse(..)) {
                continue;
            }
            // A later phi can use the result of an earlier phi at this join.
            // Only move operands that are already available on the two edges.
            let mut at_join = HashSet::new();
            for (k, op) in p.ops[end..].iter().enumerate() {
                let Op::Phi(dst, condition, a, b) = *op else {
                    break;
                };
                if k > 0 && incoming[end + k] != 0 {
                    break;
                }
                if condition == c && !at_join.contains(&a) && !at_join.contains(&b) {
                    let (src, other_src) = if on_true { (a, b) } else { (b, a) };
                    result.before_jump[jump].push(Copy {
                        dst,
                        src,
                        condition,
                    });
                    result.fallthrough[end].push(Copy {
                        dst,
                        src: other_src,
                        condition,
                    });
                    result.replaced[end + k] = true;
                }
                at_join.insert(dst);
            }
        }
        result
    }
}
