//! Type-check A-card connections before allocating analog matrix rows.
//! Provenance: XSPICE code-model port conventions and original elaboration.

use super::{
    BidiPin, Cell, Dac, Network,
    extended::{Extended, Machine},
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
        "bidi_bridge" => "ADD",
        "real_gain" => "rr",
        "real_to_v" => "ra",
        "d_to_real" => "d?r",
        "real_delay" => "rdr",
        "d_ram" => "DDDdD",
        "d_state" => "Dd?D",
        "d_genlut" => "DD",
        "d_pwm" => "ad",
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
    real: std::collections::BTreeSet<String>,
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
            let mut register = |name: &str, role: u8| {
                let digital = !matches!(role, b'a' | b'A');
                if digital
                    && result.digital.contains_key(name)
                    && result.real.contains(name) != (role == b'r')
                {
                    return Err(vec![
                        port(format!("{}: {name} mixes digital and real ports", e.name))
                            .with_span(e.span),
                    ]);
                }
                if role == b'r' {
                    result.real.insert(name.to_owned());
                }
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
                result.grounded_dac |= kind == "bidi_bridge"
                    && ports.first().is_some_and(|p| !p.differential())
                    || kind == "dac_bridge"
                    || kind == "real_to_v" && ports.get(1).is_some_and(|p| !p.differential());
                let sig = signature(kind);
                if ports.len() != sig.len() {
                    return Err(vec![
                        port(format!("{} ({kind}) requires {} ports", e.name, sig.len()))
                            .with_span(e.span),
                    ]);
                }
                for (i, (p, role)) in ports.iter().zip(sig.bytes()).enumerate() {
                    let digital = !matches!(role, b'a' | b'A');
                    let valid_mod = p.modifier.is_empty()
                        || p.modifier
                            == if role == b'r' {
                                "real"
                            } else if digital {
                                "d"
                            } else {
                                "v"
                            }
                        || matches!(
                            kind,
                            "real_to_v" | "d_pwm" | "d_osc" | "adc_bridge" | "dac_bridge"
                        ) && !digital
                            && matches!(p.modifier.as_str(), "vd" | "i" | "id");
                    let nullable = kind == "bidi_bridge" && i == 2 && p.nodes.is_empty();
                    let valid_mod = if kind == "bidi_bridge" && i == 0 {
                        matches!(p.modifier.as_str(), "" | "g" | "gd")
                    } else {
                        valid_mod
                    };
                    if !valid_mod
                        || p.vector != role.is_ascii_uppercase()
                            && !nullable
                            && !(kind == "d_state" && i == 0 && p.nodes.is_empty())
                        || p.nodes.is_empty()
                            && !nullable
                            && role != b'?'
                            && !(kind == "d_state" && i == 0)
                        || !p.vector && p.nodes.len() > 1 + usize::from(p.differential())
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
                        register(n, role)?;
                    }
                }
            } else {
                for n in e.kind.nodes() {
                    register(n, b'a')?;
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
            real: vec![false; count],
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
            net.real[id] = ports.real.contains(name);
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
        if matches!(
            model,
            Model::Adc { .. } | Model::Osc { .. } | Model::Extended(Extended::Pwm { .. })
        ) {
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
        let analog_pair = |p: &[String]| {
            [
                p.first().and_then(|a| node(a)),
                p.get(1).and_then(|b| node(b)),
            ]
        };
        let sensor = |port: &CodePort, pair: &[String], i: usize| {
            if port.modifier.starts_with('i') {
                [u.branch(&format!("{name}:{i}")), None]
            } else {
                analog_pair(pair)
            }
        };
        let dac_rows =
            |current: bool, pair: [Node; 2], i: usize| -> Result<[Node; 2], Diagnostic> {
                if current {
                    Ok(pair)
                } else {
                    Ok([
                        Some(
                            u.branch(&format!("{name}:{i}"))
                                .ok_or_else(|| port("missing bridge branch"))?,
                        ),
                        None,
                    ])
                }
            };
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
                        ("d_osc", "cntl_array" | "freq_array")
                            | ("d_lut", "table_values")
                            | (
                                "d_genlut",
                                "table_values"
                                    | "input_load"
                                    | "input_delay"
                                    | "rise_delay"
                                    | "fall_delay"
                            )
                            | ("d_pwm", "cntl_array" | "dc_array")
                    )
            })
        {
            return Err(bad(format!("{k}: unexpected array or nonfinite parameter")));
        }
        let sequential = matches!(
            k,
            "d_dff" | "d_jkff" | "d_tff" | "d_srff" | "d_dlatch" | "d_srlatch"
        );
        if k == "bidi_bridge" {
            let width = ports[1].nodes.len();
            let analog = &ports[0];
            let pair = if analog.differential() { 2 } else { 1 };
            let direction = integer("direction", 2.)?;
            let strength = integer("strength", 0.)?;
            let times = [param("t_fall", 1e-9), param("t_rise", 1e-9)];
            let delays = [param("fall_delay", 1e-9), param("rise_delay", 1e-9)];
            if times.iter().chain(&delays).any(|v| *v < 1e-12)
                || direction > 2
                || strength > 2
                || analog.nodes.len() != width * pair
                || ![0, 1, width].contains(&ports[2].nodes.len())
            {
                return Err(bad("invalid bidi_bridge timing, directions or widths"));
            }
            let thresholds = [param("in_low", 0.1), param("in_high", 0.9)];
            params.accept(&[
                "out_low",
                "out_high",
                "drive_low",
                "drive_high",
                "r_stl",
                "r_sth",
                "r_low",
                "r_high",
                "smooth",
            ]);
            for (i, a) in analog.nodes.chunks(pair).enumerate() {
                let mut rows = [0; 4];
                for (j, r) in rows.iter_mut().enumerate() {
                    *r = u
                        .branch(&format!("{name}:{i}:drive{j}"))
                        .ok_or_else(|| bad("missing bidirectional drive equation"))?;
                }
                let bus = bindings.digital[&ports[1].nodes[i]];
                let driver = self.driver(bus);
                let dir = ports[2]
                    .nodes
                    .get(i)
                    .or_else(|| ports[2].nodes.first())
                    .and_then(node_id);
                self.bidis.push(BidiPin {
                    rows,
                    bus,
                    driver,
                    dir,
                    direction,
                    strength,
                    analog: [node(&a[0]), a.get(1).and_then(|n| node(n))],
                    thresholds,
                    times,
                    delays,
                });
            }
            return self.finish_loads(k, ports, node_id, &params);
        }
        if matches!(
            k,
            "d_state"
                | "d_ram"
                | "d_genlut"
                | "real_gain"
                | "real_delay"
                | "d_to_real"
                | "real_to_v"
                | "d_pwm"
        ) {
            let polarity = |i: usize| if ports[i].inverted { -1. } else { 1. };
            let ids = |i: usize| ports[i].nodes.iter().map(node_id).collect::<Vec<_>>();
            let vector = |key: &str, n: usize, default: f64| -> Result<Vec<f64>, Diagnostic> {
                let a = array(key);
                let values = if a.is_empty() {
                    vec![param(key, default); n]
                } else {
                    a.to_vec()
                };
                if values.len() != n || values.iter().any(|&v| v < 0.) {
                    return Err(bad(format!("invalid {k} {key} width or value")));
                }
                Ok(values)
            };
            let time = |key: &str, default: f64| {
                let v = param(key, default);
                if v < 0. {
                    Err(bad(format!("negative {k} {key}")))
                } else {
                    Ok(v)
                }
            };
            if k == "real_to_v" {
                let port = &ports[1];
                let current = port.modifier.starts_with('i');
                let a = node(&port.nodes[0]);
                let b = port.nodes.get(1).and_then(|n| node(n));
                let rows = dac_rows(current, [a, b], 0)?;
                self.dacs.push(Dac {
                    rows,
                    output: a,
                    negative: b,
                    current,
                    input: pin(0).ok_or_else(|| bad("missing real input"))?,
                    real: Some(super::RealBridge {
                        gain: param("gain", 1.),
                        polarity: polarity(0),
                    }),
                    levels: [0.; 3],
                    times: [time("transition_time", 1e-9)?; 2],
                });
            } else {
                let (model, inputs, outputs) = match k {
                    "d_state" => {
                        let mut inputs = ids(0);
                        inputs.extend([pin(1), pin(2)]);
                        let machine = Machine::new(
                            &m.table
                                .as_ref()
                                .ok_or_else(|| bad("missing state_file"))?
                                .records,
                            ports[0].nodes.len(),
                            ports[3].nodes.len(),
                            integer("reset_state", 0.)?,
                            [time("clk_delay", 1e-9)?, time("reset_delay", 1e-9)?],
                        )?;
                        (Extended::State(machine), inputs, ids(3))
                    }
                    "d_ram" => {
                        let width = ports[0].nodes.len();
                        let address = ports[2].nodes.len();
                        let sc = ports[4].nodes.len();
                        let select = integer("select_value", 1.)?;
                        let initial = integer("ic", 2.)?;
                        if ports[1].nodes.len() != width
                            || sc > 16
                            || select >= 1 << sc
                            || initial > 2
                        {
                            return Err(bad("invalid RAM word width, select or ic"));
                        }
                        let mut inputs = ids(0);
                        inputs.extend(ids(2));
                        inputs.push(pin(3));
                        inputs.extend(ids(4));
                        (
                            Extended::Ram {
                                width,
                                address,
                                select,
                                initial: level(initial),
                                delay: time("read_delay", 100e-9)?,
                            },
                            inputs,
                            ids(1),
                        )
                    }
                    "d_genlut" => {
                        let n = ports[0].nodes.len();
                        let m = ports[1].nodes.len();
                        let values = array("table_values");
                        let rows = 1usize
                            .checked_shl(n as u32)
                            .filter(|v| v.checked_mul(m) == Some(values.len()))
                            .ok_or_else(|| bad("d_genlut needs M times 2^N output values"))?;
                        for (id, v) in ids(0).into_iter().zip(vector("input_load", n, 1e-12)?) {
                            if let Some(id) = id {
                                self.loads[id] += v;
                            }
                        }
                        (
                            Extended::Lut {
                                rows,
                                table: values.iter().map(|v| Value::decode(*v as u8)).collect(),
                                inputs: vector("input_delay", n, 0.)?,
                                rise: vector("rise_delay", m, 1e-9)?,
                                fall: vector("fall_delay", m, 1e-9)?,
                            },
                            ids(0),
                            ids(1),
                        )
                    }
                    "real_gain" => (
                        Extended::Gain {
                            offset: polarity(0) * param("in_offset", 0.),
                            gain: polarity(0) * polarity(1) * param("gain", 1.),
                            bias: polarity(1) * param("out_offset", 0.),
                            ic: polarity(1) * param("ic", 0.),
                            delay: time("delay", 1e-9)?,
                        },
                        ids(0),
                        ids(1),
                    ),
                    "real_delay" => (
                        Extended::Delay {
                            propagation: time("delay", 1e-9)?,
                            polarity: polarity(0) * polarity(2),
                        },
                        vec![pin(0), pin(1)],
                        ids(2),
                    ),
                    "d_to_real" => (
                        Extended::ToReal {
                            zero: polarity(2) * param("zero", 0.),
                            one: polarity(2) * param("one", 1.),
                            enable: pin(1).is_some(),
                            delay: time("delay", 1e-9)?,
                        },
                        vec![pin(0), pin(1)],
                        ids(2),
                    ),
                    _ => {
                        let mut control = array("cntl_array").to_vec();
                        let mut duty = array("dc_array").to_vec();
                        if control.is_empty() {
                            control = vec![-1., 1.];
                        }
                        if duty.is_empty() {
                            duty = vec![0., 1.];
                        }
                        let frequency = param("frequency", 1e6);
                        if frequency <= 0.
                            || control.len() < 2
                            || control.len() != duty.len()
                            || control.windows(2).any(|v| v[0] >= v[1])
                        {
                            return Err(bad("invalid PWM frequency or duty table"));
                        }
                        let p = &ports[0];
                        let input = sensor(p, &p.nodes, 0);
                        (
                            Extended::Pwm {
                                input,
                                frequency,
                                control,
                                duty,
                                phase: param("init_phase", 0.),
                                delays: [time("fall_delay", 1e-9)?, time("rise_delay", 1e-9)?],
                            },
                            vec![],
                            ids(1),
                        )
                    }
                };
                self.cell(Model::Extended(model), inputs, outputs, [0.; 2]);
            }
            return self.finish_loads(k, ports, node_id, &params);
        }
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
            let ai = usize::from(k == "dac_bridge");
            let a = &ports[ai];
            let digits = &ports[1 - ai];
            let stride = if a.differential() { 2 } else { 1 };
            if a.nodes.len() / stride != digits.nodes.len() {
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
            for (i, (pair, d)) in a.nodes.chunks(stride).zip(&digits.nodes).enumerate() {
                if k == "adc_bridge" {
                    self.cell(
                        Model::Adc {
                            input: sensor(a, pair, i),
                            low,
                            high,
                        },
                        vec![],
                        vec![node_id(d)],
                        delay,
                    );
                } else {
                    let [pos, neg] = analog_pair(pair);
                    let current = a.modifier.starts_with('i');
                    let rows = dac_rows(current, [pos, neg], i)?;
                    let times = [param("t_fall", 1e-9), param("t_rise", 1e-9)];
                    if times.iter().any(|&v| v < 0.) {
                        return Err(bad("negative DAC transition duration"));
                    }
                    self.dacs.push(Dac {
                        output: pos,
                        input: bindings.digital[d],
                        rows,
                        real: None,
                        current,
                        negative: neg,
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
                    let mut control = array("cntl_array");
                    let mut frequency = array("freq_array");
                    if control.is_empty() {
                        control = &[0., 1.];
                    }
                    if frequency.is_empty() {
                        frequency = &[1e6, 2e6];
                    }
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
                        input: sensor(&ports[0], &ports[0].nodes, 0),
                        phase: param("init_phase", 0.),
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
        self.finish_loads(k, ports, node_id, &params)
    }
    fn finish_loads(
        &mut self,
        k: &str,
        ports: &[CodePort],
        node_id: impl Fn(&String) -> Option<usize>,
        params: &ModelParams,
    ) -> Result<(), Diagnostic> {
        for &(i, keys) in load_ports(k) {
            let load = params.opt(keys).unwrap_or(1e-12);
            if load < 0. {
                return Err(bad(format!("{k}: negative input load")));
            }
            for id in ports[i].nodes.iter().filter_map(&node_id) {
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
        "adc_bridge" | "d_source" | "d_osc" | "d_pwm" | "d_genlut" | "real_gain" | "real_to_v"
        | "d_to_real" | "real_delay" => &[],
        "bidi_bridge" => &[(1, &["input_load"])],
        "d_state" => &[
            (0, &["input_load"]),
            (1, &["clk_load"]),
            (2, &["reset_load"]),
        ],
        "d_ram" => &[
            (0, &["data_load"]),
            (2, &["address_load"]),
            (3, &["enable_load"]),
            (4, &["select_load"]),
        ],
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
