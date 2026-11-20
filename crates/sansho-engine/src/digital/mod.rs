//! Deterministic event scheduling and mixed-signal synchronization.
//! Provenance: XSPICE event and node conventions, discrete-event simulation and delta cycles.

mod bind;
mod logic;
mod model;
pub(crate) use bind::Ports;
use logic::{Drive, Level, Value, resolve};
use model::{Memory, Model};
use sansho_diag::Diagnostic;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(crate) const DELTA_LIMIT: usize = 256;
pub(crate) fn oscillation(t: f64) -> Diagnostic {
    Diagnostic::error(
        "E-XSPICE-OSCILLATION",
        format!("mixed-signal network failed to settle in {DELTA_LIMIT} iterations at {t:e} s"),
    )
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
    row: usize,
    levels: [f64; 3],
    times: [f64; 2],
}
#[derive(Debug, Default)]
pub(crate) struct Network {
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

/// Private to one analysis. Keys are IEEE bits of nonnegative times, then driver ordinal.
#[derive(Debug, Clone, Default)]
pub(crate) struct Events {
    nodes: Vec<Value>,
    drivers: Vec<Value>,
    pending: Vec<VecDeque<u64>>,
    desired: Vec<Option<Value>>,
    queue: BTreeMap<(u64, usize), Value>,
    memory: Vec<Memory>,
    ramps: Vec<Ramp>,
    started: bool,
    stimulus_index: usize,
}
impl Events {
    pub(crate) fn new(net: &Network) -> Self {
        let startup = Value(Level::Low, Drive::Unknown);
        Self {
            nodes: net
                .drivers
                .iter()
                .map(|d| {
                    if d.is_empty() {
                        Value::default()
                    } else {
                        startup
                    }
                })
                .collect(),
            drivers: vec![startup; net.driver_nodes.len()],
            desired: vec![None; net.driver_nodes.len()],
            pending: vec![VecDeque::new(); net.driver_nodes.len()],
            memory: vec![Memory::default(); net.cells.len()],
            ramps: net
                .dacs
                .iter()
                .map(|d| Ramp {
                    to: d.levels[2],
                    ..Default::default()
                })
                .collect(),
            ..Self::default()
        }
    }
    /// UIC supplies the analog starting vector. Reflect the DC DAC constraints
    /// into it before sampling input bridges and initializing charge history.
    pub(crate) fn initialize_uic(
        &mut self,
        net: &Network,
        x: &mut [f64],
    ) -> Result<(), Diagnostic> {
        for _ in 0..DELTA_LIMIT {
            for (d, r) in net.dacs.iter().zip(&self.ramps) {
                if let Some(n) = d.output {
                    x[n] = r.at(0.);
                }
            }
            if !self.settle(net, x, 0., true)? {
                return Ok(());
            }
        }
        Err(oscillation(0.))
    }
    pub(crate) fn record(&self, net: &Network, axis: f64, traces: &mut sansho_output::EventTraces) {
        for (n, v) in net.names.iter().zip(&self.nodes) {
            let symbol = v.symbol();
            if let Some(trace) = traces.get_mut(n) {
                if trace.last().is_none_or(|(_, s)| s != symbol) {
                    trace.push((axis, symbol.into()));
                }
            } else {
                traces.insert(n.clone(), vec![(axis, symbol.into())]);
            }
        }
    }
    pub(crate) fn next(&self, net: &Network, t: f64) -> f64 {
        let first = self
            .queue
            .first_key_value()
            .map_or(f64::INFINITY, |((time, _), _)| f64::from_bits(*time));
        self.ramps
            .iter()
            .map(|r| r.end)
            .chain(net.stimulus.get(self.stimulus_index).map(|s| s.0))
            .chain(net.external.iter().filter_map(|&i| self.memory[i].next))
            .filter(|&s| s > t)
            .fold(first, f64::min)
    }
    pub(crate) fn stamp(&self, net: &Network, b: &mut [f64], t: f64, factor: f64) {
        for (d, r) in net.dacs.iter().zip(&self.ramps) {
            b[d.row] += factor * r.at(t);
        }
    }
    fn schedule(&mut self, id: usize, v: Value, t: f64, delay: f64) -> Result<(), Diagnostic> {
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
    fn deliver(&mut self, net: &Network, t: f64, dirty: &mut BTreeSet<usize>) {
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
            self.drivers[driver] = v;
            changed.insert(net.driver_nodes[driver]);
            self.stimulus_index += 1;
        }
        for n in changed {
            let value = resolve(net.drivers[n].iter().map(|&d| self.drivers[d]));
            if self.nodes[n] != value {
                self.nodes[n] = value;
                dirty.extend(&net.fanout[n]);
            }
        }
    }
    /// Deliver a simultaneous batch before evaluating fanout. Zero-delay responses enter
    /// the following delta. Only the final resolved node values reach the analog bridges.
    pub(crate) fn settle(
        &mut self,
        net: &Network,
        x: &[f64],
        t: f64,
        dc: bool,
    ) -> Result<bool, Diagnostic> {
        if net.names.is_empty() {
            return Ok(false);
        }
        let mut dirty: BTreeSet<_> = if self.started {
            net.external.iter().copied().collect()
        } else {
            (0..net.cells.len()).collect()
        };
        self.started = true;
        for _ in 0..DELTA_LIMIT {
            self.deliver(net, t, &mut dirty);
            for id in std::mem::take(&mut dirty) {
                let cell = &net.cells[id];
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
                        let delay = match v.0 {
                            Level::Low => cell.delay[0],
                            Level::High => cell.delay[1],
                            Level::Unknown => {
                                // Follow the requested transition even if its predecessor is pending.
                                match self.desired[driver].unwrap_or(self.drivers[driver]).0 {
                                    Level::Low => cell.delay[1],
                                    Level::High => cell.delay[0],
                                    Level::Unknown => 0.5 * cell.delay[0] + 0.5 * cell.delay[1],
                                }
                            }
                        };
                        self.schedule(
                            driver,
                            v,
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
                let mut changed = false;
                for (d, ramp) in net.dacs.iter().zip(&mut self.ramps) {
                    let to = d.levels[self.nodes[d.input].0 as usize];
                    if to != ramp.to {
                        let from = ramp.at(t);
                        let swing = (d.levels[1] - d.levels[0]).abs();
                        let duration = if dc || swing == 0. {
                            0.
                        } else {
                            d.times[usize::from(to > from)] * ((to - from) / swing).abs()
                        };
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
                        changed |= from != ramp.at(t);
                    }
                }
                return Ok(changed);
            }
        }
        Err(oscillation(t))
    }
}
