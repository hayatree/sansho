//! Word memories, table-driven machines and sampled real-valued cells.
//! Provenance: ngspice 44 user manual §§8.3.5–9, 8.4.18, 8.4.20, 8.4.23 and 8.5.2.

use super::{
    Sample,
    logic::{Drive, Level, Value},
};
use Level::{High, Low, Unknown};
use sansho_devices::{Node, volt};
use sansho_diag::Diagnostic;
use std::{collections::BTreeMap, sync::Arc};
fn invalid(s: &str) -> Diagnostic {
    Diagnostic::error("E-ELAB-PARAM", s)
}

#[derive(Debug)]
pub(super) struct State {
    output: Vec<Value>,
    edges: Vec<(Vec<Option<Level>>, u32)>,
}
#[derive(Debug)]
pub(super) struct Machine {
    states: BTreeMap<u32, State>,
    reset: u32,
    delays: [f64; 2],
}
impl Machine {
    pub(super) fn new(
        lines: &[String],
        ni: usize,
        no: usize,
        reset: u32,
        delays: [f64; 2],
    ) -> Result<Self, Diagnostic> {
        let mut states = BTreeMap::<u32, State>::new();
        let mut current = None;
        for (line_no, line) in lines.iter().enumerate() {
            let error = || invalid(&format!("invalid d_state row {}", line_no + 1));
            let fields: Vec<_> = line
                .split('*')
                .next()
                .unwrap_or("")
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .collect();
            if fields.is_empty() {
                continue;
            }
            let inputs = if fields.len() == no + ni + 3 {
                let id = fields[0].parse().map_err(|_| error())?;
                let output = fields[1..=no]
                    .iter()
                    .map(|s| {
                        sansho_netlist::digital_code(s)
                            .map(Value::decode)
                            .ok_or_else(error)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(prev) = states.get(&id) {
                    if current != Some(id) || prev.output != output {
                        return Err(error());
                    }
                } else {
                    states.insert(
                        id,
                        State {
                            output,
                            edges: Vec::new(),
                        },
                    );
                }
                current = Some(id);
                &fields[1 + no..]
            } else if fields.len() == ni + 2 {
                &fields[..]
            } else {
                return Err(error());
            };
            let pattern = inputs[..ni]
                .iter()
                .map(|s| match s.to_ascii_lowercase().as_str() {
                    "0" => Ok(Some(Low)),
                    "1" => Ok(Some(High)),
                    "u" => Ok(Some(Unknown)),
                    "x" | "-" => Ok(None),
                    _ => Err(error()),
                })
                .collect::<Result<_, _>>()?;
            let next = inputs[ni + 1].parse().map_err(|_| error())?;
            states
                .get_mut(&current.ok_or_else(error)?)
                .ok_or_else(error)?
                .edges
                .push((pattern, next));
        }
        if !states.contains_key(&reset)
            || states
                .values()
                .flat_map(|s| &s.edges)
                .any(|(_, id)| !states.contains_key(id))
        {
            return Err(invalid("d_state references an undefined state"));
        }
        Ok(Self {
            states,
            reset,
            delays,
        })
    }
}

#[derive(Debug)]
pub(super) enum Extended {
    State(Machine),
    Ram {
        width: usize,
        address: usize,
        select: u32,
        initial: Level,
        delay: f64,
    },
    Lut {
        rows: usize,
        table: Vec<Value>,
        inputs: Vec<f64>,
        rise: Vec<f64>,
        fall: Vec<f64>,
    },
    ToReal {
        zero: f64,
        one: f64,
        enable: bool,
        delay: f64,
    },
    Gain {
        offset: f64,
        gain: f64,
        bias: f64,
        ic: f64,
        delay: f64,
    },
    Delay {
        propagation: f64,
        polarity: f64,
    },
    Pwm {
        input: [Node; 2],
        control: Vec<f64>,
        duty: Vec<f64>,
        frequency: f64,
        phase: f64,
        delays: [f64; 2],
    },
}
#[derive(Clone, Default, Debug)]
pub(super) struct Storage {
    state: u32,
    clock: Option<Level>,
    started: bool,
    last: Vec<Level>,
    // Last requested symbol and its output delay, excluding the address pin delay.
    lut_outputs: Vec<(Value, f64)>,
    words: Arc<BTreeMap<Vec<bool>, Vec<Level>>>,
    erased: bool,
    sample: f64,
    high: bool,
}
impl Storage {
    fn rising(&mut self, l: Level) -> bool {
        let edge = l == High && self.clock == Some(Low);
        if l != Unknown {
            self.clock = Some(l);
        }
        edge
    }
}

impl Extended {
    pub(super) fn eval(
        &self,
        pins: &[Option<usize>],
        nodes: &[Sample],
        x: &[f64],
        t: f64,
        dc: bool,
        m: &mut super::model::Memory,
    ) -> Vec<(Sample, f64)> {
        let s = m.extended.get_or_insert_with(Default::default);
        let level = |i: usize| pins[i].map_or(Low, |n| nodes[n].logic().0);
        let real = |i: usize| pins[i].map_or(0., |n| nodes[n].real());
        let bits = |start: usize, count: usize| {
            (start..start + count)
                .map(|i| match level(i) {
                    Low => Some(false),
                    High => Some(true),
                    Unknown => None,
                })
                .collect::<Option<Vec<_>>>()
        };
        let result = match self {
            Self::State(machine) => {
                let n = pins.len() - 2;
                let reset = level(n + 1) == High;
                let rising = s.rising(level(n));
                if !s.started || dc || reset {
                    s.state = machine.reset;
                } else if rising
                    && let Some((_, next)) = machine.states[&s.state].edges.iter().find(|(p, _)| {
                        p.iter()
                            .enumerate()
                            .all(|(i, v)| v.is_none_or(|v| level(i) == v))
                    })
                {
                    s.state = *next;
                }
                machine.states[&s.state]
                    .output
                    .iter()
                    .map(|v| (Sample::Digital(*v), machine.delays[usize::from(reset)]))
                    .collect()
            }
            Self::Ram {
                width,
                address,
                select,
                initial,
                delay,
            } => {
                let en = width + address;
                let selected = bits(en + 1, pins.len() - en - 1).is_some_and(|b| {
                    b.iter()
                        .enumerate()
                        .fold(0u32, |v, (i, b)| v | (u32::from(*b) << i))
                        == *select
                });
                let addr = bits(*width, *address);
                let write = selected && level(en) == High;
                if write {
                    if let Some(addr) = &addr {
                        let data = (0..*width).map(level).collect::<Vec<_>>();
                        if s.words.get(addr) != Some(&data) {
                            Arc::make_mut(&mut s.words).insert(addr.clone(), data);
                        }
                    } else {
                        s.words = Arc::default();
                        s.erased = true;
                    }
                }
                (0..*width)
                    .map(|i| {
                        let value = if !selected {
                            Value(Unknown, Drive::HiZ)
                        } else {
                            let data = addr.as_ref().map_or(Unknown, |a| {
                                s.words
                                    .get(a)
                                    .map_or(if s.erased { Unknown } else { *initial }, |d| d[i])
                            });
                            Value(data, if write { Drive::HiZ } else { Drive::Strong })
                        };
                        (Sample::Digital(value), *delay)
                    })
                    .collect()
            }
            Self::Lut {
                rows,
                table,
                inputs,
                rise,
                fall,
            } => {
                let levels = (0..pins.len()).map(level).collect::<Vec<_>>();
                let address = |pins: &[Level]| {
                    pins.iter().enumerate().try_fold(0, |a, (i, l)| match l {
                        Low => Some(a),
                        High => Some(a | (1 << i)),
                        Unknown => None,
                    })
                };
                let next = address(&levels);
                // The first unknown pin in address order determines the result.
                // A definite address must wait for every changed pin.
                let extra = if let Some(i) = levels.iter().position(|&l| l == Unknown) {
                    inputs[i]
                } else {
                    inputs
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| s.last.get(*i) != levels.get(*i))
                        .map(|(_, d)| *d)
                        .fold(0., f64::max)
                };
                s.last = levels;
                s.lut_outputs.resize(rise.len(), (Value::default(), 0.));
                table
                    .chunks_exact(*rows)
                    .zip(&mut s.lut_outputs)
                    .enumerate()
                    .map(|(j, (col, (requested, previous_delay)))| {
                        let mut v = next.map_or(Value(Unknown, Drive::Unknown), |a| col[a]);
                        let old = if requested.1 == Drive::HiZ {
                            Unknown
                        } else {
                            requested.0
                        };
                        if v.1 == Drive::HiZ {
                            // Entering Z from Uu carries low. Keep the released
                            // level while subsequent addresses still select Z.
                            v.0 = match requested.1 {
                                Drive::Unknown => Low,
                                Drive::HiZ => requested.0,
                                _ => Unknown,
                            };
                        }
                        if v != *requested {
                            // Each leg between an unknown address and its low Z
                            // release adds one fall delay to the preceding leg.
                            *previous_delay = match (*requested, v) {
                                (Value(Unknown, Drive::Unknown), Value(Low, Drive::HiZ))
                                | (Value(Low, Drive::HiZ), Value(Unknown, Drive::Unknown)) => {
                                    *previous_delay + fall[j]
                                }
                                _ => v.0.transition_delay(old, [fall[j], rise[j]]),
                            };
                            *requested = v;
                        }
                        (Sample::Digital(v), extra + *previous_delay)
                    })
                    .collect()
            }
            Self::ToReal {
                zero,
                one,
                enable,
                delay,
            } => {
                let v = if *enable && level(1) != High {
                    0.
                } else {
                    match level(0) {
                        Low => *zero,
                        High => *one,
                        Unknown => 0.,
                    }
                };
                vec![(Sample::Real(v), *delay)]
            }
            Self::Gain {
                offset,
                gain,
                bias,
                delay,
                ..
            } => {
                let v = bias + gain * (real(0) + offset);
                vec![(Sample::Real(v), *delay)]
            }
            Self::Delay {
                propagation,
                polarity,
            } => {
                if s.rising(level(1)) && !dc {
                    s.sample = polarity * real(0);
                }
                vec![(Sample::Real(s.sample), *propagation)]
            }
            Self::Pwm {
                input,
                control,
                duty,
                frequency,
                phase,
                delays,
            } => {
                if !s.started || dc || m.next.is_some_and(|at| at <= t) {
                    let u = volt(x, input[0]) - volt(x, input[1]);
                    let j = control
                        .partition_point(|v| *v <= u)
                        .saturating_sub(1)
                        .min(control.len() - 2);
                    let d = (duty[j]
                        + (u - control[j]) * (duty[j + 1] - duty[j])
                            / (control[j + 1] - control[j]))
                        .clamp(1e-9, 1. - 1e-9);
                    let dt = if !s.started || dc {
                        let phase = (phase / 360.).rem_euclid(1.);
                        s.high = phase < d;
                        if s.high { d - phase } else { 1. - phase }
                    } else {
                        s.high = !s.high;
                        if s.high { d } else { 1. - d }
                    };
                    m.next = Some(t + dt / frequency);
                }
                vec![(
                    Sample::Digital(Value(if s.high { High } else { Low }, Drive::Strong)),
                    delays[usize::from(s.high)],
                )]
            }
        };
        s.started = true;
        result
    }
}
