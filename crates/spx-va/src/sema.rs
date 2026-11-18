//! Semantic checks and lowering to resolved analog trees.
//! Provenance: original implementation from Accellera Verilog-AMS LRM 2.4 and compiler literature.
use crate::{
    ast::{self, ExprKind as A, ItemKind as I, StmtKind as S},
    ir::*,
};
use spx_diag::{Diagnostic, Span};
use std::collections::{BTreeMap, BTreeSet};
type R<T> = Result<T, Diagnostic>;
#[derive(Clone, Copy)]
enum Symbol {
    Param(usize, Ty),
    Var(usize, Ty),
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum BranchKey {
    Named(String),
    Nodes(Option<usize>, Option<usize>),
}
struct Lower {
    potentials: BTreeSet<String>,
    function_writes: BTreeSet<usize>,
    branch_ids: BTreeMap<BranchKey, usize>,
    symbols: BTreeMap<String, Symbol>,
    nodes: BTreeMap<String, Option<usize>>,
    probes: BTreeMap<String, usize>,
    port_names: BTreeMap<String, usize>,
    optional_ports: Vec<bool>,
    named: BTreeMap<String, Vec<ast::Expr>>,
    branches: Vec<Branch>,
    branch_names: Vec<String>,
    functions: BTreeMap<String, usize>,
    signatures: Vec<Vec<(String, Ty)>>,
    definitions: Vec<ast::AnalogFunction>,
    slots: usize,
    states: usize,
    integral_count: usize,
    in_function: bool,
    arrays: BTreeSet<usize>,
    depth: usize,
    calls: Vec<BTreeSet<usize>>,
    caller: Option<usize>,
}
fn error(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-VA-SEMA", msg).with_span(span)
}
fn unsupported(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-VA-UNSUPPORTED", msg).with_span(span)
}
/// Resolve and validate a module; `None` selects the last module in the source.
pub fn compile_module(
    src: &ast::SourceFile,
    name: Option<&str>,
) -> Result<CompiledModule, Vec<Diagnostic>> {
    let module = match name {
        Some(n) => src.modules.iter().find(|m| m.name.name == n),
        None => src.modules.last(),
    };
    let Some(m) = module else {
        return Err(vec![error(src.span, "module not found")]);
    };
    let access: BTreeMap<_, _> = src
        .natures
        .iter()
        .filter_map(|n| {
            n.properties.iter().find_map(|(key, v)| {
                if key.name == "access"
                    && let A::Ident(id) = &v.kind
                {
                    Some((&n.name.name, &id.name))
                } else {
                    None
                }
            })
        })
        .collect();
    let mut potentials = BTreeSet::from(["V".into(), "Temp".into()]);
    for d in &src.disciplines {
        for (role, v) in &d.properties {
            if role.name == "potential"
                && let A::Ident(nature) = &v.kind
                && let Some(name) = access.get(&nature.name)
            {
                potentials.insert((*name).clone());
            }
        }
    }
    lower(m, potentials).map_err(|e| vec![e])
}
fn lower(m: &ast::Module, potentials: BTreeSet<String>) -> R<CompiledModule> {
    let mut l = Lower {
        potentials,
        function_writes: BTreeSet::new(),
        branch_ids: BTreeMap::new(),
        symbols: BTreeMap::new(),
        nodes: BTreeMap::new(),
        probes: BTreeMap::new(),
        port_names: m
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect(),
        optional_ports: vec![false; m.ports.len()],
        named: BTreeMap::new(),
        branches: vec![],
        branch_names: vec![],
        functions: BTreeMap::new(),
        signatures: vec![],
        definitions: vec![],
        slots: 0,
        states: 0,
        integral_count: 0,
        in_function: false,
        arrays: BTreeSet::new(),
        depth: 0,
        calls: vec![],
        caller: None,
    };
    let ports: Vec<_> = m.ports.iter().map(|p| p.name.clone()).collect();
    for (i, p) in m.ports.iter().enumerate() {
        if l.nodes.insert(p.name.clone(), Some(i)).is_some() {
            return Err(error(p.span, "duplicate port"));
        }
    }
    l.nodes.insert("0".into(), None);
    let mut grounds = BTreeSet::new();
    let mut nets = Vec::new();
    let mut seen_nets = BTreeSet::new();
    for item in &m.items {
        match &item.kind {
            I::Ground(names) => {
                for n in names {
                    grounds.insert(n.name.clone());
                }
            }
            I::NetDecl { names, .. } => {
                for n in names {
                    if !l.port_names.contains_key(&n.name) && seen_nets.insert(n.name.clone()) {
                        nets.push(n.name.clone());
                    }
                }
            }
            I::Branch { nodes, names } => {
                for n in names {
                    if l.named.insert(n.name.clone(), nodes.clone()).is_some() {
                        return Err(error(n.span, "duplicate branch"));
                    }
                }
            }
            I::Function(f) => {
                let i = l.definitions.len();
                if l.functions.insert(f.name.name.clone(), i).is_some() {
                    return Err(error(f.span, "duplicate function"));
                }
                l.signatures.push(signature(f));
                l.definitions.push(f.clone());
            }
            _ => {}
        }
    }
    // Only allocate nets actually referenced in analog expressions, including named branches.
    let mut used = Uses::default();
    for item in &m.items {
        if let I::Analog(s) = &item.kind {
            collect_stmt(s, &mut used);
        }
    }
    for item in &m.items {
        if let I::Var(v) = &item.kind
            && let Some(e) = &v.value
        {
            collect_expr(e, &mut used);
        }
    }
    while let Some(name) = used.pending.pop() {
        if let Some(&i) = l.functions.get(&name) {
            for s in &l.definitions[i].body {
                collect_stmt(s, &mut used);
            }
        }
        if let Some(nodes) = l.named.get(&name) {
            for n in nodes {
                collect_expr(n, &mut used);
            }
        }
    }
    let mut internal_nodes: Vec<_> = nets
        .into_iter()
        .filter(|n| used.names.contains(n) && !grounds.contains(n))
        .collect();
    for (i, n) in internal_nodes.iter().enumerate() {
        l.nodes.insert(n.clone(), Some(ports.len() + i));
    }
    for n in grounds {
        l.nodes.insert(n, None);
    }
    for id in used.probes {
        let Some(Some(port)) = l
            .nodes
            .get(&id)
            .copied()
            .filter(|_| l.port_names.contains_key(&id))
        else {
            return Err(error(
                m.span,
                format!("port current requires a non-ground module port: {id}"),
            ));
        };
        let inner = ports.len() + internal_nodes.len();
        internal_nodes.push(format!("{id}#sense"));
        l.nodes.insert(id.clone(), Some(inner));
        l.probes.insert(id.clone(), l.branches.len());
        l.branches.push(Branch {
            a: Some(port),
            b: Some(inner),
            current: Some(0),
            observed: true,
            contributed: false,
        });
        l.branch_names.push(format!("{id}#flow"));
    }
    let mut params: Vec<ParamInfo> = Vec::new();
    let multiplier = |s: &str| s.eq_ignore_ascii_case("m") || s.eq_ignore_ascii_case("mult");
    if !m.items.iter().any(|item| {
        matches!(&item.kind,I::Param(p) if multiplier(&p.name.name))
            || matches!(&item.kind,I::AliasParam{name,..} if multiplier(&name.name))
    }) {
        let default = ast::Expr {
            kind: A::Real(1.),
            span: m.span,
        };
        params.push(ParamInfo {
            name: "m".into(),
            ty: "real".into(),
            value: l.expr(&default)?,
            default,
            ranges: vec![],
            aliases: vec![],
            local: false,
            placement: Some(ast::ParamKind::Instance),
            limits: vec![],
        });
        // A source-level declaration can shadow the implicit multiplier.
        if !m
            .items
            .iter()
            .any(|d| matches!(&d.kind,I::Var(v) if v.name.name=="m"))
            && !l.nodes.contains_key("m")
        {
            l.symbols.insert("m".into(), Symbol::Param(0, Ty::Real));
        }
    }
    let implicit_m = !params.is_empty();
    let mut variables = Vec::new();
    for item in &m.items {
        match &item.kind {
            I::Param(p) => {
                let value = l.expr(&p.value)?;
                if !value.parameter {
                    return Err(error(p.span, "parameter default must be parameter-only"));
                }
                let ty = Ty::parse(&p.ty);
                check_conversion(ty, value.ty, p.span)?;
                let mut limits = vec![];
                for r in &p.ranges {
                    let a = l.expr(&r.lower)?;
                    let b = r.upper.as_ref().map(|e| l.expr(e)).transpose()?;
                    if !a.parameter || b.as_ref().is_some_and(|v| !v.parameter) {
                        return Err(error(r.span, "parameter range must be parameter-only"));
                    }
                    if a.ty == Ty::String
                        || b.as_ref().is_some_and(|b| b.ty == Ty::String)
                        || ty == Ty::String
                    {
                        return Err(error(r.span, "parameter ranges require numeric values"));
                    }
                    limits.push((r.exclude, a, b, r.lower_closed, r.upper_closed));
                }
                l.insert(&p.name, Symbol::Param(params.len(), ty))?;
                params.push(ParamInfo {
                    name: p.name.name.clone(),
                    ty: p.ty.clone(),
                    default: p.value.clone(),
                    ranges: p.ranges.clone(),
                    aliases: vec![],
                    local: p.local,
                    placement: p.placement,
                    value,
                    limits,
                });
            }
            I::Var(v) => {
                let d = l.variable(v)?;
                variables.push(d);
            }
            I::AliasParam { name, target } => {
                let Some(Symbol::Param(i, t)) = l.symbols.get(&target.name).copied() else {
                    return Err(error(
                        target.span,
                        "alias target is not a preceding parameter",
                    ));
                };
                l.insert(name, Symbol::Param(i, t))?;
                params[i].aliases.push(name.name.clone());
            }
            _ => {}
        }
    }
    // Discover all potential contributions before resolving any current reads.
    for item in &m.items {
        if let I::Analog(s) = &item.kind {
            l.discover(s)?;
        }
    }
    l.calls = vec![BTreeSet::new(); l.definitions.len()];
    let mut functions = vec![];
    for i in 0..l.definitions.len() {
        functions.push(l.function(i)?);
    }
    // Remove roots in topological order. Remaining edges imply recursion.
    let mut incoming = vec![0usize; l.calls.len()];
    for callee in l.calls.iter().flatten() {
        incoming[*callee] += 1;
    }
    let mut roots: Vec<_> = (0..incoming.len()).filter(|&i| incoming[i] == 0).collect();
    while let Some(i) = roots.pop() {
        for &j in &l.calls[i] {
            incoming[j] -= 1;
            if incoming[j] == 0 {
                roots.push(j);
            }
        }
    }
    if let Some(i) = incoming.iter().position(|&n| n != 0) {
        return Err(unsupported(
            l.definitions[i].span,
            "recursive analog functions are unsupported",
        ));
    }
    let mut body = vec![];
    for item in &m.items {
        if let I::Analog(s) = &item.kind {
            body.push(l.stmt(s)?);
        }
    }
    let first_integral = ports.len() + internal_nodes.len();
    internal_nodes.extend((0..l.integral_count).map(|i| format!("#idt{i}")));
    let n = ports.len() + internal_nodes.len();
    let mut branch_unknowns = vec![];
    for (i, b) in l.branches.iter_mut().enumerate() {
        if b.current.is_some() {
            b.current = Some(n + branch_unknowns.len());
            branch_unknowns.push(l.branch_names[i].clone());
        }
    }
    let mut param_index = BTreeMap::new();
    for (i, p) in params.iter().enumerate() {
        for name in std::iter::once(&p.name).chain(&p.aliases) {
            let index = param_index
                .entry(name.to_ascii_lowercase())
                .or_insert(Some(i));
            if *index != Some(i) {
                *index = None;
            }
        }
    }
    // The two SPICE instance spellings address the same multiplier unless both exist.
    for (alias, declared) in [("m", "mult"), ("mult", "m")] {
        if !param_index.contains_key(alias)
            && let Some(index) = param_index.get(declared).copied()
        {
            param_index.insert(alias.into(), index);
        }
    }
    let history = crate::eval::program::history(&body, l.slots);
    Ok(CompiledModule {
        #[cfg(feature = "jit")]
        kernels: Default::default(),
        history,
        optional_ports: l.optional_ports,
        param_index,
        name: m.name.name.clone(),
        ports,
        internal_nodes,
        branch_unknowns,
        params,
        branches: l.branches,
        variables,
        functions,
        body,
        slots: l.slots,
        function_writes: l.function_writes,
        states: l.states,
        integrals: first_integral..n,
        identity: std::sync::Arc::new(()),
        implicit_m,
    })
}
fn check_conversion(a: Ty, b: Ty, s: Span) -> R<()> {
    if (a == Ty::String) != (b == Ty::String) {
        Err(error(s, "incompatible string and numeric types"))
    } else {
        Ok(())
    }
}
impl Lower {
    fn insert(&mut self, id: &ast::Ident, s: Symbol) -> R<()> {
        if self.symbols.contains_key(&id.name)
            || self.nodes.contains_key(&id.name)
            || self.functions.contains_key(&id.name)
        {
            return Err(error(id.span, format!("duplicate symbol {}", id.name)));
        }
        self.symbols.insert(id.name.clone(), s);
        Ok(())
    }
    fn variable(&mut self, v: &ast::VarDecl) -> R<(usize, Variable)> {
        let i = self.slots;
        self.slots += 1;
        let ty = Ty::parse(&v.ty);
        let dimensions = v
            .dimensions
            .iter()
            .map(|(a, b)| Ok((self.expr(a)?, self.expr(b)?)))
            .collect::<R<Vec<_>>>()?;
        if dimensions.len() > 1 {
            return Err(unsupported(
                v.span,
                "only one-dimensional arrays are supported",
            ));
        }
        if dimensions.iter().any(|(a, b)| !a.parameter || !b.parameter) {
            return Err(error(v.span, "array bounds must be parameter-only"));
        }
        let init = v.value.as_ref().map(|e| self.expr(e)).transpose()?;
        if let Some(e) = &init {
            check_conversion(ty, e.ty, v.span)?;
            if e.reactive {
                return Err(unsupported(
                    v.span,
                    "ddt is only supported directly in contributions",
                ));
            }
        }
        if !dimensions.is_empty() {
            self.arrays.insert(i);
        }
        self.insert(&v.name, Symbol::Var(i, ty))?;
        Ok((
            i,
            Variable {
                ty,
                dimensions,
                init,
            },
        ))
    }
    fn node(&self, e: &ast::Expr) -> R<Option<usize>> {
        let name = match &e.kind {
            A::Ident(i) => i.name.as_str(),
            A::Int(0) => "0",
            _ => return Err(error(e.span, "expected net name")),
        };
        self.nodes
            .get(name)
            .copied()
            .ok_or_else(|| error(e.span, format!("unknown net {name}")))
    }
    fn port_probe(&self, args: &[ast::Expr], direct: bool) -> Option<usize> {
        let [
            ast::Expr {
                kind: A::Ident(id), ..
            },
        ] = args
        else {
            return None;
        };
        if direct {
            self.probes.get(&id.name).copied()
        } else if let Some(
            [
                ast::Expr {
                    kind: A::Port(port),
                    ..
                },
            ],
        ) = self.named.get(&id.name).map(Vec::as_slice)
        {
            self.probes.get(&port.name).copied()
        } else {
            None
        }
    }
    fn endpoints(&self, args: &[ast::Expr], span: Span) -> R<(Option<usize>, Option<usize>)> {
        let args = if args.len() == 1
            && let A::Ident(i) = &args[0].kind
            && let Some(nodes) = self.named.get(&i.name)
        {
            nodes.as_slice()
        } else {
            args
        };
        match args {
            [a] => Ok((self.node(a)?, None)),
            [a, b] => Ok((self.node(a)?, self.node(b)?)),
            _ => Err(error(span, "access requires one or two nets")),
        }
    }
    fn branch(&mut self, args: &[ast::Expr], span: Span, voltage: bool) -> R<usize> {
        let (a, b) = self.endpoints(args, span)?;
        let key = if let [
            ast::Expr {
                kind: A::Ident(id), ..
            },
        ] = args
            && self.named.contains_key(&id.name)
        {
            BranchKey::Named(id.name.clone())
        } else {
            BranchKey::Nodes(a.min(b), a.max(b))
        };
        if let Some(&i) = self.branch_ids.get(&key) {
            if voltage {
                self.branches[i].current = Some(0);
            }
            return Ok(i);
        }
        let i = self.branches.len();
        self.branch_ids.insert(key, i);
        self.branches.push(Branch {
            a,
            b,
            current: voltage.then_some(0),
            observed: false,
            contributed: false,
        });
        let name = if args.len() == 1 {
            if let A::Ident(n) = &args[0].kind {
                n.name.clone()
            } else {
                format!("branch{i}")
            }
        } else {
            format!("branch{i}")
        };
        self.branch_names.push(name);
        Ok(i)
    }
    fn discover(&mut self, s: &ast::Stmt) -> R<()> {
        match &s.kind {
            S::Contrib { access, .. } => {
                if let A::Access {
                    nature,
                    args,
                    port_flow,
                } = &access.kind
                {
                    if *port_flow || self.port_probe(args, false).is_some() {
                        return Err(unsupported(access.span, "port branches are read-only"));
                    }
                    let i =
                        self.branch(args, access.span, self.potentials.contains(&nature.name))?;
                    self.branches[i].contributed = true;
                } else {
                    return Err(error(access.span, "contribution target must be V or I"));
                }
            }
            S::Block { statements, .. } => {
                for s in statements {
                    self.discover(s)?;
                }
            }
            S::If {
                then_stmt,
                else_stmt,
                ..
            } => {
                self.discover(then_stmt)?;
                if let Some(s) = else_stmt {
                    self.discover(s)?;
                }
            }
            S::Case { arms, .. } => {
                for a in arms {
                    self.discover(&a.body)?;
                }
            }
            S::For { body, .. }
            | S::While { body, .. }
            | S::Repeat { body, .. }
            | S::Event { body, .. } => self.discover(body)?,
            _ => {}
        }
        Ok(())
    }
    fn function(&mut self, i: usize) -> R<Function> {
        let f = self.definitions[i].clone();
        let mut saved = BTreeMap::new();
        let mut shadow = |name: &str| {
            saved
                .entry(name.to_owned())
                .or_insert_with(|| self.symbols.remove(name));
        };
        shadow(&f.name.name);
        for d in &f.declarations {
            match &d.kind {
                I::Var(v) => shadow(&v.name.name),
                I::PortDecl { names, .. } => {
                    for n in names {
                        shadow(&n.name);
                    }
                }
                _ => {}
            }
        }
        self.in_function = true;
        self.caller = Some(i);
        let result = self.slots;
        self.slots += 1;
        self.symbols
            .insert(f.name.name.clone(), Symbol::Var(result, Ty::parse(&f.ty)));
        let mut args = vec![];
        let mut locals = vec![(
            result,
            Variable {
                ty: Ty::parse(&f.ty),
                dimensions: vec![],
                init: None,
            },
        )];
        for d in &f.declarations {
            if let I::PortDecl {
                names, direction, ..
            } = &d.kind
            {
                for n in names {
                    if self.symbols.contains_key(&n.name) {
                        return Err(error(n.span, "duplicate function argument"));
                    }
                    let ty = self.signatures[i][args.len()].1;
                    let slot = self.slots;
                    self.slots += 1;
                    self.symbols.insert(n.name.clone(), Symbol::Var(slot, ty));
                    args.push((slot, direction.clone()));
                    locals.push((
                        slot,
                        Variable {
                            ty,
                            dimensions: vec![],
                            init: None,
                        },
                    ));
                }
            }
        }
        for d in &f.declarations {
            if let I::Var(v) = &d.kind {
                if let Some(Symbol::Var(slot, _)) = self.symbols.get(&v.name.name).copied()
                    && let Some((_, decl)) =
                        slot.checked_sub(result).and_then(|k| locals.get_mut(k))
                {
                    if !v.dimensions.is_empty() || v.value.is_some() {
                        return Err(unsupported(
                            v.span,
                            "function argument arrays and initializers are unsupported",
                        ));
                    }
                    decl.ty = Ty::parse(&v.ty);
                    self.symbols
                        .insert(v.name.name.clone(), Symbol::Var(slot, decl.ty));
                    continue;
                }
                self.symbols.remove(&v.name.name);
                locals.push(self.variable(v)?);
            }
        }
        let body = f.body.iter().map(|s| self.stmt(s)).collect::<R<Vec<_>>>()?;
        for (name, old) in saved {
            self.symbols.remove(&name);
            if let Some(sym) = old {
                self.symbols.insert(name, sym);
            }
        }
        self.in_function = false;
        self.caller = None;
        Ok(Function {
            result,
            args,
            locals,
            body,
        })
    }
    fn lvalue(&mut self, e: &ast::Expr) -> R<Expr> {
        let v = self.expr(e)?;
        if !matches!(v.kind, E::Var(_) | E::Index(..)) {
            return Err(error(e.span, "assignment target must be a variable"));
        }
        Ok(v)
    }
    fn plain(&mut self, e: &ast::Expr) -> R<Expr> {
        let v = self.expr(e)?;
        if v.reactive {
            return Err(unsupported(
                e.span,
                "ddt is only supported linearly in contributions",
            ));
        }
        Ok(v)
    }
    fn condition(&mut self, e: &ast::Expr) -> R<Expr> {
        let v = self.plain(e)?;
        if v.ty == Ty::String {
            return Err(error(e.span, "numeric condition required"));
        }
        Ok(v)
    }
    fn stmt(&mut self, s: &ast::Stmt) -> R<Stmt> {
        Ok(match &s.kind {
            S::Empty => Stmt::Empty,
            S::Block {
                declarations,
                statements,
                ..
            } => {
                let mut saved = Vec::new();
                let mut vars = vec![];
                let mut declared = BTreeSet::new();
                for d in declarations {
                    match &d.kind {
                        I::Var(v) => {
                            if !declared.insert(&v.name.name) {
                                return Err(error(v.span, "duplicate block variable"));
                            }
                            saved.push((v.name.name.clone(), self.symbols.remove(&v.name.name)));
                            vars.push(self.variable(v)?);
                        }
                        _ => return Err(unsupported(d.span, "block parameters are unsupported")),
                    }
                }
                let body = statements
                    .iter()
                    .map(|s| self.stmt(s))
                    .collect::<R<Vec<_>>>()?;
                for (name, old) in saved {
                    self.symbols.remove(&name);
                    if let Some(sym) = old {
                        self.symbols.insert(name, sym);
                    }
                }
                Stmt::Block(vars, body)
            }
            S::Assign { lhs, rhs } => {
                let a = self.lvalue(lhs)?;
                if self.in_function
                    && let E::Var(i) = a.kind
                {
                    self.function_writes.insert(i);
                }
                let b = self.plain(rhs)?;
                check_conversion(a.ty, b.ty, s.span)?;
                Stmt::Assign(a, b)
            }
            S::Contrib { access, rhs } => {
                if self.in_function {
                    return Err(error(
                        s.span,
                        "contributions are not allowed inside functions",
                    ));
                }
                let A::Access { nature, args, .. } = &access.kind else {
                    return Err(error(s.span, "invalid contribution"));
                };
                let (a, b) = self.endpoints(args, access.span)?;
                let i = self.branch(args, access.span, false)?;
                let e = self.expr(rhs)?;
                if e.ty == Ty::String {
                    return Err(error(rhs.span, "numeric contribution required"));
                }
                let sign = if self.branches[i].a == a && self.branches[i].b == b {
                    1.
                } else {
                    -1.
                };
                Stmt::Contrib(i, self.potentials.contains(&nature.name), sign, e)
            }
            S::If {
                condition,
                then_stmt,
                else_stmt,
            } => Stmt::If(
                self.condition(condition)?,
                Box::new(self.stmt(then_stmt)?),
                else_stmt
                    .as_ref()
                    .map(|s| self.stmt(s).map(Box::new))
                    .transpose()?,
            ),
            S::Case { selector, arms } => {
                let e = self.plain(selector)?;
                let mut a = vec![];
                for arm in arms {
                    let mut labels = vec![];
                    for label in &arm.labels {
                        let v = self.plain(label)?;
                        check_conversion(e.ty, v.ty, label.span)?;
                        labels.push(v);
                    }
                    a.push((labels, self.stmt(&arm.body)?));
                }
                Stmt::Case(e, a)
            }
            S::For {
                init,
                condition,
                step,
                body,
            } => Stmt::For(
                Box::new(self.stmt(init)?),
                self.condition(condition)?,
                Box::new(self.stmt(step)?),
                Box::new(self.stmt(body)?),
            ),
            S::While { condition, body } => {
                Stmt::While(self.condition(condition)?, Box::new(self.stmt(body)?))
            }
            S::Repeat { count, body } => {
                Stmt::Repeat(self.condition(count)?, Box::new(self.stmt(body)?))
            }
            S::Event { events, body } => {
                let mut initial = false;
                let mut startup = false;
                let mut last = false;
                for e in events {
                    let n = match &e.kind {
                        A::Ident(n) => &n.name,
                        A::Call { name, args } if args.is_empty() => &name.name,
                        _ => return Err(unsupported(e.span, "unsupported analog event")),
                    };
                    if n == "final_step" {
                        last = true;
                    } else if matches!(n.as_str(), "initial_instance" | "initial_model") {
                        startup = true;
                    } else if n == "initial_step" {
                        initial = true;
                    } else {
                        return Err(unsupported(e.span, "unsupported analog event"));
                    }
                }
                Stmt::Event {
                    initial,
                    startup,
                    last,
                    body: Box::new(self.stmt(body)?),
                }
            }
            S::SysTask { name, args } => {
                if ![
                    "$strobe",
                    "$display",
                    "$write",
                    "$debug",
                    "$warning",
                    "$error",
                    "$finish",
                    "$fatal",
                    "$bound_step",
                    "$discontinuity",
                ]
                .contains(&name.name.as_str())
                {
                    return Err(unsupported(name.span, "unknown system task"));
                }
                Stmt::Task(
                    name.name.clone(),
                    args.iter().map(|e| self.plain(e)).collect::<R<_>>()?,
                )
            }
        })
    }
    fn expr(&mut self, e: &ast::Expr) -> R<Expr> {
        if self.depth >= 128 {
            return Err(unsupported(
                e.span,
                "semantic expression nesting exceeds 128",
            ));
        }
        self.depth += 1;
        let result = self.expression(e);
        self.depth -= 1;
        result
    }
    fn expression(&mut self, e: &ast::Expr) -> R<Expr> {
        let mut ty = Ty::Real;
        let mut parameter = true;
        let mut reactive = false;
        let kind = match &e.kind {
            A::List(_) => return Err(unsupported(e.span, "array literals require a noise table")),
            A::Port(_) => {
                return Err(error(
                    e.span,
                    "port branch terminal outside branch declaration",
                ));
            }
            A::Int(v) => {
                ty = Ty::Integer;
                E::Num(*v as f64)
            }
            A::Real(v) => E::Num(*v),
            A::Str(v) => {
                ty = Ty::String;
                E::Str(v.as_str().into())
            }
            A::Ident(n) => {
                if n.name == "inf" {
                    E::Num(f64::INFINITY)
                } else {
                    match self.symbols.get(&n.name) {
                        Some(Symbol::Param(i, t)) => {
                            ty = *t;
                            E::Param(*i)
                        }
                        Some(Symbol::Var(i, t)) => {
                            if self.arrays.contains(i) {
                                return Err(error(e.span, "array requires an index"));
                            }
                            ty = *t;
                            parameter = false;
                            E::Var(*i)
                        }
                        None => return Err(error(e.span, format!("undeclared name {}", n.name))),
                    }
                }
            }
            A::Index(a, b) => {
                let A::Ident(id) = &a.kind else {
                    return Err(error(e.span, "array base must be a variable name"));
                };
                let Some(Symbol::Var(i, t)) = self.symbols.get(&id.name).copied() else {
                    return Err(error(e.span, "index target must be an array variable"));
                };
                if !self.arrays.contains(&i) {
                    return Err(error(e.span, "scalar variable cannot be indexed"));
                }
                let b = self.plain(b)?;
                if b.ty == Ty::String {
                    return Err(error(e.span, "numeric index required"));
                }
                ty = t;
                parameter = false;
                E::Index(i, Box::new(b))
            }
            A::Unary { op, expr } => {
                let a = self.expr(expr)?;
                if a.ty == Ty::String {
                    return Err(error(e.span, "numeric unary operand required"));
                }
                if !matches!(op.as_str(), "+" | "-" | "!") && a.ty != Ty::Integer {
                    return Err(error(e.span, "bitwise operands must be integers"));
                }
                if a.reactive && !matches!(op.as_str(), "+" | "-") {
                    return Err(unsupported(e.span, "nonlinear ddt expression"));
                }
                ty = if op == "+" || op == "-" {
                    a.ty
                } else {
                    Ty::Integer
                };
                parameter = a.parameter;
                reactive = a.reactive;
                E::Unary(op.clone(), Box::new(a))
            }
            A::Binary { op, left, right } => {
                let a = self.expr(left)?;
                let b = self.expr(right)?;
                let cmp = matches!(
                    op.as_str(),
                    "==" | "!=" | "===" | "!==" | "<" | ">" | "<=" | ">="
                );
                if (a.ty == Ty::String || b.ty == Ty::String) && (!cmp || a.ty != b.ty) {
                    return Err(error(e.span, "strings may only be compared"));
                }
                if matches!(
                    op.as_str(),
                    "&" | "|" | "^" | "~^" | "^~" | "<<" | ">>" | "<<<" | ">>>"
                ) && (a.ty != Ty::Integer || b.ty != Ty::Integer)
                {
                    return Err(error(e.span, "bitwise operands must be integers"));
                }
                if a.reactive || b.reactive {
                    let valid = match op.as_str() {
                        "+" | "-" => true,
                        "*" => {
                            (a.reactive && b.parameter && !b.reactive)
                                || (b.reactive && a.parameter && !a.reactive)
                        }
                        "/" => a.reactive && b.parameter && !b.reactive,
                        _ => false,
                    };
                    if !valid {
                        return Err(unsupported(
                            e.span,
                            "ddt must be linear with parameter-only scaling",
                        ));
                    }
                }
                ty = if cmp
                    || matches!(
                        op.as_str(),
                        "&&" | "||" | "&" | "|" | "^" | "~^" | "^~" | "<<" | ">>" | "<<<" | ">>>"
                    )
                    || (a.ty == Ty::Integer && b.ty == Ty::Integer)
                {
                    Ty::Integer
                } else {
                    Ty::Real
                };
                if op == "%" && (a.ty != Ty::Integer || b.ty != Ty::Integer) {
                    return Err(error(e.span, "remainder requires integer operands"));
                }
                parameter = a.parameter && b.parameter;
                reactive = a.reactive || b.reactive;
                E::Binary(op.clone(), Box::new(a), Box::new(b))
            }
            A::Ternary { condition, yes, no } => {
                let c = self.condition(condition)?;
                let a = self.expr(yes)?;
                let b = self.expr(no)?;
                check_conversion(a.ty, b.ty, e.span)?;
                if a.reactive || b.reactive {
                    return Err(unsupported(e.span, "ddt inside ternary is unsupported"));
                }
                ty = if a.ty == b.ty { a.ty } else { Ty::Real };
                parameter = c.parameter && a.parameter && b.parameter;
                E::Select(Box::new(c), Box::new(a), Box::new(b))
            }
            A::Access {
                nature,
                args,
                port_flow,
            } => {
                if let Some(i) = self.port_probe(args, *port_flow) {
                    if nature.name != "I" {
                        return Err(unsupported(e.span, "port probes require flow access"));
                    }
                    return Ok(Expr {
                        kind: E::Current(i),
                        ty: Ty::Real,
                        parameter: false,
                        reactive: false,
                    });
                }
                if *port_flow {
                    return Err(error(e.span, "port current requires a module port"));
                }
                let (a, b) = self.endpoints(args, e.span)?;
                parameter = false;
                if self.potentials.contains(&nature.name) {
                    E::Voltage(a, b)
                } else {
                    let current = self.branch(args, e.span, true)?;
                    let branch = &mut self.branches[current];
                    branch.observed = true;
                    if branch.a == a && branch.b == b {
                        E::Current(current)
                    } else {
                        E::Unary(
                            "-".into(),
                            Box::new(Expr {
                                kind: E::Current(current),
                                ty: Ty::Real,
                                parameter: false,
                                reactive: false,
                            }),
                        )
                    }
                }
            }
            A::Call { name, args } | A::SysCall { name, args } => {
                let n = name.name.as_str();
                if matches!(n, "$param_given" | "$given") {
                    if args.len() != 1 {
                        return Err(error(e.span, "$param_given requires a parameter"));
                    }
                    let v = self.expr(&args[0])?;
                    let E::Param(i) = v.kind else {
                        return Err(error(e.span, "$param_given requires a parameter"));
                    };
                    ty = Ty::Integer;
                    E::Given(i)
                } else if n == "$port_connected" {
                    if args.len() != 1 {
                        return Err(error(e.span, "$port_connected requires a port"));
                    }
                    let A::Ident(port) = &args[0].kind else {
                        return Err(error(e.span, "$port_connected requires a port"));
                    };
                    let i = *self
                        .port_names
                        .get(&port.name)
                        .ok_or_else(|| error(e.span, "$port_connected requires a port"))?;
                    self.optional_ports[i] = true;
                    ty = Ty::Integer;
                    E::Connected(i)
                } else if n == "ddx" {
                    if args.len() != 2 {
                        return Err(error(e.span, "ddx requires two arguments"));
                    }
                    let v = self.plain(&args[0])?;
                    let A::Access {
                        nature,
                        args: a,
                        port_flow: false,
                    } = &args[1].kind
                    else {
                        return Err(unsupported(e.span, "ddx requires V(node)"));
                    };
                    if !self.potentials.contains(&nature.name) || a.len() != 1 {
                        return Err(unsupported(e.span, "ddx requires single-node V(node)"));
                    }
                    let i = self
                        .node(&a[0])?
                        .ok_or_else(|| unsupported(e.span, "ddx with respect to ground"))?;
                    parameter = false;
                    E::Ddx(Box::new(v), i)
                } else if n == "$limit" {
                    if self.in_function {
                        return Err(unsupported(
                            e.span,
                            "$limit inside functions is unsupported",
                        ));
                    }
                    if args.len() < 2 {
                        return Err(error(e.span, "$limit requires voltage and limiter name"));
                    }
                    let A::Access { nature, .. } = &args[0].kind else {
                        return Err(error(e.span, "$limit requires a voltage access"));
                    };
                    if !self.potentials.contains(&nature.name) {
                        return Err(error(e.span, "$limit requires V access"));
                    }
                    let A::Str(method) = &args[1].kind else {
                        return Err(error(e.span, "limiter name must be a string literal"));
                    };
                    let f = self.functions.get(method).copied();
                    let required = match method.as_str() {
                        "pnjlim" | "dummy" => Some(4),
                        "typedpnjlim_new" => Some(5),
                        "fetlim" => Some(3),
                        _ => None,
                    };
                    if let Some(k) = required {
                        if args.len() != k {
                            return Err(error(
                                e.span,
                                format!("{method} has wrong argument count"),
                            ));
                        }
                    } else if let Some(i) = f {
                        if self.signatures[i].len() != args.len() {
                            return Err(error(
                                e.span,
                                "user limiter requires (vnew, vold, args...)",
                            ));
                        }
                    } else {
                        return Err(unsupported(e.span, "unknown limiter"));
                    }
                    let mut a = vec![self.plain(&args[0])?];
                    for v in &args[2..] {
                        a.push(self.plain(v)?);
                    }
                    if a.iter().any(|v| v.ty == Ty::String) {
                        return Err(error(e.span, "limiter arguments must be numeric"));
                    }
                    let slot = self.states;
                    self.states += 1;
                    parameter = false;
                    E::Limit(slot, method.clone(), a, f)
                } else if let Some(i) = self.functions.get(n).copied() {
                    let formal = self.signatures[i].clone();
                    if formal.len() != args.len() {
                        return Err(error(e.span, "function argument count mismatch"));
                    }
                    let mut a = vec![];
                    for (v, (dir, t)) in args.iter().zip(formal) {
                        let actual = self.plain(v)?;
                        check_conversion(t, actual.ty, v.span)?;
                        if (dir == "output" || dir == "inout")
                            && !matches!(actual.kind, E::Var(_) | E::Index(..))
                        {
                            return Err(error(v.span, "output/inout arguments must be variables"));
                        }
                        if dir != "input"
                            && let E::Var(i) = actual.kind
                        {
                            self.function_writes.insert(i);
                        }
                        a.push(actual);
                    }
                    if let Some(c) = self.caller {
                        self.calls[c].insert(i);
                    }
                    ty = Ty::parse(&self.definitions[i].ty);
                    parameter = false;
                    E::Function(i, a)
                } else {
                    let arity = match n {
                        "ddt" | "exp" | "ln" | "log" | "sqrt" | "abs" | "floor" | "ceil"
                        | "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh"
                        | "tanh" | "asinh" | "acosh" | "atanh" | "limexp" | "analysis" => 1..=1,
                        "pow" | "min" | "max" | "atan2" | "hypot" => 2..=2,
                        "$temperature" | "$abstime" | "$mfactor" => 0..=0,
                        "$vt" => 0..=1,
                        "$simparam" => 1..=2,
                        "white_noise" => 1..=2,
                        "flicker_noise" => 2..=3,
                        "noise_table" => 1..=usize::MAX,
                        "idt" if args.len() == 2 => 2..=2,
                        "idt" | "cross" | "timer" => {
                            return Err(unsupported(e.span, format!("{n} is unsupported")));
                        }
                        n if n.starts_with("$rdist_") => 1..=usize::MAX,
                        _ => return Err(unsupported(e.span, format!("unknown function {n}"))),
                    };
                    if !arity.contains(&args.len()) {
                        return Err(error(e.span, format!("wrong argument count for {n}")));
                    }
                    let mut a = Vec::new();
                    for arg in args {
                        if n == "noise_table"
                            && let A::List(values) = &arg.kind
                        {
                            for value in values {
                                a.push(self.plain(value)?);
                            }
                        } else if n == "noise_table"
                            && let A::Ident(id) = &arg.kind
                            && let Some(&Symbol::Var(i, ty)) = self.symbols.get(&id.name)
                            && self.arrays.contains(&i)
                        {
                            a.push(Expr {
                                kind: E::Var(i),
                                ty,
                                parameter: false,
                                reactive: false,
                            });
                        } else {
                            a.push(self.plain(arg)?);
                        }
                    }
                    if n == "analysis" || n == "$simparam" {
                        if a.first().is_none_or(|v| v.ty != Ty::String) {
                            return Err(error(e.span, "first argument must be a string"));
                        }
                        if n == "$simparam" && a.get(1).is_some_and(|v| v.ty == Ty::String) {
                            return Err(error(e.span, "numeric default required"));
                        }
                    } else if !n.contains("noise")
                        && !n.starts_with("$rdist_")
                        && a.iter().any(|v| v.ty == Ty::String)
                    {
                        return Err(error(e.span, "numeric arguments required"));
                    }
                    parameter = a.iter().all(|v| v.parameter)
                        && (!n.starts_with('$') || n == "$simparam")
                        && !matches!(n, "analysis" | "idt")
                        && !n.contains("noise");
                    reactive = n == "ddt";
                    if reactive {
                        parameter = false;
                    }
                    if n == "analysis" {
                        ty = Ty::Integer;
                    }
                    if n.starts_with("$rdist_") {
                        E::Unsupported(n.into(), e.span)
                    } else if n == "idt" {
                        let mut args = a.into_iter();
                        let input = args
                            .next()
                            .ok_or_else(|| error(e.span, "missing integrand"))?;
                        let ic = args
                            .next()
                            .ok_or_else(|| error(e.span, "missing initial condition"))?;
                        if self.in_function {
                            return Err(unsupported(
                                e.span,
                                "idt in analog functions is unsupported",
                            ));
                        }
                        let slot = self.integral_count;
                        self.integral_count += 1;
                        E::Integral(slot, Box::new(input), Box::new(ic), e.span)
                    } else {
                        E::Call(n.into(), a)
                    }
                }
            }
        };
        Ok(Expr {
            kind,
            ty,
            parameter,
            reactive,
        })
    }
}
fn signature(f: &ast::AnalogFunction) -> Vec<(String, Ty)> {
    let types: BTreeMap<_, _> = f
        .declarations
        .iter()
        .filter_map(|d| match &d.kind {
            I::Var(v) => Some((&v.name.name, Ty::parse(&v.ty))),
            _ => None,
        })
        .collect();
    let mut args = Vec::new();
    for d in &f.declarations {
        if let I::PortDecl {
            direction,
            discipline,
            names,
        } = &d.kind
        {
            let ty = discipline.as_ref().map_or(Ty::Real, |d| Ty::parse(&d.name));
            for n in names {
                args.push((direction.clone(), types.get(&n.name).copied().unwrap_or(ty)));
            }
        }
    }
    args
}
#[derive(Default)]
struct Uses {
    pending: Vec<String>,
    names: BTreeSet<String>,
    probes: BTreeSet<String>,
}
impl Uses {
    fn visit(&mut self, name: &str) {
        if self.names.insert(name.into()) {
            self.pending.push(name.into());
        }
    }
}
fn collect_expr(e: &ast::Expr, n: &mut Uses) {
    let mut work = vec![e];
    while let Some(e) = work.pop() {
        match &e.kind {
            A::Ident(i) => {
                n.visit(&i.name);
            }
            A::Index(a, b)
            | A::Binary {
                left: a, right: b, ..
            } => {
                work.push(a);
                work.push(b);
            }
            A::Unary { expr, .. } => work.push(expr),
            A::Ternary { condition, yes, no } => {
                work.push(condition);
                work.push(yes);
                work.push(no);
            }
            A::Call { name, args } => {
                n.visit(&name.name);
                work.extend(args);
            }
            A::List(values) => work.extend(values),
            A::Port(p) => {
                n.probes.insert(p.name.clone());
            }
            A::Access {
                args,
                port_flow: true,
                ..
            } => {
                if let [
                    ast::Expr {
                        kind: A::Ident(id), ..
                    },
                ] = args.as_slice()
                {
                    n.probes.insert(id.name.clone());
                }
                work.extend(args);
            }
            A::SysCall { args, .. } | A::Access { args, .. } => work.extend(args),
            _ => {}
        }
    }
}
fn collect_stmt(s: &ast::Stmt, n: &mut Uses) {
    match &s.kind {
        S::Block {
            statements,
            declarations,
            ..
        } => {
            for d in declarations {
                if let I::Var(v) = &d.kind
                    && let Some(e) = &v.value
                {
                    collect_expr(e, n);
                }
            }
            for s in statements {
                collect_stmt(s, n);
            }
        }
        S::Assign { lhs, rhs } | S::Contrib { access: lhs, rhs } => {
            collect_expr(lhs, n);
            collect_expr(rhs, n);
        }
        S::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_expr(condition, n);
            collect_stmt(then_stmt, n);
            if let Some(s) = else_stmt {
                collect_stmt(s, n);
            }
        }
        S::Case { selector, arms } => {
            collect_expr(selector, n);
            for a in arms {
                for e in &a.labels {
                    collect_expr(e, n);
                }
                collect_stmt(&a.body, n);
            }
        }
        S::For {
            init,
            condition,
            step,
            body,
        } => {
            collect_stmt(init, n);
            collect_expr(condition, n);
            collect_stmt(step, n);
            collect_stmt(body, n);
        }
        S::While { condition, body }
        | S::Repeat {
            count: condition,
            body,
        } => {
            collect_expr(condition, n);
            collect_stmt(body, n);
        }
        S::Event { body, .. } => collect_stmt(body, n),
        S::SysTask { args, .. } => {
            for e in args {
                collect_expr(e, n);
            }
        }
        S::Empty => {}
    }
}
