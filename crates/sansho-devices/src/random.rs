//! Indexed transient random samples and two-state telegraph noise.
//! Provenance: ngspice user manual, transient noise and random sources; Box–Muller
//! Gaussian transform, exponential waiting times and octave-band noise synthesis.

use sansho_netlist::TranFn;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub(crate) struct Stream {
    key: u64,
    events: Mutex<Vec<f64>>,
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 31;
    x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
    x ^= x >> 27;
    x = x.wrapping_mul(0xa24b_aed4_963e_e407);
    x ^ (x >> 33)
}

impl Stream {
    pub(crate) fn new(name: &str, seed: u64) -> Self {
        Self {
            key: name.bytes().fold(mix(seed), |h, b| mix(h ^ u64::from(b))),
            events: Mutex::default(),
        }
    }

    fn uniform(&self, index: u64, lane: u64) -> f64 {
        let bits =
            mix(self.key ^ mix(lane.wrapping_add(1)) ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        ((bits >> 12) as f64 + 0.5) / (1_u64 << 52) as f64
    }

    fn normal(&self, index: u64, lane: u64) -> f64 {
        (-2. * self.uniform(index, lane).ln()).sqrt()
            * (std::f64::consts::TAU * self.uniform(index, lane + 1)).cos()
    }

    fn poisson(&self, index: u64, mean: f64) -> f64 {
        if mean == 0. {
            return 0.;
        }
        let mut lane = 0;
        let mut draw = || {
            lane += 1;
            self.uniform(index, lane)
        };
        if mean < 30. {
            let limit = (-mean).exp();
            let mut product = draw();
            let mut k = 0.;
            while product > limit {
                product *= draw();
                k += 1.;
            }
            return k;
        }
        // Hörmann's transformed rejection method for the Poisson distribution.
        let b = 0.931 + 2.53 * mean.sqrt();
        let a = 0.02483 * b - 0.059;
        let inv_alpha = 1.1239 + 1.1328 / (b - 3.4);
        loop {
            let u = draw() - 0.5;
            let v = draw();
            let us = 0.5 - u.abs();
            let k = ((2. * a / us + b) * u + mean + 0.43).floor();
            if us >= 0.07 && v <= 0.9277 - 3.6224 / (b - 2.) {
                return k;
            }
            if k < 0. || (us < 0.013 && v > us) {
                continue;
            }
            let log_factorial = if k < 16. {
                (2..=k as u32).map(|n| f64::from(n).ln()).sum()
            } else {
                (k + 0.5) * k.ln() - k + 0.5 * std::f64::consts::TAU.ln() + 1. / (12. * k)
                    - 1. / (360. * k.powi(3))
            };
            if (v * inv_alpha / (a / us.powi(2) + b)).ln() <= -mean + k * mean.ln() - log_factorial
            {
                return k;
            }
        }
    }

    // Sample stationary independent octave bands. A band of width f needs
    // variance proportional to f^(1-alpha) for a density proportional to f^-alpha.
    fn colored(&self, index: u64, alpha: f64) -> f64 {
        let mut value = 0.;
        let mut variance = 0.;
        for octave in 0..20 {
            let width = 1_u64 << octave;
            let k = index / width;
            let fraction = (index % width) as f64 / width as f64;
            let weight = (width as f64).powf((alpha - 1.) * 0.5);
            let lane = 8 + 2 * octave;
            value += weight
                * ((1. - fraction) * self.normal(k, lane)
                    + fraction * self.normal(k.saturating_add(1), lane));
            variance += weight
                * weight
                * (if octave == 0 {
                    1.
                } else {
                    2. / 3. + 1. / (3. * (width as f64).powi(2))
                });
        }
        value / variance.sqrt()
    }

    fn telegraph(&self, t: f64, capture: f64, emission: f64) -> (bool, f64) {
        if !t.is_finite() || !(capture > 0. && emission > 0.) {
            return (false, f64::INFINITY);
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        while events.last().is_none_or(|&last| last <= t) {
            let k = events.len();
            let start = events.last().copied().unwrap_or(0.);
            let tau = if k.is_multiple_of(2) {
                capture
            } else {
                emission
            };
            let end = start - tau * self.uniform(k as u64, 100).ln();
            events.push(end.max(start.next_up()));
        }
        let k = events.partition_point(|&v| v <= t);
        (!k.is_multiple_of(2), events[k])
    }

    pub(crate) fn value(&self, f: &TranFn, time: f64) -> f64 {
        match f {
            TranFn::TrRandom(p) => {
                let delay = p[2];
                if time < delay {
                    return 0.;
                }
                let index = sample_index(time - delay, p[1]).0;
                let a = p[3];
                let b = p[4];
                b + match p[0] as u8 {
                    1 => a * (2. * self.uniform(index, 0) - 1.),
                    2 => a * self.normal(index, 0),
                    3 => -a * self.uniform(index, 0).ln(),
                    _ => self.poisson(index, a),
                }
            }
            TranFn::TrNoise(p) => {
                let mut result = 0.;
                if p[1] > 0. {
                    let (k, frac) = sample_index(time, p[1]);
                    let sample = |n| {
                        let flicker = p[3];
                        p[0] * self.normal(n, 0)
                            + if flicker != 0. {
                                flicker * self.colored(n, p[2])
                            } else {
                                0.
                            }
                    };
                    result = (1. - frac) * sample(k) + frac * sample(k.saturating_add(1));
                }
                if p[4] != 0. && self.telegraph(time, p[5], p[6]).0 {
                    result += p[4];
                }
                result
            }
            _ => 0.,
        }
    }

    pub(crate) fn next(&self, f: &TranFn, time: f64) -> f64 {
        let (step, delay, edge) = match f {
            TranFn::TrRandom(p) => (p[1], p[2], f64::INFINITY),
            TranFn::TrNoise(p) => (
                p[1],
                0.,
                if p[4] != 0. {
                    self.telegraph(time, p[5], p[6]).1
                } else {
                    f64::INFINITY
                },
            ),
            _ => return f64::INFINITY,
        };
        if time < delay {
            return delay;
        }
        if step <= 0. {
            return edge;
        }
        let index = sample_index(time - delay, step).0;
        edge.min(delay + step * (index as f64 + 1.))
    }
}

fn sample_index(time: f64, step: f64) -> (u64, f64) {
    let u = time / step;
    let near = u.round();
    let u = if (u - near).abs() <= 4. * f64::EPSILON * u.max(1.) {
        near
    } else {
        u
    };
    (u as u64, u.fract())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moments(values: impl Iterator<Item = f64>) -> (f64, f64) {
        let (mut n, mut sum, mut square) = (0., 0., 0.);
        for x in values {
            n += 1.;
            sum += x;
            square += x * x;
        }
        (sum / n, square / n - (sum / n).powi(2))
    }

    #[test]
    fn distribution_moments_and_repeatable_random_access() {
        let source = Stream::new("v1", 7654);
        for (kind, a, mean, variance) in [
            (1., 2., 3., 4_f64 / 3.),
            (2., 2., 3., 4.),
            (3., 2., 5., 4.),
            (4., 2., 5., 2.),
            (4., 80., 83., 80.),
        ] {
            let f = TranFn::TrRandom([kind, 1., 2., a, 3.]);
            let (m, v) = moments((2..65538).map(|k| source.value(&f, f64::from(k))));
            assert!((m - mean).abs() < 0.025 * variance.sqrt(), "{kind}: {m}");
            assert!((v / variance - 1.).abs() < 0.04, "{kind}: {v}");
            assert_eq!(source.value(&f, 1.), 0.);
            assert_eq!(
                source.value(&f, 180.4),
                Stream::new("v1", 7654).value(&f, 180.9)
            );
        }
        let f = TranFn::TrNoise([2., 1., 0., 0., 0., 0., 0.]);
        let (m, v) = moments((1..65537).map(|k| source.value(&f, f64::from(k))));
        assert!(m.abs() < 0.04 && (v / 4. - 1.).abs() < 0.03);
        let left = source.value(&f, 19.);
        assert_eq!(source.value(&f, 19.5), 0.5 * (left + source.value(&f, 20.)));
    }

    #[test]
    fn flicker_spectral_slope() {
        // Average periodogram over independent source names and integer bins
        // spanning three octaves. A Hann window suppresses low-frequency leakage.
        let mut power = [0.; 2];
        let n = 2048;
        for seed in 0..16 {
            let source = Stream::new("inoise", seed);
            let data: Vec<_> = (0..n)
                .map(|k| {
                    source.colored(k, 1.)
                        * (0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos())
                })
                .collect();
            for (band, &first) in [8, 64].iter().enumerate() {
                for bin in first..first * 2 {
                    let phase = std::f64::consts::TAU * f64::from(bin) / n as f64;
                    let (re, im) = data.iter().enumerate().fold((0., 0.), |(re, im), (k, x)| {
                        let (s, c) = (phase * k as f64).sin_cos();
                        (re + x * c, im + x * s)
                    });
                    power[band] += (re * re + im * im) / f64::from(first);
                }
            }
        }
        let slope = (power[1] / power[0]).ln() / 8_f64.ln();
        assert!((slope + 1.).abs() < 0.16, "{slope}");
    }

    #[test]
    fn telegraph_dwell_times() {
        let f = TranFn::TrNoise([0., 0., 0., 0., 0.5, 2., 5.]);
        let stream = Stream::new("itele", 42);
        let mut sum = [0.; 2];
        let mut time = 0.;
        for k in 0..20000 {
            let end = stream.next(&f, time);
            let state = k % 2;
            sum[state] += end - time;
            assert_eq!(stream.value(&f, (end + time) * 0.5), state as f64 * 0.5);
            time = end;
        }
        for (sum, tau) in sum.into_iter().zip([2., 5.]) {
            assert!((sum / 10000. / tau - 1.).abs() < 0.04);
        }
        assert_eq!(stream.value(&f, 0.), 0.);
    }
}
