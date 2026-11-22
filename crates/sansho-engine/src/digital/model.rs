//! Event-domain transfer functions and sequential storage.
//! Provenance: XSPICE digital model interfaces and standard logic truth tables.

use super::logic::{Drive, Level, Value};
use Level::{High, Low, Unknown};
use sansho_devices::{Node, volt};

#[derive(Debug, Clone, Copy)]
pub(super) enum Function {
    Buffer,
    And,
    Or,
    Xor,
    Tristate,
    OpenCollector,
    OpenEmitter,
    Pull(Level),
}
#[derive(Debug, Clone, Copy)]
pub(super) enum Storage {
    D,
    Jk,
    T,
    Sr,
}

#[derive(Debug)]
pub(super) enum Model {
    Extended(super::extended::Extended),
    Gate(Function, bool),
    Lut(Vec<Level>),
    Adc {
        input: [Node; 2],
        low: f64,
        high: f64,
    },
    Osc {
        input: [Node; 2],
        control: Vec<f64>,
        frequency: Vec<f64>,
        duty: f64,
        phase: f64,
    },
    Sequential {
        kind: Storage,
        latch: bool,
        initial: Level,
        control_delay: [f64; 3],
        data_delay: f64,
    },
    Divider {
        modulus: u32,
        high: u32,
        initial: u32,
    },
}

#[derive(Debug, Default, Clone)]
pub(super) struct Memory {
    pub(super) extended: Option<Box<super::extended::Storage>>,
    q: Level,
    clock: Option<Level>,
    count: u32,
    pub(super) next: Option<f64>,
    started: bool,
}
impl Model {
    pub(super) fn eval(
        &self,
        inputs: &[Option<usize>],
        nodes: &[super::Sample],
        x: &[f64],
        t: f64,
        dc: bool,
        m: &mut Memory,
    ) -> (Value, Option<f64>) {
        let pin = |i: usize| {
            inputs
                .get(i)
                .copied()
                .flatten()
                .map_or(Low, |n| nodes[n].logic().0)
        };
        let strong = |v| Value(v, Drive::Strong);
        let levels = || inputs.iter().flatten().map(|&n| nodes[n].logic().0);
        let mut delay = None;
        let out = match self {
            Self::Extended(_) => return (Value::default(), None),
            Self::Gate(f, invert) => {
                let q = match f {
                    Function::Pull(v) => return (Value(*v, Drive::Resistive), None),
                    Function::OpenCollector | Function::OpenEmitter => {
                        let active = if matches!(f, Function::OpenCollector) {
                            Low
                        } else {
                            High
                        };
                        let level = pin(0);
                        return (
                            Value(
                                level,
                                if level == active {
                                    Drive::Strong
                                } else if level == Unknown {
                                    Drive::Unknown
                                } else {
                                    Drive::HiZ
                                },
                            ),
                            None,
                        );
                    }
                    Function::Tristate => {
                        return (
                            Value(
                                pin(0),
                                match pin(1) {
                                    High => Drive::Strong,
                                    Low => Drive::HiZ,
                                    Unknown => Drive::Unknown,
                                },
                            ),
                            None,
                        );
                    }
                    Function::And => levels().fold(High, Level::and),
                    Function::Or => levels().fold(Low, Level::or),
                    Function::Xor => levels().fold(Low, Level::xor),
                    Function::Buffer => pin(0),
                };
                strong(if *invert { q.invert() } else { q })
            }
            Self::Lut(table) => {
                let mut index = 0;
                for (i, l) in levels().enumerate() {
                    match l {
                        Low => (),
                        High => index |= 1usize << i,
                        Unknown => return (strong(Unknown), None),
                    }
                }
                strong(table[index])
            }
            Self::Adc { input, low, high } => {
                let v = volt(x, input[0]) - volt(x, input[1]);
                strong(if v <= *low {
                    Low
                } else if v >= *high {
                    High
                } else {
                    Unknown
                })
            }
            Self::Osc {
                input,
                control,
                frequency,
                duty,
                phase,
            } => {
                if dc || !m.started || m.next.is_some_and(|at| at <= t) {
                    m.q = if dc || !m.started {
                        Unknown // Before the first driven edge the output is 0u.
                    } else if m.q == High {
                        Low
                    } else {
                        High
                    };
                    let v = volt(x, input[0]) - volt(x, input[1]);
                    let i = control.partition_point(|&c| c <= v);
                    let f = if i == 0 {
                        frequency[0]
                    } else if i == control.len() {
                        frequency[i - 1]
                    } else {
                        frequency[i - 1]
                            + (v - control[i - 1]) / (control[i] - control[i - 1])
                                * (frequency[i] - frequency[i - 1])
                    };
                    let mut remaining = if m.q == High { *duty } else { 1. - duty };
                    if (dc || !m.started) && *phase != 0. {
                        let angle = (phase / 360.).rem_euclid(1.);
                        m.q = if angle >= 1. - duty { High } else { Low };
                        remaining = if m.q == High {
                            1. - angle
                        } else {
                            1. - duty - angle
                        };
                    }
                    m.next = Some(t + remaining / f);
                }
                if m.q == Unknown {
                    Value(Low, Drive::Unknown)
                } else {
                    strong(m.q)
                }
            }
            Self::Sequential {
                kind,
                latch,
                initial,
                control_delay,
                data_delay,
            } => {
                let data_count = if matches!(kind, Storage::Jk | Storage::Sr) {
                    2
                } else {
                    1
                };
                let clock = pin(data_count);
                let set = pin(data_count + 1);
                let reset = pin(data_count + 2);
                if dc || !m.started {
                    m.q = *initial;
                }
                let transparent = *latch && m.clock == Some(High) && clock == High;
                let rising = clock == High && m.clock == Some(Low);
                if clock != Unknown {
                    m.clock = Some(clock);
                }
                delay = Some(if reset != Low {
                    control_delay[2]
                } else if set != Low {
                    control_delay[1]
                } else if transparent {
                    *data_delay
                } else {
                    control_delay[0]
                });
                m.q = match (set, reset) {
                    (High, Low) => High,
                    (Low, High) => Low,
                    (Low, Low) if (*latch && clock == High) || (!dc && !*latch && rising) => {
                        match kind {
                            Storage::D => pin(0),
                            Storage::T => m.q.xor(pin(0)),
                            Storage::Sr => match (pin(0), pin(1)) {
                                (Low, Low) => m.q,
                                (High, Low) => High,
                                (Low, High) => Low,
                                _ => Unknown,
                            },
                            Storage::Jk => match (pin(0), pin(1)) {
                                (High, Low) => High,
                                (Low, High) => Low,
                                (Low, Low) => m.q,
                                (High, High) => m.q.invert(),
                                (j, k) => j.and(m.q.invert()).or(k.invert().and(m.q)),
                            },
                        }
                    }
                    (Low, Low) => m.q,
                    _ => Unknown,
                };
                strong(m.q)
            }
            Self::Divider {
                modulus,
                high,
                initial,
            } => {
                if !m.started {
                    m.count = *initial;
                }
                let c = pin(0);
                if !dc && c == High && m.clock == Some(Low) {
                    m.count = (m.count + 1) % modulus;
                }
                if c != Unknown {
                    m.clock = Some(c);
                }
                strong(if high == modulus || (m.count > 0 && m.count <= *high) {
                    High
                } else {
                    Low
                })
            }
        };
        m.started = true;
        (out, delay)
    }
}
