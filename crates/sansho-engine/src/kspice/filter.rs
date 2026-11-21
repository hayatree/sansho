//! Scalar modal admittance and delay-extracted propagation filters.
//! Provenance: Lin and Kuh (1992), recursive convolution of lossy interconnect functions.
//! Original rational quadrature of the scalar telegrapher branch cut.

use std::f64::consts::PI;

#[derive(Debug, Clone)]
struct Pole {
    rate: f64,
    // DC weights for rate / (s + rate).
    local: f64,
    feedthrough: f64,
    first_weight: f64,
    tail_weight: f64,
    tail_rate: f64,
}
#[derive(Debug, Clone)]
pub(super) struct Kernel {
    pub delay: f64,
    dc_gain: f64,
    poles: Vec<Pole>,
}
impl Kernel {
    pub fn new(delay: f64, a: f64, b: f64) -> Self {
        // sqrt((s+b)/(s+a)) is the Chebyshev average of (s+b)/(s+p).
        // Multiplying by (s+a) gives the delay-extracted propagation exponent
        // as a sum of nonnegative relaxation terms D*s/(s+p).
        let dc_gain = (-(delay * a.sqrt()) * b.sqrt()).exp();
        let mut poles = Vec::new();
        if a > b {
            const ORDER: usize = 32;
            let mut terms = Vec::with_capacity(ORDER);
            for i in 0..ORDER {
                let angle = PI * (i as f64 + 0.5) / (2 * ORDER) as f64;
                let from_low = (a - b) * angle.cos().powi(2);
                let from_high = (a - b) * angle.sin().powi(2);
                let rate = b + from_low;
                let local = (from_low / rate) / ORDER as f64;
                terms.push((rate, local, from_high * local));
            }
            let root_ratio = (b / a).sqrt();
            let difference = ((a - b) / a) / (1. + root_ratio);
            let local_sum: f64 = terms.iter().map(|v| v.1).sum();
            let loss_sum: f64 = terms.iter().map(|v| v.2).sum();
            let excess = 0.5 * ((a - b) * difference) / (1. + root_ratio);
            for (rate, local, loss) in terms {
                let u = if loss_sum > 0. {
                    delay * (excess * (loss / loss_sum))
                } else {
                    0.
                };
                // Expand exp(-u*s/(s+p)) as exp(-u) * exp(u*p/(s+p)).
                // Keep the direct wave and the first relaxation exactly. Give the
                // remaining mass one additional pole chosen to preserve its DC slope.
                let feedthrough = (-u).exp();
                let first_weight = if feedthrough == 0. {
                    0.
                } else {
                    u * feedthrough
                };
                let (tail_weight, rate_ratio) = if u < 1e-3 {
                    let mass = 0.5 + u * (-1. / 3. + u * (1. / 8. - u / 30.));
                    let moment = 0.5 + u * (-1. / 6. + u * (1. / 24. - u / 120.));
                    (u * u * mass, mass / moment)
                } else {
                    let total = -(-u).exp_m1();
                    let mass = total - first_weight;
                    (mass, mass / (u - total))
                };
                poles.push(Pole {
                    rate,
                    local: -(local / local_sum) * difference,
                    feedthrough,
                    first_weight,
                    tail_weight,
                    tail_rate: rate * rate_ratio,
                });
            }
        }
        Self {
            delay,
            dc_gain,
            poles,
        }
    }

    pub fn memory(&self) -> Memory {
        Memory {
            states: vec![[0.; 4]; self.poles.len()],
            drive: 0.,
        }
    }

    /// Local filtered drive = offset + slope * trial endpoint.
    pub fn local(&self, mem: &Memory, h: f64) -> (f64, f64) {
        let mut offset = 0.;
        let mut slope = 1.;
        for (p, x) in self.poles.iter().zip(&mem.states) {
            let (decay, left, right) = weights(p.rate * h);
            offset += p.local * (decay * x[0] + left * mem.drive);
            slope += p.local * right;
        }
        (offset, slope)
    }

    /// Commit a piecewise-linear segment and return its filtered outgoing wave.
    pub fn accept(&self, mem: &mut Memory, h: f64, drive: f64, other: f64) -> f64 {
        let mut wave = drive + other;
        for (p, x) in self.poles.iter().zip(&mut mem.states) {
            let (decay, left, right) = weights(p.rate * h);
            x[0] = decay * x[0] + left * mem.drive + right * drive;
            wave += p.local * x[0];
        }
        for (p, x) in self.poles.iter().zip(&mut mem.states) {
            let previous = x[1];
            let (decay, left, right) = weights(p.rate * h);
            x[1] = decay * x[1] + left * x[2] + right * wave;
            x[2] = wave;
            let (decay, left, right) = weights(p.tail_rate * h);
            x[3] = decay * x[3] + left * previous + right * x[1];
            wave = p.feedthrough * wave + p.first_weight * x[1] + p.tail_weight * x[3];
        }
        mem.drive = drive;
        self.dc_gain * wave
    }
}

#[derive(Debug)]
pub(super) struct Memory {
    states: Vec<[f64; 4]>,
    drive: f64,
}

/// Exact first-order filter response to a ramp, including very short steps.
fn weights(x: f64) -> (f64, f64, f64) {
    let mass = -(-x).exp_m1();
    let right = if x < 1e-4 {
        x * (0.5 + x * (-1. / 6. + x / 24.))
    } else {
        1. - mass / x
    };
    ((-x).exp(), mass - right, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_functions_track_the_delay_extracted_transforms() {
        for (a, b, delay) in [(1., 0., 0.2), (3., 0.3, 0.5), (2., 2., 0.5)] {
            let k = Kernel::new(delay, a, b);
            for s in [0.01, 0.1, 1., 10., 100.] {
                let y = 1.
                    + k.poles
                        .iter()
                        .map(|p| p.local * p.rate / (s + p.rate))
                        .sum::<f64>();
                let h = k.poles.iter().fold(k.dc_gain, |v, p| {
                    v * (p.feedthrough
                        + p.rate / (s + p.rate)
                            * (p.first_weight + p.tail_weight * p.tail_rate / (s + p.tail_rate)))
                });
                let admittance = ((s + b) / (s + a)).sqrt();
                let propagation = (-delay * (((s + a) * (s + b)).sqrt() - s)).exp();
                assert!((y - admittance).abs() < 2e-6, "{y} vs {admittance}");
                assert!((h - propagation).abs() < 3e-5, "{h} vs {propagation}");
            }
            let wavefront = k.poles.iter().fold(k.dc_gain, |v, p| v * p.feedthrough);
            assert!((wavefront - (-0.5 * delay * (a + b)).exp()).abs() < 1e-14);
            // First term after the delay-extracted front in the large-s expansion.
            let tail: f64 = k
                .poles
                .iter()
                .map(|p| p.first_weight * p.rate / p.feedthrough)
                .sum();
            assert!((tail - delay * (a - b).powi(2) / 8.).abs() < 1e-13);
        }
    }

    #[test]
    fn series_loss_dc_moment_survives_pole_discretization() {
        for r in [0.2, 2., 20., 100.] {
            let k = Kernel::new(1., r, 0.);
            let y_prime: f64 = k.poles.iter().map(|p| -p.local / p.rate).sum();
            let minus_h_prime = k.delay
                + k.poles
                    .iter()
                    .map(|p| {
                        (p.first_weight + p.tail_weight) / p.rate + p.tail_weight / p.tail_rate
                    })
                    .sum::<f64>();
            assert!((minus_h_prime / y_prime - r).abs() < 1e-11);
        }
    }
}
