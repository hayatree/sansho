//! Recording explicitly requested operating-point quantities at accepted points.
//! Provenance: ngspice user manual, device outputs and .options savecurrents.
//! Transient capacitor currents use the integrator's device-local charge history.

use crate::{circuit::Circuit, solve::Workspace};
use sansho_devices::{EvalCtx, Slots, StateRef};
use sansho_diag::Diagnostic;
use sansho_netlist::Netlist;
use sansho_output::{Quantity, Variable};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn currents(nl: &Netlist) -> impl Iterator<Item = String> + '_ {
    nl.elements.iter().flat_map(|e| {
        let names: &[&str] = match e.name.as_bytes().first() {
            Some(b'r' | b'c' | b'l' | b'b' | b'e' | b'g' | b'f' | b'h' | b's' | b'w' | b'v') => {
                &["i"]
            }
            Some(b'i') => &["current"],
            Some(b'd') => &["id"],
            Some(b'q') => &["ic", "ib", "ie", "is"],
            Some(b'j' | b'z') => &["id", "ig", "is"],
            Some(b'm') if matches!(e.kind, sansho_netlist::ElementKind::Vdmos { .. }) => {
                &["id", "ig", "is", "ibd"]
            }
            Some(b'm') => &["id", "is", "ig", "ib", "ibd", "ibs"],
            _ => &[],
        };
        names.iter().map(|p| format!("@{}[{p}]", e.name))
    })
}

struct Group {
    index: usize,
    slots: Slots,
    state: Vec<f64>,
    fields: Vec<Field>,
}

struct Field {
    instance: usize,
    parameter: String,
    variable: Variable,
    values: Vec<f64>,
    derivative: Vec<(usize, f64)>,
}

#[derive(Default)]
pub(crate) struct Probes {
    groups: Vec<Group>,
}

fn diagnostic(name: &str) -> Diagnostic {
    Diagnostic::error(
        "E-OUTPUT-VECTOR",
        format!("unknown device or output parameter in {name}"),
    )
}

fn quantity(kind: &str, dev: &str, p: &str) -> Option<Quantity> {
    use Quantity::*;
    let mos = matches!(
        kind,
        "mos1" | "mos2" | "mos3" | "mos6" | "mos9" | "bsim1" | "bsim2" | "bsim3" | "bsim4"
    );
    let bipolar = matches!(kind, "bjt" | "vbic");
    let bsim = matches!(kind, "bsim1" | "bsim2" | "bsim3" | "bsim4");
    Some(match p {
        "i" if matches!(
            kind,
            "resistor"
                | "capacitor"
                | "inductor"
                | "vsource"
                | "isource"
                | "vcvs"
                | "vccs"
                | "ccvs"
                | "cccs"
                | "switch"
                | "behavioral"
                | "poly"
        ) =>
        {
            Current
        }
        "current" if kind == "isource" => Current,
        "p" if matches!(kind, "resistor" | "vsource" | "isource")
            || (kind == "behavioral" && dev.starts_with('r')) =>
        {
            Power
        }
        "p" if kind == "vbic" => Power,
        "vbe" | "vbc" if kind == "vbic" => Voltage,
        "ic" | "ie" if bipolar => Current,
        "ig" if mos || matches!(kind, "jfet" | "jfet2" | "mesfet" | "vdmos") => Current,
        "is" if mos || matches!(kind, "vbic" | "bjt" | "jfet" | "jfet2" | "mesfet" | "vdmos") => {
            Current
        }
        "ib" if mos || bipolar => Current,
        "id" if mos || matches!(kind, "diode" | "jfet" | "jfet2" | "mesfet" | "vdmos") => Current,
        "ibd" if kind == "vdmos" => Current,
        "vgs" | "vds" if kind == "vdmos" => Voltage,
        "cgs" | "cgd" if matches!(kind, "vdmos" | "jfet2") => Capacitance,
        "ibd" | "ibs" if mos => Current,
        "isub" if matches!(kind, "bsim3" | "bsim4") => Current,
        "igs" | "igd" | "igb" | "igcd" | "igcs" if kind == "bsim4" => Current,
        "gm" if mos || matches!(kind, "vbic" | "bjt" | "jfet" | "jfet2" | "mesfet" | "vdmos") => {
            Admittance
        }
        "gds" if mos || matches!(kind, "jfet" | "jfet2" | "mesfet" | "vdmos") => Admittance,
        "qgs" | "qgd" if matches!(kind, "mesfet" | "jfet2") => Charge,
        "vgs" | "vgd" if matches!(kind, "mesfet" | "jfet2") => Voltage,
        "i1" | "i2" if matches!(kind, "urc" | "ltra" | "tline") => Current,
        "v1" | "v2" if matches!(kind, "ltra" | "tline") => Voltage,
        "capcur" if kind == "diode" => Current,
        "gd" if kind == "diode" => Admittance,
        "go" | "gpi" | "gmu" if bipolar => Admittance,
        "gmbs" if mos => Admittance,
        "cd" if kind == "mos9" => Current,
        "vgs" | "vds" | "vbs" | "vbd" if kind == "mos9" => Voltage,
        "cd" if kind == "diode" => Capacitance,
        "charge" if kind == "diode" => Charge,
        "cpi" | "cmu" if bipolar => Capacitance,
        "vth" | "vdsat" if mos => Voltage,
        "von" if mos && !bsim => Voltage,
        "cgg" | "cgs" | "cgd" if mos => Capacitance,
        "qg" | "qd" | "qs" | "qb" | "qbs" | "qbd" if bsim => Charge,
        "cgb" | "csd" | "css" | "csg" | "csb" | "cdg" | "cdb" | "cds" | "cdd" | "cbs" | "cbg"
        | "cbd" | "cbb" | "capbd" | "capbs"
            if bsim =>
        {
            Capacitance
        }
        _ => return None,
    })
}

impl Probes {
    pub(crate) fn bind(
        ckt: &Circuit,
        nl: &Netlist,
        savecurrents: bool,
    ) -> Result<Self, Diagnostic> {
        let mut requests: BTreeSet<_> = nl
            .saves
            .iter()
            .filter(|s| s.starts_with('@'))
            .cloned()
            .collect();
        for s in nl
            .prints
            .iter()
            .flat_map(|c| &c.signals)
            .chain(nl.measures.iter().flat_map(|m| m.kind.signals()))
        {
            if s.is_vector() && s.pos.starts_with('@') {
                requests.insert(s.pos.clone());
            }
        }
        if savecurrents {
            requests.extend(currents(nl));
        }
        if requests.is_empty() && nl.control_probes.is_empty() {
            return Ok(Self::default());
        }
        let mut devices = BTreeMap::new();
        for (k, g) in ckt.groups.iter().enumerate() {
            for i in 0..g.len() {
                if let Some(name) = g.instance_name(i) {
                    devices.insert(name, (k, i));
                }
            }
        }
        // A control expression may refer to either a static model/instance parameter or
        // an observable. Only the latter needs sampling. Explicit save requests above
        // still diagnose unknown names.
        for name in &nl.control_probes {
            if let Some((dev, param)) = name
                .strip_prefix('@')
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.split_once('['))
                && let Some(&(k, _)) = devices.get(dev)
                && quantity(ckt.groups[k].type_name(), dev, param).is_some()
            {
                requests.insert(name.clone());
            }
        }
        let mut grouped = BTreeMap::<usize, Vec<Field>>::new();
        let mut offsets = Vec::new();
        let mut offset = ckt.dim();
        for layout in &ckt.layouts {
            offsets.push(offset);
            offset += layout.lte_q.len();
        }
        for name in requests {
            let (dev, p) = name
                .strip_prefix('@')
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.split_once('['))
                .ok_or_else(|| diagnostic(&name))?;
            let &(k, i) = devices.get(dev).ok_or_else(|| diagnostic(&name))?;
            let kind = ckt.groups[k].type_name();
            let quantity = quantity(kind, dev, p).ok_or_else(|| diagnostic(&name))?;
            let derivative = ckt.groups[k]
                .output_charges(i, p)
                .into_iter()
                .map(|(slot, sign)| {
                    ckt.layouts[k]
                        .lte_q
                        .binary_search(&slot)
                        .map(|j| (offsets[k] + j, sign))
                        .map_err(|_| diagnostic(&name))
                })
                .collect::<Result<_, _>>()?;
            grouped.entry(k).or_default().push(Field {
                instance: i,
                parameter: p.into(),
                variable: Variable { name, quantity },
                values: Vec::new(),
                derivative,
            });
        }
        let groups = grouped
            .into_iter()
            .map(|(index, fields)| {
                let mut slots = Slots::for_layout(&ckt.layouts[index]);
                slots.outputs.resize(ckt.groups[index].len(), Vec::new());
                Group {
                    index,
                    fields,
                    slots,
                    state: vec![0.; ckt.state_len[index]],
                }
            })
            .collect();
        Ok(Self { groups })
    }

    pub(crate) fn sample(
        &mut self,
        ckt: &Circuit,
        ws: &Workspace,
        ctx: &EvalCtx,
        x: &[f64],
        qdot: &[f64],
    ) -> Result<(), Diagnostic> {
        let ctx = EvalCtx {
            limit: false,
            init_junctions: false,
            ..*ctx
        };
        for group in &mut self.groups {
            let k = group.index;
            let accepted = &ws.accepted[k];
            group.state.copy_from_slice(accepted);
            for values in &mut group.slots.outputs {
                values.clear();
            }
            ckt.groups[k].eval(
                &ctx,
                x,
                StateRef {
                    accepted,
                    current: &mut group.state,
                },
                &mut group.slots,
            );
            if let Some(error) = group.slots.error.take() {
                return Err(error);
            }
            for field in &mut group.fields {
                let v = group.slots.outputs[field.instance]
                    .iter()
                    .find(|(p, _)| *p == field.parameter)
                    .ok_or_else(|| diagnostic(&field.variable.name))?
                    .1;
                let total = field.derivative.iter().fold(v, |a, &(i, sign)| {
                    a + sign * qdot.get(i).copied().unwrap_or(0.)
                });
                field.values.push(total);
            }
        }
        Ok(())
    }

    /// Bias quantities are real constants across the frequency sweep. Branch
    /// currents and two-terminal small-signal currents follow the AC solution.
    pub(crate) fn ac_columns(
        self,
        ckt: &Circuit,
        freqs: &[f64],
        vars: &mut Vec<Variable>,
        cols: &mut Vec<Vec<(f64, f64)>>,
    ) -> Result<(), Diagnostic> {
        for group in self.groups {
            let device = &ckt.groups[group.index];
            for field in group.fields {
                let column = if matches!(field.parameter.as_str(), "i" | "current") {
                    let name = device.instance_name(field.instance).unwrap_or("");
                    if let Some(branch) = ckt.unknowns.branch(name) {
                        cols[branch].clone()
                    } else {
                        let mut terms = Vec::new();
                        let (re, im) = device
                            .ac_current(field.instance, &group.slots, &mut |node, g, c| {
                                if let Some(node) = node {
                                    terms.push((node, g, c));
                                }
                            })
                            .ok_or_else(|| diagnostic(&field.variable.name))?;
                        freqs
                            .iter()
                            .enumerate()
                            .map(|(i, f)| {
                                let w = std::f64::consts::TAU * f;
                                terms.iter().fold((re, im), |(a, b), &(k, g, c)| {
                                    let (vr, vi) = cols[k][i];
                                    (a + g * vr - w * c * vi, b + g * vi + w * c * vr)
                                })
                            })
                            .collect()
                    }
                } else {
                    vec![(field.values[0], 0.); freqs.len()]
                };
                vars.push(field.variable);
                cols.push(column);
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self, vars: &mut Vec<Variable>, cols: &mut Vec<Vec<f64>>) {
        for field in self.groups.into_iter().flat_map(|g| g.fields) {
            vars.push(field.variable);
            cols.push(field.values);
        }
    }
}
