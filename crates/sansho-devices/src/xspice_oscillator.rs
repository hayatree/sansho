//! Voltage-controlled oscillator phase and output waveforms.
//! Provenance: continuous phase is the time integral of cycles per second. Waveforms are original analytic constructions.

use super::{Curve, D, invalid};
use crate::{Mode, ModelParams};
use sansho_diag::Diagnostic;

#[derive(Debug)]
enum Shape {
    Sine,
    Triangle,
    Square { rise: f64, fall: f64 },
}
#[derive(Debug)]
pub(super) struct Oscillator {
    curve: Curve,
    low: f64,
    high: f64,
    duty: f64,
    shape: Shape,
    max_frequency: f64,
}
impl Oscillator {
    pub(super) fn new(
        kind: &str,
        p: &ModelParams,
        control: Vec<f64>,
        frequency: Vec<f64>,
    ) -> Result<Self, Diagnostic> {
        let max_frequency = frequency.iter().copied().fold(0., f64::max);
        if frequency.iter().any(|v| *v <= 0.) {
            return Err(invalid("oscillator frequencies must be positive"));
        }
        let curve = Curve::new(control, frequency, 0., false)?;
        let low = p.get("out_low", -1.);
        let high = p.get("out_high", 1.);
        let duty = if kind == "sine" {
            0.5
        } else {
            p.get("duty_cycle", 0.5)
        };
        if high <= low || !(0. < duty && duty < 1.) {
            return Err(invalid("invalid oscillator range or duty cycle"));
        }
        let shape = match kind {
            "triangle" => Shape::Triangle,
            "square" => {
                let rise = p.get("rise_time", 1e-9);
                let fall = p.get("fall_time", 1e-9);
                if rise <= 0.
                    || fall <= 0.
                    || rise * max_frequency >= duty
                    || fall * max_frequency >= 1. - duty
                {
                    return Err(invalid("square wave transitions must fit within the pulse"));
                }
                Shape::Square { rise, fall }
            }
            _ => Shape::Sine,
        };
        Ok(Self {
            curve,
            low,
            high,
            duty,
            shape,
            max_frequency,
        })
    }
    pub(super) fn frequency(&self, u: D) -> D {
        let (start, end) = (self.curve.x[0], self.curve.x[self.curve.x.len() - 1]);
        self.curve.eval(if u.v < start || u.v > end {
            D::cst(u.v.clamp(start, end))
        } else {
            u
        })
    }
    pub(super) fn value(&self, mode: Mode, u: D, phase: D) -> D {
        let cycles = phase;
        let phase = if mode == Mode::Tran {
            cycles - cycles.v.floor()
        } else {
            D::cst(0.)
        };
        let unit = match self.shape {
            Shape::Sine => {
                let a = std::f64::consts::TAU * phase;
                D {
                    v: 0.5 * (1. + a.v.sin()),
                    d: [0.5 * a.v.cos() * a.d[0]],
                }
            }
            Shape::Triangle if phase.v < self.duty => phase / self.duty,
            Shape::Triangle => (1. - phase) / (1. - self.duty),
            Shape::Square { rise, fall } => {
                let f = self.frequency(u);
                let rising = 1. - self.duty;
                if mode != Mode::Tran || cycles.v < rising {
                    // Startup has no preceding high pulse to fall from.
                    D::cst(0.)
                } else if phase.v < fall * f.v {
                    1. - phase / (fall * f)
                } else if phase.v < rising {
                    D::cst(0.)
                } else if phase.v < rising + rise * f.v {
                    (phase - rising) / (rise * f)
                } else {
                    D::cst(1.)
                }
            }
        };
        self.low + (self.high - self.low) * unit
    }
    pub(super) fn max_step(&self) -> f64 {
        1. / (24. * self.max_frequency)
    }
    pub(super) fn crossing(&self, phase: [f64; 2], frequency: [f64; 2]) -> Option<f64> {
        let [a, b] = phase;
        if b <= a {
            return None;
        }
        let mut first = None;
        let mut edge = |at: [f64; 2]| {
            let start = a - at[0];
            let end = b - at[1];
            // Phase accumulated by the DAE can land a few ulps below an edge.
            let resolution = 64. * f64::EPSILON * start.abs().max(1.);
            let next = (start + resolution).floor() + 1.;
            if next <= end && end > start {
                let t = (next - start) / (end - start);
                first = Some(first.map_or(t, |v: f64| v.min(t)));
            }
        };
        edge([0.; 2]);
        if matches!(self.shape, Shape::Triangle) {
            edge([self.duty; 2]);
        }
        if let Shape::Square { rise, fall } = self.shape {
            let rising = 1. - self.duty;
            edge([rising; 2]);
            edge(frequency.map(|f| rising + f * rise));
            if b >= 1. {
                edge(frequency.map(|f| f * fall));
            }
        }
        first
    }
}
