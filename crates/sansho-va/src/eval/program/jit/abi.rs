//! Native call boundary and ownership of finalized executable pages.
//! Provenance: original implementation using the public Cranelift JIT module API.
#![allow(unsafe_code)]

use super::*;
use cranelift_jit::JITModule;
use cranelift_module::FuncId;

pub(super) struct Queries<'a> {
    pub keys: &'a [String],
    pub callback: &'a dyn Fn(&str) -> Option<f64>,
    pub log: &'a mut Vec<Option<f64>>,
    pub panic: Option<Box<dyn std::any::Any + Send>>,
}

pub(super) extern "C" fn query(q: &mut Queries<'_>, i: i32, default: f64) -> f64 {
    // An embedding callback may unwind. Resume it back in Rust after native code returns.
    if q.panic.is_some() {
        return default;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (q.callback)(&q.keys[i as usize])
    })) {
        Ok(v) => {
            q.log.push(v);
            v.unwrap_or(default)
        }
        Err(p) => {
            q.panic = Some(p);
            default
        }
    }
}

#[repr(C)]
pub(super) struct Frame<'a> {
    pub bridge: *mut f64,
    pub math: *const [arithmetic::Shim; 3],
    pub query_fn: extern "C" fn(&mut Queries<'_>, i32, f64) -> f64,
    pub pn_fn: extern "C" fn(f64, f64, f64, f64) -> f64,
    pub fet_fn: extern "C" fn(f64, f64, f64, f64) -> f64,
    pub x: *const f64,
    pub constants: *const f64,
    pub history: *mut f64,
    pub accepted: *const f64,
    pub current: *mut f64,
    pub f: *mut f64,
    pub q: *mut f64,
    pub df: *mut f64,
    pub dq: *mut f64,
    pub queries: *mut std::ffi::c_void,
    pub time: f64,
    pub gmin: f64,
    pub tnom: f64,
    pub initial: f64,
    pub last: f64,
    pub limit: f64,
    pub dc: f64,
    pub ac: f64,
    pub tran: f64,
    pub noise: f64,
    pub integral_initial: f64,
    borrow: std::marker::PhantomData<&'a mut ()>,
}
impl<'a> Frame<'a> {
    pub fn new(
        input: &'a EvalInput<'_>,
        tnom: f64,
        state: &'a mut StateIo<'_>,
        out: &'a mut EvalStamp<'_>,
        constants: &'a [f64],
        history: &'a mut [f64],
        queries: &'a mut Queries<'_>,
    ) -> Self {
        Self {
            bridge: std::ptr::null_mut(),
            math: arithmetic::TABLE.as_ptr(),
            query_fn: query,
            pn_fn: lower::pnjlim,
            fet_fn: lower::fetlim,
            x: input.x.as_ptr(),
            constants: constants.as_ptr(),
            history: history.as_mut_ptr(),
            accepted: state.accepted.as_ptr(),
            current: state.current.as_mut_ptr(),
            f: out.f.as_mut_ptr(),
            q: out.q.as_mut_ptr(),
            df: out.df.as_mut_ptr(),
            dq: out.dq.as_mut_ptr(),
            queries: std::ptr::from_mut(queries).cast(),
            time: input.abstime,
            gmin: input.gmin,
            tnom,
            initial: input.initial_step.into(),
            last: input.final_step.into(),
            limit: input.limit.into(),
            dc: (input.analysis == Analysis::Dc).into(),
            ac: (input.analysis == Analysis::Ac).into(),
            tran: (input.analysis == Analysis::Tran).into(),
            noise: (input.analysis == Analysis::Noise).into(),
            integral_initial: (input.analysis == Analysis::Dc
                || (input.analysis == Analysis::Tran && input.abstime == 0.))
                .into(),
            borrow: std::marker::PhantomData,
        }
    }
}

// The mutex gives a Send-only JITModule shared ownership. Execution never locks it.
pub(super) struct Pages(pub Mutex<Option<JITModule>>);
impl Drop for Pages {
    fn drop(&mut self) {
        if let Some(m) = self.0.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: the last owner is dropping, no invocation or external pointer survives it.
            unsafe { m.free_memory() }
        }
    }
}

type Entry = unsafe extern "C" fn(&Frame<'_>) -> i32;

pub(super) struct Code {
    entry: Entry,
    continuation: Option<(Entry, usize)>,
    _pages: Pages,
}
impl std::fmt::Debug for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Code").finish_non_exhaustive()
    }
}
impl Code {
    pub fn finalize(
        mut pages: Pages,
        id: FuncId,
        next: Option<FuncId>,
        slots: usize,
    ) -> Result<Self, String> {
        let module = pages
            .0
            .get_mut()
            .map_err(|e| e.to_string())?
            .as_mut()
            .ok_or("missing JIT module")?;
        module.finalize_definitions().map_err(|e| e.to_string())?;
        let entry = |id| {
            let ptr = module.get_finalized_function(id);
            // SAFETY: both entries use the host C ABI with one Frame pointer and
            // an i32 result. Pages owns both for the lifetime of Code.
            unsafe { std::mem::transmute::<*const u8, Entry>(ptr) }
        };
        Ok(Self {
            entry: entry(id),
            continuation: next.map(|id| (entry(id), slots)),
            _pages: pages,
        })
    }
    pub fn run(&self, frame: &mut Frame<'_>, bridge: &mut Vec<f64>) -> i32 {
        if let Some((next, slots)) = self.continuation {
            bridge.resize(slots, 0.);
            frame.bridge = bridge.as_mut_ptr();
            // SAFETY: the same checked compilation fixes both handoff layouts.
            // The first half only exports values and both calls borrow this buffer.
            unsafe {
                (self.entry)(frame);
                return next(frame);
            }
        }
        // SAFETY: evaluation validated the unknown/state/output lengths against
        // this binding. The compiler bounds displacements by the same layout.
        // All call-local slices and callback borrows live through this call.
        unsafe { (self.entry)(frame) }
    }
}
