//! Elaboration: netlist → compiled circuit (unknowns, device groups, fixed matrix pattern).
//!
//! Provenance: original work. Topology checks (no DC path to ground, loops of voltage-defined
//! branches) are standard MNA well-posedness conditions; see Ho, Ruehli & Brennan (1975).
//! See `docs/PROVENANCE.md`.

use spx_devices::va::VaGroup;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use spx_devices::bjt::BjtModel;
use spx_devices::bsim3::Bsim3s;
use spx_devices::bsim4::{Binding, Bsim4s, PreparedModel};
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

/// A ground-referenced voltage constraint in the last elimination column.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedVoltage {
    pub node: usize,
    pub equation: usize,
    pub voltage: f64,
}

/// A compiled, immutable circuit.
#[derive(Debug)]
pub struct Circuit {
    /// `.ic` values as (unknown index, voltage).
    pub(crate) ics: Vec<(usize, f64)>,
    /// `.nodeset` values as (unknown index, voltage).
    pub(crate) nodesets: Vec<(usize, f64)>,
    pub(crate) topology: Arc<CircuitTopology>,
    pub(crate) ordering: Option<Arc<spx_sparse::BatchOrdering>>,
    pub(crate) groups: Vec<Arc<dyn DeviceGroup>>,
    bindings: Option<Arc<BatchBindings>>,
    pub(crate) fixed_voltage: Option<FixedVoltage>,
    pub(crate) discrete: Vec<usize>,
    pub(crate) linear: bool,
    /// Per group: number of state values.
    pub(crate) state_len: Vec<usize>,
}

impl std::ops::Deref for Circuit {
    type Target = CircuitTopology;
    fn deref(&self) -> &CircuitTopology {
        &self.topology
    }
}

/// Immutable connectivity, node numbering, slot maps and sparse structure.
#[derive(Debug)]
pub struct CircuitTopology {
    node_index: HashMap<String, usize>,
    group_offsets: Vec<usize>,
    va_ports: Option<VaPorts>,
    pub(crate) va_modules: BTreeMap<String, Arc<spx_va::CompiledModule>>,
    pub(crate) unknowns: Unknowns,
    pub(crate) aliases: Vec<(String, Node)>,
    pub(crate) num_nodes: usize,
    pub(crate) layouts: Vec<StampLayout>,
    pub(crate) pattern: CscPattern,
    /// Per unknown: value index of the diagonal if it is a node voltage, else [`DISCARD`]
    /// (for rshunt / gmin stepping).
    pub(crate) node_diag: Vec<usize>,
    /// Nodes without a DC path to ground (for diagnostics).
    pub(crate) no_dc_path: Vec<String>,
    /// Gather maps from group slots to assembled values (parallelism level L2).
    pub(crate) gather: GatherMaps,
    /// Whether the circuit is large enough for parallel evaluation to pay off.
    pub(crate) parallel: bool,
    /// Per external node: names of the attached elements, nonlinear devices first.
    pub(crate) node_elements: Vec<Vec<String>>,
}

/// A source of one assembled value: `(group, slot)`.
pub(crate) type Src = (u32, u32);

/// Circuit and group indices fit in sixteen bits for many transistor blocks.
#[derive(Debug)]
pub(crate) enum GroupScatter {
    Small(Vec<(u16, u16)>),
    Wide(Vec<(usize, usize)>),
}

impl GroupScatter {
    fn new(entries: Vec<(usize, usize)>) -> Self {
        if entries
            .iter()
            .all(|&(k, d)| k.max(d) <= usize::from(u16::MAX))
        {
            Self::Small(
                entries
                    .into_iter()
                    .map(|(k, d)| (k as u16, d as u16))
                    .collect(),
            )
        } else {
            Self::Wide(entries)
        }
    }
    pub(crate) fn sum(&self, values: &[f64], dst: &mut [f64]) {
        match self {
            Self::Small(entries) => {
                for &(slot, row) in entries {
                    dst[usize::from(row)] += values[usize::from(slot)];
                }
            }
            Self::Wide(entries) => {
                for &(slot, row) in entries {
                    dst[row] += values[slot];
                }
            }
        }
    }
}

/// For every destination index, the contributing slots in a fixed order (CSR layout).
///
/// Every destination is the sum of its sources, accumulated left to right starting from zero.
/// The order is fixed at elaboration, so the floating-point result does not depend on how the
/// destinations are distributed over threads (architecture D4).
#[derive(Debug)]
pub(crate) struct Gather {
    pub(crate) by_group: Vec<GroupScatter>,
    pub(crate) tiles: Vec<Vec<(usize, GroupScatter)>>,
    pub(crate) contributions: u64,
    pub(crate) by_destination: Option<(Vec<usize>, Vec<Src>)>,
}

impl Gather {
    /// Keep the source order for contiguous single-worker reads. The optional
    /// transpose allows parallel workers to own disjoint destination ranges.
    fn build(
        len: usize,
        split_rows: bool,
        tile_rows: bool,
        entries: impl Iterator<Item = (usize, usize, usize)> + Clone,
    ) -> Self {
        let mut by_group = Vec::<Vec<_>>::new();
        for (g, k, d) in entries.clone() {
            if by_group.len() <= g {
                by_group.resize_with(g + 1, Vec::new);
            }
            by_group[g].push((k, d));
        }
        let contributions = by_group.iter().map(|g| g.len() as u64).sum();
        let mut tiles = Vec::new();
        if tile_rows && contributions >= 32768 {
            let size = crate::solve::SCATTER_TILE;
            tiles.resize_with(len.div_ceil(size), Vec::new);
            let mut bins = vec![Vec::new(); tiles.len()];
            let mut touched = Vec::new();
            for (group, entries) in by_group.iter().enumerate() {
                for &(slot, row) in entries {
                    let id = row / size;
                    if bins[id].is_empty() {
                        touched.push(id);
                    }
                    bins[id].push((slot, row % size));
                }
                for id in touched.drain(..) {
                    tiles[id].push((group, GroupScatter::new(std::mem::take(&mut bins[id]))));
                }
            }
        }
        let by_group = by_group.into_iter().map(GroupScatter::new).collect();
        let by_destination = (split_rows && len > crate::solve::GATHER_CHUNK).then(|| {
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
            (ptr, src)
        });
        Self {
            by_group,
            tiles,
            contributions,
            by_destination,
        }
    }
}
/// Gather maps for `G`, `C` (pattern values) and `f`, `q`, `b` (unknowns).
#[derive(Debug)]
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
const CHUNK_VA_SMALL: usize = 4;
const CHUNK_VA_LARGE: usize = 16;
const PARALLEL_MIN_VA: usize = 8;
/// Minimum number of nonlinear instances for parallel evaluation.
pub(crate) const PARALLEL_MIN_NONLINEAR: usize = 256;

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
            let kind = g.type_name();
            if let Some((_, count)) = out.iter_mut().find(|(name, _)| *name == kind) {
                *count += g.len();
            } else {
                out.push((kind, g.len()));
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
            .filter_map(Arc::get_mut)
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

fn va_model_type(card: &ModelCard) -> &str {
    if matches!(card.kind.as_str(), "npn" | "pnp")
        && card.params.iter().any(|(k, v)| k == "level" && *v == 9.)
    {
        "vbic13_5t"
    } else {
        &card.kind
    }
}

fn compile_hdl(
    nl: &Netlist,
    _cache: bool,
) -> Result<BTreeMap<String, Arc<spx_va::CompiledModule>>, Vec<Diagnostic>> {
    let mut modules = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let mut declared = HashSet::new();
    #[cfg(feature = "jit")]
    let used: HashSet<_> = nl
        .elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::Va { model, .. } => Some(model.as_str()),
            _ => None,
        })
        .collect();
    #[cfg(feature = "jit")]
    let wanted: HashSet<_> = nl
        .models
        .iter()
        .filter(|card| used.contains(card.name.as_str()))
        .flat_map(|card| {
            let name = va_model_type(card);
            [
                name.to_owned(),
                name.strip_suffix("_va")
                    .map(|s| format!("{s}va"))
                    .unwrap_or_default(),
            ]
        })
        .collect();
    for h in &nl.hdl {
        if !seen.insert(&h.path) {
            continue;
        }
        let loader = HdlLoader::new(&h.path);
        #[cfg(feature = "jit")]
        let source = if _cache {
            spx_va::compile_source_cached(&h.path, &h.text, &loader, |name| {
                wanted.contains(&name.to_ascii_lowercase())
            })
        } else {
            spx_va::compile_source(&h.text, &loader)
        };
        #[cfg(not(feature = "jit"))]
        let source = spx_va::compile_source(&h.text, &loader);
        match source {
            Err((ds, files)) => errors.extend(ds.into_iter().map(|d| hdl_diagnostic(d, h, &files))),
            Ok(source) => {
                for (span, name) in source.declarations {
                    if !declared.insert(name.to_ascii_lowercase()) {
                        errors.push(hdl_diagnostic(
                            Diagnostic::error("E-VA-SEMA", format!("duplicate module {name}"))
                                .with_span(span),
                            h,
                            &source.sources,
                        ));
                    }
                }
                modules.extend(
                    source
                        .modules
                        .into_iter()
                        .map(|m| (m.name.to_ascii_lowercase(), Arc::new(m))),
                );
            }
        }
    }
    if errors.is_empty() {
        Ok(modules)
    } else {
        Err(errors)
    }
}

fn swept_sources(card: &spx_netlist::AnalysisCard) -> impl Iterator<Item = &str> {
    let (inner, outer) = match card {
        spx_netlist::AnalysisCard::Dc { inner, outer, .. } => (Some(inner), outer.as_ref()),
        _ => (None, None),
    };
    inner.into_iter().chain(outer).map(|a| a.source.as_str())
}

/// Compile a netlist. Returns the circuit and warnings, or all diagnostics on failure.
pub fn compile(
    nl: &Netlist,
    opts: &SimOptions,
) -> Result<(Circuit, Vec<Diagnostic>), Vec<Diagnostic>> {
    let va_modules = compile_hdl(nl, opts.va_cache)?;
    let dc_only = nl.analyses.iter().all(|a| {
        matches!(
            a,
            spx_netlist::AnalysisCard::Dc { .. } | spx_netlist::AnalysisCard::Op { .. }
        )
    });
    compile_cached(nl, opts, &va_modules, dc_only)
}

/// Re-elaborate with immutable compiled modules shared by all analyses of this run.
pub(crate) fn compile_cached(
    nl: &Netlist,
    opts: &SimOptions,
    va_modules: &BTreeMap<String, Arc<spx_va::CompiledModule>>,
    dc_only: bool,
) -> Result<(Circuit, Vec<Diagnostic>), Vec<Diagnostic>> {
    #[cfg(not(feature = "jit"))]
    let _ = dc_only;
    if ![opts.temp, opts.tnom]
        .iter()
        .all(|t| t.is_finite() && *t > -273.15)
    {
        return Err(vec![Diagnostic::error(
            "E-ELAB-PARAM",
            "absolute temperatures must be positive and finite",
        )]);
    }
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
    let swept: HashSet<_> = nl.analyses.iter().flat_map(swept_sources).collect();
    let (mut va, va_count) = bind_va(nl, opts, va_modules, &node_index, &swept, false)?;
    let mut nonlinear_count = va_count;
    #[cfg(feature = "jit")]
    let va_instances = va_count;
    let mut va_ports = None;
    let mut aliases = Vec::new();
    if !va.is_empty() {
        let ground = node_names.len();
        let mut sets = UnionFind::new(ground + 1);
        let shorts = terminal_shorts(&va);
        let original_nodes = node_index.clone();
        for &(a, b) in &shorts {
            grounded |= a.is_none() || b.is_none();
            sets.union(a.unwrap_or(ground), b.unwrap_or(ground));
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
        remap_ports(&mut va, &indices)?;
        va_ports = Some(VaPorts {
            original_nodes,
            indices,
            shorts,
        });
        node_names = new_names;
        touches = counts;
    }
    #[cfg(feature = "jit")]
    prepare_va_kernels(
        nl,
        opts,
        &node_index,
        node_names.len(),
        dc_only,
        &mut va,
        &mut diags,
    );
    if !grounded {
        return Err(vec![
            Diagnostic::error(
                "E-ELAB-NOGROUND",
                "no element is connected to ground (node 0)",
            )
            .with_note("every circuit needs a reference node named `0` (or `gnd`)"),
        ]);
    }
    let mut node_elements: Vec<Vec<String>> = vec![Vec::new(); node_names.len()];
    for nonlinear in [true, false] {
        for e in &nl.elements {
            if e.kind.model().is_some() != nonlinear {
                continue;
            }
            // A device can touch a node at multiple terminals. Element names
            // are unique and all terminals of this device are visited together.
            for n in e.kind.nodes() {
                if let Some(&i) = node_index.get(n)
                    && node_elements[i].last() != Some(&e.name)
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
            ElementKind::Switch { .. }
            | ElementKind::Resistor { .. }
            | ElementKind::Diode { .. } => {
                dc.union(p, n);
            }
            ElementKind::Jfet { .. } | ElementKind::Bjt { .. } => {
                // Both intrinsic junctions have a DC conductance.
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
            ElementKind::Line { ports, .. } => {
                // Each port carries a differential current; its common mode is independent.
                let (a, b) = (g_id(node(&ports[2])), g_id(node(&ports[3])));
                let wire = if n == b {
                    Some((p, a))
                } else if p == a {
                    Some((n, b))
                } else {
                    None
                };
                if let Some((u, v)) = wire {
                    dc.union(u, v);
                    if !vloop.union(u, v) {
                        diags.push(
                            Diagnostic::warning(
                                "W-ELAB-VLOOP",
                                format!("{} closes a loop of DC voltage constraints", e.name),
                            )
                            .with_span(e.span),
                        );
                    }
                } else {
                    dc.union(p, n);
                    dc.union(a, b);
                }
            }
            ElementKind::Inductor { .. }
            | ElementKind::Polynomial { voltage: true, .. }
            | ElementKind::Behavioral { voltage: true, .. }
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

    let (mut groups, native_count) = native_groups(&nl.elements, nl, opts, node, &mut diags)?;
    nonlinear_count += native_count;
    #[cfg(feature = "jit")]
    let first_va = groups.len();
    for group in va.into_values() {
        group.into_groups(&mut groups);
    }

    // --- Unknowns and stamps ---
    for g in &groups {
        for i in g.probe_nodes() {
            touches[i] += 1;
        }
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
    let (unknowns, layouts, group_offsets) = setup_groups(&mut groups, &node_names, &mut diags)?;
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
    #[cfg(feature = "jit")]
    let fixed_voltage = if opts.va_jit && spx_va::jit_enabled() && !dc_only && va_instances >= 8 {
        let _phase = spx_diag::Phase::start("jit_wall");
        let fixed = spx_sparse::NewtonLu::new(&pattern)
            .ok()
            .and_then(|lu| lu.last_column())
            .and_then(|last| {
                nl.elements.iter().find_map(|e| {
                    let ElementKind::VoltageSource { p, n, source } = &e.kind else {
                        return None;
                    };
                    if source.tran.is_some() || swept.contains(e.name.as_str()) {
                        return None;
                    }
                    let dc = spx_devices::dc_value(source);
                    let voltage = match (node(p), node(n)) {
                        (Some(r), None) if r == last => dc,
                        (None, Some(r)) if r == last => -dc,
                        _ => return None,
                    };
                    let equation = unknowns.branch(&e.name)?;
                    // The source owns its branch equation. Controlled sources can read its
                    // current, but never add a residual or a charge to this row.
                    voltage.is_finite().then_some(FixedVoltage {
                        node: last,
                        equation,
                        voltage,
                    })
                })
            });
        use rayon::prelude::*;
        groups[first_va..]
            .par_iter_mut()
            .for_each(|g| g.prepare_newton_pairs(opts.temp + 273.15, fixed.map(|v| v.node)));
        fixed
    } else {
        None
    };
    #[cfg(not(feature = "jit"))]
    let fixed_voltage = None;

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
    // Large native groups stream their source-ordered maps, with G and C in
    // separate tasks. The smaller VA groups use a destination-parallel gather.
    let split_rows = va_count > 0;
    let tile_rows = !split_rows
        && n >= 2048
        && nonlinear_count >= PARALLEL_MIN_NONLINEAR
        && (rayon::current_thread_index().is_none() || rayon::current_num_threads() > 1);
    let gather = GatherMaps {
        g: Gather::build(nnz, split_rows, tile_rows, slots_to(&gmap).into_iter()),
        c: Gather::build(nnz, split_rows, tile_rows, slots_to(&cmap).into_iter()),
        f: Gather::build(n, split_rows, tile_rows, rows_to(|l| &l.f).into_iter()),
        q: Gather::build(n, split_rows, tile_rows, rows_to(|l| &l.q).into_iter()),
        b: Gather::build(n, split_rows, tile_rows, rows_to(|l| &l.b).into_iter()),
    };
    // Shunts connect physical electrical nodes, including internal device terminals.
    let node_diag = (0..n)
        .map(|i| {
            if unknowns.is_physical_node(i) {
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
            ordering: None,
            ics,
            nodesets,
            discrete: groups
                .iter()
                .enumerate()
                .filter_map(|(i, g)| (g.type_name() == "switch").then_some(i))
                .collect(),
            groups: groups.into_iter().map(Arc::from).collect(),
            bindings: None,
            state_len,
            linear,
            fixed_voltage,
            topology: Arc::new(CircuitTopology {
                va_modules: va_modules.clone(),
                node_index,
                group_offsets,
                va_ports,
                unknowns,
                aliases,
                num_nodes: nn,
                layouts,
                pattern,
                node_diag,
                no_dc_path,
                gather,
                parallel: va_count >= PARALLEL_MIN_VA || nonlinear_count >= PARALLEL_MIN_NONLINEAR,
                node_elements,
            }),
        },
        diags,
    ))
}

type VaChunks = BTreeMap<String, Chunked<VaGroup>>;

#[derive(Debug)]
struct VaPorts {
    original_nodes: HashMap<String, usize>,
    indices: Vec<Node>,
    shorts: Vec<(Node, Node)>,
}

fn terminal_shorts(va: &VaChunks) -> Vec<(Node, Node)> {
    va.values()
        .flat_map(|c| c.done.iter().chain([&c.cur]))
        .flat_map(VaGroup::terminal_shorts)
        .collect()
}

fn remap_ports(va: &mut VaChunks, indices: &[Node]) -> Result<(), Vec<Diagnostic>> {
    for g in va
        .values_mut()
        .flat_map(|c| c.done.iter_mut().chain([&mut c.cur]))
    {
        g.remap_ports(indices).map_err(|d| vec![d])?
    }
    Ok(())
}

fn bind_va(
    nl: &Netlist,
    opts: &SimOptions,
    va_modules: &BTreeMap<String, Arc<spx_va::CompiledModule>>,
    node_index: &HashMap<String, usize>,
    swept: &HashSet<&str>,
    statistical: bool,
) -> Result<(VaChunks, usize), Vec<Diagnostic>> {
    let mut va: BTreeMap<String, Chunked<VaGroup>> = BTreeMap::new();
    let mut va_models: HashMap<String, Arc<spx_va::Bound>> = HashMap::new();
    // Flattened subcircuits give identical model cards different hierarchical names.
    let mut card_bindings = HashMap::new();
    let mut count = 0;

    let binding = spx_diag::Phase::start("bind");
    let mut errors = Vec::new();
    let va_instances = nl
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Va { .. }))
        .count();
    let va_chunk = if cfg!(feature = "jit") && opts.va_jit && va_instances >= 8 {
        if va_instances >= 32 {
            CHUNK_VA_LARGE
        } else {
            CHUNK_VA_SMALL
        }
    } else if va_instances >= 32 {
        6
    } else {
        2
    };
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
            let module_name = va_model_type(card);
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
                let key = (
                    card.kind.as_str(),
                    card.params
                        .iter()
                        .map(|(name, v)| (name.as_str(), v.to_bits()))
                        .collect::<Vec<_>>(),
                );
                let bound = match card_bindings.entry(key) {
                    std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let mut values = card.params.clone();
                        if module_name == "vbic13_5t" && matches!(card.kind.as_str(), "npn" | "pnp")
                        {
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
                            Ok(mut bound) => {
                                if statistical {
                                    bound.use_run_cache();
                                }
                                let bound = Arc::new(bound);
                                entry.insert(Arc::clone(&bound));
                                bound
                            }
                            Err(ds) => {
                                errors.extend(ds.into_iter().map(|d| d.with_span(card.span)));
                                continue;
                            }
                        }
                    }
                };
                va_models.insert(model.clone(), Arc::clone(&bound));
                bound
            };
            let group = va
                .entry(card.kind.clone())
                .or_insert_with(|| Chunked::new(va_chunk));
            let chunk = group.cur();
            match chunk.push(
                Arc::clone(module),
                &e.name,
                nodes.iter().map(|n| node_index.get(n).copied()).collect(),
                bound,
                params,
            ) {
                Ok(()) => {
                    count += 1;
                    if swept.contains(e.name.as_str()) {
                        chunk.prepare_sweep(&e.name);
                    }
                }
                Err(ds) => errors.extend(ds.into_iter().map(|d| d.with_span(e.span))),
            }
        }
    }
    drop(binding);

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok((va, count))
}

#[cfg(feature = "jit")]
fn prepare_va_kernels(
    nl: &Netlist,
    opts: &SimOptions,
    node_index: &HashMap<String, usize>,
    nodes: usize,
    dc_only: bool,
    va: &mut VaChunks,
    diags: &mut Vec<Diagnostic>,
) {
    if !va.is_empty() {
        let _phase = spx_diag::Phase::start("jit_wall");
        let enabled = opts.va_jit && spx_va::jit_enabled();
        let kelvin = opts.temp + 273.15;
        let origins: HashMap<_, _> = nl
            .elements
            .iter()
            .map(|e| (e.name.as_str(), e.span))
            .collect();
        let mut seen = HashSet::new();
        let mut jobs = Vec::new();
        let mut rails = vec![None; nodes];
        let mut range = [0.0f64; 2];
        for e in &nl.elements {
            if let ElementKind::VoltageSource { p, n, source } = &e.kind
                && source.tran.is_none()
            {
                let rail = match (node_index.get(p), node_index.get(n)) {
                    (Some(&r), None) => Some((r, spx_devices::dc_value(source))),
                    (None, Some(&r)) => Some((r, -spx_devices::dc_value(source))),
                    _ => None,
                };
                if let Some((r, v)) = rail
                    && v.is_finite()
                {
                    rails[r] = Some(v);
                    range = [range[0].min(v), range[1].max(v)];
                }
            }
        }
        let count = va
            .values()
            .flat_map(|c| c.done.iter().chain([&c.cur]))
            .map(DeviceGroup::len)
            .sum::<usize>();
        for chunk in va
            .values_mut()
            .flat_map(|v| v.done.iter_mut().chain([&mut v.cur]))
        {
            chunk.specialize_stamps(dc_only);
            if !dc_only && count >= 8 && range[0] < range[1] {
                chunk.predict_bias(&rails, range);
            }
            for (module, bound, name) in chunk.jit_bindings(enabled) {
                if enabled && seen.insert((&module.name, bound.specialization_key(kelvin, 1.))) {
                    jobs.push((module, bound, name));
                }
            }
        }
        use rayon::prelude::*;
        let results: Vec<_> = jobs
            .par_iter()
            .map(|&(m, b, name)| {
                m.prepare_jit(b, kelvin, 1.)
                    .map(|d| d.with_span(origins[name]))
            })
            .collect();
        diags.extend(results.into_iter().flatten());
    }
}

type Groups = Vec<Box<dyn DeviceGroup>>;

fn instance_temperature(
    inst: &ModelParams,
    opts: &SimOptions,
    nominal: Option<f64>,
) -> Result<spx_devices::temperature::Temperature, Diagnostic> {
    let kelvin = inst.get("temp", opts.temp + inst.get("dtemp", 0.)) + 273.15;
    let nominal = nominal.unwrap_or(opts.tnom) + 273.15;
    if ![kelvin, nominal].iter().all(|v| v.is_finite() && *v > 0.) {
        return Err(Diagnostic::error(
            "E-ELAB-PARAM",
            "absolute temperatures must be positive and finite",
        ));
    }
    Ok(spx_devices::temperature::Temperature { kelvin, nominal })
}

fn passive_value(
    value: f64,
    params: &[(String, f64)],
    model: Option<&ModelCard>,
    opts: &SimOptions,
) -> Result<f64, Diagnostic> {
    if params.is_empty() && model.is_none() {
        return Ok(value);
    }
    let p = ModelParams::new(params);
    let m = model
        .map(|m| ModelParams::new(&m.params))
        .unwrap_or_default();
    let t = instance_temperature(&p, opts, m.opt(&["tnom"]))?;
    let d = t.kelvin - t.nominal;
    let result =
        value * (1. + d * (p.get("tc1", m.get("tc1", 0.)) + d * p.get("tc2", m.get("tc2", 0.))));
    if !result.is_finite() {
        return Err(Diagnostic::error(
            "E-ELAB-PARAM",
            "non-finite temperature-adjusted component value",
        ));
    }
    Ok(result)
}

#[derive(Clone)]
enum PreparedMos {
    Bsim3(Arc<spx_devices::bsim3::Model>),
    Bsim4(Arc<PreparedModel>),
}

fn native_groups<'a>(
    elements: impl IntoIterator<Item = &'a Element>,
    nl: &Netlist,
    opts: &SimOptions,
    node: impl Fn(&str) -> Node,
    diags: &mut Vec<Diagnostic>,
) -> Result<(Groups, usize), Vec<Diagnostic>> {
    let mut errors = vec![];
    let mut nonlinear_count = 0;
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
    let mut classic_mos = [1, 2, 3].map(|_| Chunked::<Mos1s>::new(CHUNK_NONLINEAR));
    let mut bsim3 = Chunked::<Bsim3s>::new(CHUNK_NONLINEAR);
    let mut compact_models = HashMap::<_, PreparedMos>::new();
    let mut bsim4 = Chunked::<Bsim4s>::new(CHUNK_NONLINEAR);
    let mut model_cache = ModelCache::new(&nl.models);
    let mut controlled: Groups = Vec::new();
    let mut windings = HashMap::new();
    if nl
        .elements
        .iter()
        .any(|e| matches!(e.kind, ElementKind::Coupling { .. }))
    {
        for e in &nl.elements {
            if let ElementKind::Inductor {
                value,
                params,
                model,
                ..
            } = &e.kind
            {
                windings.insert(
                    e.name.as_str(),
                    passive_value(
                        *value,
                        params,
                        model.as_ref().and_then(|name| model_cache.card(name)),
                        opts,
                    ),
                );
            }
        }
    }
    for e in elements {
        match &e.kind {
            ElementKind::Va { .. } => {}
            ElementKind::Line { ports, z0, td } => {
                match spx_devices::line::Line::new(
                    &e.name,
                    ports.each_ref().map(|s| node(s)),
                    *z0,
                    *td,
                ) {
                    Ok(line) => controlled.push(Box::new(line)),
                    Err(d) => errors.push(d.with_span(e.span)),
                }
            }
            ElementKind::Polynomial {
                p,
                n,
                voltage,
                controls,
                coefficients,
            } => {
                match spx_devices::behavioral::Behavioral::polynomial(
                    &e.name,
                    [node(p), node(n)],
                    *voltage,
                    controls,
                    coefficients,
                    &node,
                ) {
                    Ok(mut g) => {
                        g.span = Some(e.span);
                        controlled.push(Box::new(g));
                    }
                    Err(d) => errors.push(d.with_span(e.span)),
                }
                nonlinear_count += 1;
            }
            ElementKind::Jfet {
                d,
                g,
                s,
                model,
                params,
                off,
            } => {
                let Some(card) = model_cache.card(model) else {
                    errors.push(
                        Diagnostic::error("E-ELAB-MODEL", format!("undefined JFET model {model}"))
                            .with_span(e.span),
                    );
                    continue;
                };
                let Some((p, polarity)) = model_cache.prepare(card, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                let result = instance_temperature(&inst, opts, p.opt(&["tnom"])).and_then(|t| {
                    spx_devices::jfet::Jfet::new(
                        &e.name,
                        [node(d), node(g), node(s)],
                        &p,
                        polarity,
                        t,
                        inst.get("area", 1.),
                        inst.opt(&["m", "mult"]).unwrap_or(1.),
                        *off,
                    )
                });
                match result {
                    Ok(g) => controlled.push(Box::new(g)),
                    Err(d) => errors.push(d.with_span(e.span)),
                }
                ModelCache::warn_ignored(card, &p.unused(), diags);
                warn_instance(e, &inst, diags);
                nonlinear_count += 1;
            }
            ElementKind::Behavioral {
                p,
                n,
                voltage,
                expr,
                table,
            } => {
                let source = spx_devices::behavioral::Behavioral::new(
                    &e.name,
                    [node(p), node(n)],
                    *voltage,
                    expr,
                    table,
                    |s| {
                        if s == GROUND
                            || (node(s).is_none()
                                && nl.elements.iter().any(|e| e.kind.nodes().contains(&s)))
                        {
                            Some(None)
                        } else {
                            node(s).map(Some)
                        }
                    },
                );
                match source {
                    Ok(mut g) => {
                        g.span = Some(e.span);
                        controlled.push(Box::new(g));
                    }
                    Err(d) => errors.push(d.with_span(e.span)),
                }
                nonlinear_count += 1;
            }
            ElementKind::Coupling { first, second, k } => {
                let mut inductance = [0.; 2];
                for (dst, w) in inductance.iter_mut().zip([first, second]) {
                    match windings.get(w.as_str()) {
                        Some(Ok(v)) if *v > 0. => *dst = *v,
                        _ => errors.push(
                            Diagnostic::error(
                                "E-COUPLING-L",
                                format!("{} needs a positive inductor named {w}", e.name),
                            )
                            .with_span(e.span),
                        ),
                    }
                }
                controlled.push(Box::new(spx_devices::controlled::Coupling {
                    name: e.name.clone(),
                    windings: [first.clone(), second.clone()],
                    mutual: k * inductance[0].sqrt() * inductance[1].sqrt(),
                    branches: [0; 2],
                }));
            }
            ElementKind::Switch {
                p,
                n,
                model,
                control,
                on,
            } => {
                use spx_devices::controlled::{Control, Switch};
                let Some(card) = model_cache.card(model) else {
                    errors.push(
                        Diagnostic::error("E-SWITCH-MODEL", format!("undefined {model}"))
                            .with_span(e.span),
                    );
                    continue;
                };
                let m = ModelParams::new(&card.params);
                let (control, vt, vh) = match control {
                    spx_netlist::SwitchControl::Voltage(a, b) => {
                        (Control::Voltage(node(a), node(b)), "vt", "vh")
                    }
                    spx_netlist::SwitchControl::Current(a) => {
                        (Control::Current(a.clone(), None), "it", "ih")
                    }
                };
                let ron = m.get("ron", 1.);
                // The default off conductance is GMIN, including an ideal open at GMIN=0.
                let roff = m.opt(&["roff"]);
                let goff = roff.map_or(opts.gmin, |r| 1. / r);
                let (threshold, width) = (m.get(vt, 0.), m.get(vh, 0.).abs());
                if !(ron.is_finite() && ron > 0. && (1. / ron).is_finite())
                    || roff.is_some_and(|r| !r.is_finite() || r <= 0.)
                    || !goff.is_finite()
                    || !width.is_finite()
                    || !threshold.is_finite()
                {
                    errors.push(
                        Diagnostic::error(
                            "E-SWITCH-MODEL",
                            format!("invalid switch resistances or thresholds in {model}"),
                        )
                        .with_span(card.span),
                    );
                    continue;
                }
                ModelCache::warn_ignored(card, &m.unused(), diags);
                controlled.push(Box::new(Switch {
                    span: e.span,
                    name: e.name.clone(),
                    p: node(p),
                    n: node(n),
                    control,
                    threshold,
                    width,
                    gon: 1. / ron,
                    goff,
                    initially_on: *on,
                }));
                nonlinear_count += 1;
            }
            ElementKind::Resistor {
                p,
                n,
                value,
                model,
                params,
            }
            | ElementKind::Capacitor {
                p,
                n,
                value,
                model,
                params,
            }
            | ElementKind::Inductor {
                p,
                n,
                value,
                model,
                params,
            } => {
                let model = model.as_ref().and_then(|name| model_cache.card(name));
                let value = match passive_value(*value, params, model, opts) {
                    Ok(value) => value,
                    Err(d) => {
                        errors.push(d.with_span(e.span));
                        continue;
                    }
                };
                let (p, n) = (node(p), node(n));
                let ic = params
                    .iter()
                    .rev()
                    .find(|(k, _)| k == "ic")
                    .map(|(_, v)| *v);
                match e.kind {
                    ElementKind::Resistor { .. } if value == 0. => errors.push(
                        Diagnostic::error("E-ELAB-PARAM", "zero temperature-adjusted resistance")
                            .with_span(e.span),
                    ),
                    ElementKind::Resistor { .. } => {
                        let get = |key: &str| {
                            params.iter().rev().find(|(k, _)| k == key).map(|(_, v)| *v)
                        };
                        let kelvin =
                            get("temp").unwrap_or(opts.temp + get("dtemp").unwrap_or(0.)) + 273.15;
                        res.cur().push_at(
                            &e.name,
                            p,
                            n,
                            value,
                            if get("noisy") == Some(0.) { 0. } else { kelvin },
                        );
                    }
                    ElementKind::Capacitor { .. } => {
                        cap.cur().push_initial(&e.name, p, n, value, ic)
                    }
                    _ => ind.cur().push_initial(&e.name, p, n, value, ic),
                }
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
                let Some(mut m) = model_cache.diode(model, diags, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                match instance_temperature(&inst, opts, m.tnom) {
                    Ok(t) => m.at_temperature(t),
                    Err(d) => {
                        errors.push(d.with_span(e.span));
                        continue;
                    }
                }
                let copies = inst.opt(&["m", "mult"]).unwrap_or(1.);
                m.perimeter = inst.get("pj", 0.) * copies;
                let area = inst.get("area", 1.) * copies;
                if !area.is_finite() || area <= 0. || m.perimeter < 0. || !m.perimeter.is_finite() {
                    errors.push(
                        Diagnostic::error(
                            "E-ELAB-PARAM",
                            format!("{}: invalid effective area or junction perimeter", e.name),
                        )
                        .with_span(e.span),
                    );
                    continue;
                }
                warn_instance(e, &inst, diags);
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
                let Some(mut m) = model_cache.bjt(model, diags, &mut errors) else {
                    continue;
                };
                let inst = ModelParams::new(params);
                match instance_temperature(&inst, opts, m.tnom) {
                    Ok(t) => m.at_temperature(t),
                    Err(d) => {
                        errors.push(d.with_span(e.span));
                        continue;
                    }
                }
                let area = inst.get("area", 1.0) * inst.opt(&["m", "mult"]).unwrap_or(1.);
                if !area.is_finite() || area <= 0. {
                    errors.push(
                        Diagnostic::error(
                            "E-ELAB-PARAM",
                            format!("{}: effective area must be positive", e.name),
                        )
                        .with_span(e.span),
                    );
                    continue;
                }
                warn_instance(e, &inst, diags);
                let sub = s.as_deref().and_then(&node);
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
                let inst = ModelParams::new(params);
                let dflt = Mos1Geometry::default();
                let scaled = |name: &str, default: f64, power: i32| {
                    inst.opt(&[name])
                        .map_or(default, |v| v * opts.scale.powi(power))
                };
                let geom = Mos1Geometry {
                    l: scaled("l", opts.defl.unwrap_or(dflt.l), 1),
                    w: scaled("w", opts.defw.unwrap_or(dflt.w), 1),
                    ad: scaled("ad", opts.defad.unwrap_or(0.0), 2),
                    as_: scaled("as", opts.defas.unwrap_or(0.0), 2),
                    pd: scaled("pd", 0.0, 1),
                    ps: scaled("ps", 0.0, 1),
                };
                if let Some(card) = model_cache.card(model)
                    && matches!(card.kind.as_str(), "nmos" | "pmos")
                    && card
                        .params
                        .iter()
                        .rev()
                        .find(|(k, _)| k == "level")
                        .is_some_and(|(_, v)| matches!(*v, 8. | 49. | 54.))
                {
                    // Temperature, scale and geometry defaults are fixed for this compile.
                    // Preserve parameter order (later aliases/duplicates can override).
                    let key = (
                        model.as_str(),
                        params
                            .iter()
                            .map(|(k, v)| (k.as_str(), v.to_bits()))
                            .collect::<Vec<_>>(),
                    );
                    let nodes = [node(d), node(g), node(s), node(b)];
                    let prepared = if let Some(cached) = compact_models.get(&key) {
                        cached.clone()
                    } else {
                        let p = ModelParams::new(&card.params);
                        let binding = Binding {
                            model: &p,
                            instance: &inst,
                            geometry: geom,
                            polarity: if card.kind == "pmos" { -1. } else { 1. },
                            temp: inst.get("temp", opts.temp + inst.get("dtemp", 0.)),
                            tnom: opts.tnom,
                            scale: opts.scale,
                        };
                        let prepared = if p.get("level", 0.) == 54. {
                            PreparedModel::new(&e.name, binding)
                                .map(|(m, w)| (PreparedMos::Bsim4(m), w))
                        } else {
                            spx_devices::bsim3::Model::bind(
                                &e.name,
                                &binding,
                                card.version.as_deref(),
                            )
                            .map(|(m, w)| (PreparedMos::Bsim3(m), w))
                        };
                        let (prepared, warnings) = match prepared {
                            Ok(result) => result,
                            Err(d) => {
                                errors.push(d.with_span(e.span));
                                continue;
                            }
                        };
                        let before = diags.len();
                        diags.extend(warnings.into_iter().map(|d| d.with_span(e.span)));
                        ModelCache::warn_ignored(card, &p.unused(), diags);
                        warn_instance(e, &inst, diags);
                        if diags.len() == before {
                            compact_models.insert(key, prepared.clone());
                        }
                        prepared
                    };
                    match prepared {
                        PreparedMos::Bsim3(m) => bsim3.cur().push_prepared(&e.name, nodes, *off, m),
                        PreparedMos::Bsim4(m) => bsim4.cur().push_prepared(&e.name, nodes, *off, m),
                    }
                    nonlinear_count += 1;
                    continue;
                }
                let Some(mut m) = model_cache.mos(model, opts.tnom, diags, &mut errors) else {
                    continue;
                };
                let copies = inst.opt(&["m", "mult"]).unwrap_or(1.);
                if !copies.is_finite() || copies <= 0. {
                    errors.push(
                        Diagnostic::error(
                            "E-ELAB-PARAM",
                            format!("{}: multiplicity must be positive", e.name),
                        )
                        .with_span(e.span),
                    );
                    continue;
                }
                match instance_temperature(&inst, opts, m.tnom) {
                    Ok(t) => m.at_temperature(t),
                    Err(d) => {
                        errors.push(d.with_span(e.span));
                        continue;
                    }
                }
                m.apply_squares(&inst);
                m.multiply(copies);
                warn_instance(e, &inst, diags);
                if let Some(msg) = Mos1s::check_geometry(&m, &geom) {
                    errors.push(
                        Diagnostic::error("E-ELAB-PARAM", format!("`{}`: {msg}", e.name))
                            .with_span(e.span),
                    );
                    continue;
                }
                nonlinear_count += 1;
                classic_mos[m.level_index()].cur().push(
                    &e.name,
                    [node(d), node(g), node(s), node(b)],
                    m,
                    geom,
                    *off,
                );
            }
        }
    }
    if !errors.is_empty() {
        errors.append(diags);
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
    for devices in classic_mos {
        devices.into_groups(&mut groups);
    }
    bsim3.into_groups(&mut groups);
    bsim4.into_groups(&mut groups);
    groups.extend(controlled);
    Ok((groups, nonlinear_count))
}

fn setup_groups(
    groups: &mut Groups,
    node_names: &[String],
    diags: &mut Vec<Diagnostic>,
) -> Result<(Unknowns, Vec<StampLayout>, Vec<usize>), Vec<Diagnostic>> {
    let mut errors = vec![];
    let mut unknowns = Unknowns::with_nodes(node_names.iter().map(String::as_str));
    let mut offsets = vec![unknowns.len()];
    for g in groups.iter_mut() {
        g.alloc_unknowns(&mut unknowns);
        offsets.push(unknowns.len());
    }
    let mut layouts = Vec::with_capacity(groups.len());
    for g in groups.iter_mut() {
        let mut sb = StampBuilder::new(&unknowns);
        match g.setup(&mut sb) {
            Ok(()) => layouts.push(sb.finish()),
            Err(mut d) => {
                if let Some(span) = g.instance_span() {
                    d = d.with_span(span);
                }
                errors.push(d);
                layouts.push(StampLayout::default());
            }
        }
    }
    if !errors.is_empty() {
        errors.append(diags);
        return Err(errors);
    }

    Ok((unknowns, layouts, offsets))
}

/// Indices of numeric device chunks to replace in each statistical instance.
#[derive(Debug)]
struct BatchBindings {
    elements: Vec<usize>,
    groups: Vec<usize>,
}

impl Circuit {
    pub(crate) fn prepare_batch(
        &mut self,
        params: &spx_netlist::ParameterPlan,
    ) -> Result<(), Vec<Diagnostic>> {
        self.ordering = Some(
            spx_sparse::BatchOrdering::new(&self.pattern)
                .map_err(|e| vec![Diagnostic::error("E-SOLVE-BACKEND", e.to_string())])?,
        );
        let nl = params.nominal();
        let mut changed = params.varying_instances();
        for e in &nl.elements {
            if let ElementKind::Coupling { first, second, .. } = &e.kind
                && (changed.contains(first.as_str()) || changed.contains(second.as_str()))
            {
                changed.insert(&e.name);
            }
        }
        for a in &nl.analyses {
            changed.extend(swept_sources(a));
        }
        let indices: HashMap<_, _> = nl
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.as_str(), i))
            .collect();
        let mut bindings = BatchBindings {
            elements: vec![],
            groups: vec![],
        };
        for (i, g) in self.groups.iter().enumerate() {
            if g.type_name() == "verilog-a" {
                bindings.groups.push(i);
                continue;
            }
            if (0..g.len())
                .filter_map(|k| g.instance_name(k))
                .any(|name| changed.contains(name))
            {
                bindings.groups.push(i);
                bindings
                    .elements
                    .extend((0..g.len()).filter_map(|k| indices.get(g.instance_name(k)?).copied()));
            }
        }
        self.bindings = Some(Arc::new(bindings));
        Ok(())
    }

    pub(crate) fn bind_run(
        &self,
        nl: &Netlist,
        opts: &SimOptions,
    ) -> Result<(Circuit, Vec<Diagnostic>), Vec<Diagnostic>> {
        let Some(bindings) = &self.bindings else {
            return Err(vec![Diagnostic::error(
                "E-ELAB-INTERNAL",
                "missing batch parameter bindings",
            )]);
        };
        let mut diagnostics = vec![];
        let (mut changed, _) = native_groups(
            bindings.elements.iter().map(|&i| &nl.elements[i]),
            nl,
            opts,
            |n| self.node_index.get(n).copied(),
            &mut diagnostics,
        )?;
        if let Some(ports) = &self.va_ports {
            let swept = nl.analyses.iter().flat_map(swept_sources).collect();
            let (mut va, _) = bind_va(
                nl,
                opts,
                &self.va_modules,
                &ports.original_nodes,
                &swept,
                true,
            )?;
            if terminal_shorts(&va) != ports.shorts {
                return Err(topology_error());
            }
            remap_ports(&mut va, &ports.indices)?;
            #[cfg(feature = "jit")]
            {
                let dc_only = nl.analyses.iter().all(|a| {
                    matches!(
                        a,
                        spx_netlist::AnalysisCard::Op { .. } | spx_netlist::AnalysisCard::Dc { .. }
                    )
                });
                prepare_va_kernels(
                    nl,
                    opts,
                    &self.node_index,
                    self.num_nodes,
                    dc_only,
                    &mut va,
                    &mut diagnostics,
                );
            }
            for chunk in va.into_values() {
                chunk.into_groups(&mut changed);
            }
        }
        let mut groups = self.groups.clone();
        if changed.len() != bindings.groups.len() {
            return Err(topology_error());
        }
        for (&i, mut group) in bindings.groups.iter().zip(changed) {
            let range = self.group_offsets[i]..self.group_offsets[i + 1];
            let mut local = Unknowns::at_offset(range.start);
            group.alloc_unknowns(&mut local);
            if group.type_name() != groups[i].type_name()
                || local.names() != &self.unknowns.names()[range]
            {
                return Err(topology_error());
            }
            let mut stamp = StampBuilder::new(&self.unknowns);
            group.setup(&mut stamp).map_err(|e| vec![e])?;
            if stamp.finish() != self.layouts[i] {
                return Err(topology_error());
            }
            groups[i] = Arc::from(group);
        }
        Ok((self.with_run_groups(groups, nl), diagnostics))
    }

    fn with_run_groups(&self, groups: Vec<Arc<dyn DeviceGroup>>, nl: &Netlist) -> Self {
        let node_values = |list: &[spx_netlist::NodeValue]| {
            list.iter()
                .filter_map(|v| Some((*self.node_index.get(&v.node)?, v.value)))
                .collect()
        };
        Self {
            ordering: self.ordering.clone(),
            ics: node_values(&nl.initial_conditions),
            nodesets: node_values(&nl.nodesets),
            bindings: self.bindings.clone(),
            topology: self.topology.clone(),
            fixed_voltage: None,
            discrete: self.discrete.clone(),
            linear: groups.iter().all(|g| g.is_linear()),
            state_len: groups.iter().map(|g| g.state_len()).collect(),
            groups,
        }
    }
}

fn topology_error() -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        "E-MC-TOPOLOGY",
        "sample changed internal unknowns or device stamp positions",
    )]
}

/// Resolves `.model` cards into typed models once per model name, emitting warnings once.
struct ModelCache<'a> {
    cards: HashMap<&'a str, &'a ModelCard>,

    diodes: HashMap<String, Option<DiodeModel>>,
    bjts: HashMap<String, Option<BjtModel>>,
    mos: HashMap<String, Option<Mos1Model>>,
}

impl<'a> ModelCache<'a> {
    fn new(cards: &'a [ModelCard]) -> Self {
        Self {
            cards: cards.iter().map(|c| (c.name.as_str(), c)).collect(),

            diodes: HashMap::new(),
            bjts: HashMap::new(),
            mos: HashMap::new(),
        }
    }

    fn card(&self, name: &str) -> Option<&'a ModelCard> {
        self.cards.get(name).copied()
    }

    /// Common checks; returns the parameter list and polarity.
    fn prepare(
        &self,
        card: &ModelCard,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<(ModelParams, f64)> {
        let params = ModelParams::new(&card.params);
        let level = params.get("level", 1.0);
        if !(level == 1.
            || matches!(card.kind.as_str(), "nmos" | "pmos") && matches!(level, 2. | 3.))
        {
            let mut d = Diagnostic::error(
                "E-ELAB-MODEL",
                format!("model `{}`: level {level} is not supported", card.name),
            )
            .with_span(card.span);
            if matches!(card.kind.as_str(), "nmos" | "pmos") {
                d = d.with_note("built-in MOSFET levels are 1, 2, 3, 8, 49 and 54");
            }
            errors.push(d);
            return None;
        }
        let polarity = match card.kind.as_str() {
            "pnp" | "pmos" | "pjf" => -1.0,
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
        let m = self.prepare(card, errors).and_then(|(p, _)| {
            let (m, ignored) = DiodeModel::from_params(&p);
            Self::warn_ignored(card, &ignored, diags);
            if [m.n, m.nr, m.ns, m.nbv, m.php, m.vj]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.)
                || [m.fcs, m.fc].iter().any(|v| !(0. ..1.).contains(v))
                || [m.jsw, m.cjp, m.isr, m.ikf, m.ibv]
                    .iter()
                    .any(|v| !v.is_finite() || *v < 0.)
            {
                errors.push(
                    Diagnostic::error("E-ELAB-PARAM", "invalid diode junction parameter")
                        .with_span(card.span),
                );
                None
            } else {
                Some(m)
            }
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
        let m = self.prepare(card, errors).and_then(|(p, pol)| {
            let (m, ignored) = BjtModel::from_params(&p, pol);
            Self::warn_ignored(card, &ignored, diags);
            if !(0. ..=1.).contains(&m.xcjc)
                || m.rbm < 0.
                || m.rbm > m.rb
                || m.irb < 0.
                || m.ptf < 0.
                || !matches!(m.subs, 1. | -1.)
                || m.iss < 0.
                || m.ns <= 0.
                || m.nkf <= 0.
            {
                errors.push(
                    Diagnostic::error(
                        "E-ELAB-PARAM",
                        "invalid BJT base, transport or substrate parameter",
                    )
                    .with_span(card.span),
                );
                None
            } else {
                Some(m)
            }
        });
        self.bjts.insert(name.to_string(), m.clone());
        m
    }

    fn mos(
        &mut self,
        name: &str,
        tnom: f64,
        diags: &mut Vec<Diagnostic>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<Mos1Model> {
        if let Some(m) = self.mos.get(name) {
            return m.clone();
        }
        let card = self.card(name)?;
        let m = self.prepare(card, errors).map(|(p, pol)| {
            let (m, ignored) = Mos1Model::from_params_at(&p, pol, tnom);
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
