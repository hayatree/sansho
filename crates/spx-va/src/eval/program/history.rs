//! Live-in analysis across model invocations.
//! Provenance: original implementation from standard compiler data-flow analysis.

use crate::ir::*;

/// Variables that may be read before the current invocation defines them. These
/// are the only externally observable stores at the back edge between invocations.
pub(crate) fn history(body: &[Stmt], slots: usize) -> Vec<usize> {
    fn read(e: &Expr, defined: &[bool], live: &mut [bool]) {
        match &e.kind {
            E::Var(i) => live[*i] |= !defined[*i],
            E::Index(i, e) => {
                live[*i] |= !defined[*i];
                read(e, defined, live);
            }
            E::Unary(_, a) | E::Ddx(a, _) => read(a, defined, live),
            E::Binary(_, a, b) | E::Integral(_, a, b, _) => {
                read(a, defined, live);
                read(b, defined, live);
            }
            E::Select(c, a, b) => {
                read(c, defined, live);
                read(a, defined, live);
                read(b, defined, live);
            }
            E::Call(_, args) => {
                for a in args {
                    read(a, defined, live)
                }
            }
            E::Limit(..) | E::Function(..) => live.fill(true),
            _ => {}
        }
    }
    fn visit(s: &Stmt, d: &mut [bool], l: &mut [bool]) {
        match s {
            Stmt::Block(vars, body) => {
                for (_, v) in vars {
                    if let Some(e) = &v.init {
                        read(e, d, l)
                    }
                    for (a, b) in &v.dimensions {
                        read(a, d, l);
                        read(b, d, l)
                    }
                }
                for s in body {
                    visit(s, d, l)
                }
            }
            Stmt::Assign(a, b) => {
                read(b, d, l);
                if let E::Var(i) = a.kind {
                    d[i] = true
                } else {
                    read(a, d, l)
                }
            }
            Stmt::Contrib(_, _, _, e) => read(e, d, l),
            Stmt::If(c, a, b) => {
                read(c, d, l);
                let mut then = d.to_vec();
                visit(a, &mut then, l);
                if let Some(b) = b {
                    visit(b, d, l)
                }
                for (x, y) in d.iter_mut().zip(then) {
                    *x &= y;
                }
            }
            Stmt::Event { body, .. } => visit(body, &mut d.to_vec(), l),
            Stmt::While(c, b) | Stmt::Repeat(c, b) => {
                read(c, d, l);
                visit(b, &mut d.to_vec(), l)
            }
            Stmt::For(i, c, step, b) => {
                visit(i, d, l);
                read(c, d, l);
                let mut inner = d.to_vec();
                visit(b, &mut inner, l);
                visit(step, &mut inner, l);
            }
            Stmt::Case(c, arms) => {
                read(c, d, l);
                for (keys, s) in arms {
                    for k in keys {
                        read(k, d, l)
                    }
                    visit(s, &mut d.to_vec(), l)
                }
            }
            Stmt::Task(_, args) => {
                for a in args {
                    read(a, d, l)
                }
            }
            Stmt::Empty => {}
        }
    }
    let mut d = vec![false; slots];
    let mut l = d.clone();
    for s in body {
        visit(s, &mut d, &mut l);
    }
    l.into_iter()
        .enumerate()
        .filter_map(|(i, v)| v.then_some(i))
        .collect()
}

/// A bias-only program may execute from the first call when startup events
/// and declaration initializers have no work to do.
#[cfg(feature = "jit")]
pub(crate) fn has_startup(s: &Stmt) -> bool {
    match s {
        Stmt::Event { startup, body, .. } => *startup || has_startup(body),
        Stmt::Block(_, body) => body.iter().any(has_startup),
        Stmt::If(_, a, b) => has_startup(a) || b.as_deref().is_some_and(has_startup),
        Stmt::Case(_, arms) => arms.iter().any(|(_, s)| has_startup(s)),
        Stmt::For(init, _, step, body) => {
            has_startup(init) || has_startup(step) || has_startup(body)
        }
        Stmt::While(_, body) | Stmt::Repeat(_, body) => has_startup(body),
        _ => false,
    }
}
