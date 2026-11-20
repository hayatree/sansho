//! Type-check A-card connections before allocating analog matrix rows.
//! Provenance: XSPICE code-model port conventions and original elaboration.

use super::{
    Cell, Dac, Network,
    logic::{Level, Value},
    model::{Function, Model, Storage},
};
use sansho_devices::{ModelParams, Node, Unknowns};
use sansho_diag::Diagnostic;
use sansho_netlist::{CodePort, ElementKind, ModelCard, Netlist, digital_model};
use std::collections::{BTreeMap, HashMap};

fn bad(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-ELAB-PARAM", message)
}
fn port(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("E-XSPICE-PORT", message)
}

// a/A = analog scalar/vector, d/D = digital scalar/vector, ? = nullable digital scalar.
fn signature(kind: &str) -> &'static str {
    match kind {
        "dac_bridge" => "DA",
        "adc_bridge" => "AD",
        "d_osc" => "ad",
        "d_and" | "d_or" | "d_nand" | "d_nor" | "d_xor" | "d_xnor" | "d_lut" => "Dd",
        "d_source" => "D",
        "d_pullup" | "d_pulldown" => "d",
        "d_tristate" => "ddd",
        "d_dff" | "d_tff" | "d_dlatch" => "dd????",
        "d_jkff" | "d_srff" | "d_srlatch" => "ddd????",
        _ => "dd",
    }
}

#[derive(Default)]
pub(crate) struct Ports {
    pub(crate) grounded_dac: bool,
    pub(crate) digital: BTreeMap<String, usize>,
}
impl Ports {
    pub(crate) fn check(nl: &Netlist) -> Result<Self, Vec<Diagnostic>> {
        let mut result = Self::default();
        if !nl.models.iter().any(|m| digital_model(&m.kind)) {
            return Ok(result);
        }
        let models: HashMap<_, _> = nl
            .models
            .iter()
            .map(|m| (m.name.as_str(), m.kind.as_str()))
            .collect();
        let mut analog = std::collections::HashSet::from([String::from("0")]);
        for e in &nl.elements {
            let mut register = |name: &str, digital: bool| {
                if digital {
                    if analog.contains(name) {
                        return Err(vec![
                            port(format!(
                                "{}: {name} connects analog and digital terminals",
                                e.name
                            ))
                            .with_span(e.span),
                        ]);
                    }
                    let id = result.digital.len();
                    result.digital.entry(name.into()).or_insert(id);
                } else if result.digital.contains_key(name) {
                    return Err(vec![
                        port(format!(
                            "{}: {name} connects analog and digital terminals",
                            e.name
                        ))
                        .with_span(e.span),
                    ]);
                } else {
                    analog.insert(name.into());
                }
                Ok(())
            };
            if let ElementKind::CodeModel { ports, model } = &e.kind
                && let Some(&kind) = models.get(model.as_str())
                && digital_model(kind)
            {
                result.grounded_dac |= kind == "dac_bridge";
                let sig = signature(kind);
                if ports.len() != sig.len() {
                    return Err(vec![
                        port(format!("{} ({kind}) requires {} ports", e.name, sig.len()))
                            .with_span(e.span),
                    ]);
                }
                for (i, (p, role)) in ports.iter().zip(sig.bytes()).enumerate() {
                    let digital = !matches!(role, b'a' | b'A');
                    let valid_mod =
                        p.modifier.is_empty() || p.modifier == if digital { "d" } else { "v" };
                    if !valid_mod
                        || p.vector != role.is_ascii_uppercase()
                        || p.nodes.is_empty() && role != b'?'
                        || !p.vector && p.nodes.len() > 1
                    {
                        return Err(vec![
                            port(format!(
                                "{} ({kind}): invalid type, width or connection on port {}",
                                e.name,
                                i + 1
                            ))
                            .with_span(e.span),
                        ]);
                    }
                    for n in &p.nodes {
                        register(n, digital)?;
                    }
                }
            } else {
                for n in e.kind.nodes() {
                    register(n, false)?;
                }
            }
        }
        Ok(result)
    }
}

impl Network {
    pub(crate) fn bind(
        nl: &Netlist,
        ports: &Ports,
        node: impl Fn(&str) -> Node,
        unknowns: &Unknowns,
    ) -> Result<Self, Vec<Diagnostic>> {
        let count = ports.digital.len();
        let mut net = Network {
            names: vec![String::new(); count],
            loads: vec![0.; count],
            drivers: vec![vec![]; count],
            fanout: vec![vec![]; count],
            ..Default::default()
        };
        if count == 0 {
            return Ok(net);
        }
        for (name, &id) in &ports.digital {
            net.names[id] = name.clone();
        }
        let models: HashMap<_, _> = nl.models.iter().map(|m| (m.name.as_str(), m)).collect();
        for e in &nl.elements {
            if let ElementKind::CodeModel {
                ports: p,
                model: name,
            } = &e.kind
                && let Some(m) = models.get(name.as_str())
                && digital_model(&m.kind)
            {
                net.add(&e.name, p, m, ports, &node, unknowns)
                    .map_err(|d| vec![d.with_span(e.span)])?;
            }
        }
        net.stimulus
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        Ok(net)
    }
    fn driver(&mut self, n: usize) -> usize {
        let driver = self.driver_nodes.len();
        self.driver_nodes.push(n);
        self.drivers[n].push(driver);
        driver
    }
    fn cell(
        &mut self,
        model: Model,
        inputs: Vec<Option<usize>>,
        outputs: Vec<Option<usize>>,
        delay: [f64; 2],
    ) {
        let id = self.cells.len();
        for &n in inputs.iter().flatten() {
            self.fanout[n].push(id);
        }
        let outputs = outputs
            .into_iter()
            .map(|n| n.map(|n| self.driver(n)))
            .collect();
        if matches!(model, Model::Adc { .. } | Model::Osc { .. }) {
            self.external.push(id);
        }
        self.cells.push(Cell {
            model,
            inputs,
            outputs,
            delay,
        });
    }
    fn add(
        &mut self,
        name: &str,
        ports: &[CodePort],
        m: &ModelCard,
        bindings: &Ports,
        node: &impl Fn(&str) -> Node,
        u: &Unknowns,
    ) -> Result<(), Diagnostic> {
        m.check_code_model_params()?;
        let k = m.kind.as_str();
        let params = ModelParams::new(&m.params);
        let param = |key: &str, default: f64| params.get(key, default);
        let node_id = |n: &String| bindings.digital.get(n).copied();
        let pin = |i: usize| ports[i].nodes.first().and_then(node_id);
        let mut delay = [0.; 2];
        let array = |key: &str| {
            m.arrays
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_slice())
                .unwrap_or_default()
        };
        let integer = |key: &str, default: f64| {
            let v = param(key, default);
            if !(0. ..=u32::MAX as f64).contains(&v) || v.fract() != 0. {
                Err(bad(format!("{k}: invalid integer {key}")))
            } else {
                Ok(v as u32)
            }
        };
        let level = |v: u32| match v {
            0 => Level::Low,
            1 => Level::High,
            _ => Level::Unknown,
        };
        if m.params.iter().any(|(_, v)| !v.is_finite())
            || m.arrays.iter().any(|(key, values)| {
                values.iter().any(|v| !v.is_finite())
                    || !matches!(
                        (k, key.as_str()),
                        ("d_osc", "cntl_array" | "freq_array") | ("d_lut", "table_values")
                    )
            })
        {
            return Err(bad(format!("{k}: unexpected array or nonfinite parameter")));
        }
        let sequential = matches!(
            k,
            "d_dff" | "d_jkff" | "d_tff" | "d_srff" | "d_dlatch" | "d_srlatch"
        );
        if !matches!(k, "dac_bridge" | "d_pullup" | "d_pulldown" | "d_source") {
            let default = if k == "d_osc" { 0. } else { 1e-9 };
            delay = [param("fall_delay", default), param("rise_delay", default)];
        }
        if matches!(k, "d_open_c" | "d_open_e" | "d_tristate")
            && let Some(d) = params.opt(&["delay"])
        {
            delay = [d; 2];
        }
        if delay.iter().any(|&v| v < 0.) {
            return Err(bad(format!("{k}: negative propagation delay")));
        }
        // The oscillator drives at the phase crossing even with explicit delay parameters.
        if k == "d_osc" {
            delay = [0.; 2];
        }
        if matches!(k, "adc_bridge" | "dac_bridge") {
            if ports[0].nodes.len() != ports[1].nodes.len() {
                return Err(port(format!("{name}: bridge vector widths differ")));
            }
            let low = if k == "adc_bridge" {
                param("in_low", 0.1)
            } else {
                param("out_low", 0.)
            };
            let high = if k == "adc_bridge" {
                param("in_high", 0.9)
            } else {
                param("out_high", 1.)
            };
            if k == "adc_bridge" && low > high {
                return Err(bad("adc_bridge requires in_low <= in_high"));
            }
            for (i, (a, b)) in ports[0].nodes.iter().zip(&ports[1].nodes).enumerate() {
                if k == "adc_bridge" {
                    self.cell(
                        Model::Adc {
                            input: node(a),
                            low,
                            high,
                        },
                        vec![],
                        vec![node_id(b)],
                        delay,
                    );
                } else {
                    let row = u
                        .branch(&format!("{name}:{i}"))
                        .ok_or_else(|| port("missing DAC branch"))?;
                    let times = [param("t_fall", 1e-9), param("t_rise", 1e-9)];
                    if times.iter().any(|&v| v < 0.) {
                        return Err(bad("negative DAC transition duration"));
                    }
                    self.dacs.push(Dac {
                        output: node(b),
                        input: bindings.digital[a],
                        row,
                        levels: [low, high, param("out_undef", 0.5 * low + 0.5 * high)],
                        times,
                    });
                }
            }
        } else if k == "d_source" {
            let rows = &m
                .table
                .as_ref()
                .ok_or_else(|| bad("d_source needs input_file"))?
                .rows;
            if rows.is_empty()
                || rows.iter().any(|r| {
                    r.len() != 1 + ports[0].nodes.len()
                        || !r[0].is_finite()
                        || r[0] < 0.
                        || r[1..]
                            .iter()
                            .any(|v| !(0. ..=11.).contains(v) || v.fract() != 0.)
                })
                || rows.windows(2).any(|p| p[0][0] >= p[1][0])
            {
                return Err(bad(
                    "d_source needs ordered times and one column per output",
                ));
            }
            for (i, n) in ports[0].nodes.iter().enumerate() {
                let driver = self.driver(bindings.digital[n]);
                if rows[0][0] > 0. {
                    self.stimulus.push((0., driver, Value::decode(2)));
                }
                self.stimulus.extend(
                    rows.iter()
                        .map(|r| (r[0], driver, Value::decode(r[i + 1] as u8))),
                );
            }
        } else {
            let n = ports.len();
            let outputs = if sequential {
                vec![pin(n - 2), pin(n - 1)]
            } else {
                vec![pin(n - 1)]
            };
            if outputs.iter().all(Option::is_none) {
                return Err(port(format!("{name} needs a connected output")));
            }
            let inputs = if ports[0].vector {
                ports[0].nodes.iter().map(node_id).collect()
            } else if k == "d_osc" {
                vec![]
            } else {
                (0..n - outputs.len()).map(pin).collect()
            };
            let model = match k {
                "d_osc" => {
                    let control = array("cntl_array");
                    let frequency = array("freq_array");
                    let duty = param("duty_cycle", 0.5);
                    if control.len() < 2
                        || control.len() != frequency.len()
                        || control.windows(2).any(|s| s[1] <= s[0])
                        || frequency.iter().any(|&v| v <= 0.)
                        || !(0. < duty && duty < 1.)
                    {
                        return Err(bad("invalid d_osc frequency table or duty_cycle"));
                    }
                    Model::Osc {
                        input: node(&ports[0].nodes[0]),
                        control: control.to_vec(),
                        frequency: frequency.to_vec(),
                        duty,
                    }
                }
                "d_fdiv" => {
                    let modulus = integer("div_factor", 2.)?;
                    let high = integer("high_cycles", 1.)?;
                    let initial = integer("i_count", 0.)?;
                    if modulus == 0 || high == 0 || high > modulus || initial >= modulus {
                        return Err(bad("invalid divider modulus, high_cycles or i_count"));
                    }
                    Model::Divider {
                        modulus,
                        high,
                        initial,
                    }
                }
                "d_lut" => {
                    let values = array("table_values");
                    if 1usize.checked_shl(inputs.len() as u32) != Some(values.len()) {
                        return Err(bad("d_lut requires 2^N table_values"));
                    }
                    Model::Lut(values.iter().map(|&v| level(v as u32)).collect())
                }
                _ if sequential => {
                    let latch = k.ends_with("latch");
                    let kind = match k {
                        "d_dff" | "d_dlatch" => Storage::D,
                        "d_jkff" => Storage::Jk,
                        "d_tff" => Storage::T,
                        _ => Storage::Sr,
                    };
                    let initial = integer("ic", 0.)?;
                    if initial > 2 {
                        return Err(bad("ic must be 0, 1 or 2"));
                    }
                    let control_delay = [
                        param(if latch { "enable_delay" } else { "clk_delay" }, 1e-9),
                        param("set_delay", 1e-9),
                        param("reset_delay", 1e-9),
                    ];
                    if control_delay.iter().any(|&v| v < 0.) {
                        return Err(bad("negative sequential delay"));
                    }
                    let data_delay = if latch {
                        let delay = param(
                            if k == "d_srlatch" {
                                "sr_delay"
                            } else {
                                "data_delay"
                            },
                            1e-9,
                        );
                        if delay < 0. {
                            return Err(bad("negative latch data delay"));
                        }
                        delay
                    } else {
                        0.
                    };
                    Model::Sequential {
                        data_delay,
                        kind,
                        latch,
                        initial: level(initial),
                        control_delay,
                    }
                }
                _ => {
                    let f = match k {
                        "d_pullup" => Function::Pull(Level::High),
                        "d_pulldown" => Function::Pull(Level::Low),
                        "d_tristate" => Function::Tristate,
                        "d_open_c" => Function::OpenCollector,
                        "d_open_e" => Function::OpenEmitter,
                        "d_and" | "d_nand" => Function::And,
                        "d_or" | "d_nor" => Function::Or,
                        "d_xor" | "d_xnor" => Function::Xor,
                        _ => Function::Buffer,
                    };
                    Model::Gate(f, matches!(k, "d_inverter" | "d_nand" | "d_nor" | "d_xnor"))
                }
            };
            self.cell(model, inputs, outputs, delay);
        }
        for &(i, keys) in load_ports(k) {
            let load = params.opt(keys).unwrap_or(1e-12);
            if load < 0. {
                return Err(bad(format!("{k}: negative input load")));
            }
            for id in ports[i].nodes.iter().filter_map(node_id) {
                self.loads[id] += load;
                if !self.loads[id].is_finite() {
                    return Err(bad(format!("{k}: node load overflow")));
                }
            }
        }
        if let Some(key) = params.unused().first() {
            return Err(bad(format!("{k}: unsupported parameter {key}")));
        }
        Ok(())
    }
}

/// Port indices with a load parameter, including the common load of paired data inputs.
fn load_ports(kind: &str) -> &'static [(usize, &'static [&'static str])] {
    match kind {
        "adc_bridge" | "d_source" | "d_osc" => &[],
        "d_dff" => &[
            (0, &["data_load"]),
            (1, &["clk_load"]),
            (2, &["set_load"]),
            (3, &["reset_load"]),
        ],
        "d_tff" => &[
            (0, &["t_load", "data_load"]),
            (1, &["clk_load"]),
            (2, &["set_load"]),
            (3, &["reset_load"]),
        ],
        "d_dlatch" => &[
            (0, &["data_load"]),
            (1, &["enable_load"]),
            (2, &["set_load"]),
            (3, &["reset_load"]),
        ],
        "d_jkff" => &[
            (0, &["jk_load", "j_load"]),
            (1, &["jk_load", "k_load"]),
            (2, &["clk_load"]),
            (3, &["set_load"]),
            (4, &["reset_load"]),
        ],
        "d_srff" => &[
            (0, &["sr_load", "s_load"]),
            (1, &["sr_load", "r_load"]),
            (2, &["clk_load"]),
            (3, &["set_load"]),
            (4, &["reset_load"]),
        ],
        "d_srlatch" => &[
            (0, &["sr_load", "s_load"]),
            (1, &["sr_load", "r_load"]),
            (2, &["enable_load"]),
            (3, &["set_load"]),
            (4, &["reset_load"]),
        ],
        "d_tristate" => &[(0, &["input_load"]), (1, &["enable_load"])],
        "d_fdiv" => &[(0, &["freq_in_load", "input_load"])],
        "d_pullup" | "d_pulldown" => &[(0, &["load"])],
        _ => &[(0, &["input_load"])],
    }
}
