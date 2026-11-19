//! Allocate separate derivative equations for the iterations of an analog-for loop.
//! Provenance: original implementation from Accellera Verilog-AMS LRM 2.4, analog operators and genvars.

use super::*;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(super) struct Derivatives {
    pub extra: usize,
    sites: BTreeMap<usize, Site>,
}

#[derive(Debug, Default)]
struct Site {
    genvars: Vec<usize>,
    slots: BTreeMap<Vec<i64>, usize>,
}

impl CompiledModule {
    pub(super) fn operator_node(&self, slot: usize) -> usize {
        if slot < self.operator_rows.len() {
            self.operator_rows.start + slot
        } else {
            self.unknown_len() + (slot - self.operator_rows.len())
        }
    }

    pub(super) fn operator_nodes(&self, b: &Bound) -> impl Iterator<Item = usize> {
        self.operator_rows
            .clone()
            .chain(self.unknown_len()..self.unknown_len() + b.generated.extra)
    }
}

impl Derivatives {
    pub(super) fn bind(
        m: &CompiledModule,
        bound: &Bound,
        input: &EvalInput<'_>,
    ) -> Result<Self, Diagnostic> {
        let mut scratch = Scratch {
            slots: vec![
                Slot {
                    ty: Ty::Integer,
                    lower: 0,
                    values: vec![Value::num(0.)]
                };
                m.slots
            ],
            ..Default::default()
        };
        let mut out = EvalStamp::default();
        let mut runtime = Runtime {
            module: m,
            bound,
            input,
            scratch: &mut scratch,
            out: &mut out,
            n: 0,
            depth: 0,
            drives: vec![],
            operator_rows: vec![],
            state: StateIo {
                accepted: &[],
                current: &mut [],
            },
        };
        let mut result = Self::default();
        let mut next = m.operator_rows.len();
        for (&template, loops) in &m.derivative_loops {
            for (i, scope) in loops.iter().enumerate() {
                if !constant(&scope.initial, &loops[..i])
                    || !constant(&scope.condition, &loops[..=i])
                    || !constant(&scope.next, &loops[..=i])
                {
                    return Err(Diagnostic::error(
                        "E-VA-SEMA",
                        "derivative genvar bounds must be constant at elaboration",
                    ));
                }
            }
            let mut site = Site {
                genvars: loops.iter().map(|l| l.var).collect(),
                ..Default::default()
            };
            enumerate(&mut runtime, loops, &mut Vec::new(), &mut |key| {
                // The first iteration uses the template row. Additional rows
                // follow all pre-existing module unknowns, including currents.
                let slot = if site.slots.is_empty() {
                    template
                } else {
                    let k = next;
                    next += 1;
                    k
                };
                site.slots.insert(key.to_vec(), slot);
            })?;
            result.sites.insert(template, site);
        }
        result.extra = next - m.operator_rows.len();
        Ok(result)
    }

    pub(super) fn slot(
        &self,
        template: usize,
        mut value: impl FnMut(usize) -> Option<f64>,
    ) -> Option<usize> {
        let Some(site) = self.sites.get(&template) else {
            return Some(template);
        };
        let key = site
            .genvars
            .iter()
            .map(|&i| value(i).map(|v| v as i64))
            .collect::<Option<Vec<_>>>()?;
        site.slots.get(&key).copied()
    }
}

fn constant(e: &Expr, scope: &[GenLoop]) -> bool {
    if e.parameter {
        return true;
    }
    match &e.kind {
        E::Var(i) => scope.iter().any(|s| s.var == *i),
        E::Unary(_, e) => constant(e, scope),
        E::Binary(_, a, b) => constant(a, scope) && constant(b, scope),
        E::Select(c, a, b) => [c, a, b].iter().all(|e| constant(e, scope)),
        E::Call(name, args) => {
            crate::math::Math::named(name).is_some() && args.iter().all(|e| constant(e, scope))
        }
        _ => false,
    }
}

fn enumerate(
    runtime: &mut Runtime<'_, '_, '_>,
    loops: &[GenLoop],
    key: &mut Vec<i64>,
    visit: &mut impl FnMut(&[i64]),
) -> Result<(), Diagnostic> {
    let Some((active, nested)) = loops.split_first() else {
        visit(key);
        return Ok(());
    };
    let first = runtime.expr(&active.initial);
    runtime.write(active.var, 0, first);
    while runtime.tick() && runtime.expr(&active.condition).truth() {
        key.push(runtime.load(active.var, None).f.v as i64);
        enumerate(runtime, nested, key, visit)?;
        key.pop();
        let next = runtime.expr(&active.next);
        runtime.write(active.var, 0, next);
    }
    if let Some(error) = &runtime.out.error {
        return Err(Diagnostic::error(
            "E-VA-SEMA",
            format!("cannot elaborate genvar loop: {error}"),
        ));
    }
    Ok(())
}
