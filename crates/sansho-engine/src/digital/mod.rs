//! Deterministic event scheduling and mixed-signal synchronization.
//! Provenance: XSPICE event and node conventions, discrete-event simulation and delta cycles.

mod bind;
mod extended;
mod logic;
mod model;
pub(crate) use bind::Ports;
use logic::{Drive, Level, Value, resolve};
use model::{Memory, Model};
use sansho_devices::Mode;
use sansho_diag::Diagnostic;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) const DELTA_LIMIT: usize = 256;
pub(crate) fn oscillation(t: f64) -> Diagnostic {
    Diagnostic::error(
        "E-XSPICE-OSCILLATION",
        format!("mixed-signal network failed to settle in {DELTA_LIMIT} iterations at {t:e} s"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Sample {
    Digital(Value),
    Real(f64),
}
impl Sample {
    fn logic(self) -> Value {
        if let Self::Digital(v) = self {
            v
        } else {
            Value::default()
        }
    }
    fn real(self) -> f64 {
        if let Self::Real(v) = self { v } else { 0. }
    }
}

#[derive(Debug)]
struct Cell {
    model: Model,
    inputs: Vec<Option<usize>>,
    outputs: Vec<Option<usize>>,
    delay: [f64; 2],
}
#[derive(Debug)]
struct Dac {
    output: sansho_devices::Node,
    input: usize,
    rows: [Option<usize>; 2],
    real: Option<RealBridge>,
    current: bool,
    negative: sansho_devices::Node,
    levels: [f64; 3],
    times: [f64; 2],
}
#[derive(Debug)]
struct RealBridge {
    gain: f64,
    polarity: f64,
}
#[derive(Debug)]
struct BidiPin {
    rows: [usize; 4],
    bus: usize,
    driver: usize,
    dir: Option<usize>,
    direction: u32,
    strength: u32,
    analog: [sansho_devices::Node; 2],
    thresholds: [f64; 2],
    times: [f64; 2],
    delays: [f64; 2],
}
#[derive(Clone, Default, Debug)]
struct BidiState {
    ramps: [Ramp; 4],
    level: Level,
}
#[derive(Debug, Default)]
pub(crate) struct Network {
    bidis: Vec<BidiPin>,
    real: Vec<bool>,
    pub(crate) names: Vec<String>,
    pub(super) loads: Vec<f64>,
    cells: Vec<Cell>,
    external: Vec<usize>,
    drivers: Vec<Vec<usize>>,
    driver_nodes: Vec<usize>,
    fanout: Vec<Vec<usize>>,
    dacs: Vec<Dac>,
    // Immutable file records share delivery batches with gate output events.
    stimulus: Vec<(f64, usize, Value)>,
}
#[derive(Debug, Clone, Copy, Default)]
struct Ramp {
    begin: f64,
    end: f64,
    from: f64,
    to: f64,
}
impl Ramp {
    fn at(self, t: f64) -> f64 {
        if t >= self.end {
            self.to
        } else {
            self.from
                + (self.to - self.from) * ((t - self.begin) / (self.end - self.begin)).clamp(0., 1.)
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct DacState {
    ramp: Ramp,
    // Emitted sample before re-anchoring the internal real state at this time.
    emitted: f64,
}

impl DacState {
    /// Internal state and analog output at the end of this trial interval.
    fn sample(&self, dac: &Dac, t: f64) -> (f64, f64) {
        let r = self.ramp;
        let Some(bridge) = &dac.real else {
            let v = r.at(t);
            return (v, v);
        };
        let dt = t - r.begin;
        if dt == 0. {
            (r.from, self.emitted)
        } else if dt >= dac.times[0] {
            (r.to, bridge.gain * r.to)
        } else {
            let increment = (dt / dac.times[0]) * (r.to - r.from);
            (r.from + increment, r.from + bridge.gain * increment)
        }
    }
}

impl Network {
    /// Event deadlines are continuous analog boundaries when all outputs
    /// reach the analog system through finite-time real bridges.
    pub(crate) fn smooth_real_outputs(&self) -> bool {
        self.bidis.is_empty()
            && !self.dacs.is_empty()
            && self
                .dacs
                .iter()
                .all(|d| d.real.is_some() && d.times[0] > 0.)
    }
}

/// Private to one analysis. Keys are IEEE bits of nonnegative times, then driver ordinal.
#[derive(Debug, Clone, Default)]
pub(crate) struct Events {
    nodes: Vec<Sample>,
    drivers: Vec<Sample>,
    pending: Vec<VecDeque<u64>>,
    desired: Vec<Option<Sample>>,
    queue: BTreeMap<(u64, usize), Sample>,
    memory: Vec<Memory>,
    bidis: Vec<BidiState>,
    dacs: Vec<DacState>,
    started: bool,
    stimulus_index: usize,
}
impl Events {
    pub(crate) fn new(net: &Network) -> Self {
        let startup = Value(Level::Low, Drive::Unknown);
        let mut events = Self {
            nodes: net
                .drivers
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    if net.real[i] {
                        Sample::Real(0.)
                    } else {
                        Sample::Digital(if d.is_empty() {
                            Value::default()
                        } else {
                            startup
                        })
                    }
                })
                .collect(),
            drivers: net
                .driver_nodes
                .iter()
                .map(|&n| {
                    if net.real[n] {
                        Sample::Real(0.)
                    } else {
                        Sample::Digital(startup)
                    }
                })
                .collect(),
            desired: vec![None; net.driver_nodes.len()],
            pending: vec![VecDeque::new(); net.driver_nodes.len()],
            bidis: vec![BidiState::default(); net.bidis.len()],
            memory: vec![Memory::default(); net.cells.len()],
            dacs: net
                .dacs
                .iter()
                .map(|d| DacState {
                    ramp: Ramp {
                        to: d.levels[2],
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .collect(),
            ..Self::default()
        };
        for cell in &net.cells {
            if let Model::Extended(extended::Extended::Gain { ic, .. }) = cell.model
                && let Some(&Some(driver)) = cell.outputs.first()
            {
                events.drivers[driver] = Sample::Real(ic);
            }
        }
        for (i, drivers) in net.drivers.iter().enumerate().filter(|(i, _)| net.real[*i]) {
            events.nodes[i] =
                Sample::Real(drivers.iter().map(|&id| events.drivers[id].real()).sum());
        }
        events
    }
    /// UIC supplies the analog starting vector. Reflect the DC DAC constraints
    /// into it before sampling input bridges and initializing charge history.
    pub(crate) fn initialize_uic(
        &mut self,
        net: &Network,
        x: &mut [f64],
    ) -> Result<(), Diagnostic> {
        for _ in 0..DELTA_LIMIT {
            for (d, r) in net.dacs.iter().zip(&self.dacs) {
                if let Some(n) = d.output
                    && !d.current
                {
                    x[n] = r.sample(d, 0.).1 + sansho_devices::volt(x, d.negative);
                }
            }
            if !self.settle(net, x, 0., Mode::Tran)? {
                return Ok(());
            }
        }
        Err(oscillation(0.))
    }
    pub(crate) fn record(&self, net: &Network, axis: f64, traces: &mut sansho_output::EventTraces) {
        for (n, v) in net.names.iter().zip(&self.nodes) {
            let symbol = match v {
                Sample::Digital(v) => v.symbol().to_owned(),
                Sample::Real(v) => format!("{v:e}"),
            };
            if let Some(trace) = traces.get_mut(n) {
                if trace.last().is_none_or(|(_, s)| s != &symbol) {
                    trace.push((axis, symbol));
                }
            } else {
                traces.insert(n.clone(), vec![(axis, symbol)]);
            }
        }
    }
    pub(crate) fn next(&self, net: &Network, t: f64) -> f64 {
        let first = self
            .queue
            .first_key_value()
            .map_or(f64::INFINITY, |((time, _), _)| f64::from_bits(*time));
        net.dacs
            .iter()
            .zip(&self.dacs)
            .filter(|(d, _)| d.real.is_none())
            .map(|(_, state)| &state.ramp)
            .chain(self.bidis.iter().flat_map(|b| &b.ramps))
            .map(|r| r.end)
            .chain(net.stimulus.get(self.stimulus_index).map(|s| s.0))
            .chain(net.external.iter().filter_map(|&i| self.memory[i].next))
            .filter(|&s| s > t)
            .fold(first, f64::min)
    }
    pub(crate) fn stamp(&self, net: &Network, b: &mut [f64], t: f64, factor: f64) {
        for (pin, state) in net.bidis.iter().zip(&self.bidis) {
            for (row, r) in pin.rows.iter().zip(&state.ramps) {
                b[*row] += factor * r.at(t);
            }
        }
        for (d, r) in net.dacs.iter().zip(&self.dacs) {
            let drive = factor * r.sample(d, t).1 * if d.current { -1. } else { 1. };
            if let Some(row) = d.rows[0] {
                b[row] += drive;
            }
            if let Some(row) = d.rows[1] {
                b[row] -= drive;
            }
        }
    }
    fn schedule(&mut self, id: usize, v: Sample, t: f64, delay: f64) -> Result<(), Diagnostic> {
        if let Sample::Real(x) = v
            && !x.is_finite()
        {
            return Err(Diagnostic::error("E-XSPICE-VALUE", "nonfinite real event"));
        }
        if self.desired[id] == Some(v) {
            return Ok(());
        }
        self.desired[id] = Some(v);
        let at = t + delay;
        if !at.is_finite() {
            return Err(Diagnostic::error("E-XSPICE-TIME", "event time overflow"));
        }
        // Keep earlier arrivals, and supersede all transitions this one overtakes.
        let at = at.to_bits();
        let pending = &mut self.pending[id];
        while let Some(old) = pending.pop_back_if(|old| *old >= at) {
            self.queue.remove(&(old, id));
        }
        let preceding = pending.back().and_then(|old| self.queue.get(&(*old, id)));
        if preceding.unwrap_or(&self.drivers[id]) != &v {
            self.queue.insert((at, id), v);
            pending.push_back(at);
        }
        Ok(())
    }
    fn deliver(
        &mut self,
        net: &Network,
        t: f64,
        dirty: &mut BTreeSet<usize>,
    ) -> Result<(), Diagnostic> {
        let mut changed = BTreeSet::new();
        while self
            .queue
            .first_key_value()
            .is_some_and(|((time, _), _)| *time <= t.to_bits())
        {
            if let Some(((_, driver), v)) = self.queue.pop_first() {
                self.pending[driver].pop_front();
                self.drivers[driver] = v;
                changed.insert(net.driver_nodes[driver]);
            }
        }
        while let Some(&(time, driver, v)) = net.stimulus.get(self.stimulus_index) {
            if time > t {
                break;
            }
            self.drivers[driver] = Sample::Digital(v);
            changed.insert(net.driver_nodes[driver]);
            self.stimulus_index += 1;
        }
        for n in changed {
            let value = if net.real[n] {
                let total = net.drivers[n]
                    .iter()
                    .map(|&d| self.drivers[d].real())
                    .sum::<f64>();
                if !total.is_finite() {
                    return Err(Diagnostic::error(
                        "E-XSPICE-VALUE",
                        "real node sum overflow",
                    ));
                }
                Sample::Real(total)
            } else {
                Sample::Digital(resolve(
                    net.drivers[n].iter().map(|&d| self.drivers[d].logic()),
                ))
            };
            if self.nodes[n] != value {
                self.nodes[n] = value;
                dirty.extend(&net.fanout[n]);
            }
        }
        Ok(())
    }
    /// Deliver a simultaneous batch before evaluating fanout. Zero-delay responses enter
    /// the following delta. Only the final resolved node values reach the analog bridges.
    pub(crate) fn settle(
        &mut self,
        net: &Network,
        x: &[f64],
        t: f64,
        mode: Mode,
    ) -> Result<bool, Diagnostic> {
        if net.names.is_empty() {
            return Ok(false);
        }
        let dc = t == 0.;
        let mut dirty: BTreeSet<_> = if self.started {
            net.external.iter().copied().collect()
        } else {
            (0..net.cells.len()).collect()
        };
        self.started = true;
        let mut bidi_changed = false;
        for _ in 0..DELTA_LIMIT {
            self.deliver(net, t, &mut dirty)?;
            for (index, pin) in net.bidis.iter().enumerate() {
                let external = resolve(
                    net.drivers[pin.bus]
                        .iter()
                        .filter(|&&id| id != pin.driver)
                        .map(|&id| self.drivers[id].logic()),
                );
                let direction = match pin.direction {
                    0 => Level::Low,
                    1 => Level::High,
                    _ => pin
                        .dir
                        .map_or(Level::Unknown, |id| self.nodes[id].logic().0),
                };
                let dac = direction != Level::High
                    && matches!(external.1, Drive::Strong | Drive::Resistive)
                    && external.0 != Level::Unknown;
                let adc = direction == Level::High || direction == Level::Unknown && !dac;
                let st = &mut self.bidis[index];
                let v =
                    sansho_devices::volt(x, pin.analog[0]) - sansho_devices::volt(x, pin.analog[1]);
                let [lo, hi] = pin.thresholds;
                st.level = if lo > hi && v > hi && v < lo {
                    st.level
                } else if v <= lo.min(hi) {
                    Level::Low
                } else if v >= hi.max(lo) {
                    Level::High
                } else {
                    Level::Unknown
                };
                let output = if adc {
                    Value(
                        st.level,
                        match pin.strength {
                            0 => Drive::Strong,
                            1 => Drive::Resistive,
                            _ => Drive::HiZ,
                        },
                    )
                } else {
                    Value(Level::Unknown, Drive::HiZ)
                };
                let active = usize::from(external.1 == Drive::Resistive) * 2
                    + usize::from(external.0 == Level::High);
                for (j, r) in st.ramps.iter_mut().enumerate() {
                    let to = f64::from(dac && j == active);
                    if r.to != to {
                        let from = r.at(t);
                        let end = t + if dc {
                            0.
                        } else {
                            pin.times[usize::from(external.0 == Level::High)] * (to - from).abs()
                        };
                        *r = Ramp {
                            begin: t,
                            end,
                            from,
                            to,
                        };
                        bidi_changed |= from != r.at(t);
                    }
                }
                self.schedule(
                    pin.driver,
                    Sample::Digital(output),
                    t,
                    if dc {
                        0.
                    } else {
                        pin.delays[usize::from(output.0 == Level::High)]
                    },
                )?;
            }
            for id in std::mem::take(&mut dirty) {
                let cell = &net.cells[id];
                if let Model::Extended(model) = &cell.model {
                    let values =
                        model.eval(&cell.inputs, &self.nodes, x, t, dc, &mut self.memory[id]);
                    if self.memory[id]
                        .next
                        .is_some_and(|at| !at.is_finite() || at <= t)
                    {
                        return Err(Diagnostic::error(
                            "E-XSPICE-TIME",
                            "PWM edge cannot advance",
                        ));
                    }
                    for (driver, (v, d)) in cell.outputs.iter().zip(values) {
                        if let Some(driver) = driver {
                            let delay = if !dc {
                                d
                            } else if mode == Mode::Tran
                                && matches!(model, extended::Extended::Gain { .. })
                            {
                                // Hold the IC through transient initialization.
                                // The settled input's first publication is due at 2D.
                                2. * d
                            } else {
                                0.
                            };
                            self.schedule(*driver, v, t, delay)?;
                        }
                    }
                    continue;
                }
                let (value, control) =
                    cell.model
                        .eval(&cell.inputs, &self.nodes, x, t, dc, &mut self.memory[id]);
                if self.memory[id]
                    .next
                    .is_some_and(|at| !at.is_finite() || at <= t)
                {
                    return Err(Diagnostic::error(
                        "E-XSPICE-TIME",
                        format!("next digital source edge cannot advance beyond {t:e} s"),
                    ));
                }
                for (index, &driver) in cell.outputs.iter().enumerate() {
                    if let Some(driver) = driver {
                        let mut v = value;
                        if index == 1 {
                            v.0 = v.0.invert();
                        }
                        // The preceding request can still be in flight.
                        let previous = self.desired[driver].unwrap_or(self.drivers[driver]).logic();
                        let delay = v.0.transition_delay(previous.0, cell.delay);
                        self.schedule(
                            driver,
                            Sample::Digital(v),
                            t,
                            if dc {
                                0.
                            } else {
                                delay + control.unwrap_or(0.)
                            },
                        )?;
                    }
                }
            }
            if self
                .queue
                .first_key_value()
                .is_none_or(|((time, _), _)| *time > t.to_bits())
            {
                let mut changed = bidi_changed;
                for (d, state) in net.dacs.iter().zip(&mut self.dacs) {
                    let (mut from, before) = state.sample(d, t);
                    let ramp = &mut state.ramp;
                    let (to, duration) = if let Some(bridge) = &d.real {
                        let input = bridge.polarity * self.nodes[d.input].real();
                        if dc || d.times[0] == 0. {
                            from = input;
                            state.emitted = bridge.gain * input;
                        } else {
                            // Only the unscaled increment feeds the next internal state.
                            state.emitted = before;
                        }
                        if !state.emitted.is_finite() {
                            return Err(Diagnostic::error(
                                "E-XSPICE-VALUE",
                                "real bridge output overflow",
                            ));
                        }
                        (input, d.times[0])
                    } else {
                        let to = d.levels[self.nodes[d.input].logic().0 as usize];
                        if to == ramp.to {
                            continue;
                        }
                        let swing = (d.levels[1] - d.levels[0]).abs();
                        let duration = if dc || swing == 0. {
                            0.
                        } else {
                            d.times[usize::from(to > from)] * ((to - from) / swing).abs()
                        };
                        (to, duration)
                    };
                    if !to.is_finite() {
                        return Err(Diagnostic::error(
                            "E-XSPICE-VALUE",
                            "bridge output overflow",
                        ));
                    }
                    if !(t + duration).is_finite() {
                        return Err(Diagnostic::error(
                            "E-XSPICE-TIME",
                            "DAC ramp endpoint overflow",
                        ));
                    }
                    *ramp = Ramp {
                        from,
                        to,
                        begin: t,
                        end: t + duration,
                    };
                    changed |= before != state.sample(d, t).1;
                }
                return Ok(changed);
            }
        }
        Err(oscillation(t))
    }
}
