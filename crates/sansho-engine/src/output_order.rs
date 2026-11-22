//! Column order for public circuit plots.
//! Provenance: original implementation from ngspice 44.2 black-box raw headers
//! collected with the Sansho verification decks and R22b/R22c probes.

use std::cmp::Reverse;
use std::collections::HashMap;

use sansho_netlist::{ElementKind, Netlist};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};

fn circuit_plot(kind: AnalysisKind) -> bool {
    kind.is_distortion()
        || matches!(
            kind,
            AnalysisKind::Op
                | AnalysisKind::Dc
                | AnalysisKind::Ac
                | AnalysisKind::Tran
                | AnalysisKind::Pss
                | AnalysisKind::PssFreq
                | AnalysisKind::Sens
                | AnalysisKind::Sp
        )
}

pub(crate) fn model_instances(nl: &Netlist) -> HashMap<&str, (usize, usize)> {
    let mut first_use = HashMap::new();
    nl.elements
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let model = e
                .kind
                .model()
                .map_or(0, |m| *first_use.entry(m).or_insert(i));
            (e.name.as_str(), (model, i))
        })
        .collect()
}

/// Append the internal node of each RF port source to a default-output circuit
/// plot. ngspice 44.2 lists `v(<port>#res)` in every circuit plot except
/// sensitivity (R22f probes). The node sits between the ideal source and its
/// series reference impedance, so by Kirchhoff's voltage law it equals
/// `v(p) - z0 * i(port)` in every analysis, real or complex.
pub(crate) fn add_port_nodes(plot: &mut AnalysisResult, nl: &Netlist) {
    if !circuit_plot(plot.kind) || matches!(plot.kind, AnalysisKind::Sens | AnalysisKind::Sp) {
        return;
    }
    for e in &nl.elements {
        let ElementKind::VoltageSource { source, p, .. } = &e.kind else {
            continue;
        };
        let Some((_, z)) = source.port else {
            continue;
        };
        let name = format!("v({}#res)", e.name);
        let find = |n: &str| plot.variables.iter().position(|v| v.name == n);
        if find(&name).is_some() {
            continue;
        }
        let Some(branch) = find(&format!("i({})", e.name)) else {
            continue;
        };
        let pin = if p == "0" || p.eq_ignore_ascii_case("gnd") {
            None
        } else {
            let Some(i) = find(&format!("v({p})")) else {
                continue;
            };
            Some(i)
        };
        match &mut plot.data {
            Data::Real { columns } => {
                let column = columns[branch]
                    .iter()
                    .enumerate()
                    .map(|(k, &i)| pin.map_or(0., |c| columns[c][k]) - z * i)
                    .collect();
                columns.push(column);
            }
            Data::Complex { columns } => {
                let column = columns[branch]
                    .iter()
                    .enumerate()
                    .map(|(k, &(re, im))| {
                        let (vr, vi) = pin.map_or((0., 0.), |c| columns[c][k]);
                        (vr - z * re, vi - z * im)
                    })
                    .collect();
                columns.push(column);
            }
        }
        plot.variables.push(Variable {
            name,
            quantity: Quantity::Voltage,
        });
    }
}

/// Move columns together with their names after analysis. Solver unknown indices
/// and the order of all floating-point stamp contributions are independent of this.
pub(crate) fn default_order(plot: &mut AnalysisResult, nl: &Netlist, external: usize) {
    // Sensitivity columns refer to parameters of devices and models.
    let external = if plot.kind == AnalysisKind::Sens {
        0
    } else {
        external
    };
    if !circuit_plot(plot.kind) || external >= plot.variables.len() {
        return;
    }
    let devices = model_instances(nl);
    // A behavioral or current-controlled source can introduce a controlling
    // branch while its own family is being set up. Keep the earliest such visit.
    let family = |e: &sansho_netlist::Element| match e.kind {
        ElementKind::Va { .. } | ElementKind::CodeModel { .. } => u16::from(b'z') * 2,
        ElementKind::Vcvs { .. } => u16::from(b'l') * 2 + 1, // after inductors
        _ => u16::from(e.name.bytes().next().unwrap_or(u8::MAX)) * 2,
    };
    let mut controls = HashMap::new();
    if plot.kind != AnalysisKind::Sens {
        for &(model, instance) in devices.values() {
            let e = &nl.elements[instance];
            let branches = match &e.kind {
                ElementKind::Behavioral { expr, .. } if expr.contains("i(") => {
                    sansho_netlist::current_probe_branches(expr).unwrap_or_default()
                }
                ElementKind::Ccvs { control, .. } | ElementKind::Cccs { control, .. } => {
                    vec![control.clone()]
                }
                _ => continue,
            };
            for (j, branch) in branches.into_iter().enumerate() {
                let visit = (family(e), Reverse(model), Reverse(instance), j + 1);
                controls
                    .entry(branch)
                    .and_modify(|prior| *prior = visit.min(*prior))
                    .or_insert(visit);
            }
        }
    }
    let mut saved_parameters = HashMap::new();
    for (i, name) in nl
        .saves
        .iter()
        .enumerate()
        .filter(|(_, name)| name.starts_with('@'))
    {
        saved_parameters.entry(name.as_str()).or_insert(i);
    }
    let mut indices: Vec<_> = (external..plot.variables.len()).collect();
    indices.sort_by_cached_key(|&i| {
        let name = &plot.variables[i].name;
        let id = name
            .strip_prefix("i(")
            .or_else(|| name.strip_prefix("v("))
            .and_then(|s| s.strip_suffix(')'));
        let (device, suffix) = if plot.kind == AnalysisKind::Sens {
            if devices.contains_key(name.as_str()) {
                (name.as_str(), "")
            } else {
                name.split_once(':')
                    .or_else(|| name.rsplit_once('_'))
                    .unwrap_or((name.as_str(), ""))
            }
        } else {
            id.map_or(("", ""), |s| s.split_once('#').unwrap_or((s, "")))
        };
        let Some(&(model, instance)) = devices.get(device) else {
            return (
                if name.starts_with('@') {
                    u16::MAX
                } else {
                    u16::MAX - 1
                },
                Reverse(0),
                Reverse(0),
                saved_parameters
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(usize::MAX),
            );
        };
        // SP ports put the series-resistance node immediately after the branch.
        // MOS body networks introduce drain-body, body, then source-body.
        let terminal = match suffix {
            "dbody" => 1,
            "body" => 2,
            "sbody" => 3,
            "res" => 1,
            _ => 0,
        };
        let own = (
            family(&nl.elements[instance]),
            Reverse(model),
            Reverse(instance),
            terminal,
        );
        if suffix.is_empty() && name.starts_with("i(") {
            controls
                .get(device)
                .copied()
                .map_or(own, |visit| visit.min(own))
        } else {
            own
        }
    });
    // The sorted source indices describe a permutation. Consume each cycle
    // in place so transient waveforms never have to be copied.
    for i in 0..indices.len() {
        let mut j = i;
        while indices[j] != external + i {
            let k = indices[j] - external;
            plot.variables.swap(external + j, external + k);
            match &mut plot.data {
                Data::Real { columns } => columns.swap(external + j, external + k),
                Data::Complex { columns } => columns.swap(external + j, external + k),
            }
            indices[j] = external + j;
            j = k;
        }
        indices[j] = external + j;
    }
}
