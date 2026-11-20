//! Operating-point transfer, sensitivity, noise and finite pole-zero analyses.
//! Provenance: original MNA formulation from Director & Rohrer (1969), Rohrer,
//! Nagel, Meyer & Weber (1971) and Vlach & Singhal, Computer Methods for Circuit Analysis.

use crate::analysis::{DelayedStamp, ac_matrix};
use crate::{
    Circuit, SimOptions,
    analysis::linearize,
    solve::{Workspace, assemble},
};
use sansho_devices::{Mode, Node};
use sansho_diag::Diagnostic;
use sansho_netlist::{ElementKind, Netlist, Probe};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};
use sansho_sparse::{SerialLu, SolverError, c64};
use std::{collections::BTreeMap, f64::consts::TAU};

pub(super) fn error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-ANALYSIS", msg)
}
pub(super) fn solver(e: SolverError) -> Diagnostic {
    Diagnostic::error("E-SOLVE-SINGULAR", e.to_string())
}
pub(super) fn var(name: impl Into<String>, quantity: Quantity) -> Variable {
    Variable {
        name: name.into(),
        quantity,
    }
}
pub(super) fn node(c: &Circuit, s: &str) -> Result<Node, Diagnostic> {
    if s == "0" {
        return Ok(None);
    }
    let name = format!("v({s})");
    c.unknowns
        .names()
        .iter()
        .position(|v| v == &name)
        .map(Some)
        .or_else(|| {
            c.aliases
                .iter()
                .find(|(alias, _)| alias == &name)
                .map(|(_, n)| *n)
        })
        .ok_or_else(|| error(format!("unknown node {s}")))
}
type Port = [Node; 2];
fn port(c: &Circuit, p: &Probe) -> Result<Port, Diagnostic> {
    match p {
        Probe::Voltage(a, b) => Ok([node(c, a)?, node(c, b)?]),
        Probe::Current(b) => Ok([
            Some(
                c.unknowns
                    .branch(b)
                    .ok_or_else(|| error(format!("unknown current branch {b}")))?,
            ),
            None,
        ]),
    }
}
struct Input {
    drive: Port,
    voltage: bool,
}
fn input(c: &Circuit, nl: &Netlist, name: &str) -> Result<Input, Diagnostic> {
    let element = nl.elements.iter().find(|e| e.name == name);
    match element.map(|e| &e.kind) {
        Some(ElementKind::VoltageSource { .. }) => {
            let k = port(c, &Probe::Current(name.into()))?;
            Ok(Input {
                drive: k,
                voltage: true,
            })
        }
        Some(ElementKind::CurrentSource { p, n, .. }) => {
            let drive = [node(c, n)?, node(c, p)?];
            Ok(Input {
                drive,
                voltage: false,
            })
        }
        _ => Err(error(format!("expected independent input source {name}"))),
    }
}
fn inject<T: From<f64> + Copy + std::ops::AddAssign>(x: &mut [T], [p, n]: Port) {
    if let Some(i) = p {
        x[i] += 1.0.into()
    }
    if let Some(i) = n {
        x[i] += (-1.0).into()
    }
}
pub(super) fn read<T: Default + Copy + std::ops::Sub<Output = T>>(x: &[T], [p, n]: Port) -> T {
    p.map_or(T::default(), |i| x[i]) - n.map_or(T::default(), |i| x[i])
}
fn dc_lu(c: &Circuit, w: &Workspace) -> Result<SerialLu<f64>, Diagnostic> {
    let mut lu = SerialLu::new(&c.pattern).map_err(solver)?;
    lu.factor(&w.asm.g).map_err(solver)?;
    Ok(lu)
}
fn scalars(kind: AnalysisKind, values: Vec<(Variable, f64)>) -> AnalysisResult {
    let (variables, columns) = values.into_iter().map(|(v, x)| (v, vec![x])).unzip();
    AnalysisResult {
        kind,
        variables,
        data: Data::Real { columns },
        scale: None,
        scale_values: vec![],
    }
}
pub(super) fn tf(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    output: &Probe,
    source: &str,
) -> Result<AnalysisResult, Diagnostic> {
    let out = port(c, output)?;
    let src = input(c, nl, source)?;
    let (_, ws, _) = linearize(c, opts, Mode::Dc)?;
    let mut lu = dc_lu(c, &ws)?;
    let mut solve = |p| {
        let mut x = vec![0.; c.dim()];
        inject(&mut x, p);
        lu.solve(&mut x).map_err(solver)?;
        Ok::<_, Diagnostic>(x)
    };
    let x = solve(src.drive)?;
    let reciprocal = |v: f64| if v.abs() < 1e-20 { 1e20 } else { -1. / v };
    let input_response = read(&x, src.drive);
    let rin = if src.voltage {
        reciprocal(input_response)
    } else {
        input_response
    };
    let test = read(&solve(out)?, out);
    let voltage = matches!(output, Probe::Voltage(..));
    let rout = if voltage { test } else { reciprocal(test) };
    let gain_unit = match (voltage, src.voltage) {
        (true, false) => Quantity::Resistance,
        (false, true) => Quantity::Admittance,
        _ => Quantity::Scalar,
    };
    let out_name = match output {
        Probe::Current(b) => format!("{b}#output_impedance"),
        _ => format!("output_impedance_at_{output}"),
    };
    Ok(scalars(
        AnalysisKind::Tf,
        vec![
            (var("transfer_function", gain_unit), read(&x, out)),
            (
                var(format!("{source}#input_impedance"), Quantity::Resistance),
                rin,
            ),
            (var(out_name, Quantity::Resistance), rout),
        ],
    ))
}

/// Exact integral of the power law through adjacent positive spectral samples.
fn area(f0: f64, f1: f64, a: f64, b: f64) -> f64 {
    if a == b || a == 0. || b == 0. {
        return (f1 - f0) * (0.5 * a + 0.5 * b);
    }
    let log_f = ((f1 - f0) / f0).ln_1p();
    let z = (b.ln() - a.ln()) + log_f;
    if z.abs() < 1e-8 {
        a * f0 * log_f * (1. + 0.5 * z)
    } else {
        (b * f1 - a * f0) * log_f / z
    }
}
pub(super) fn noise(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    output: &Probe,
    source: &str,
    freq: Vec<f64>,
) -> Result<Vec<AnalysisResult>, Diagnostic> {
    let out = port(c, output)?;
    let src = input(c, nl, source)?;
    let (x, ws, ctx) = linearize(c, opts, Mode::Noise)?;
    let mut generators = vec![];
    for (group, slots) in c.groups.iter().zip(&ws.slots) {
        group.noise(&ctx, &x, slots, &mut generators)?;
    }
    let mut by_device = BTreeMap::new();
    for g in &generators {
        by_device.insert(g.device.clone(), 0);
    }
    for (i, idx) in by_device.values_mut().enumerate() {
        *idx = i;
    }
    let device_indices: Vec<_> = generators.iter().map(|g| by_device[&g.device]).collect();
    let mut columns = vec![Vec::with_capacity(freq.len()); 2 + by_device.len()];
    let mut totals = vec![[0.; 2]; generators.len()];
    let mut previous = totals.clone();
    let mut lu = SerialLu::new(&c.pattern).map_err(solver)?;
    let delayed = DelayedStamp::collect(c, &ws);
    let mut matrix = vec![c64::default(); c.nnz()];
    for (k, &f) in freq.iter().enumerate() {
        let omega = TAU * f;
        ac_matrix(&ws.asm.g, &ws.asm.c, &delayed, omega, &mut matrix);
        let mut adj = vec![c64::default(); c.dim()];
        inject(&mut adj, out);
        lu.factor(&matrix)
            .and_then(|_| lu.solve_transpose(&mut adj))
            .map_err(solver)?;
        let gain = read(&adj, src.drive).norm_sqr();
        if gain == 0. || !gain.is_finite() {
            return Err(error(format!(
                "undefined input-referred noise at {f} Hz (zero or non-finite gain)"
            )));
        }
        let mut powers = vec![0.; by_device.len()];
        for (j, g) in generators.iter().enumerate() {
            let transfer: c64 = g
                .rows
                .iter()
                .map(|&(i, a, b)| adj[i] * c64::new(a, b * omega))
                .sum();
            let psd = g.spectral_power(f) * transfer.norm_sqr();
            let pair = [psd, psd / gain];
            if pair.iter().any(|p| !p.is_finite() || *p < 0.) {
                return Err(error(format!(
                    "invalid noise power from {} at {f} Hz",
                    g.device
                )));
            }
            powers[device_indices[j]] += psd;
            if k > 0 {
                for (i, &value) in pair.iter().enumerate() {
                    totals[j][i] += area(freq[k - 1], f, previous[j][i], value);
                }
            }
            previous[j] = pair;
        }
        let sum = powers.iter().sum::<f64>();
        columns[0].push(sum.sqrt());
        columns[1].push((sum / gain).sqrt());
        for (col, p) in columns[2..].iter_mut().zip(powers) {
            col.push(p.sqrt());
        }
    }
    let mut vars = vec![
        var("onoise_spectrum", Quantity::VoltageDensity),
        var(
            "inoise_spectrum",
            if src.voltage {
                Quantity::VoltageDensity
            } else {
                Quantity::CurrentDensity
            },
        ),
    ];
    for name in by_device.keys() {
        vars.push(var(format!("onoise.{name}"), Quantity::VoltageDensity));
    }
    let spectrum = AnalysisResult {
        kind: AnalysisKind::Noise,
        variables: vars,
        data: Data::Real { columns },
        scale: Some(var("frequency", Quantity::Frequency)),
        scale_values: freq,
    };
    let mut values = vec![
        (
            var("onoise_total", Quantity::Voltage),
            totals.iter().map(|v| v[0]).sum::<f64>().sqrt(),
        ),
        (
            var(
                "inoise_total",
                if src.voltage {
                    Quantity::Voltage
                } else {
                    Quantity::Current
                },
            ),
            totals.iter().map(|v| v[1]).sum::<f64>().sqrt(),
        ),
    ];
    let mut parts = vec![0.; by_device.len()];
    for (&i, v) in device_indices.iter().zip(&totals) {
        parts[i] += v[0];
    }
    for (name, i) in by_device {
        values.push((
            var(format!("onoise_total.{name}"), Quantity::Voltage),
            parts[i].sqrt(),
        ));
    }
    Ok(vec![
        spectrum,
        scalars(AnalysisKind::IntegratedNoise, values),
    ])
}

#[derive(Clone, Copy)]
enum Parameter {
    Element(usize),
    Model(usize, usize),
    Instance(usize, usize),
    LineImpedance(usize),
    LineDelay(usize),
    Polynomial(usize, usize),
}
impl Parameter {
    fn perturbations(self, nl: &Netlist, value: f64) -> (f64, f64) {
        let h = if value == 0. {
            1e-6
        } else {
            value.abs() * 1e-5
        };
        if let Self::Model(m, p) = self {
            let card = &nl.models[m];
            if card.kind == "vdmos" {
                let get = |name: &str| {
                    card.params
                        .iter()
                        .rev()
                        .find(|(key, _)| key == name)
                        .map_or(0., |(_, v)| *v)
                };
                match card.params[p].0.as_str() {
                    "cgdmin" => return ((value + h).min(get("cgdmax")), (value - h).max(0.)),
                    "cgdmax" => return (value + h, (value - h).max(get("cgdmin"))),
                    _ => {}
                }
            }
        }
        let lower_bound = value == 0.
            && matches!(self, Self::Model(m,p) if matches!(nl.models[m].params[p].0.as_str(),"gamma"|"kp"|"beta"|"is"));
        (value + h, if lower_bound { value } else { value - h })
    }

    fn set(self, nl: &mut Netlist, v: f64) {
        match self {
            Self::LineImpedance(i) => {
                if let ElementKind::Line { z0, .. } = &mut nl.elements[i].kind {
                    *z0 = v;
                }
            }
            Self::LineDelay(i) => {
                if let ElementKind::Line { td, .. } = &mut nl.elements[i].kind {
                    *td = v;
                }
            }
            Self::Polynomial(i, j) => {
                if let ElementKind::Polynomial { coefficients, .. } = &mut nl.elements[i].kind {
                    coefficients[j] = v;
                }
            }
            Self::Model(m, p) => nl.models[m].params[p].1 = v,
            Self::Instance(i, j) => match &mut nl.elements[i].kind {
                ElementKind::Bjt { params, .. }
                | ElementKind::Urc { params, .. }
                | ElementKind::Vdmos { params, .. }
                | ElementKind::Mesfet { params, .. } => params[j].1 = v,
                _ => {}
            },
            Self::Element(i) => match &mut nl.elements[i].kind {
                ElementKind::Resistor { value, .. }
                | ElementKind::Capacitor { value, .. }
                | ElementKind::Inductor { value, .. } => *value = v,
                ElementKind::VoltageSource { source, .. }
                | ElementKind::CurrentSource { source, .. } => source.dc = Some(v),
                _ => {}
            },
        }
    }
}
fn parameters(nl: &Netlist) -> (Netlist, Vec<(String, Parameter, f64)>) {
    let mut nominal = nl.clone();
    let mut params = vec![];
    for (i, e) in nl.elements.iter().enumerate() {
        let value = match &e.kind {
            ElementKind::Bjt {
                params: instance, ..
            }
            | ElementKind::Urc {
                params: instance, ..
            }
            | ElementKind::Vdmos {
                params: instance, ..
            }
            | ElementKind::Mesfet {
                params: instance, ..
            } => {
                let selected: &[&str] = match &e.kind {
                    ElementKind::Bjt { .. } => &["areab", "areac"],
                    ElementKind::Urc { .. } => &["l"],
                    ElementKind::Mesfet { .. } => &["area", "m", "mult"],
                    _ => &["m", "mult"],
                };
                for (j, (key, v)) in instance.iter().enumerate() {
                    if selected.contains(&key.as_str())
                        && *v > 0.
                        && !instance[j + 1..].iter().any(|(k, _)| k == key)
                    {
                        params.push((format!("{}:{key}", e.name), Parameter::Instance(i, j), *v));
                    }
                }
                continue;
            }
            ElementKind::Line { z0, td, .. } => {
                params.push((format!("{}:z0", e.name), Parameter::LineImpedance(i), *z0));
                params.push((format!("{}:td", e.name), Parameter::LineDelay(i), *td));
                continue;
            }
            ElementKind::Polynomial {
                coefficients,
                controls,
                ..
            } => {
                for (j, &v) in coefficients.iter().enumerate() {
                    let term = if coefficients.len() == 1 && controls.len() == 1 {
                        1
                    } else {
                        j
                    };
                    params.push((
                        format!("{}:p{term}", e.name),
                        Parameter::Polynomial(i, j),
                        v,
                    ));
                }
                continue;
            }
            ElementKind::Resistor { value, .. }
            | ElementKind::Capacitor { value, .. }
            | ElementKind::Inductor { value, .. } => *value,
            ElementKind::VoltageSource { source, .. }
            | ElementKind::CurrentSource { source, .. } => sansho_devices::dc_value(source),
            _ => continue,
        };
        params.push((e.name.clone(), Parameter::Element(i), value));
    }
    for (i, m) in nominal.models.iter_mut().enumerate() {
        let level = m
            .params
            .iter()
            .find(|(k, _)| k == "level")
            .map_or(1., |(_, v)| *v);
        let defaults: &[(&str, f64)] = match m.kind.as_str() {
            "d" => &[("is", 1e-14), ("n", 1.)],
            "nmf" | "pmf" => &[("vto", -2.), ("beta", 2.5e-3), ("alpha", 2.)],
            "njf" | "pjf" => &[("vto", -2.), ("beta", 1e-4), ("lambda", 0.)],
            "npn" | "pnp" => &[("is", 1e-16), ("bf", 100.), ("br", 1.)],
            "nmos" | "pmos" if level == 1. => &[("gamma", 0.), ("lambda", 0.), ("phi", 0.6)],
            _ => &[],
        };
        for &(key, v) in defaults {
            if !m.params.iter().any(|(k, _)| k == key) {
                m.params.push((key.into(), v));
            }
        }
        if level == 1. && matches!(m.kind.as_str(), "nmos" | "pmos") {
            let binding = sansho_devices::ModelParams::new(&m.params);
            let (model, _) = sansho_devices::mos1::Mos1Model::from_params(
                &binding,
                if m.kind == "nmos" { 1. } else { -1. },
            );
            if binding.opt(&["vto", "vt0"]).is_none() {
                m.params.push(("vto".into(), model.vto));
            }
            // Preserve the mobility/oxide dependence of the derived KP when
            // those parameters are supplied. Perturb KP only on its own card.
            if binding.opt(&["kp", "tox", "u0", "uo"]).is_none() {
                m.params.push(("kp".into(), model.kp));
            }
        }
        for (j, (key, v)) in m.params.iter().enumerate() {
            if m.params[j + 1..].iter().any(|(k, _)| k == key) {
                continue;
            }

            if matches!(
                key.as_str(),
                "is" | "n"
                    | "bf"
                    | "br"
                    | "nf"
                    | "nr"
                    | "vaf"
                    | "ikf"
                    | "ise"
                    | "vto"
                    | "vt0"
                    | "gamma"
                    | "kp"
                    | "lambda"
                    | "phi"
                    | "u0"
                    | "uo"
                    | "vth0"
                    | "k1"
                    | "k2"
                    | "vsat"
                    | "tox"
                    | "toxe"
                    | "tnom"
                    | "tc1"
                    | "tc2"
            ) || (matches!(m.kind.as_str(), "nmos" | "pmos")
                && matches!(level, 2. | 3.)
                && *v > 0.
                && matches!(
                    key.as_str(),
                    "theta"
                        | "eta"
                        | "vmax"
                        | "delta"
                        | "nsub"
                        | "nfs"
                        | "ucrit"
                        | "uexp"
                        | "kappa"
                        | "xj"
                ))
                || (matches!(m.kind.as_str(), "njf" | "pjf" | "nmf" | "pmf")
                    && matches!(key.as_str(), "beta" | "alpha" | "b" | "pb" | "cgs" | "cgd")
                    && *v != 0.)
                || (matches!(m.kind.as_str(), "urc" | "ltra")
                    && *v > 0.
                    && matches!(
                        key.as_str(),
                        "r" | "l" | "g" | "c" | "len" | "rperl" | "cperl" | "isperl" | "rsperl"
                    ))
                || (matches!(m.kind.as_str(), "npn" | "pnp")
                    && *v > 0.
                    && matches!(key.as_str(), "rco" | "vo" | "qco"))
                || (m.kind == "d"
                    && matches!(
                        key.as_str(),
                        "trs" | "trs1" | "trs2" | "ttt1" | "ttt2" | "tm1" | "tm2"
                    ))
                || (m.kind == "vdmos"
                    && *v > 0.
                    && matches!(
                        key.as_str(),
                        "mtriode"
                            | "theta"
                            | "cgs"
                            | "cgdmin"
                            | "cgdmax"
                            | "a"
                            | "rg"
                            | "cjo"
                            | "tt"
                            | "bv"
                    ))
                || (*v > 0. && matches!(key.as_str(), "rs" | "rd" | "rb" | "rc" | "re"))
            {
                params.push((format!("{}:{key}", m.name), Parameter::Model(i, j), *v));
            }
        }
    }
    (nominal, params)
}
fn bind(deck: &Netlist, c: &Circuit, opts: &SimOptions) -> Result<Circuit, Diagnostic> {
    let (other, _) =
        crate::circuit::compile_cached(deck, opts, &c.va_modules, false).map_err(|d| {
            d.into_iter()
                .next()
                .unwrap_or_else(|| error("parameter binding failed"))
        })?;
    if other.unknowns != c.unknowns
        || other.state_len != c.state_len
        || other.pattern.col_ptr() != c.pattern.col_ptr()
        || other.pattern.row_idx() != c.pattern.row_idx()
    {
        return Err(error("sensitivity perturbation changed topology"));
    }
    Ok(other)
}
pub(super) fn sens(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    output: &Probe,
) -> Result<AnalysisResult, Diagnostic> {
    let out = port(c, output)?;
    let (x, ws, ctx) = linearize(c, opts, Mode::Dc)?;
    let mut adj = vec![0.; c.dim()];
    inject(&mut adj, out);
    dc_lu(c, &ws)?.solve_transpose(&mut adj).map_err(solver)?;
    let (nominal, params) = parameters(nl);
    let evaluate = |deck: &Netlist| {
        let other = bind(deck, c, opts)?;
        let mut w = Workspace::new(&other);
        w.accepted.clone_from(&ws.accepted);
        w.restore();
        assemble(&other, &ctx, &x, &mut w, opts, 0.);
        if let Some(d) = w.slots.iter().find_map(|s| s.error.clone()) {
            return Err(d);
        }
        Ok(std::mem::take(&mut w.slots))
    };
    let mut values = vec![];
    for (name, param, v) in params {
        let (upper, lower) = param.perturbations(&nominal, v);
        let mut deck = nominal.clone();
        param.set(&mut deck, upper);
        let plus = evaluate(&deck)?;
        param.set(&mut deck, lower);
        let minus = evaluate(&deck)?;
        let mut derivative = 0.;
        for ((a, b), layout) in plus.iter().zip(&minus).zip(&c.layouts) {
            for (rows, ap, bp, sign) in [(&layout.b, &a.b, &b.b, 1.), (&layout.f, &a.f, &b.f, -1.)]
            {
                for ((row, av), bv) in rows.iter().zip(ap).zip(bp) {
                    if let Some(r) = row {
                        derivative += sign * adj[*r] * (av - bv);
                    }
                }
            }
        }
        let value = derivative / (upper - lower);
        if !value.is_finite() {
            return Err(error(format!("non-finite sensitivity for {name}")));
        }
        values.push((var(name, Quantity::Scalar), value));
    }
    Ok(scalars(AnalysisKind::Sens, values))
}

pub(super) fn pz(
    c: &Circuit,
    opts: &SimOptions,
    nodes: &[String; 4],
    voltage: bool,
    poles: bool,
    zeros: bool,
) -> Result<AnalysisResult, Diagnostic> {
    if c.groups
        .iter()
        .any(|g| g.line().is_some() || g.ltra().is_some_and(|line| line.values.c > 0.))
    {
        return Err(error(
            "pole-zero analysis requires lumped elements; a distributed line has an exponential delay",
        ));
    }
    if c.dim() > 256 {
        return Err(error(
            "pole-zero analysis supports at most 256 circuit unknowns",
        ));
    }
    let input = [node(c, &nodes[0])?, node(c, &nodes[1])?];
    let out = [node(c, &nodes[2])?, node(c, &nodes[3])?];
    if input[0] == input[1] || out[0] == out[1] {
        return Err(error("coincident pole-zero port terminals"));
    }
    let (_, ws, _) = linearize(c, opts, Mode::Ac)?;
    if !DelayedStamp::collect(c, &ws).is_empty() {
        return Err(error(
            "pole-zero analysis does not support exponential transport delay",
        ));
    }
    let mut variables = vec![];
    let mut columns = vec![];
    // Open-circuit current drive uses the natural pencil. Voltage drive adds a
    // short at the input for natural modes. Transmission zeros enforce vout=0
    // while admitting an arbitrary input current for either drive convention.
    for (label, border, enable) in [("pole", voltage, poles), ("zero", true, zeros)] {
        if !enable {
            continue;
        }
        let n = c.dim() + usize::from(border);
        let mut g = vec![0.; n * n];
        let mut cap = g.clone();
        for col in 0..c.dim() {
            for k in c.pattern.col_ptr()[col]..c.pattern.col_ptr()[col + 1] {
                let row = c.pattern.row_idx()[k];
                g[row * n + col] = ws.asm.g[k];
                cap[row * n + col] = ws.asm.c[k];
            }
        }
        if border {
            for (i, sign) in [(input[0], 1.), (input[1], -1.)] {
                if let Some(i) = i {
                    g[i * n + n - 1] += sign;
                }
            }
            let sense = if label == "pole" { input } else { out };
            for (i, sign) in [(sense[0], 1.), (sense[1], -1.)] {
                if let Some(i) = i {
                    g[(n - 1) * n + i] += sign;
                }
            }
        }
        let roots = sansho_sparse::pencil_roots(&g, &cap, n)
            .map_err(|e| error(format!("{label} pencil: {e}")))?;
        for (i, z) in roots.into_iter().enumerate() {
            variables.push(var(format!("{label}({})", i + 1), Quantity::PoleZero));
            columns.push(vec![(z.re, z.im)]);
        }
    }
    Ok(AnalysisResult {
        kind: AnalysisKind::Pz,
        variables,
        scale: None,
        scale_values: vec![],
        data: Data::Complex { columns },
    })
}

/// Differentiate the complex AC response, including the parameter dependence
/// of the bias point. Both nominal right-hand sides use the same numeric LU.
pub(super) fn ac_sens(
    c: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    output: &Probe,
    grid: sansho_netlist::FrequencySweep,
) -> Result<AnalysisResult, Diagnostic> {
    let port = port(c, output)?;
    let (_, ws, _) = linearize(c, opts, Mode::Ac)?;
    let excitation = crate::analysis::ac_rhs(c);
    let freq = crate::analysis::ac_frequencies(grid.sweep, grid.points, grid.fstart, grid.fstop)?;
    let mut states = Vec::with_capacity(freq.len());
    let mut lu = SerialLu::new(&c.pattern).map_err(solver)?;
    let delayed = DelayedStamp::collect(c, &ws);
    let mut matrix = vec![c64::default(); c.nnz()];
    for &f in &freq {
        let omega = TAU * f;
        ac_matrix(&ws.asm.g, &ws.asm.c, &delayed, omega, &mut matrix);
        let mut forward = excitation.clone();
        let mut adjoint = vec![c64::default(); c.dim()];
        inject(&mut adjoint, port);
        lu.factor(&matrix)
            .and_then(|_| lu.solve(&mut forward))
            .and_then(|()| lu.solve_transpose(&mut adjoint))
            .map_err(solver)?;
        states.push((forward, adjoint));
    }
    let (nominal, params) = parameters(nl);
    let mut variables = vec![];
    let mut columns = vec![];
    for (name, param, value) in params {
        let (upper, lower) = param.perturbations(&nominal, value);
        let mut deck = nominal.clone();
        let mut sample = |v| {
            param.set(&mut deck, v);
            let other = bind(&deck, c, opts)?;
            let (_, mut ws, _) = linearize(&other, opts, Mode::Ac)?;
            let delayed = DelayedStamp::collect(&other, &ws);
            Ok::<_, Diagnostic>((
                std::mem::take(&mut ws.asm.g),
                std::mem::take(&mut ws.asm.c),
                delayed,
            ))
        };
        let (g1, c1, d1) = sample(upper)?;
        let (g0, c0, d0) = sample(lower)?;
        let dp = upper - lower;
        let mut column = Vec::with_capacity(freq.len());
        for (&f, (forward, adjoint)) in freq.iter().zip(&states) {
            let omega = TAU * f;
            for (k, z) in matrix.iter_mut().enumerate() {
                *z = c64::new(g1[k] - g0[k], omega * (c1[k] - c0[k]));
            }
            for (sign, stamps) in [(1., &d1), (-1., &d0)] {
                for stamp in stamps {
                    stamp.apply(omega, sign, &mut matrix);
                }
            }
            let mut delta = c64::default();
            for (j, &u) in forward.iter().enumerate() {
                let range = c.pattern.col_ptr()[j]..c.pattern.col_ptr()[j + 1];
                for (&row, &entry) in c.pattern.row_idx()[range.clone()]
                    .iter()
                    .zip(&matrix[range])
                {
                    delta -= adjoint[row] * entry * u;
                }
            }
            let derivative = delta / dp;
            if !derivative.re.is_finite() || !derivative.im.is_finite() {
                return Err(error(format!(
                    "non-finite AC sensitivity for {name} at {f} Hz"
                )));
            }
            column.push((derivative.re, derivative.im));
        }
        variables.push(var(name, Quantity::Scalar));
        columns.push(column);
    }
    Ok(AnalysisResult {
        kind: AnalysisKind::Sens,
        variables,
        scale: Some(var("frequency", Quantity::Frequency)),
        scale_values: freq,
        data: Data::Complex { columns },
    })
}
