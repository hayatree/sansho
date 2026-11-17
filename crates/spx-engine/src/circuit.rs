//! Elaboration: netlist → compiled circuit (unknowns, device groups, fixed matrix pattern).
//!
//! Provenance: original work. Topology checks (no DC path to ground, loops of voltage-defined
//! branches) are standard MNA well-posedness conditions; see Ho, Ruehli & Brennan (1975).
//! See `docs/PROVENANCE.md`.

use std::collections::HashMap;

use spx_devices::{
    Capacitors, Cccs, Ccvs, CurrentSources, DeviceGroup, Inductors, Node, Resistors, Slots,
    StampBuilder, StampLayout, Unknowns, Vccs, Vcvs, VoltageSources,
};
use spx_diag::Diagnostic;
use spx_netlist::{ElementKind, GROUND, Netlist};
use spx_sparse::{CscPattern, PatternBuilder};

use crate::options::SimOptions;

/// Marker for slots that touch ground and are discarded.
pub(crate) const DISCARD: usize = usize::MAX;

/// A compiled, immutable circuit.
#[derive(Debug)]
pub struct Circuit {
    pub(crate) unknowns: Unknowns,
    pub(crate) num_nodes: usize,
    pub(crate) groups: Vec<Box<dyn DeviceGroup>>,
    pub(crate) layouts: Vec<StampLayout>,
    pub(crate) pattern: CscPattern,
    /// Per group: G slot → value index (or [`DISCARD`]).
    pub(crate) gmap: Vec<Vec<usize>>,
    /// Per group: C slot → value index (or [`DISCARD`]).
    pub(crate) cmap: Vec<Vec<usize>>,
    /// Value indices of the node-voltage diagonal (for rshunt / gmin).
    pub(crate) node_diag: Vec<usize>,
    pub(crate) linear: bool,
    /// Nodes without a DC path to ground (for diagnostics).
    pub(crate) no_dc_path: Vec<String>,
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

/// Compile a netlist. Returns the circuit and warnings, or all diagnostics on failure.
pub fn compile(
    nl: &Netlist,
    opts: &SimOptions,
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
    let node = |name: &str| -> Node {
        if name == GROUND {
            None
        } else {
            node_index.get(name).copied()
        }
    };

    // --- Topology checks: DC path to ground, loops of voltage-defined branches ---
    let nn = node_names.len();
    let g_id = |n: Node| n.unwrap_or(nn);
    let mut dc = UnionFind::new(nn + 1);
    let mut vloop = UnionFind::new(nn + 1);
    for e in &nl.elements {
        let (p, n) = {
            let ns = e.kind.nodes();
            (g_id(node(ns[0])), g_id(node(ns[1])))
        };
        match &e.kind {
            ElementKind::Resistor { .. } => {
                dc.union(p, n);
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
    let mut res = Resistors::default();
    let mut cap = Capacitors::default();
    let mut ind = Inductors::default();
    let mut vsrc = VoltageSources::default();
    let mut isrc = CurrentSources::default();
    let mut e_src = Vcvs::default();
    let mut g_src = Vccs::default();
    let mut f_src = Cccs::default();
    let mut h_src = Ccvs::default();
    for e in &nl.elements {
        match &e.kind {
            ElementKind::Resistor { p, n, value } => res.push(&e.name, node(p), node(n), *value),
            ElementKind::Capacitor { p, n, value } => cap.push(&e.name, node(p), node(n), *value),
            ElementKind::Inductor { p, n, value } => ind.push(&e.name, node(p), node(n), *value),
            ElementKind::VoltageSource { p, n, source } => {
                vsrc.push(&e.name, node(p), node(n), source.clone());
            }
            ElementKind::CurrentSource { p, n, source } => {
                isrc.push(&e.name, node(p), node(n), source.clone());
            }
            ElementKind::Vcvs { p, n, cp, cn, gain } => {
                e_src.push(&e.name, [node(p), node(n), node(cp), node(cn)], *gain);
            }
            ElementKind::Vccs { p, n, cp, cn, gm } => {
                g_src.push(&e.name, [node(p), node(n), node(cp), node(cn)], *gm);
            }
            ElementKind::Cccs {
                p,
                n,
                control,
                gain,
            } => {
                f_src.push(&e.name, node(p), node(n), control, *gain);
            }
            ElementKind::Ccvs { p, n, control, r } => {
                h_src.push(&e.name, node(p), node(n), control, *r);
            }
        }
    }
    let mut groups: Vec<Box<dyn DeviceGroup>> = vec![
        Box::new(res),
        Box::new(cap),
        Box::new(ind),
        Box::new(vsrc),
        Box::new(isrc),
        Box::new(e_src),
        Box::new(g_src),
        Box::new(f_src),
        Box::new(h_src),
    ];
    groups.retain(|g| !g.is_empty());

    // --- Unknowns and stamps ---
    let mut unknowns = Unknowns::with_nodes(node_names.iter().map(String::as_str));
    for g in &mut groups {
        g.alloc_unknowns(&mut unknowns);
    }
    let mut layouts = Vec::with_capacity(groups.len());
    let mut errors = Vec::new();
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
    let node_diag = (0..nn)
        .map(|i| pattern.diag(i).unwrap_or(DISCARD))
        .collect();
    let linear = groups.iter().all(|g| g.is_linear());

    Ok((
        Circuit {
            unknowns,
            num_nodes: nn,
            groups,
            layouts,
            pattern,
            gmap,
            cmap,
            node_diag,
            linear,
            no_dc_path,
        },
        diags,
    ))
}
