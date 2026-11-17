//! Binding-time constant propagation and per-instance MNA node equivalence.
//! Provenance: original implementation from Accellera Verilog-AMS LRM 2.4 and elementary union-find.
use super::*;

/// Compact unknown order and projection from the module's uncollapsed numbering.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub(super) map: Vec<Option<usize>>,
    representatives: Vec<usize>,
}
impl Layout {
    pub(super) fn retain_all(&mut self) {
        self.representatives = (0..self.map.len()).collect();
        self.map = self.representatives.iter().copied().map(Some).collect();
    }
    /// Original module index for each retained unknown (terminals, internal nodes, currents).
    pub fn representatives(&self) -> &[usize] {
        &self.representatives
    }
    /// Compact unknown index, or None for ground and eliminated branch currents.
    pub fn index(&self, original: usize) -> Option<usize> {
        self.map.get(original).copied().flatten()
    }
    /// Number of unknowns in vectors supplied to the bound evaluator.
    pub fn unknown_len(&self) -> usize {
        self.representatives.len()
    }
}

#[derive(Clone, Default)]
struct Mode {
    short: bool,
    voltage: bool,
    flow: bool,
}

pub(super) fn layout(m: &CompiledModule, b: &Bound, input: &EvalInput<'_>) -> Layout {
    let mut modes = vec![Mode::default(); m.branches.len()];
    if m.branches.iter().any(|br| br.current.is_some()) {
        // Only scalar writes proven on every evaluation may feed a collapse decision.
        // Do not propagate across functions with potential global or copy-back writes.
        let mut scratch = Scratch {
            slots: vec![
                Slot {
                    values: vec![],
                    lower: 0,
                    ty: Ty::Real
                };
                m.slots
            ],
            ..Scratch::default()
        };
        let mut out = EvalOutput::default();
        let runtime = Runtime {
            module: m,
            bound: b,
            input,
            scratch: &mut scratch,
            out: &mut out,
            state: StateIo {
                accepted: &[],
                current: &mut [],
            },
            n: 0,
            depth: 0,
            drives: vec![],
            integrals: vec![],
        };
        let mut pass = Propagation {
            runtime,
            modes: &mut modes,
        };
        for s in &m.body {
            pass.visit(s, true);
        }
    }
    let nodes = m.ports.len() + m.internal_nodes.len();
    // Index zero is ground, so the smallest representative prefers ground, then ports.
    let mut parent: Vec<_> = (0..=nodes).collect();
    fn find(p: &mut [usize], mut k: usize) -> usize {
        while p[k] != k {
            p[k] = p[p[k]];
            k = p[k];
        }
        k
    }
    let mut currents = vec![false; m.branch_unknowns.len()];
    for (br, mode) in m.branches.iter().zip(modes) {
        let Some(i) = br.current else { continue };
        if mode.short && !mode.voltage && !mode.flow && !br.observed {
            let a = find(&mut parent, br.a.map_or(0, |a| a + 1));
            let b = find(&mut parent, br.b.map_or(0, |b| b + 1));
            parent[a.max(b)] = a.min(b);
        } else {
            currents[i - nodes] = br.observed || mode.voltage || mode.short;
        }
    }

    let mut result = Layout::default();
    for i in 0..m.unknown_len() {
        let index = if i < nodes {
            let rep = find(&mut parent, i + 1);
            if rep == 0 {
                None
            } else if rep == i + 1 {
                Some(result.representatives.len())
            } else {
                result.map[rep - 1]
            }
        } else {
            currents[i - nodes].then_some(result.representatives.len())
        };
        if index == Some(result.representatives.len()) {
            result.representatives.push(i);
        }
        result.map.push(index);
    }
    result
}

struct Propagation<'a, 'b> {
    runtime: Runtime<'a, 'b>,
    modes: &'b mut [Mode],
}
impl Propagation<'_, '_> {
    fn known(&self, e: &Expr) -> bool {
        match &e.kind {
            E::Num(_) | E::Str(_) | E::Param(_) | E::Given(_) | E::Connected(_) => true,
            E::Var(i) => !self.runtime.scratch.slots[*i].values.is_empty(),
            E::Unary(_, a) => self.known(a),
            E::Binary(_, a, b) => self.known(a) && self.known(b),
            E::Select(c, a, b) => self.known(c) && self.known(a) && self.known(b),
            E::Call(name, args) => {
                !name.starts_with('$')
                    && !matches!(name.as_str(), "analysis" | "ddt" | "idt")
                    && !name.contains("noise")
                    && args.iter().all(|a| self.known(a))
            }
            _ => false,
        }
    }
    fn value(&mut self, e: &Expr) -> Option<Value> {
        if !self.known(e) {
            return None;
        }
        let v = self.runtime.expr(e);
        if self.runtime.out.error.take().is_some() || !v.f.v.is_finite() {
            None
        } else {
            Some(v)
        }
    }
    fn visit(&mut self, s: &Stmt, certain: bool) {
        match s {
            Stmt::Block(_, body) => {
                for s in body {
                    self.visit(s, certain);
                }
            }
            Stmt::Assign(a, b) => {
                if let E::Var(i) = a.kind {
                    let v = if certain && !self.runtime.module.function_writes.contains(&i) {
                        self.value(b)
                    } else {
                        None
                    };
                    let values = &mut self.runtime.scratch.slots[i].values;
                    values.clear();
                    if let Some(v) = v {
                        values.push(v.cast(a.ty, &mut self.runtime.scratch.gradients));
                    }
                }
            }
            Stmt::Contrib(i, voltage, _, rhs) => {
                let zero = *voltage && certain && self.value(rhs).is_some_and(|v| v.f.v == 0.);
                let mode = &mut self.modes[*i];
                if !voltage {
                    mode.flow = true;
                } else if zero {
                    mode.short = true;
                } else {
                    mode.voltage = true;
                }
            }
            Stmt::If(c, a, b) => {
                if let Some(v) = certain.then(|| self.value(c)).flatten() {
                    if v.truth() {
                        self.visit(a, true);
                    } else if let Some(b) = b {
                        self.visit(b, true);
                    }
                } else {
                    self.visit(a, false);
                    if let Some(b) = b {
                        self.visit(b, false);
                    }
                }
            }
            Stmt::Case(c, arms) => {
                if certain && let Some(v) = self.value(c) {
                    let mut default = None;
                    for (labels, body) in arms {
                        if labels.is_empty() {
                            default = Some(body);
                        }
                        for label in labels {
                            let Some(key) = self.value(label) else {
                                for (_, s) in arms {
                                    self.visit(s, false);
                                }
                                return;
                            };
                            if v.equal(&key) {
                                self.visit(body, true);
                                return;
                            }
                        }
                    }
                    if let Some(s) = default {
                        self.visit(s, true);
                    }
                } else {
                    for (_, s) in arms {
                        self.visit(s, false);
                    }
                }
            }
            Stmt::For(init, _, step, body) => {
                self.visit(init, false);
                self.visit(step, false);
                self.visit(body, false);
            }
            Stmt::While(_, body) | Stmt::Repeat(_, body) | Stmt::Event { body, .. } => {
                self.visit(body, false)
            }
            Stmt::Empty | Stmt::Task(..) => {}
        }
    }
}
