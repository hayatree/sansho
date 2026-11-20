//! Transient source waveforms and timestep constraints.
//!
//! Provenance: original work. Waveform definitions and parameter defaults follow the SPICE2
//! independent-source descriptions in Nagel (1975) and the widely documented SPICE3 behaviour
//! (zero or missing rise/fall times default to `tstep`, missing width/period to `tstop`).
//! AM and SFFM follow the ngspice user manual, sections 4.1.5 and 4.1.6.

use std::f64::consts::PI;

use sansho_netlist::TranFn;

use crate::TranParams;

fn param(p: &[f64], i: usize) -> Option<f64> {
    p.get(i).copied()
}

/// Parameter `i`, or `default` when missing or (for `zero_is_default`) equal to zero.
fn param_or(p: &[f64], i: usize, default: f64, zero_is_default: bool) -> f64 {
    match param(p, i) {
        Some(v) if !(zero_is_default && v == 0.0) => v,
        _ => default,
    }
}

struct Pulse {
    v1: f64,
    v2: f64,
    td: f64,
    tr: f64,
    tf: f64,
    pw: f64,
    per: f64,
    np: Option<f64>,
}

impl Pulse {
    fn new(p: &[f64], tp: &TranParams) -> Self {
        Self {
            v1: param_or(p, 0, 0.0, false),
            v2: param_or(p, 1, 0.0, false),
            td: param_or(p, 2, 0.0, false),
            tr: param_or(p, 3, tp.tstep, true),
            tf: param_or(p, 4, tp.tstep, true),
            pw: param_or(p, 5, tp.tstop, true),
            per: param_or(p, 6, tp.tstop, true),
            np: param(p, 7).filter(|&n| n > 0.0),
        }
    }

    fn period_index(&self, t: f64) -> (f64, f64) {
        // Returns (local time within period, period index).
        let tl = t - self.td;
        if self.per <= 0.0 {
            return (tl, 0.0);
        }
        let k = (tl / self.per).floor();
        if let Some(np) = self.np
            && k >= np
        {
            // After the last pulse: stay at v1.
            return (f64::INFINITY, k);
        }
        (tl - k * self.per, k)
    }

    fn value(&self, t: f64) -> f64 {
        if t < self.td {
            return self.v1;
        }
        let (tl, _) = self.period_index(t);
        if tl <= 0.0 {
            self.v1
        } else if tl < self.tr {
            self.v1 + (self.v2 - self.v1) * tl / self.tr
        } else if tl <= self.tr + self.pw {
            self.v2
        } else if tl < self.tr + self.pw + self.tf {
            self.v2 + (self.v1 - self.v2) * (tl - self.tr - self.pw) / self.tf
        } else {
            self.v1
        }
    }

    fn breakpoints(&self, tstop: f64, out: &mut Vec<f64>) {
        let corners = [0.0, self.tr, self.tr + self.pw, self.tr + self.pw + self.tf];
        let mut k = 0.0;
        loop {
            if self.np.is_some_and(|np| k >= np) {
                break;
            }
            let base = self.td + k * self.per;
            if base > tstop {
                break;
            }
            out.extend(corners.iter().map(|c| base + c).filter(|&t| t <= tstop));
            if self.per <= 0.0 {
                break;
            }
            k += 1.0;
        }
    }
}

/// Common time base for AM and SFFM, with phase measured from the delay.
struct Modulation {
    fc: f64,
    fm: f64,
    td: f64,
    phase_m: f64,
    phase_c: f64,
}

impl Modulation {
    fn new(p: &[f64], carrier: usize, modulator: usize, tp: &TranParams) -> Self {
        Self {
            fc: param_or(p, carrier, 500. / tp.tstop, false),
            fm: param_or(p, modulator, 5. / tp.tstop, false),
            td: param_or(p, 5, 0., false),
            phase_m: param_or(p, 6, 0., false).to_radians(),
            phase_c: param_or(p, 7, 0., false).to_radians(),
        }
    }

    fn angles(&self, t: f64) -> (f64, f64) {
        let angle = 2. * PI * (t - self.td);
        (
            self.fc * angle + self.phase_c,
            self.fm * angle + self.phase_m,
        )
    }

    fn index(&self, p: &[f64]) -> f64 {
        let index = param_or(p, 3, 90., false).max(0.);
        if self.fm == 0. {
            index
        } else {
            index.min((self.fc / self.fm).abs())
        }
    }
}

/// Start time and maximum step for a modulated source. Bound the upper AM sideband
/// or the maximum instantaneous FM frequency, including a rapidly varying modulator.
pub fn step_limit(f: &TranFn, tp: &TranParams) -> Option<(f64, f64)> {
    let (m, frequency) = match f {
        TranFn::TrRandom(p) => {
            return p
                .get(1)
                .copied()
                .filter(|v| *v > 0.)
                .map(|step| (p.get(2).copied().unwrap_or(0.), step));
        }
        TranFn::TrNoise(p) => {
            return p
                .get(1)
                .into_iter()
                .chain(p.get(5..).unwrap_or_default())
                .copied()
                .filter(|v| *v > 0.)
                .reduce(f64::min)
                .map(|step| (0., step));
        }
        TranFn::Am(p) => {
            let m = Modulation::new(p, 4, 3, tp);
            let frequency = m.fc.abs() + m.fm.abs();
            (m, frequency)
        }
        TranFn::Sffm(p) => {
            let m = Modulation::new(p, 2, 4, tp);
            let index = m.index(p);
            let mut frequency = m.fc.abs() + index * m.fm.abs();
            if index > 0. {
                frequency = frequency.max(m.fm.abs());
            }
            (m, frequency)
        }
        _ => return None,
    };
    (frequency > 0. && m.td < tp.tstop).then(|| (m.td.max(0.), frequency.recip() / 64.))
}

/// Evaluate a transient function at time `t`.
pub fn value(f: &TranFn, t: f64, tp: &TranParams) -> f64 {
    match f {
        TranFn::TrNoise(_) | TranFn::TrRandom(_) => crate::random::Stream::default().value(f, t),
        TranFn::Pulse(p) => Pulse::new(p, tp).value(t),
        TranFn::Sin(p) => {
            let vo = param_or(p, 0, 0.0, false);
            let va = param_or(p, 1, 0.0, false);
            let default_freq = if tp.tstop > 0.0 { 1.0 / tp.tstop } else { 0.0 };
            let freq = param_or(p, 2, default_freq, true);
            let td = param_or(p, 3, 0.0, false);
            let theta = param_or(p, 4, 0.0, false);
            let phase = param_or(p, 5, 0.0, false) * PI / 180.0;
            if t < td {
                vo + va * phase.sin()
            } else {
                let tl = t - td;
                vo + va * (-tl * theta).exp() * (2.0 * PI * freq * tl + phase).sin()
            }
        }
        TranFn::Exp(p) => {
            let v1 = param_or(p, 0, 0.0, false);
            let v2 = param_or(p, 1, 0.0, false);
            let td1 = param_or(p, 2, 0.0, false);
            let tau1 = param_or(p, 3, tp.tstep, true);
            let td2 = param_or(p, 4, td1 + tp.tstep, true);
            let tau2 = param_or(p, 5, tp.tstep, true);
            let mut v = v1;
            if t >= td1 {
                v += (v2 - v1) * (1.0 - (-(t - td1) / tau1).exp());
            }
            if t >= td2 {
                v += (v1 - v2) * (1.0 - (-(t - td2) / tau2).exp());
            }
            v
        }
        TranFn::Sffm(p) => {
            let m = Modulation::new(p, 2, 4, tp);
            if t <= m.td {
                return 0.;
            }
            let (carrier, modulator) = m.angles(t);
            param_or(p, 0, 0., false)
                + param_or(p, 1, 0., false) * (carrier + m.index(p) * modulator.sin()).sin()
        }
        TranFn::Am(p) => {
            let m = Modulation::new(p, 4, 3, tp);
            if t <= m.td {
                return 0.;
            }
            let (carrier, modulator) = m.angles(t);
            let envelope = param_or(p, 1, 0., false) + param_or(p, 2, 1., false) * modulator.sin();
            param_or(p, 0, 0., false) + envelope * carrier.sin()
        }
        TranFn::Pwl(pts) => pwl(pts, t),
    }
}

fn pwl(pts: &[(f64, f64)], t: f64) -> f64 {
    let Some(&(t0, v0)) = pts.first() else {
        return 0.0;
    };
    if t <= t0 {
        return v0;
    }
    for w in pts.windows(2) {
        let ((ta, va), (tb, vb)) = (w[0], w[1]);
        if t <= tb {
            if tb == ta {
                return vb;
            }
            return va + (vb - va) * (t - ta) / (tb - ta);
        }
    }
    pts.last().map_or(0.0, |p| p.1)
}

/// Value at t = 0 using only the parameters given (no analysis-dependent defaults). Used as the
/// DC value of sources that have a transient function but no explicit DC value.
pub fn initial_value(f: &TranFn) -> f64 {
    match f {
        TranFn::TrNoise(_) | TranFn::TrRandom(_) => 0.,
        TranFn::Pulse(p) | TranFn::Exp(p) => param_or(p, 0, 0.0, false),
        TranFn::Sin(p) => {
            let phase = param_or(p, 5, 0.0, false).to_radians();
            param_or(p, 0, 0.0, false) + param_or(p, 1, 0.0, false) * phase.sin()
        }
        TranFn::Sffm(_) | TranFn::Am(_) => value(
            f,
            0.,
            &TranParams {
                tstep: 1.,
                tstop: 1.,
            },
        ),
        TranFn::Pwl(pts) => pwl(pts, 0.0),
    }
}

/// Append the breakpoints of a transient function within `[0, tstop]`.
pub fn breakpoints(f: &TranFn, tp: &TranParams, out: &mut Vec<f64>) {
    let tstop = tp.tstop;
    match f {
        TranFn::TrNoise(_) | TranFn::TrRandom(_) => {}
        TranFn::Pulse(p) => Pulse::new(p, tp).breakpoints(tstop, out),
        TranFn::Sin(p) => out.extend(param(p, 3).filter(|&td| td > 0.0 && td <= tstop)),
        TranFn::Exp(p) => {
            let td1 = param_or(p, 2, 0.0, false);
            let td2 = param_or(p, 4, td1 + tp.tstep, true);
            out.extend([td1, td2].into_iter().filter(|&t| t > 0.0 && t <= tstop));
        }
        TranFn::Am(p) | TranFn::Sffm(p) => {
            out.extend(param(p, 5).filter(|&td| td > 0. && td <= tstop))
        }
        TranFn::Pwl(pts) => out.extend(pts.iter().map(|p| p.0).filter(|&t| t >= 0.0 && t <= tstop)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TP: TranParams = TranParams {
        tstep: 1e-9,
        tstop: 1e-6,
    };

    #[test]
    fn pulse_shape_and_breakpoints() {
        let f = TranFn::Pulse(vec![0.0, 1.0, 10e-9, 2e-9, 4e-9, 20e-9, 100e-9]);
        assert_eq!(value(&f, 5e-9, &TP), 0.0);
        assert!((value(&f, 11e-9, &TP) - 0.5).abs() < 1e-12);
        assert_eq!(value(&f, 20e-9, &TP), 1.0);
        assert!((value(&f, 34e-9, &TP) - 0.5).abs() < 1e-12);
        assert_eq!(value(&f, 50e-9, &TP), 0.0);
        // Periodic repetition.
        assert!((value(&f, 111e-9, &TP) - 0.5).abs() < 1e-9);
        let mut bp = Vec::new();
        breakpoints(&f, &TP, &mut bp);
        assert_eq!(bp.len(), 40);
        assert!((bp[1] - 12e-9).abs() < 1e-18);
    }

    #[test]
    fn pulse_defaults() {
        // tr = tf = tstep, pw = per = tstop
        let f = TranFn::Pulse(vec![0.0, 2.0]);
        assert!((value(&f, 0.5e-9, &TP) - 1.0).abs() < 1e-12);
        assert_eq!(value(&f, 0.5e-6, &TP), 2.0);
    }

    #[test]
    fn modulated_sources_at_quadrature_and_delay() {
        let fm = TranFn::Sffm(vec![1., 2., 4e6, 0.5, 1e6, 1e-7, 90., 30.]);
        let am = TranFn::Am(vec![2., 1., 3., 1e6, 5e6, 1e-7, 0., 30.]);
        for f in [&am, &fm] {
            assert_eq!(initial_value(f), 0.);
            assert_eq!(value(f, 1e-7, &TP), 0.);
            let mut points = Vec::new();
            breakpoints(f, &TP, &mut points);
            assert_eq!(points, [1e-7]);
        }
        assert!((value(&fm, 0.35e-6, &TP) - 2.).abs() < 1e-12);
        assert!((value(&am, 0.35e-6, &TP) - (2. + 2. * 3_f64.sqrt())).abs() < 1e-12);

        let t = TP.tstop / 2000.;
        let modulator = (2. * PI * 5. * t / TP.tstop).sin();
        let fm = TranFn::Sffm(vec![1., 2.]);
        let am = TranFn::Am(vec![2., 1.]);
        assert!((value(&fm, t, &TP) - (1. + 2. * (90. * modulator).cos())).abs() < 1e-12);
        assert!((value(&am, t, &TP) - (3. + modulator)).abs() < 1e-12);
    }

    #[test]
    fn sin_exp_pwl() {
        let s = TranFn::Sin(vec![1.0, 2.0, 1e6]);
        assert!((value(&s, 0.25e-6, &TP) - 3.0).abs() < 1e-12);
        let e = TranFn::Exp(vec![0.0, 1.0, 0.0, 1e-9, 1.0, 1e-9]);
        assert!((value(&e, 1e-9, &TP) - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
        let p = TranFn::Pwl(vec![(0.0, 0.0), (1e-9, 1.0), (2e-9, 1.0), (3e-9, 0.0)]);
        assert_eq!(value(&p, 0.5e-9, &TP), 0.5);
        assert_eq!(value(&p, 2.5e-9, &TP), 0.5);
        assert_eq!(value(&p, 5e-9, &TP), 0.0);
    }
}
