//! Elaboration: netlist → compiled circuit (unknowns, device groups, fixed matrix pattern).
//!
//! Provenance: original work. Topology checks (no DC path to ground, loops of voltage-defined
//! branches) are standard MNA well-posedness conditions; see Ho, Ruehli & Brennan (1975).
//! See `docs/PROVENANCE.md`.

use spx_devices::va::VaGroup;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use spx_devices::bjt::BjtModel;
use spx_devices::diode::DiodeModel;
use spx_devices::mos1::{Mos1Geometry, Mos1Model};
use spx_devices::{
    Bjts, Capacitors, Cccs, Ccvs, CurrentSources, DeviceGroup, Diodes, Inductors, ModelParams,
    Mos1s, Node, Resistors, Slots, StampBuilder, StampLayout, Unknowns, Vccs, Vcvs, VoltageSources,
};
use spx_diag::Diagnostic;
use spx_netlist::{Element, ElementKind, GROUND, ModelCard, Netlist};
use spx_sparse::{CscPattern, PatternBuilder};

use crate::options::SimOptions;

/// Marker for slots that touch ground and are discarded.
pub(crate) const DISCARD: usize = usize::MAX;

/// A compiled, immutable circuit.
#[derive(Debug)]
pub struct Circuit {
    pub(crate) va_modules: BTreeMap<String, Arc<spx_va::CompiledModule>>,
    pub(crate) unknowns: Unknowns,
    pub(crate) aliases: Vec<(String, Node)>,
    pub(crate) num_nodes: usize,
    pub(crate) groups: Vec<Box<dyn DeviceGroup>>,
    pub(crate) layouts: Vec<StampLayout>,
    pub(crate) pattern: CscPattern,
    /// Per unknown: value index of the diagonal if it is a node voltage, else [`DISCARD`]
    /// (for rshunt / gmin stepping).
    pub(crate) node_diag: Vec<usize>,
    pub(crate) linear: bool,
    /// Nodes without a DC path to ground (for diagnostics).
    pub(crate) no_dc_path: Vec<String>,
    /// Per group: number of state values.
    pub(crate) state_len: Vec<usize>,
    /// Gather maps from group slots to assembled values (parallelism level L2).
    pub(crate) gather: GatherMaps,
    /// Whether the circuit is large enough for parallel evaluation to pay off.
    pub(crate) parallel: bool,
    /// Per external node: names of the attached elements, nonlinear devices first.
    pub(crate) node_elements: Vec<Vec<String>>,
    /// `.ic` values as (unknown index, voltage).
    pub(crate) ics: Vec<(usize, f64)>,
    /// `.nodeset` values as (unknown index, voltage).
    pub(crate) nodesets: Vec<(usize, f64)>,
}

/// A source of one assembled value: `(group, slot)`.
pub(crate) type Src = (u32, u32);

/// For every destination index, the contributing slots in a fixed order (CSR layout).
///
/// Every destination is the sum of its sources, accumulated left to right starting from zero.
/// The order is fixed at elaboration, so the floating-point result does not depend on how the
/// destinations are distributed over threads (architecture D4).
#[derive(Debug, Default)]
pub(crate) struct Gather {
    pub ptr: Vec<usize>,
    pub src: Vec<Src>,
}

impl Gather {
    /// Build from `(group, slot, destination)` triples given in accumulation order.
    fn build(len: usize, entries: impl Iterator<Item = (usize, usize, usize)> + Clone) -> Self {
        let mut ptr = vec![0usize; len + 1];
        for (_, _, d) in entries.clone() {
            ptr[d + 1] += 1;
        }
        for i in 0..len {
            ptr[i + 1] += ptr[i];
        }
        let mut fill = ptr.clone();
        let mut src = vec![(0u32, 0u32); ptr[len]];
        for (g, k, d) in entries {
            src[fill[d]] = (g as u32, k as u32);
            fill[d] += 1;
        }
        Self { ptr, src }
    }

    /// Sources of destination `i`.
    #[inline]
    pub fn of(&self, i: usize) -> &[Src] {
        &self.src[self.ptr[i]..self.ptr[i + 1]]
    }
}

/// Gather maps for `G`, `C` (pattern values) and `f`, `q`, `b` (unknowns).
#[derive(Debug, Default)]
pub(crate) struct GatherMaps {
    pub g: Gather,
    pub c: Gather,
    pub f: Gather,
    pub q: Gather,
    pub b: Gather,
}

/// Instances per chunk of a linear device group.
const CHUNK_LINEAR: usize = 1024;
/// Instances per chunk of a nonlinear device group.
const CHUNK_NONLINEAR: usize = 64;
/// Interpreted models have substantially more work per instance than built-in devices.
const CHUNK_VA: usize = 2;
const PARALLEL_MIN_VA: usize = 8;
/// Minimum number of nonlinear instances for parallel evaluation.
const PARALLEL_MIN_NONLINEAR: usize = 256;

/// A device group split into fixed-size chunks (the unit of parallel evaluation, L1).
///
/// Chunk boundaries depend only on the netlist, never on the thread count.
struct Chunked<T> {
    size: usize,
    done: Vec<T>,
    cur: T,
}

impl<T: DeviceGroup + Default + 'static> Chunked<T> {
    fn new(size: usize) -> Self {
        Self {
            size,
            done: Vec::new(),
            cur: T::default(),
        }
    }

    fn cur(&mut self) -> &mut T {
        if self.cur.len() >= self.size {
            self.done.push(std::mem::take(&mut self.cur));
        }
        &mut self.cur
    }

    fn into_groups(self, out: &mut Vec<Box<dyn DeviceGroup>>) {
        for g in self.done.into_iter().chain([self.cur]) {
            if !g.is_empty() {
                out.push(Box::new(g));
            }
        }
    }
}

impl Circuit {
    /// Unknowns (node voltages, then branch currents).
    pub fn unknowns(&self) -> &Unknowns {
        &self.unknowns
    }

    /// Number of node-voltage unknowns (they come first).
    pub fn num_nodes(&self) -> usize {
        self.num_nodes
    }

    /// Number of unknowns.
    pub fn dim(&self) -> usize {
        self.unknowns.len()
    }

    /// Elements attached to unknown `index` (external node or device-internal node).
    pub(crate) fn devices_at(&self, index: usize) -> Vec<String> {
        if let Some(v) = self.node_elements.get(index) {
            return v.clone();
        }
        // Internal nodes are named `v(<device>#<suffix>)`.
        self.unknowns
            .names()
            .get(index)
            .and_then(|n| n.strip_prefix("v("))
            .and_then(|n| n.split_once('#'))
            .map(|(dev, _)| vec![dev.to_string()])
            .unwrap_or_default()
    }

    /// Number of device instances per device type, in group order.
    pub fn device_counts(&self) -> Vec<(&'static str, usize)> {
        let mut out: Vec<(&'static str, usize)> = Vec::new();
        for g in &self.groups {
            match out.last_mut() {
                Some((t, n)) if *t == g.type_name() => *n += g.len(),
                _ => out.push((g.type_name(), g.len())),
            }
        }
        out
    }

    /// Number of structurally non-zero matrix entries.
    pub fn nnz(&self) -> usize {
        self.pattern.nnz()
    }

    /// Whether every device is linear.
    pub fn is_linear(&self) -> bool {
        self.linear
    }

    /// Allocate evaluation slots for every group.
    pub(crate) fn new_slots(&self) -> Vec<Slots> {
        self.layouts.iter().map(Slots::for_layout).collect()
    }

    /// Set an instance parameter on whichever group owns the instance.
    pub(crate) fn set_param(&mut self, instance: &str, param: &str, value: f64) -> bool {
        self.groups
            .iter_mut()
            .any(|g| g.set_param(instance, param, value))
    }

    /// Read an instance parameter.
    pub(crate) fn param(&self, instance: &str, param: &str) -> Option<f64> {
        self.groups.iter().find_map(|g| g.param(instance, param))
    }
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, mut a: usize) -> usize {
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }
    /// Returns false if `a` and `b` were already connected.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        self.0[ra] = rb;
        ra != rb
    }
}

/// Include loader rooted at the HDL file; synthetic source loaders get built-ins only.
struct HdlLoader<'a> {
    path: &'a str,
    missing: std::cell::RefCell<Option<std::path::PathBuf>>,
}
impl<'a> HdlLoader<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            missing: Default::default(),
        }
    }

    fn resolve(&self, path: &str, from: Option<&str>) -> std::path::PathBuf {
        let from = from.filter(|p| *p != "<input>").unwrap_or(self.path);
        let base = std::path::Path::new(from)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        base.join(path)
    }
}

/// Files read by the VA preprocessor, for content-addressed black-box compilation.
/// Built-in standard headers are supplied by each compiler. Missing user inputs
/// return an I/O error of kind NotFound with the resolved include path.
pub fn hdl_files(h: &spx_netlist::HdlSource) -> std::io::Result<Vec<std::path::PathBuf>> {
    use std::{io, path::PathBuf};
    let path = std::fs::canonicalize(&h.path)?;
    let name = path.to_string_lossy();
    let loader = HdlLoader::new(&name);
    let (expanded, diagnostics) = spx_va::preproc::preprocess(&h.text, &loader);
    if let Some(missing) = loader.missing.into_inner() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            missing.display().to_string(),
        ));
    }
    if let Some(d) = diagnostics.iter().find(|d| d.is_error()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, d.to_string()));
    }
    let mut files = vec![path];
    files.extend(
        expanded
            .sources
            .into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute()),
    );
    files.sort();
    files.dedup();
    Ok(files)
}
impl spx_va::VaLoader for HdlLoader<'_> {
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        self.standard_header(path, from)?.ok_or_else(|| {
            let full = self.resolve(path, from);
            let error = format!("include {} not found", full.display());
            self.missing.replace(Some(full));
            error
        })
    }
    fn standard_header(
        &self,
        path: &str,
        from: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        if !std::path::Path::new(self.path).is_file() {
            return Ok(None);
        }
        let full = self.resolve(path, from);
        match std::fs::read_to_string(&full) {
            Ok(text) => Ok(Some((full.display().to_string(), text))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", full.display())),
        }
    }
}

fn hdl_diagnostic(mut d: Diagnostic, h: &spx_netlist::HdlSource, sources: &[String]) -> Diagnostic {
    if let Some(span) = d.span {
        let path = if span.file == 0 {
            h.path.as_str()
        } else {
            sources
                .get(span.file as usize)
                .map_or("VA include", String::as_str)
        };
        d = d.with_note(format!("Verilog-A {path}:{}:{}", span.line, span.col));
    }
    d.with_span(h.span)
}

fn compile_hdl(
    nl: &Netlist,
) -> Result<BTreeMap<String, Arc<spx_va::CompiledModule>>, Vec<Diagnostic>> {
    let mut modules = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    for h in &nl.hdl {
        if !seen.insert(&h.path) {
            continue;
        }
        match spx_va::parse_va(&h.text, &HdlLoader::new(&h.path)) {
            Err(ds) => errors.extend(ds.into_iter().map(|d| hdl_diagnostic(d, h, &[]))),
            Ok(source) => {
                for module in &source.modules {
                    match spx_va::compile_module(&source, Some(&module.name.name)) {
                        Ok(compiled) => {
                            let key = compiled.name.to_ascii_lowercase();
                            if let std::collections::btree_map::Entry::Vacant(entry) =
                                modules.entry(key)
                            {
                                entry.insert(Arc::new(compiled));
                            } else {
                                errors.push(hdl_diagnostic(
                                    Diagnostic::error(
                                        "E-VA-SEMA",
                                        format!("duplicate module {}", compiled.name),
                                    )
                                    .with_span(module.span),
                                    h,
                                    &source.sources,
                                ));
                            }
                        }
                        Err(ds) => errors.extend(
                            ds.into_iter()
                                .map(|d| hdl_diagnostic(d, h, &source.sources)),
                        ),
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(modules)
    } else {
        Err(errors)
    }
}

/// Compile a netlist. Returns the circuit and warnings, or all diagnostics on failure.
pub fn compile(
    nl: &Netlist,
    opts: &SimOptions,
) -> Result<(Circuit, Vec<Diagnostic>), Vec<Diagnostic>> {
    let va_modules = compile_hdl(nl)?;
    compile_cached(nl, opts, &va_modules)
}

/// Re-elaborate with immutable compiled modules shared by all analyses of this run.
pub(crate) fn compile_cached(
    nl: &Netlist,
    opts: &SimOptions,
    va_modules: &BTreeMap<String, Arc<spx_va::CompiledModule>>,
) -> Result<(Circuit, Vec<Diagnostic>), Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    // --- Node numbering (order of first appearance, ground excluded) ---
    let mut node_index: HashMap<String, usize> = HashMap::new();
    let mut node_names: Vec<String> = Vec::new();
    let mut touches = Vec::<usize>::new();
    let mut grounded = false;
    for e in &nl.elements {
        for n in e.kind.nodes() {
            if n == GROUND {
                grounded = true;
                continue;
            }
            let idx = *node_index.entry(n.to_string()).or_insert_with(|| {
                node_names.push(n.to_string());
                touches.push(0);
                node_names.len() - 1
            });
            touches[idx] += 1;
        }
    }
    if nl.elements.is_empty() {
        return Err(vec![Diagnostic::error(
            "E-ELAB-EMPTY",
            "the netlist has no elements",
        )]);
    }
    let mut va: BTreeMap<String, Chunked<VaGroup>> = BTreeMap::new();
    let swept: HashSet<_> = nl
        .analyses
        .iter()
        .filter_map(|card| match card {
            spx_netlist::AnalysisCard::Dc { source, .. } => Some(source.as_str()),
            _ => None,
        })
        .collect();
    let mut va_models: HashMap<String, Arc<spx_va::Bound>> = HashMap::new();
    let mut nonlinear_count = 0usize;

    let mut errors = Vec::new();
    for e in &nl.elements {
        if let ElementKind::Va {
            nodes,
            model,
            params,
        } = &e.kind
        {
            let Some(card) = nl.models.iter().find(|m| m.name == *model) else {
                errors.push(
                    Diagnostic::error("E-ELAB-PARAM", format!("{}: unknown model {model}", e.name))
                        .with_span(e.span),
                );
                continue;
            };
            let vbic = matches!(card.kind.as_str(), "npn" | "pnp")
                && card.params.iter().any(|(k, v)| k == "level" && *v == 9.);
            let module_name = if vbic { "vbic13_5t" } else { &card.kind };
            let Some(module) = va_modules.get(module_name).or_else(|| {
                card.kind
                    .strip_suffix("_va")
                    .and_then(|base| va_modules.get(&format!("{base}va")))
            }) else {
                errors.push(
                    Diagnostic::error(
                        "E-ELAB-PARAM",
                        format!("{}: unknown Verilog-A module {module_name}", e.name),
                    )
                    .with_span(card.span),
                );
                continue;
            };
            let bound = if let Some(bound) = va_models.get(model) {
                Arc::clone(bound)
            } else {
                let mut values = card.params.clone();
                if vbic {
                    // level selects the device. The three SOA limits are ngspice checks.
                    values.retain(|(k, _)| {
                        !matches!(k.as_str(), "level" | "vbe_max" | "vce_max" | "vbc_max")
                    });
                    values.push(("type".into(), if card.kind == "pnp" { -1. } else { 1. }));
                }
                match module.bind_spice_model(
                    &values,
                    spx_va::BindOptions {
                        tnom: opts.tnom,
                        gmin: opts.gmin,
                    },
                ) {
                    Ok(bound) => {
                        let bound = Arc::new(bound);
                        va_models.insert(model.clone(), Arc::clone(&bound));
                        bound
                    }
                    Err(ds) => {
                        errors.extend(ds.into_iter().map(|d| d.with_span(card.span)));
                        continue;
                    }
                }
            };
            let group = va
                .entry(card.kind.clone())
                .or_insert_with(|| Chunked::new(CHUNK_VA));
            let chunk = group.cur();
            match chunk.push(
                Arc::clone(module),
                &e.name,
                nodes.iter().map(|n| node_index.get(n).copied()).collect(),
                bound,
                params,
            ) {
                Ok(()) => {
                    nonlinear_count += 1;
                    if swept.contains(e.name.as_str()) {
                        chunk.prepare_sweep(&e.name);
                    }
                    #[cfg(feature = "jit")]
                    if let Some(d) = chunk.prepare_jit(opts.va_jit, opts.temp + 273.15) {
                        diags.push(d.with_span(e.span));
                    }
                }
                Err(ds) => errors.extend(ds.into_iter().map(|d| d.with_span(e.span))),
            }
        }
    }
    let va_count = nonlinear_count;
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut aliases = Vec::new();
    if !va.is_empty() {
        let ground = node_names.len();
        let mut sets = UnionFind::new(ground + 1);
        for group in va
            .values()
            .flat_map(|chunks| chunks.done.iter().chain([&chunks.cur]))
        {
            for (a, b) in group.terminal_shorts() {
                grounded |= a.is_none() || b.is_none();
                sets.union(a.unwrap_or(ground), b.unwrap_or(ground));
            }
        }
        let ground_root = sets.find(ground);
        let mut new_names = Vec::new();
        let mut counts = Vec::new();
        let mut roots = HashMap::new();
        let mut indices = Vec::new();
        for (i, name) in node_names.iter().enumerate() {
            let root = sets.find(i);
            let index = if root == ground_root {
                None
            } else {
                Some(*roots.entry(root).or_insert_with(|| {
                    new_names.push(name.clone());
                    counts.push(0);
                    new_names.len() - 1
                }))
            };
            indices.push(index);
            if let Some(k) = index {
                counts[k] += touches[i];
                node_index.insert(name.clone(), k);
            } else {
                node_index.remove(name);
            }
            if index.is_none_or(|k| new_names[k] != *name) {
                aliases.push((format!("v({name})"), index));
            }
        }
        for group in va
            .values_mut()
            .flat_map(|chunks| chunks.done.iter_mut().chain([&mut chunks.cur]))
        {
            group.remap_ports(&indices).map_err(|d| vec![d])?;
        }
        node_names = new_names;
        touches = counts;
    }
    if !grounded {
        return Err(vec![
            Diagnostic::error(
                "E-ELAB-NOGROUND",
                "no element is connected to ground (node 0)",
            )
            .with_note("every circuit needs a reference node named `0` (or `gnd`)"),
        ]);
    }
    for (i, &t) in touches.iter().enumerate() {
        if t < 2 {
            diags.push(
                Diagnostic::warning(
                    "W-ELAB-DANGLING",
                    format!("node `{}` is connected to only one terminal", node_names[i]),
                )
                .with_note("a dangling node usually indicates a typo in a node name"),
            );
        }
    }
    let mut node_elements: Vec<Vec<String>> = vec![Vec::new(); node_names.len()];
    for nonlinear in [true, false] {
        for e in &nl.elements {
            if e.kind.model().is_some() != nonlinear {
                continue;
            }
            for n in e.kind.nodes() {
                if let Some(&i) = node_index.get(n)
                    && !node_elements[i].contains(&e.name)
                {
                    node_elements[i].push(e.name.clone());
                }
            }
        }
    }
    let node = |name: &str| -> Node {
        if name == GROUND {
            None
        } else {
            node_index.get(name).copied()
        }
    };

    // --- Initial conditions and node sets ---
    let mut node_values = |list: &[spx_netlist::NodeValue], what: &str| -> Vec<(usize, f64)> {
        let mut out = Vec::new();
        for v in list {
            match node_index.get(v.node.as_str()) {
                Some(&i) => out.push((i, v.value)),
                None if v.node == GROUND => {}
                None => diags.push(
                    Diagnostic::warning(
                        "W-ELAB-NODE",
                        format!("{what} refers to unknown node `{}`; ignored", v.node),
                    )
                    .with_span(v.span),
                ),
            }
        }
        out
    };
    let ics = node_values(&nl.initial_conditions, ".ic");
    let nodesets = node_values(&nl.nodesets, ".nodeset");

    // --- Topology checks: DC path to ground, loops of voltage-defined branches ---
    let nn = node_names.len();
    let g_id = |n: Node| n.unwrap_or(nn);
    let mut dc = UnionFind::new(nn + 1);
    let mut vloop = UnionFind::new(nn + 1);
    for e in &nl.elements {
        let (p, n) = {
            let ns = e.kind.nodes();
            (
                ns.first().map_or(nn, |n| g_id(node(n))),
                ns.get(1).map_or(nn, |n| g_id(node(n))),
            )
        };
        match &e.kind {
            ElementKind::Va { nodes, .. } => {
                if let Some(first) = nodes.first() {
                    for n in &nodes[1..] {
                        dc.union(g_id(node(first)), g_id(node(n)));
                    }
                }
            }
            ElementKind::Resistor { .. } | ElementKind::Diode { .. } => {
                dc.union(p, n);
            }
            ElementKind::Bjt { .. } => {
                // Collector, base and emitter are connected through junctions (gmin).
                let b = g_id(node(e.kind.nodes()[2]));
                dc.union(p, n);
                dc.union(n, b);
            }
            ElementKind::Mosfet { d, s, b, .. } => {
                // The gate has no DC path; drain/source/bulk are connected via the junctions.
                let (d, s, b) = (g_id(node(d)), g_id(node(s)), g_id(node(b)));
                dc.union(d, b);
                dc.union(s, b);
            }
            ElementKind::Inductor { .. }
            | ElementKind::VoltageSource { .. }
            | ElementKind::Vcvs { .. }
            | ElementKind::Ccvs { .. } => {
                dc.union(p, n);
                if !vloop.union(p, n) {
                    diags.push(
                        Diagnostic::warning(
                            "W-ELAB-VLOOP",
                            format!(
                                "`{}` closes a loop of voltage sources and/or inductors",
                                e.name
                            ),
                        )
                        .with_span(e.span)
                        .with_note("such loops make the circuit matrix singular"),
                    );
                }
            }
            _ => {}
        }
    }
    if opts.rshunt.is_some() {
        for i in 0..nn {
            dc.union(i, nn);
        }
    }
    let ground_root = dc.find(nn);
    let no_dc_path: Vec<String> = (0..nn)
        .filter(|&i| dc.find(i) != ground_root)
        .map(|i| node_names[i].clone())
        .collect();
    if !no_dc_path.is_empty() {
        diags.push(
            Diagnostic::warning(
                "W-ELAB-NODC",
                format!(
                    "no DC path to ground from node(s): {}",
                    no_dc_path.join(", ")
                ),
            )
            .with_note("add a resistor to ground or set `.options rshunt=<large value>`"),
        );
    }

    // --- Device groups ---
    let mut res = Chunked::<Resistors>::new(CHUNK_LINEAR);
    let mut cap = Chunked::<Capacitors>::new(CHUNK_LINEAR);
    let mut ind = Chunked::<Inductors>::new(CHUNK_LINEAR);
    let mut vsrc = Chunked::<VoltageSources>::new(CHUNK_LINEAR);
    let mut isrc = Chunked::<CurrentSources>::new(CHUNK_LINEAR);
    let mut e_src = Chunked::<Vcvs>::new(CHUNK_LINEAR);
    let mut g_src = Chunked::<Vccs>::new(CHUNK_LINEAR);
    let mut f_src = Chunked::<Cccs>::new(CHUNK_LINEAR);
    let mut h_src = Chunked::<Ccvs>::new(CHUNK_LINEAR);
    let mut diodes = Chunked::<Diodes>::new(CHUNK_NONLINEAR);
    let mut bjts = Chunked::<Bjts>::new(CHUNK_NONLINEAR);
    let mut mos1 = Chunked::<Mos1s>::new(CHUNK_NONLINEAR);
    let mut model_cache = ModelCache::new(&nl.models, opts);
    for e in &nl.elements {
        match &e.kind {
            ElementKind::Va { .. } => {}
            ElementKind::Resistor { p, n, value } => {
                res.cur().push(&e.name, node(p), node(n), *value)
            }
            ElementKind::Capacitor { p, n, value } => {
                cap.cur().push(&e.name, node(p), node(n), *value)
            }
            ElementKind::Inductor { p, n, value } => {
                ind.cur().push(&e.name, node(p), node(n), *value)
            }
            ElementKind::VoltageSource { p, n, source } => {
                vsrc.cur().push(&e.name, node(p), node(n), source.clone());
            }
            ElementKind::CurrentSource { p, n, source } => {
                isrc.cur().push(&e.name, node(p), node(n), source.clone());
            }
            ElementKind::Vcvs { p, n, cp, cn, gain } => {
                e_src
                    .cur()
                    .push(&e.name, [node(p), node(n), node(cp), node(cn)], *gain);
            }
            ElementKind::Vccs { p, n, cp, cn, gm } => {
                g_src
                    .cur()
                    .push(&e.name, [node(p), node(n), node(cp), node(cn)], *gm);
            }
            ElementKind::Cccs {
                p,
                n,
                control,
                gain,
            } => {
                f_src.cur().push(&e.name, node(p), node(n), control, *gain);
            }
            ElementKind::Ccvs { p, n, control, r } => {
                h_src.cur().push(&e.name, node(p), node(n), control, *r);
            }
            ElementKind::Diode {
                p,
                n,
                model,
                params,
                off,
            } => {
                let Some(m) = model_cache.diode(model, &mut diags, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                let area = inst.get("area", 1.0);
                warn_instance(e, &inst, &mut diags);
                nonlinear_count += 1;
                diodes.cur().push(&e.name, node(p), node(n), m, area, *off);
            }
            ElementKind::Bjt {
                c,
                b,
                e: em,
                s,
                model,
                params,
                off,
            } => {
                let Some(m) = model_cache.bjt(model, &mut diags, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                let area = inst.get("area", 1.0);
                warn_instance(e, &inst, &mut diags);
                let sub = s.as_deref().and_then(node);
                nonlinear_count += 1;
                bjts.cur()
                    .push(&e.name, [node(c), node(b), node(em), sub], m, area, *off);
            }
            ElementKind::Mosfet {
                d,
                g,
                s,
                b,
                model,
                params,
                off,
            } => {
                let Some(m) = model_cache.mos(model, &mut diags, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                let dflt = Mos1Geometry::default();
                let geom = Mos1Geometry {
                    l: inst.get("l", opts.defl.unwrap_or(dflt.l)),
                    w: inst.get("w", opts.defw.unwrap_or(dflt.w)),
                    ad: inst.get("ad", opts.defad.unwrap_or(0.0)),
                    as_: inst.get("as", opts.defas.unwrap_or(0.0)),
                    pd: inst.get("pd", 0.0),
                    ps: inst.get("ps", 0.0),
                };
                warn_instance(e, &inst, &mut diags);
                if let Some(msg) = Mos1s::check_geometry(&m, &geom) {
                    errors.push(
                        Diagnostic::error("E-ELAB-PARAM", format!("`{}`: {msg}", e.name))
                            .with_span(e.span),
                    );
                    continue;
                }
                nonlinear_count += 1;
                mos1.cur()
                    .push(&e.name, [node(d), node(g), node(s), node(b)], m, geom, *off);
            }
        }
    }
    if !errors.is_empty() {
        errors.extend(diags);
        return Err(errors);
    }
    let mut groups: Vec<Box<dyn DeviceGroup>> = Vec::new();
    res.into_groups(&mut groups);
    cap.into_groups(&mut groups);
    ind.into_groups(&mut groups);
    vsrc.into_groups(&mut groups);
    isrc.into_groups(&mut groups);
    e_src.into_groups(&mut groups);
    g_src.into_groups(&mut groups);
    f_src.into_groups(&mut groups);
    h_src.into_groups(&mut groups);
    diodes.into_groups(&mut groups);
    bjts.into_groups(&mut groups);
    mos1.into_groups(&mut groups);
    for group in va.into_values() {
        group.into_groups(&mut groups);
    }

    // --- Unknowns and stamps ---
    let mut unknowns = Unknowns::with_nodes(node_names.iter().map(String::as_str));
    for g in &mut groups {
        g.alloc_unknowns(&mut unknowns);
    }
    let mut layouts = Vec::with_capacity(groups.len());
    for g in &mut groups {
        let mut sb = StampBuilder::new(&unknowns);
        match g.setup(&mut sb) {
            Ok(()) => layouts.push(sb.finish()),
            Err(d) => {
                errors.push(d);
                layouts.push(StampLayout::default());
            }
        }
    }
    if !errors.is_empty() {
        errors.extend(diags);
        return Err(errors);
    }

    let n = unknowns.len();
    let mut pb = PatternBuilder::new(n);
    let req = |l: &[(Node, Node)], pb: &mut PatternBuilder| -> Vec<usize> {
        l.iter()
            .map(|&(r, c)| match (r, c) {
                (Some(r), Some(c)) => pb.add(r, c),
                _ => DISCARD,
            })
            .collect()
    };
    let greq: Vec<Vec<usize>> = layouts.iter().map(|l| req(&l.g, &mut pb)).collect();
    let creq: Vec<Vec<usize>> = layouts.iter().map(|l| req(&l.c, &mut pb)).collect();
    let (pattern, map) = pb.build();
    let resolve = |reqs: Vec<Vec<usize>>| -> Vec<Vec<usize>> {
        reqs.into_iter()
            .map(|v| {
                v.into_iter()
                    .map(|r| if r == DISCARD { DISCARD } else { map[r] })
                    .collect()
            })
            .collect()
    };
    let gmap = resolve(greq);
    let cmap = resolve(creq);
    let slots_to = |maps: &[Vec<usize>]| {
        maps.iter()
            .enumerate()
            .flat_map(|(g, m)| m.iter().enumerate().map(move |(k, &d)| (g, k, d)))
            .filter(|&(_, _, d)| d != DISCARD)
            .collect::<Vec<_>>()
    };
    let rows_to = |pick: fn(&StampLayout) -> &[Node]| {
        layouts
            .iter()
            .enumerate()
            .flat_map(move |(g, l)| {
                pick(l)
                    .iter()
                    .enumerate()
                    .filter_map(move |(k, r)| r.map(|r| (g, k, r)))
            })
            .collect::<Vec<_>>()
    };
    let nnz = pattern.nnz();
    let gather = GatherMaps {
        g: Gather::build(nnz, slots_to(&gmap).into_iter()),
        c: Gather::build(nnz, slots_to(&cmap).into_iter()),
        f: Gather::build(n, rows_to(|l| &l.f).into_iter()),
        q: Gather::build(n, rows_to(|l| &l.q).into_iter()),
        b: Gather::build(n, rows_to(|l| &l.b).into_iter()),
    };
    // Diagonal of every node voltage, including internal device nodes.
    let node_diag = (0..n)
        .map(|i| {
            if unknowns.kinds()[i] == spx_devices::UnknownKind::Voltage {
                pattern.diag(i).unwrap_or(DISCARD)
            } else {
                DISCARD
            }
        })
        .collect();
    let state_len = groups.iter().map(|g| g.state_len()).collect();
    let linear = groups.iter().all(|g| g.is_linear());

    Ok((
        Circuit {
            va_modules: va_modules.clone(),
            unknowns,
            aliases,
            num_nodes: nn,
            groups,
            layouts,
            pattern,
            node_diag,
            linear,
            no_dc_path,
            state_len,
            gather,
            parallel: va_count >= PARALLEL_MIN_VA || nonlinear_count >= PARALLEL_MIN_NONLINEAR,
            node_elements,
            ics,
            nodesets,
        },
        diags,
    ))
}

/// Resolves `.model` cards into typed models once per model name, emitting warnings once.
struct ModelCache<'a> {
    cards: &'a [ModelCard],
    temp_differs: bool,
    diodes: HashMap<String, Option<DiodeModel>>,
    bjts: HashMap<String, Option<BjtModel>>,
    mos: HashMap<String, Option<Mos1Model>>,
}

impl<'a> ModelCache<'a> {
    fn new(cards: &'a [ModelCard], opts: &SimOptions) -> Self {
        Self {
            cards,
            temp_differs: (opts.temp - opts.tnom).abs() > 1e-9,
            diodes: HashMap::new(),
            bjts: HashMap::new(),
            mos: HashMap::new(),
        }
    }

    fn card(&self, name: &str) -> Option<&'a ModelCard> {
        self.cards.iter().find(|c| c.name == name)
    }

    /// Common checks; returns the parameter list and polarity.
    fn prepare(
        &self,
        card: &ModelCard,
        diags: &mut Vec<Diagnostic>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<(ModelParams, f64)> {
        let params = ModelParams::new(&card.params);
        let level = params.get("level", 1.0);
        if level != 1.0 {
            let mut d = Diagnostic::error(
                "E-ELAB-MODEL",
                format!(
                    "model `{}`: level {level} is not supported (only level 1)",
                    card.name
                ),
            )
            .with_span(card.span);
            if matches!(card.kind.as_str(), "nmos" | "pmos") {
                d = d.with_note("advanced MOSFET models will be supported through OSDI (planned)");
            }
            errors.push(d);
            return None;
        }
        if let Some(t) = params.opt(&["tnom"])
            && (t - 27.0).abs() > 1e-9
        {
            diags.push(
                Diagnostic::warning(
                    "W-MODEL-TEMP",
                    format!(
                        "model `{}`: TNOM = {t} is not supported yet; parameters are used as \
                         given at the simulation temperature",
                        card.name
                    ),
                )
                .with_span(card.span),
            );
        }
        if self.temp_differs {
            diags.push(
                Diagnostic::warning(
                    "W-MODEL-TEMP",
                    format!(
                        "model `{}`: temperature scaling is not implemented yet; parameters are \
                         not adjusted to TEMP",
                        card.name
                    ),
                )
                .with_span(card.span),
            );
        }
        let polarity = match card.kind.as_str() {
            "pnp" | "pmos" => -1.0,
            _ => 1.0,
        };
        Some((params, polarity))
    }

    fn warn_ignored(card: &ModelCard, ignored: &[String], diags: &mut Vec<Diagnostic>) {
        if !ignored.is_empty() {
            diags.push(
                Diagnostic::warning(
                    "W-MODEL-IGNORED",
                    format!(
                        "model `{}`: unsupported parameter(s) ignored: {}",
                        card.name,
                        ignored.join(", ")
                    ),
                )
                .with_span(card.span),
            );
        }
    }

    fn diode(
        &mut self,
        name: &str,
        diags: &mut Vec<Diagnostic>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<DiodeModel> {
        if let Some(m) = self.diodes.get(name) {
            return m.clone();
        }
        let card = self.card(name)?;
        let m = self.prepare(card, diags, errors).map(|(p, _)| {
            let (m, ignored) = DiodeModel::from_params(&p);
            Self::warn_ignored(card, &ignored, diags);
            m
        });
        self.diodes.insert(name.to_string(), m.clone());
        m
    }

    fn bjt(
        &mut self,
        name: &str,
        diags: &mut Vec<Diagnostic>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<BjtModel> {
        if let Some(m) = self.bjts.get(name) {
            return m.clone();
        }
        let card = self.card(name)?;
        let m = self.prepare(card, diags, errors).map(|(p, pol)| {
            let (m, ignored) = BjtModel::from_params(&p, pol);
            Self::warn_ignored(card, &ignored, diags);
            m
        });
        self.bjts.insert(name.to_string(), m.clone());
        m
    }

    fn mos(
        &mut self,
        name: &str,
        diags: &mut Vec<Diagnostic>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<Mos1Model> {
        if let Some(m) = self.mos.get(name) {
            return m.clone();
        }
        let card = self.card(name)?;
        let m = self.prepare(card, diags, errors).map(|(p, pol)| {
            let (m, ignored) = Mos1Model::from_params(&p, pol);
            Self::warn_ignored(card, &ignored, diags);
            m
        });
        self.mos.insert(name.to_string(), m.clone());
        m
    }
}

/// Warn about instance parameters that were not consumed.
fn warn_instance(e: &Element, inst: &ModelParams, diags: &mut Vec<Diagnostic>) {
    let unused = inst.unused();
    if !unused.is_empty() {
        diags.push(
            Diagnostic::warning(
                "W-PARSE-IGNORED",
                format!(
                    "`{}`: unsupported instance parameter(s) ignored: {}",
                    e.name,
                    unused.join(", ")
                ),
            )
            .with_span(e.span),
        );
    }
}
