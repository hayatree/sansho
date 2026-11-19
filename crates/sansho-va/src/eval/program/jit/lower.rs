//! Lower the packed register program to a host function with scalar SSA tangents.
//! Provenance: original implementation from forward-mode AD and SSA construction.

mod batch;

use super::{
    abi::{self, Frame},
    *,
};
use cranelift_codegen::{
    ir::{self, AbiParam, InstBuilder, MemFlagsData, Value as V, condcodes::FloatCC, types::*},
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Module, default_libcall_names};
use std::mem::offset_of;
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) enum Partial {
    Scalar(Variable),
    Constant(f64),
    Lane(Variable, u8),
}

pub(super) struct Lower<'a, 'b> {
    pub fb: FunctionBuilder<'a>,
    pub ptr: ir::Type,
    ty: ir::Type,
    stamp_mask: Option<V>,
    pure_calls: HashMap<ir::Inst, (usize, usize)>,
    frame: V,
    pub(super) program: &'b Program,
    pub(super) n: usize,
    pub(super) primal: Vec<Option<Variable>>,
    // An absent variable denotes a structural +0, including the displacement column.
    pub(super) derivatives: Vec<Vec<Option<Partial>>>,
    pairs: Vec<(usize, usize)>,
    pub(super) valid: Variable,
    pub(super) guard_sum: Variable,
    vector_sum: Option<Variable>,
    literals: HashMap<u64, V>,
    fields: HashMap<usize, V>,
    constants: HashMap<Reg, V>,
    loaded_constants: HashMap<V, f64>,
    packed_constants: Option<&'b HashMap<u64, usize>>,
    noncon: Variable,
    query_index: i64,
    regular: bool,
    circuit: Option<&'b CircuitStamp>,
    // Known primals can still have nonzero partials (a tied voltage difference).
    fixed: Vec<Option<f64>>,
    unknowns: Vec<Option<usize>>,
    demand: Vec<Vec<bool>>,
}

pub(super) fn compile(
    p: &Program,
    n: usize,
    regular: bool,
    disk: Option<&crate::cache::Artifact>,
    circuit: Option<&CircuitStamp>,
    packed_constants: Option<&HashMap<u64, usize>>,
) -> Result<abi::Code, String> {
    let batch = packed_constants.is_some();
    let profile_blocks = std::env::var_os("SANSHO_PROFILE_VA_BLOCKS").is_some();
    let mut flags = settings::builder();
    let opt = std::env::var("SANSHO_CLIF_OPT").unwrap_or_else(|_| "none".into());
    let regalloc = std::env::var("SANSHO_CLIF_REGALLOC").unwrap_or_else(|_| "backtracking".into());
    for (name, value) in [
        // The register program is already optimized. The e-graph duplicates long
        // floating-point expressions on this workload and increases register spills.
        ("opt_level", opt.as_str()),
        ("regalloc_algorithm", regalloc.as_str()),
        (
            "machine_code_cfg_info",
            if profile_blocks { "true" } else { "false" },
        ),
        ("enable_probestack", "true"),
        ("probestack_strategy", "inline"),
        ("use_colocated_libcalls", "false"),
        (
            "is_pic",
            if cfg!(target_arch = "x86_64") {
                "true"
            } else {
                "false"
            },
        ),
    ] {
        flags.set(name, value).map_err(|e| e.to_string())?;
    }
    let isa = cranelift_native::builder()?
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())?;
    let artifact = disk.filter(|_| !profile_blocks).map(|entry| {
        let target = format!(
            "{} {} {}",
            isa.triple(),
            isa.flags(),
            isa.isa_flags()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
        entry.child(&[target.as_bytes(), &n.to_le_bytes(), &[batch as u8]])
    });
    let module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
    let mut pages = abi::Pages(Mutex::new(Some(module)));
    let module = pages
        .0
        .get_mut()
        .map_err(|e| e.to_string())?
        .as_mut()
        .ok_or("missing JIT module")?;
    let config = module.target_config();
    let ptr = config.pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(ptr));
    signature.returns.push(AbiParam::new(I32));
    let mut declare = || {
        module
            .declare_anonymous_function(&signature)
            .map_err(|e| e.to_string())
    };
    let id = declare()?;
    let cut = (!batch).then(|| split::cut(p, regular)).flatten();
    let next = cut.map(|_| declare()).transpose()?;
    let ids: Vec<_> = std::iter::once(id).chain(next).collect();
    if let Some(payload) = artifact.as_ref().and_then(|a| a.read())
        && let Some((slots, fragments)) = decode_image(&payload, ids.len())
    {
        for (&func, (align, bytes)) in ids.iter().zip(fragments) {
            module
                .define_function_bytes(func, align.into(), bytes, &[])
                .map_err(|e| e.to_string())?;
        }
        return abi::Code::finalize(pages, id, next, slots);
    }
    let mut handoff = split::Handoff::default();
    let mut image = artifact.as_ref().map(|_| vec![0; 4]);
    let mut block_maps = Vec::new();
    let mid = cut.unwrap_or(p.ops.len());
    for (func, range) in ids.into_iter().zip([0..mid, mid..p.ops.len()]) {
        let mut ctx = module.make_context();
        ctx.func.signature = signature.clone();
        emit(
            &mut ctx,
            config,
            p,
            n,
            regular,
            range,
            &mut handoff,
            circuit,
            packed_constants,
        )?;
        module
            .define_function(func, &mut ctx)
            .map_err(|e| format!("{e:?}"))?;
        if let Some(compiled) = ctx.compiled_code() {
            let bytes = compiled.code_buffer();
            if profile_blocks {
                let mut starts = compiled.bb_starts.clone();
                starts.extend([0, bytes.len() as u32]);
                starts.sort_unstable();
                starts.dedup();
                block_maps.push((func, starts));
            }
            if std::env::var_os("SANSHO_PROFILE").is_some() {
                use std::io::Write;
                let _ = writeln!(std::io::stderr().lock(), "sansho-code {}", bytes.len());
            }
            if !compiled.buffer.relocs().is_empty() || bytes.len() > u32::MAX as usize {
                image = None;
            } else if let Some(payload) = &mut image {
                payload.extend(compiled.buffer.min_alignment.to_le_bytes());
                payload.extend((bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytes);
            }
        } else {
            image = None;
        }
    }
    if let (Some(mut image), Some(entry)) = (image, artifact) {
        image[..4].copy_from_slice(&(handoff.slots as u32).to_le_bytes());
        entry.write(&image);
    }
    if profile_blocks {
        module.finalize_definitions().map_err(|e| e.to_string())?;
        use std::io::Write;
        let mut log = std::io::stderr().lock();
        for (func, starts) in block_maps {
            let base = module.get_finalized_function(func) as usize;
            let tag = if batch { "pair" } else { "scalar" };
            for (block, offsets) in starts.windows(2).enumerate() {
                let _ = writeln!(
                    log,
                    "sansho-va-block {:x} {:x} {tag}-{}-{}-{block}",
                    base + offsets[0] as usize,
                    offsets[1] - offsets[0],
                    p.ops.len(),
                    func.as_u32()
                );
            }
        }
    }
    abi::Code::finalize(pages, id, next, handoff.slots)
}

type Fragments<'a> = Vec<(u32, &'a [u8])>;

fn decode_image(mut bytes: &[u8], count: usize) -> Option<(usize, Fragments<'_>)> {
    fn word(input: &mut &[u8]) -> Option<u32> {
        let (word, rest) = input.split_at_checked(4)?;
        *input = rest;
        Some(u32::from_le_bytes(word.try_into().ok()?))
    }
    let slots = word(&mut bytes)? as usize;
    if slots > i32::MAX as usize / 8 || (count == 1 && slots != 0) || (count == 2 && slots < 2) {
        return None;
    }
    let mut fragments = Vec::new();
    for _ in 0..count {
        let align = word(&mut bytes)?;
        let len = word(&mut bytes)? as usize;
        if !align.is_power_of_two() || align > 65536 || len == 0 {
            return None;
        }
        let (code, rest) = bytes.split_at_checked(len)?;
        fragments.push((align, code));
        bytes = rest;
    }
    bytes.is_empty().then_some((slots, fragments))
}

#[allow(clippy::too_many_arguments)]
fn emit(
    ctx: &mut cranelift_codegen::Context,
    config: cranelift_codegen::isa::TargetFrontendConfig,
    p: &Program,
    n: usize,
    regular: bool,
    range: Range<usize>,
    handoff: &mut split::Handoff,
    circuit: Option<&CircuitStamp>,
    packed_constants: Option<&HashMap<u64, usize>>,
) -> Result<(), String> {
    let batch = packed_constants.is_some();
    let ptr = config.pointer_type();
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    fb.seal_block(entry);
    let frame = fb.block_params(entry)[0];
    let ty = if batch { F64X2 } else { F64 };
    let guard_sum = fb.declare_var(ty);
    let zero = fb.ins().f64const(0.);
    let initial_sum = if batch {
        fb.ins().splat(ty, zero)
    } else {
        zero
    };
    fb.def_var(guard_sum, initial_sum);
    let valid = fb.declare_var(I8);
    let noncon = fb.declare_var(I8);
    let yes = fb.ins().iconst(I8, 1);
    let no = fb.ins().iconst(I8, 0);
    fb.def_var(valid, yes);
    fb.def_var(noncon, no);
    let joins = joins::Joins::new(p);
    let checks = if regular {
        guards::required(p, &joins)
    } else {
        vec![(true, true); p.ops.len()]
    };
    let pairs = if batch {
        Vec::new()
    } else {
        super::packed::pairs(p, circuit)
    };
    let vector_sum = (!pairs.is_empty()).then(|| {
        let var = fb.declare_var(F64X2);
        let value = fb.ins().splat(F64X2, zero);
        fb.def_var(var, value);
        var
    });
    let mut lower = Lower {
        fb,
        ptr,
        frame,
        ty,
        stamp_mask: None,
        pure_calls: HashMap::new(),
        program: p,
        circuit,
        fixed: vec![None; p.values.len()],
        unknowns: vec![None; p.values.len()],
        demand: guards::tangents(p, n, &checks),
        n,
        primal: vec![None; p.values.len()],
        derivatives: vec![vec![None; n + 1]; p.gradient_count],
        pairs,
        vector_sum,
        valid,
        guard_sum,
        literals: HashMap::new(),
        fields: HashMap::new(),
        constants: HashMap::new(),
        loaded_constants: HashMap::new(),
        packed_constants,
        noncon,
        query_index: 0,
        regular,
    };
    if batch {
        lower.batch_body(&checks)?;
    } else {
        lower.body(range, handoff, &joins, &checks)?;
    }
    let calls = lower.pure_calls;
    lower.fb.finalize(config);
    super::simplify::simplify(&mut ctx.func, &calls);
    Ok(())
}

impl Lower<'_, '_> {
    pub fn num(&mut self, x: f64) -> V {
        *self.literals.entry(x.to_bits()).or_insert_with(|| {
            let v = self.fb.ins().f64const(x);
            if self.ty == F64 {
                v
            } else {
                self.fb.ins().splat(self.ty, v)
            }
        })
    }
    fn field(&mut self, ty: ir::Type, offset: usize) -> V {
        *self.fields.entry(offset).or_insert_with(|| {
            self.fb.ins().load(
                ty,
                MemFlagsData::trusted().with_readonly().with_can_move(),
                self.frame,
                offset as i32,
            )
        })
    }
    pub(super) fn load(&mut self, field: usize, index: usize) -> V {
        let ptr = self.field(self.ptr, field);
        let mut flags = MemFlagsData::trusted();
        if [
            offset_of!(Frame, constants),
            offset_of!(Frame, x),
            offset_of!(Frame, accepted),
        ]
        .contains(&field)
        {
            flags = flags.with_readonly().with_can_move();
        }
        if self.ty == F64 || field == offset_of!(Frame, constants) {
            let v = self.fb.ins().load(F64, flags, ptr, (8 * index) as i32);
            if self.ty == F64 {
                v
            } else {
                self.fb.ins().splat(self.ty, v)
            }
        } else {
            self.fb.ins().load(
                self.ty,
                MemFlagsData::new().with_notrap(),
                ptr,
                (16 * index) as i32,
            )
        }
    }
    pub(super) fn store(&mut self, field: usize, index: usize, value: V) {
        let ptr = self.field(self.ptr, field);
        self.fb.ins().store(
            MemFlagsData::new().with_notrap(),
            value,
            ptr,
            (self.ty.bytes() as usize * index) as i32,
        );
    }
    fn get(&mut self, r: Reg) -> V {
        if let Some(value) = self.fixed[r] {
            return self.num(value);
        }
        if let Some(var) = self.primal[r] {
            return self.fb.use_var(var);
        }
        let x = self.program.values[r];
        if x.abs() == 1. || x == 0. {
            return self.num(x);
        }
        if let Some(indices) = self.packed_constants {
            let Some(&i) = indices.get(&x.to_bits()) else {
                return self.num(x);
            };
            let base = self.field(self.ptr, offset_of!(Frame, constants));
            let v = self.fb.ins().load(
                F64X2,
                MemFlagsData::new()
                    .with_notrap()
                    .with_readonly()
                    .with_can_move(),
                base,
                (i * 16) as i32,
            );
            self.loaded_constants.insert(v, x);
            return v;
        }
        if let Some(&v) = self.constants.get(&r) {
            return v;
        }
        let v = self.load(offset_of!(Frame, constants), r);
        self.constants.insert(r, v);
        v
    }
    pub(super) fn set(&mut self, r: Reg, v: V) {
        let var = self.fb.declare_var(self.ty);
        self.fb.def_var(var, v);
        self.primal[r] = Some(var);
    }
    pub(super) fn grad(&mut self, r: Reg, i: usize) -> Option<V> {
        self.derivatives
            .get(r)
            .and_then(|row| row[i])
            .map(|part| match part {
                Partial::Scalar(var) => self.fb.use_var(var),
                Partial::Constant(value) => self.num(value),
                Partial::Lane(var, lane) => {
                    let v = self.fb.use_var(var);
                    self.fb.ins().extractlane(v, lane)
                }
            })
    }
    pub(super) fn constant(&self, v: V) -> Option<f64> {
        let dfg = &self.fb.func.dfg;
        let v = dfg.resolve_aliases(v);
        if let Some(&x) = self.loaded_constants.get(&v) {
            return Some(x);
        }
        let inst = dfg.value_def(v).inst()?;
        match dfg.insts[inst] {
            ir::InstructionData::Unary {
                opcode: ir::Opcode::Splat,
                arg,
            } => self.constant(arg),
            ir::InstructionData::UnaryIeee64 { imm, .. } => Some(f64::from_bits(imm.bits())),
            _ => None,
        }
    }
    pub(super) fn set_grad(&mut self, r: Reg, i: usize, v: V) {
        if !self.demand[r][i] || (self.regular && self.constant(v) == Some(0.)) {
            return;
        }
        if let Some(value) = self.constant(v) {
            self.derivatives[r][i] = Some(Partial::Constant(value));
        } else {
            let var = self.fb.declare_var(self.ty);
            self.fb.def_var(var, v);
            self.derivatives[r][i] = Some(Partial::Scalar(var));
        }
    }
    pub(super) fn predicate(&self, value: V) -> Option<V> {
        let dfg = &self.fb.func.dfg;
        let inst = dfg.value_def(dfg.resolve_aliases(value)).inst()?;
        if let ir::InstructionData::Ternary {
            opcode: ir::Opcode::Select,
            args: [c, t, f],
        } = dfg.insts[inst]
            && self.constant(t) == Some(1.)
            && self.constant(f) == Some(0.)
        {
            return Some(c);
        }
        if let ir::InstructionData::Binary {
            opcode: ir::Opcode::Band,
            args: [mask, one],
        } = dfg.insts[inst]
            && self.constant(one) == Some(1.)
            && let Some(mask) = dfg.value_def(mask).inst()
            && dfg.insts[mask].opcode() == ir::Opcode::Bitcast
        {
            return dfg.inst_args(mask).first().copied();
        }
        None
    }

    pub(super) fn truth(&mut self, v: V) -> V {
        if let Some(c) = self.predicate(v) {
            return c;
        }
        let z = self.num(0.);
        self.fb.ins().fcmp(FloatCC::NotEqual, v, z)
    }
    pub fn number_bool(&mut self, flag: V) -> V {
        let one = self.num(1.);
        let zero = self.num(0.);
        if self.ty == F64X2 {
            let mask = self.fb.ins().bitcast(F64X2, MemFlagsData::new(), flag);
            self.fb.ins().band(mask, one)
        } else {
            self.select(flag, one, zero)
        }
    }
    pub fn math_call(&mut self, row: usize, column: usize, x: V, y: V) -> V {
        let table = self.field(self.ptr, offset_of!(Frame, math));
        let offset = (row * 3 + column) * self.ptr.bytes() as usize;
        let f = self.fb.ins().load(
            self.ptr,
            MemFlagsData::trusted().with_readonly(),
            table,
            offset as i32,
        );
        let shim = |this: &mut Self, a, b| {
            let v = this.call(f, &[a, b]);
            if let Some(i) = this.fb.func.dfg.value_def(v).inst() {
                this.pure_calls.insert(i, (row, column));
            }
            v
        };
        let value = if self.ty == F64 {
            shim(self, x, y)
        } else {
            let mut lane = |i| {
                let a = self.fb.ins().extractlane(x, i);
                let b = self.fb.ins().extractlane(y, i);
                shim(self, a, b)
            };
            let a = lane(0);
            let b = lane(1);
            let v = self.fb.ins().splat(self.ty, a);
            self.fb.ins().insertlane(v, b, 1)
        };
        self.clear_block();
        value
    }
    fn call(&mut self, f: V, args: &[V]) -> V {
        let mut sig = ir::Signature::new(self.fb.func.signature.call_conv);
        sig.params.extend(
            args.iter()
                .map(|a| AbiParam::new(self.fb.func.dfg.value_type(*a))),
        );
        sig.returns.push(AbiParam::new(F64));
        let sig = self.fb.import_signature(sig);
        let call = self.fb.ins().call_indirect(sig, f, args);
        self.fb.inst_results(call)[0]
    }
    pub(super) fn select(&mut self, mask: V, x: V, y: V) -> V {
        if self.ty == F64 {
            self.fb.ins().select(mask, x, y)
        } else {
            let mask = self.fb.ins().bitcast(self.ty, MemFlagsData::new(), mask);
            self.fb.ins().bitselect(mask, x, y)
        }
    }
    fn finite(&mut self, v: V) -> V {
        let abs = self.fb.ins().fabs(v);
        let inf = self.num(f64::INFINITY);
        self.fb.ins().fcmp(FloatCC::LessThan, abs, inf)
    }
    fn body(
        &mut self,
        range: Range<usize>,
        handoff: &mut split::Handoff,
        joins: &joins::Joins,
        checks: &[(bool, bool)],
    ) -> Result<(), String> {
        if range.start > 0 {
            handoff.restore(self);
        }
        let ops = &self.program.ops;
        let mut blocks = vec![None; ops.len() + 1];
        for (pc, op) in ops.iter().enumerate() {
            if let Op::Jump(j) | Op::IfFalse(_, j) = *op {
                for k in [pc + 1, j] {
                    blocks[k].get_or_insert_with(|| self.fb.create_block());
                }
            }
        }
        let mut reached = vec![false; ops.len() + 1];
        let mut terminated = false;
        let mut checked = false;
        for (pc, block) in blocks
            .iter()
            .copied()
            .enumerate()
            .take(range.end + 1)
            .skip(range.start)
        {
            if terminated && !reached[pc] {
                // Callback numbers index the original program, including dead arms.
                self.query_index += i64::from(matches!(ops.get(pc), Some(Op::Simparam(..))));
                continue;
            }
            if let Some(block) = block {
                if !terminated {
                    for &copy in &joins.fallthrough[pc] {
                        self.edge_copy(copy);
                    }
                    self.fb.ins().jump(block, &[]);
                }
                self.clear_block();
                self.fb.switch_to_block(block);
                self.fb.seal_block(block);
                terminated = false;
            }
            if pc == range.end {
                break;
            }
            for &copy in &joins.before_jump[pc] {
                self.edge_copy(copy);
            }
            let op = &ops[pc];
            match *op {
                Op::IfFalse(c, j) => {
                    if let Some(value) = self.fixed[c] {
                        let target = if value != 0. { pc + 1 } else { j };
                        reached[target] = true;
                        self.fb
                            .ins()
                            .jump(blocks[target].ok_or("missing constant target")?, &[]);
                        terminated = true;
                        continue;
                    }
                    reached[pc + 1] = true;
                    reached[j] = true;
                    let c = self.get(c);
                    let test = self.truth(c);
                    self.fb.ins().brif(
                        test,
                        blocks[pc + 1].ok_or("missing true block")?,
                        &[],
                        blocks[j].ok_or("missing false block")?,
                        &[],
                    );
                    terminated = true;
                }
                Op::Jump(j) => {
                    reached[j] = true;
                    self.fb
                        .ins()
                        .jump(blocks[j].ok_or("missing branch target")?, &[]);
                    terminated = true;
                }
                Op::Unknown(d, i) => self.unknown(d, i),
                Op::Math(d, m, a, b) | Op::Charge(d, m, a, b) => self.arithmetic(d, m, a, b),
                Op::Phi(d, c, a, b) => {
                    if !joins.replaced[pc] || self.fixed[c].is_some() {
                        self.join(d, c, a, b);
                    }
                }
                Op::Ddx(d, a, i) => {
                    let v = self.grad(a, i).unwrap_or_else(|| self.num(0.));
                    self.set(d, v);
                }
                Op::Time(d) | Op::IntegralInitial(d) | Op::LimitEnabled(d) | Op::Analysis(d, _) => {
                    let field = match op {
                        Op::Time(_) => offset_of!(Frame, time),
                        Op::IntegralInitial(_) => offset_of!(Frame, integral_initial),
                        Op::LimitEnabled(_) => offset_of!(Frame, limit),
                        Op::Analysis(_, s) => match s.as_str() {
                            "dc" | "static" => offset_of!(Frame, dc),
                            "tran" => offset_of!(Frame, tran),
                            "ac" => offset_of!(Frame, ac),
                            "ic" => offset_of!(Frame, initial),
                            "noise" => offset_of!(Frame, noise),
                            _ => {
                                let zero = self.num(0.);
                                self.set(d, zero);
                                continue;
                            }
                        },
                        _ => return Err("invalid context instruction".into()),
                    };
                    let v = self.field(F64, field);
                    self.set(d, v);
                }
                Op::Event(d, initial, last) => {
                    let a = self.field(F64, offset_of!(Frame, initial));
                    let b = self.field(F64, offset_of!(Frame, last));
                    let v = match (initial, last) {
                        (true, true) => self.fb.ins().fmax(a, b),
                        (true, false) => a,
                        (false, true) => b,
                        _ => self.num(0.),
                    };
                    self.set(d, v);
                }
                Op::History(d, i, q) => {
                    let v = self.load(offset_of!(Frame, history), 2 * i + usize::from(q));
                    self.set(d, v);
                }
                Op::Simparam(d, ref key, default) => {
                    let fallback = match key.as_str() {
                        "gmin" => self.field(F64, offset_of!(Frame, gmin)),
                        "tnom" => self.field(F64, offset_of!(Frame, tnom)),
                        "scale" => self.num(1.),
                        "shrink" => self.num(0.),
                        "imelt" | "imax" if default.is_none() => self.num(1.),
                        _ => {
                            if let Some(r) = default {
                                self.get(r)
                            } else {
                                self.num(0.)
                            }
                        }
                    };
                    let context = self.field(self.ptr, offset_of!(Frame, queries));
                    let idx = self.fb.ins().iconst(I32, self.query_index);
                    self.query_index += 1;
                    let f = self.field(self.ptr, offset_of!(Frame, query_fn));
                    let v = self.call(f, &[context, idx, fallback]);
                    self.set(d, v);
                }
                Op::Previous(d, i) => {
                    let current = self.load(offset_of!(Frame, current), i);
                    let accepted = self.load(offset_of!(Frame, accepted), i);
                    let finite = self.finite(current);
                    let v = self.fb.ins().select(finite, current, accepted);
                    self.set(d, v);
                }
                Op::LimitValue {
                    dst,
                    junction,
                    new,
                    old,
                    arg,
                    critical,
                } => {
                    let args = [
                        self.get(new),
                        self.get(old),
                        self.get(arg),
                        self.get(critical),
                    ];
                    let offset = if junction {
                        offset_of!(Frame, pn_fn)
                    } else {
                        offset_of!(Frame, fet_fn)
                    };
                    let f = self.field(self.ptr, offset);
                    let v = self.call(f, &args);
                    self.set(dst, v);
                }
                Op::Limited {
                    dst,
                    slot,
                    new,
                    limited,
                } => self.limited(dst, slot, new, limited),
                Op::CheckZeroProduct(r, div) => {
                    let v = self.get(r);
                    let (primal, grad) = checks[pc];
                    if !primal && !grad && !div {
                        continue;
                    }
                    self.charge_guard(r, v, primal, grad, div);
                }
                Op::Stamp(row, pair, sign) => self.stamp(row, pair, sign),
                Op::Remember(i, v, _) => {
                    if !checked {
                        self.check();
                        checked = true;
                    }
                    let f = self.get(v.f);
                    let q = self.get(v.q);
                    self.store(offset_of!(Frame, history), 2 * i, f);
                    self.store(offset_of!(Frame, history), 2 * i + 1, q);
                }
                Op::Nop => {}
            }
        }
        if let Some(var) = self.vector_sum {
            let lanes = self.fb.use_var(var);
            let a = self.fb.ins().extractlane(lanes, 0);
            let b = self.fb.ins().extractlane(lanes, 1);
            let scalar = self.fb.use_var(self.guard_sum);
            let sum = self.fb.ins().fadd(scalar, a);
            let sum = self.fb.ins().fadd(sum, b);
            self.fb.def_var(self.guard_sum, sum);
        }
        if range.end < ops.len() {
            return handoff.save(self, range.end);
        }
        let valid = self.fb.use_var(self.valid);
        let sum = self.fb.use_var(self.guard_sum);
        let finite = self.finite(sum);
        let valid = self.fb.ins().band(valid, finite);
        let flag = self.fb.use_var(self.noncon);
        let flag = self.fb.ins().ishl_imm_u(flag, 1);
        let status = self.fb.ins().bor(valid, flag);
        let status = self.fb.ins().uextend(I32, status);
        self.fb.ins().return_(&[status]);
        Ok(())
    }
    fn unknown(&mut self, d: Reg, i: usize) {
        self.unknowns[d] = Some(i);
        if self.circuit.is_some_and(|s| s.wires[i].is_none()) {
            let z = self.num(0.);
            self.fixed[d] = Some(0.);
            self.set(d, z);
            return;
        }
        let v = self.load(offset_of!(Frame, x), i);
        self.set(d, v);
        if self.circuit.is_some_and(|s| s.fixed_columns.contains(&i)) {
            return;
        }
        let one = self.num(1.);
        self.set_grad(d, i, one);
    }
    fn arithmetic(&mut self, d: Reg, m: Math, a: Reg, b: Reg) {
        let x = self.get(a);
        let y = self.get(b);
        let known = |r: Reg| {
            self.fixed[r].or_else(|| self.primal[r].is_none().then_some(self.program.values[r]))
        };
        let tied = m == Math::Sub
            && self.circuit.is_some_and(|s| {
                self.unknowns[a]
                    .zip(self.unknowns[b])
                    .is_some_and(|(i, j)| i != j && s.wires[i] == s.wires[j])
            });
        let slope_used = |src: Reg| {
            self.derivatives
                .get(src)
                .zip(self.demand.get(d))
                .is_some_and(|(row, used)| {
                    row.iter()
                        .zip(used)
                        .any(|(part, &used)| used && part.is_some())
                })
        };
        let (v, sa, sb) = if let (Some(x), Some(y)) = (known(a), known(b)) {
            let (v, sa, sb) = m.apply(x, y);
            self.fixed[d] = Some(v);
            (self.num(v), self.num(sa), self.num(sb))
        } else if tied {
            self.fixed[d] = Some(0.);
            (self.num(0.), self.num(1.), self.num(-1.))
        } else {
            self.math(m, x, y, slope_used(a), slope_used(b))
        };
        self.set(d, v);
        if d < self.program.gradient_count {
            for index in 0..self.pairs.len() {
                let (i, j) = self.pairs[index];
                if self.fixed[d].is_none() && self.demand[d][i] && self.demand[d][j] {
                    self.packed_derivative(d, (i, j), [(a, sa), (b, sb)]);
                }
            }
            for i in 0..=self.n {
                if !self.demand[d][i] || self.derivatives[d][i].is_some() {
                    continue;
                }
                let left = self.grad(a, i).map(|g| self.scaled(g, sa, m, false));
                let right = if a == b && sa == sb {
                    left
                } else {
                    self.grad(b, i).map(|g| self.scaled(g, sb, m, true))
                };
                // Even adding +0 to a lone -0 must match the row interpreter.
                let v = match (left, right) {
                    (None, None) => continue,
                    (Some(x), Some(y)) => match (self.constant(x), self.constant(y)) {
                        (Some(a), Some(b)) => self.num(a + b),
                        _ => self.fb.ins().fadd(x, y),
                    },
                    (Some(x), None) if !self.regular && b < self.program.gradient_count => {
                        let z = self.num(0.);
                        self.fb.ins().fadd(x, z)
                    }
                    (None, Some(y)) if !self.regular && a < self.program.gradient_count => {
                        let z = self.num(0.);
                        self.fb.ins().fadd(z, y)
                    }
                    (Some(x), None) | (None, Some(x)) => x,
                };
                self.set_grad(d, i, v);
            }
        }
    }
    fn edge_copy(
        &mut self,
        joins::Copy {
            dst,
            src,
            condition,
        }: joins::Copy,
    ) {
        if self.fixed[condition].is_some() {
            return;
        }
        let value = self.get(src);
        let var = *self.primal[dst].get_or_insert_with(|| self.fb.declare_var(self.ty));
        self.fb.def_var(var, value);
        if dst >= self.derivatives.len() {
            return;
        }
        for col in 0..=self.n {
            if !self.demand[dst][col] {
                continue;
            }
            let g = self.grad(src, col);
            let existing = self.derivatives[dst][col];
            if existing.is_none() && g.is_none() {
                continue;
            }
            let v = g.unwrap_or_else(|| self.num(0.));
            let var = if let Some(Partial::Scalar(var)) = existing {
                var
            } else {
                self.fb.declare_var(self.ty)
            };
            self.fb.def_var(var, v);
            self.derivatives[dst][col] = Some(Partial::Scalar(var));
        }
    }

    fn join(&mut self, d: Reg, c: Reg, a: Reg, b: Reg) {
        if let Some(choice) = self.fixed[c] {
            let r = if choice != 0. { a } else { b };
            self.fixed[d] = self.fixed[r]
                .or_else(|| self.primal[r].is_none().then_some(self.program.values[r]));
            let v = self.get(r);
            self.set(d, v);
            for i in 0..=self.n {
                if let Some(g) = self.grad(r, i) {
                    self.set_grad(d, i, g);
                }
            }
            return;
        }
        let c = self.get(c);
        let test = self.truth(c);
        let x = self.get(a);
        let y = self.get(b);
        let v = self.select(test, x, y);
        self.set(d, v);
        for i in 0..=self.n {
            let x = self.grad(a, i);
            let y = self.grad(b, i);
            if x.is_some() || y.is_some() {
                let x = x.unwrap_or_else(|| self.num(0.));
                let y = y.unwrap_or_else(|| self.num(0.));
                let v = self.select(test, x, y);
                self.set_grad(d, i, v);
            }
        }
    }
    fn limited(&mut self, dst: Reg, slot: usize, new: Reg, limited: Reg) {
        let v = self.get(limited);
        let x = self.get(new);
        self.store(offset_of!(Frame, current), slot, v);
        self.set(dst, v);
        let delta = self.fb.ins().fsub(x, v);
        let abs = self.fb.ins().fabs(x);
        let one = self.num(1.);
        let tol = self.fb.ins().fadd(one, abs);
        let eps = self.num(1e-14);
        let tol = self.fb.ins().fmul(eps, tol);
        let abs = self.fb.ins().fabs(delta);
        let change = self.fb.ins().fcmp(FloatCC::GreaterThan, abs, tol);
        let old = self.fb.use_var(self.noncon);
        let flag = self.fb.ins().bor(old, change);
        self.fb.def_var(self.noncon, flag);
        for i in 0..self.n {
            if let Some(g) = self.grad(new, i) {
                self.set_grad(dst, i, g);
            }
        }
        let t = self.grad(new, self.n).unwrap_or_else(|| self.num(0.));
        let t = self.fb.ins().fadd(t, delta);
        self.set_grad(dst, self.n, t);
    }
    fn stamp(&mut self, row: usize, pair: Pair, sign: f64) {
        if self.circuit.is_some_and(|s| s.wires[row].is_none()) {
            return;
        }
        for (r, res, jac) in [
            (pair.f, offset_of!(Frame, f), offset_of!(Frame, df)),
            (pair.q, offset_of!(Frame, q), offset_of!(Frame, dq)),
        ] {
            if res == offset_of!(Frame, q) && self.circuit.is_some_and(|s| s.dc) {
                continue;
            }
            let x = self.get(r);
            let t = self.grad(r, self.n).unwrap_or_else(|| self.num(0.));
            let v = self.fb.ins().fadd(x, t);
            self.accumulate(res, row, sign, v);
            for j in 0..self.n {
                if let Some(g) = self.grad(r, j) {
                    self.accumulate(jac, row * self.n + j, sign, g);
                }
            }
        }
    }
    fn charge_guard(&mut self, r: Reg, v: V, primal: bool, grad: bool, div: bool) {
        if self.fixed[r].is_some_and(|x| x.is_finite() && (!div || x * x != 0.))
            && (!grad
                || self.derivatives.get(r).is_none_or(|row| {
                    row.iter().all(|g| {
                        g.is_none() || matches!(g, Some(Partial::Constant(x)) if x.is_finite())
                    })
                }))
        {
            return;
        }
        let mut valid = self.fb.use_var(self.valid);
        if div {
            let square = self.fb.ins().fmul(v, v);
            let nz = self.truth(square);
            valid = self.fb.ins().band(valid, nz);
        }
        if self.regular {
            // A nonfinite term cannot yield a finite sum. Overflow from finite
            // terms is conservative and takes the same exact replay path.
            let mut sum = if primal { v } else { self.num(0.) };
            if grad {
                for i in 0..self.n {
                    if let Some(acc) = self.vector_sum
                        && let Some(Partial::Lane(var, lane)) =
                            self.derivatives.get(r).and_then(|row| row[i])
                    {
                        if lane == 0 {
                            let v = self.fb.use_var(var);
                            let prev = self.fb.use_var(acc);
                            let next = self.fb.ins().fadd(prev, v);
                            self.fb.def_var(acc, next);
                        }
                    } else if let Some(g) = self.grad(r, i) {
                        sum = self.fb.ins().fadd(sum, g);
                    }
                }
            }
            let previous = self.fb.use_var(self.guard_sum);
            let sum = self.fb.ins().fadd(previous, sum);
            self.fb.def_var(self.guard_sum, sum);
        } else {
            if primal {
                let ok = self.finite(v);
                valid = self.fb.ins().band(valid, ok);
            }
            if grad {
                for i in 0..=self.n {
                    if let Some(g) = self.grad(r, i) {
                        let ok = self.finite(g);
                        valid = self.fb.ins().band(valid, ok);
                    }
                }
            }
        }
        self.fb.def_var(self.valid, valid);
    }
    fn clear_block(&mut self) {
        self.literals.clear();
        self.constants.clear();
        self.fields.clear();
    }
    fn check(&mut self) {
        self.clear_block();
        let ok = self.fb.create_block();
        let failed = self.fb.create_block();
        let valid = self.fb.use_var(self.valid);
        self.fb.ins().brif(valid, ok, &[], failed, &[]);
        self.fb.switch_to_block(failed);
        self.fb.seal_block(failed);
        let zero = self.fb.ins().iconst(I32, 0);
        self.fb.ins().return_(&[zero]);
        self.fb.switch_to_block(ok);
        self.fb.seal_block(ok);
    }
    fn accumulate(&mut self, field: usize, index: usize, sign: f64, v: V) {
        let v = if self.regular {
            if self.constant(v) == Some(0.) {
                return;
            }
            if sign == 1. { v } else { self.fb.ins().fneg(v) }
        } else {
            let s = self.num(sign);
            self.fb.ins().fmul(s, v)
        };
        let old = self.load(field, index);
        let sum = self.fb.ins().fadd(old, v);
        let sum = if let Some(mask) = self.stamp_mask {
            self.select(mask, sum, old)
        } else {
            sum
        };
        self.store(field, index, sum);
    }
    fn scaled(&mut self, g: V, slope: V, op: Math, right: bool) -> V {
        let partial = self.constant(g);
        let factor = self.constant(slope);
        if let (Some(g), Some(s)) = (partial, factor) {
            return self.num(crate::dual::scaled(g, s));
        }
        if self.regular {
            if factor == Some(1.) {
                return g;
            }
            if factor == Some(-1.) {
                return self.fb.ins().fneg(g);
            }
            if partial == Some(1.) {
                return slope;
            }
            if partial == Some(-1.) {
                return self.fb.ins().fneg(slope);
            }
            // Signed zeros disappear in dense stamps accumulated from +0.
            // A suppressed 0 * nonfinite would instead yield NaN. It either
            // dies at a branch or reaches a charge guard / Jacobian output,
            // where the call is replayed with the interpreter's exact scaled().
            return match (op, right) {
                (Math::Add, _) | (Math::Sub, false) => g,
                (Math::Sub, true) | (Math::Neg, false) => self.fb.ins().fneg(g),
                _ => self.fb.ins().fmul(g, slope),
            };
        }
        let nz = self.truth(g);
        let z = self.num(0.);
        let product = self.fb.ins().fmul(g, slope);
        self.fb.ins().select(nz, product, z)
    }
}

pub(super) extern "C" fn pnjlim(x: f64, y: f64, arg: f64, crit: f64) -> f64 {
    limit::pnjlim(x, y, arg, crit)
}
pub(super) extern "C" fn fetlim(x: f64, y: f64, arg: f64, _: f64) -> f64 {
    limit::fetlim(x, y, arg)
}
