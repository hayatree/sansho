//! Lower the packed register program to a host function with scalar SSA tangents.
//! Provenance: original implementation from forward-mode AD and SSA construction.

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

pub(super) struct Lower<'a, 'b> {
    pub fb: FunctionBuilder<'a>,
    pub ptr: ir::Type,
    frame: V,
    pub(super) program: &'b Program,
    pub(super) n: usize,
    pub(super) primal: Vec<Option<Variable>>,
    // An absent variable denotes a structural +0, including the displacement column.
    derivatives: Vec<Vec<Option<Variable>>>,
    pub(super) valid: Variable,
    pub(super) guard_sum: Variable,
    literals: HashMap<u64, V>,
    fields: HashMap<usize, V>,
    constants: HashMap<Reg, V>,
    noncon: Variable,
    query_index: i64,
    regular: bool,
}

pub(super) fn compile(
    p: &Program,
    n: usize,
    regular: bool,
    disk: Option<&crate::cache::Artifact>,
) -> Result<abi::Code, String> {
    let mut flags = settings::builder();
    let opt = std::env::var("SPX_CLIF_OPT").unwrap_or_else(|_| "none".into());
    let regalloc = std::env::var("SPX_CLIF_REGALLOC").unwrap_or_else(|_| "backtracking".into());
    for (name, value) in [
        // The register program is already optimized. The e-graph duplicates long
        // floating-point expressions on this workload and increases register spills.
        ("opt_level", opt.as_str()),
        ("regalloc_algorithm", regalloc.as_str()),
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
    let artifact = disk.map(|entry| {
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
        entry.child(&[target.as_bytes(), &n.to_le_bytes()])
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
    let cut = split::cut(p, regular);
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
    let mid = cut.unwrap_or(p.ops.len());
    for (func, range) in ids.into_iter().zip([0..mid, mid..p.ops.len()]) {
        let mut ctx = module.make_context();
        ctx.func.signature = signature.clone();
        emit(&mut ctx, config, p, n, regular, range, &mut handoff)?;
        module
            .define_function(func, &mut ctx)
            .map_err(|e| format!("{e:?}"))?;
        if let Some(compiled) = ctx.compiled_code() {
            let bytes = compiled.code_buffer();
            if std::env::var_os("SPX_PROFILE").is_some() {
                use std::io::Write;
                let _ = writeln!(std::io::stderr().lock(), "spx-code {}", bytes.len());
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

fn emit(
    ctx: &mut cranelift_codegen::Context,
    config: cranelift_codegen::isa::TargetFrontendConfig,
    p: &Program,
    n: usize,
    regular: bool,
    range: Range<usize>,
    handoff: &mut split::Handoff,
) -> Result<(), String> {
    let ptr = config.pointer_type();
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
    let entry = fb.create_block();
    fb.append_block_params_for_function_params(entry);
    fb.switch_to_block(entry);
    fb.seal_block(entry);
    let frame = fb.block_params(entry)[0];
    let guard_sum = fb.declare_var(F64);
    let zero = fb.ins().f64const(0.);
    fb.def_var(guard_sum, zero);
    let valid = fb.declare_var(I8);
    let noncon = fb.declare_var(I8);
    let yes = fb.ins().iconst(I8, 1);
    let no = fb.ins().iconst(I8, 0);
    fb.def_var(valid, yes);
    fb.def_var(noncon, no);
    let mut lower = Lower {
        fb,
        ptr,
        frame,
        program: p,
        n,
        primal: vec![None; p.values.len()],
        derivatives: vec![vec![None; n + 1]; p.gradient_count],
        valid,
        guard_sum,
        literals: HashMap::new(),
        fields: HashMap::new(),
        constants: HashMap::new(),
        noncon,
        query_index: 0,
        regular,
    };
    lower.body(range, handoff)?;
    lower.fb.finalize(config);
    Ok(())
}

impl Lower<'_, '_> {
    pub fn num(&mut self, x: f64) -> V {
        *self
            .literals
            .entry(x.to_bits())
            .or_insert_with(|| self.fb.ins().f64const(x))
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
        self.fb.ins().load(F64, flags, ptr, (8 * index) as i32)
    }
    pub(super) fn store(&mut self, field: usize, index: usize, value: V) {
        let ptr = self.field(self.ptr, field);
        self.fb
            .ins()
            .store(MemFlagsData::trusted(), value, ptr, (8 * index) as i32);
    }
    fn get(&mut self, r: Reg) -> V {
        if let Some(var) = self.primal[r] {
            return self.fb.use_var(var);
        }
        let x = self.program.values[r];
        if x.abs() == 1. || x == 0. {
            return self.num(x);
        }
        if let Some(&v) = self.constants.get(&r) {
            return v;
        }
        let v = self.load(offset_of!(Frame, constants), r);
        self.constants.insert(r, v);
        v
    }
    pub(super) fn set(&mut self, r: Reg, v: V) {
        let var = self.fb.declare_var(F64);
        self.fb.def_var(var, v);
        self.primal[r] = Some(var);
    }
    pub(super) fn grad(&mut self, r: Reg, i: usize) -> Option<V> {
        self.derivatives
            .get(r)
            .and_then(|row| row[i])
            .map(|var| self.fb.use_var(var))
    }
    pub(super) fn constant(&self, v: V) -> Option<f64> {
        let dfg = &self.fb.func.dfg;
        let inst = dfg.value_def(dfg.resolve_aliases(v)).inst()?;
        match dfg.insts[inst] {
            ir::InstructionData::UnaryIeee64 { imm, .. } => Some(f64::from_bits(imm.bits())),
            _ => None,
        }
    }
    pub(super) fn set_grad(&mut self, r: Reg, i: usize, v: V) {
        if self.regular && self.constant(v) == Some(0.) {
            return;
        }
        let var = self.fb.declare_var(F64);
        self.fb.def_var(var, v);
        self.derivatives[r][i] = Some(var);
    }
    pub(super) fn truth(&mut self, v: V) -> V {
        let z = self.num(0.);
        self.fb.ins().fcmp(FloatCC::NotEqual, v, z)
    }
    pub fn number_bool(&mut self, flag: V) -> V {
        let one = self.num(1.);
        let zero = self.num(0.);
        self.fb.ins().select(flag, one, zero)
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
        self.call(f, &[x, y])
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
    fn finite(&mut self, v: V) -> V {
        let abs = self.fb.ins().fabs(v);
        let inf = self.num(f64::INFINITY);
        self.fb.ins().fcmp(FloatCC::LessThan, abs, inf)
    }
    fn body(&mut self, range: Range<usize>, handoff: &mut split::Handoff) -> Result<(), String> {
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
        let checks = if self.regular {
            super::guards::required(self.program)
        } else {
            vec![(true, true); ops.len()]
        };
        let mut terminated = false;
        let mut checked = false;
        for (pc, block) in blocks
            .iter()
            .copied()
            .enumerate()
            .take(range.end + 1)
            .skip(range.start)
        {
            if let Some(block) = block {
                if !terminated {
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
            let op = &ops[pc];
            match *op {
                Op::IfFalse(c, j) => {
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
                    self.fb
                        .ins()
                        .jump(blocks[j].ok_or("missing branch target")?, &[]);
                    terminated = true;
                }
                Op::Unknown(d, i) => {
                    let v = self.load(offset_of!(Frame, x), i);
                    self.set(d, v);
                    let one = self.num(1.);
                    self.set_grad(d, i, one);
                }
                Op::Math(d, m, a, b) | Op::Charge(d, m, a, b) => self.arithmetic(d, m, a, b),
                Op::Phi(d, c, a, b) => self.join(d, c, a, b),
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
                Op::Noise(d) => {
                    let z = self.num(0.);
                    self.set(d, z);
                }
                Op::Nop => {}
            }
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
    fn arithmetic(&mut self, d: Reg, m: Math, a: Reg, b: Reg) {
        let x = self.get(a);
        let y = self.get(b);
        let (v, sa, sb) = self.math(
            m,
            x,
            y,
            d < self.program.gradient_count && a < self.program.gradient_count,
            d < self.program.gradient_count && b < self.program.gradient_count,
        );
        self.set(d, v);
        if d < self.program.gradient_count {
            for i in 0..=self.n {
                let left = self.grad(a, i).map(|g| self.scaled(g, sa, m, false));
                let right = self.grad(b, i).map(|g| self.scaled(g, sb, m, true));
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
    fn join(&mut self, d: Reg, c: Reg, a: Reg, b: Reg) {
        let c = self.get(c);
        let test = self.truth(c);
        let x = self.get(a);
        let y = self.get(b);
        let v = self.fb.ins().select(test, x, y);
        self.set(d, v);
        for i in 0..=self.n {
            let x = self.grad(a, i);
            let y = self.grad(b, i);
            if x.is_some() || y.is_some() {
                let x = x.unwrap_or_else(|| self.num(0.));
                let y = y.unwrap_or_else(|| self.num(0.));
                let v = self.fb.ins().select(test, x, y);
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
        for (r, res, jac) in [
            (pair.f, offset_of!(Frame, f), offset_of!(Frame, df)),
            (pair.q, offset_of!(Frame, q), offset_of!(Frame, dq)),
        ] {
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
                    if let Some(g) = self.grad(r, i) {
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
        let s = self.num(sign);
        let v = self.fb.ins().fmul(s, v);
        let old = self.load(field, index);
        let sum = self.fb.ins().fadd(old, v);
        self.store(field, index, sum);
    }
    fn scaled(&mut self, g: V, slope: V, op: Math, right: bool) -> V {
        let partial = self.constant(g);
        let factor = self.constant(slope);
        if let (Some(g), Some(s)) = (partial, factor) {
            return self.num(crate::dual::scaled(g, s));
        }
        if self.regular {
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
