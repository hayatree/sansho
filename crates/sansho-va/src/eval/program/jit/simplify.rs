//! Local value numbering and dead instruction elimination after tangent expansion.
//! Provenance: original implementation from standard SSA compiler literature.

use cranelift_codegen::ir::{Function, Inst, Value};
use std::collections::{HashMap, HashSet};

pub(super) fn simplify(f: &mut Function, pure_calls: &HashMap<Inst, (usize, usize)>) {
    f.dfg.resolve_all_aliases();
    let blocks: Vec<_> = f.layout.blocks().collect();
    let mut replace = HashMap::<Value, Value>::new();
    for &b in &blocks {
        let mut ops = HashMap::new();
        let mut calls = HashMap::new();
        let mut cursor = f.layout.first_inst(b);
        while let Some(inst) = cursor {
            cursor = f.layout.next_inst(inst);
            f.dfg
                .map_inst_values(inst, |v| replace.get(&v).copied().unwrap_or(v));
            let data = f.dfg.insts[inst];
            let [out] = *f.dfg.inst_results(inst) else {
                continue;
            };
            let op = data.opcode();
            let duplicate = if let Some(&key) = pure_calls.get(&inst) {
                let args = f.dfg.inst_args(inst);
                calls.insert((key, args[1], args[2]), out)
            } else if !op.is_call()
                && !op.is_terminator()
                && !op.can_store()
                && !op.other_side_effects()
                && !op.can_trap()
                && !op.can_load()
            {
                ops.insert((data, f.dfg.value_type(out)), out)
            } else {
                None
            };
            if let Some(same) = duplicate {
                let same = replace.get(&same).copied().unwrap_or(same);
                replace.insert(out, same);
                f.layout.remove_inst(inst);
            }
        }
    }
    let mut live = HashSet::new();
    let mut unused = HashMap::<_, Vec<usize>>::new();
    for &b in blocks.iter().rev() {
        let mut cursor = f.layout.last_inst(b);
        while let Some(inst) = cursor {
            cursor = f.layout.prev_inst(inst);
            let op = f.dfg.insts[inst].opcode();
            // Successors have already been sliced: a dead phi must not keep
            // its incoming arithmetic alive on either predecessor.
            if op.is_branch() {
                let dfg = &mut f.dfg;
                for edge in dfg.insts[inst]
                    .branch_destination_mut(&mut dfg.jump_tables, &mut dfg.exception_tables)
                {
                    if let Some(indices) = unused.get(&edge.block(&dfg.value_lists)) {
                        for &idx in indices {
                            edge.remove(idx, &mut dfg.value_lists);
                        }
                    }
                }
            }
            if !op.is_terminator()
                && (!op.is_call() || pure_calls.contains_key(&inst))
                && !op.can_store()
                && (!op.other_side_effects() || pure_calls.contains_key(&inst))
                && (!op.can_trap() || op.can_load() || pure_calls.contains_key(&inst))
                && !f.dfg.inst_results(inst).iter().any(|v| live.contains(v))
            {
                f.layout.remove_inst(inst);
            } else {
                live.extend(f.dfg.inst_values(inst));
            }
        }
        if Some(b) != f.layout.entry_block() {
            let params = f.dfg.block_params(b).to_vec();
            let mut indices = Vec::new();
            for (idx, v) in params.into_iter().enumerate().rev() {
                if !live.contains(&v) {
                    f.dfg.remove_block_param(v);
                    indices.push(idx);
                }
            }
            if !indices.is_empty() {
                unused.insert(b, indices);
            }
        }
    }
}
