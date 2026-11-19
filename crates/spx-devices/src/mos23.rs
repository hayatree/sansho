//! Classical strong-inversion MOS2 charge-sheet and MOS3 mobility models.
//! Provenance: Vladimirescu & Liu, The Simulation of MOS Integrated Circuits
//! Using SPICE2 (1980), and Massobrio & Antognetti, MOS device modeling.

use crate::{dual::Dual, model::ModelParams};

const EPS_SI: f64 = 11.7 * 8.854_187_812_8e-12;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Channel {
    SquareLaw,
    ChargeSheet {
        narrow: f64,
        critical: f64,
        exponent: f64,
        depth: f64,
        depletion: f64,
        velocity: f64,
    },
    SemiEmpirical {
        narrow: f64,
        theta: f64,
        shortening: f64,
        eta: f64,
        velocity: f64,
    },
}

impl Channel {
    pub(crate) fn bind(level: f64, p: &ModelParams, cox: f64, nsub: Option<f64>) -> Self {
        if level == 1. {
            return Self::SquareLaw;
        }
        let narrow = p.get("delta", 0.) * std::f64::consts::FRAC_PI_4 * EPS_SI / cox;
        let depletion = nsub.map_or(0., |n| (2. * EPS_SI / (crate::CHARGE * n * 1e6)).sqrt());
        if level == 2. {
            // Critical normal field (V/cm) converted to gate overdrive.
            Self::ChargeSheet {
                narrow,
                critical: p.get("ucrit", 1e4) * 100. * EPS_SI / cox,
                exponent: p.get("uexp", 0.),
                depth: p.get("xj", 0.),
                depletion,
                velocity: p.get("vmax", 0.),
            }
        } else {
            Self::SemiEmpirical {
                narrow,
                theta: p.get("theta", 0.),
                shortening: depletion * p.get("kappa", 0.2).sqrt(),
                eta: p.get("eta", 0.) * 8.15e-22 / cox,
                velocity: p.get("vmax", 0.),
            }
        }
    }
    pub(crate) fn valid(&self) -> bool {
        let valid = |x: f64| x.is_finite() && x >= 0.;
        match *self {
            Self::SquareLaw => true,
            Self::ChargeSheet {
                critical,
                exponent,
                narrow,
                depth,
                depletion,
                velocity,
            } => {
                critical > 0.
                    && [critical, exponent, narrow, depth, depletion, velocity]
                        .into_iter()
                        .all(valid)
            }
            Self::SemiEmpirical {
                narrow,
                theta,
                eta,
                velocity,
                shortening,
            } => eta.is_finite() && [narrow, theta, velocity, shortening].into_iter().all(valid),
        }
    }
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::SquareLaw => "mos1",
            Self::ChargeSheet { .. } => "mos2",
            Self::SemiEmpirical { .. } => "mos3",
        }
    }

    /// Returns drain current, threshold overdrive and saturation voltage for Meyer partitioning.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn evaluate<const N: usize>(
        &self,
        overdrive: Dual<N>,
        vd: Dual<N>,
        root: Dual<N>,
        gamma: f64,
        beta: f64,
        lambda: f64,
        width: f64,
        length: f64,
        mobility: f64,
        surface_states: f64,
        vt: f64,
    ) -> (Dual<N>, f64, f64) {
        let (narrow, dibl) = match self {
            Self::SquareLaw => (0., 0.),
            Self::ChargeSheet { narrow, .. } => (*narrow / width, 0.),
            Self::SemiEmpirical { narrow, eta, .. } => (*narrow / width, *eta / length.powi(3)),
        };
        // In MOS2 the source and drain junctions support part of the bulk
        // depletion charge. Each encroaches from its own biased end.
        let sharing = match *self {
            Self::ChargeSheet {
                depth, depletion, ..
            } if depth > 0. && depletion > 0. => {
                let encroach = |surface: Dual<N>| {
                    ((surface * (2. * depletion / depth) + 1.).sqrt() - 1.)
                        * (depth / (2. * length))
                };
                let shared = encroach(root) + encroach((root.sq() + vd).sqrt());
                if shared.v < 1. { shared } else { Dual::cst(1.) }
            }
            _ => Dual::cst(0.),
        };
        let u = overdrive + root * sharing * gamma - root.sq() * narrow + vd * dibl;
        // Match the exponential weak-inversion characteristic at one
        // subthreshold swing voltage above the strong-inversion threshold.
        let swing = if surface_states > 0. {
            let depletion = if root.v > 0. {
                (1. - sharing) * gamma / (root * 2.)
            } else {
                Dual::cst(0.)
            };
            (depletion + 1. + surface_states + narrow) * vt
        } else {
            Dual::cst(0.)
        };
        let gate_overdrive = u.v - swing.v;
        let (u, weak) = if surface_states > 0. && u.v < swing.v {
            (swing, (u / swing - 1.).exp())
        } else {
            (u, Dual::cst(1.))
        };
        if u.v <= 0. {
            return (Dual::cst(0.), u.v, 0.);
        }
        let clip = |sat: Dual<N>| if vd.v < sat.v { vd } else { sat };
        let clm = vd * lambda + 1.;
        match *self {
            Self::SquareLaw => {
                let id = if u.v <= vd.v {
                    u.sq() * (0.5 * beta) * clm
                } else {
                    vd * (u - vd * 0.5) * beta * clm
                };
                (id, u.v, u.v)
            }
            Self::ChargeSheet {
                critical,
                exponent,
                velocity,
                ..
            } => {
                // Local inversion charge vanishes when u = V + gamma*(sqrt(P+V)-sqrt(P)).
                let gamma = (1. - sharing) * gamma;
                let a = root * 2. + gamma;
                let dr = u * 2. / ((a.sq() + u * 4.).sqrt() + a);
                let sat = dr * (root * 2. + dr);
                let integral = |v: Dual<N>| {
                    let end = (root.sq() + v).sqrt();
                    // Integral of sqrt(P+y)-sqrt(P), evaluated without subtracting cubes.
                    let sum = end + root;
                    let body = if gamma.v == 0. || sum.v == 0. {
                        Dual::cst(0.)
                    } else {
                        (end * 2. + root) / (sum.sq() * 3.) * gamma
                    };
                    v * (u - v * (body + 0.5))
                };
                let mobility_factor = if exponent != 0. && u.v > critical {
                    (u / critical).powf(-exponent)
                } else {
                    Dual::cst(1.)
                };
                // Velocity saturation occurs when the integrated drift current
                // equals W*Cox*vmax times the local drain-end inversion charge.
                let mut sat = sat;
                if velocity > 0. {
                    let vc = (velocity * length / mobility) / mobility_factor;
                    let mut lo = Dual::cst(0.);
                    let mut hi = sat;
                    sat = hi * 0.5;
                    for _ in 0..64 {
                        let end = (root.sq() + sat).sqrt();
                        let charge = u - sat - gamma * (end - root);
                        let residual = integral(sat) - vc * charge;
                        let slope = charge + vc * (1. + gamma / (end * 2.));
                        let next = sat - residual / slope;
                        if (next.v - sat.v).abs() <= 1e-13 * (1. + sat.v.abs()) {
                            sat = next;
                            break;
                        }
                        if residual.v > 0. {
                            hi = sat;
                        } else {
                            lo = sat;
                        }
                        sat = if next.v > lo.v && next.v < hi.v {
                            next
                        } else {
                            (lo + hi) * 0.5
                        };
                    }
                }
                let id =
                    integral(clip(sat)) * beta * mobility_factor / remaining_length(vd * lambda);
                (id * weak, gate_overdrive, sat.v)
            }
            Self::SemiEmpirical {
                theta,
                velocity,
                shortening,
                ..
            } => {
                let slope = if root.v > 0. {
                    Dual::cst(gamma) / (root * 4.)
                } else {
                    Dual::cst(0.)
                };
                let bulk = slope + narrow + 1.;
                let long = u / bulk;
                let vertical = u * theta + 1.;
                let field_voltage = vertical * (velocity * length / mobility);
                let sat = if velocity > 0. {
                    // Rationalized form of Vlong + Vc - hypot(Vlong,Vc).
                    long * field_voltage * 2.
                        / (long + field_voltage + (long.sq() + field_voltage.sq()).sqrt())
                } else {
                    long
                };
                let v = clip(sat);
                let lateral = if velocity > 0. {
                    v / field_voltage + 1.
                } else {
                    Dual::cst(1.)
                };
                // Drain depletion widens beyond pinch-off. Continue the length
                // smoothly beyond half the channel to keep Leff positive.
                let loss = if shortening > 0. && vd.v > sat.v {
                    (vd - sat).sqrt() * (shortening / length)
                } else {
                    Dual::cst(0.)
                };
                let id =
                    v * (u - bulk * v * 0.5) * beta / (vertical * lateral * remaining_length(loss));
                (id * weak, gate_overdrive, sat.v)
            }
        }
    }
}

/// Fraction of the effective channel left after drain-end shortening.
/// The continuation agrees in value and slope at half the nominal length.
fn remaining_length<const N: usize>(loss: Dual<N>) -> Dual<N> {
    if loss.v <= 0.5 {
        1. - loss
    } else {
        0.25 / loss
    }
}

pub(crate) fn body_coefficient(nsub: f64, cox: f64) -> f64 {
    (2. * crate::CHARGE * EPS_SI * nsub * 1e6).sqrt() / cox
}

#[cfg(test)]
mod tests {
    use super::*;
    fn channel(level: f64, extra: &[(&str, f64)]) -> Channel {
        let pairs = extra
            .iter()
            .map(|&(k, v)| (k.into(), v))
            .collect::<Vec<_>>();
        Channel::bind(level, &ModelParams::new(&pairs), 1e-3, None)
    }
    fn evaluate(model: &Channel, gate: f64, drain: f64) -> Dual<2> {
        model
            .evaluate(
                Dual::var(gate, 0),
                Dual::var(drain, 1),
                Dual::cst(0.8),
                0.6,
                2e-4,
                0.,
                10e-6,
                2e-6,
                0.06,
                0.,
                0.026,
            )
            .0
    }
    #[test]
    fn level_two_integrates_channel_charge_and_pinches_off() {
        let model = channel(2., &[]);
        let gate = 1.3;
        for vd in [0.2, 0.7, 1.8] {
            // Integrate the local sheet charge independently using midpoint quadrature.
            let step = vd / 8192.;
            let charge = (0..8192)
                .map(|k| {
                    let v = (f64::from(k) + 0.5) * step;
                    (gate - v - 0.6 * ((0.64 + v).sqrt() - 0.8)).max(0.)
                })
                .sum::<f64>()
                * step;
            assert!((evaluate(&model, gate, vd).v - charge * 2e-4).abs() < 2e-12);
        }
    }
    #[test]
    fn level_three_body_slope_and_vertical_mobility() {
        let model = channel(3., &[("theta", 0.25)]);
        let u = 1.6;
        let slope = 1. + 0.6 / (4. * 0.8);
        let expected = 2e-4 * 0.5 * u * u / (slope * (1. + 0.25 * u));
        assert!((evaluate(&model, u, 2.).v - expected).abs() < 1e-17);
    }
    #[test]
    fn incremental_conductances_follow_each_channel_current() {
        for m in [
            channel(2., &[("vmax", 8e4), ("ucrit", 2e3), ("uexp", 0.4)]),
            channel(3., &[("theta", 0.2), ("vmax", 8e4)]),
        ] {
            for (vg, vd) in [(0.8, 0.1), (1.1, 1.8)] {
                let id = evaluate(&m, vg, vd);
                let h = 1e-6;
                let gm = (evaluate(&m, vg + h, vd).v - evaluate(&m, vg - h, vd).v) / (2. * h);
                let gds = (evaluate(&m, vg, vd + h).v - evaluate(&m, vg, vd - h).v) / (2. * h);
                assert!((gm - id.d[0]).abs() < 1e-12);
                assert!((gds - id.d[1]).abs() < 1e-12);
            }
        }
    }
}
