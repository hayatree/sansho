//! Classical strong-inversion MOS2 charge-sheet and MOS3 mobility models.
//! Provenance: Vladimirescu & Liu, The Simulation of MOS Integrated Circuits
//! Using SPICE2 (1980); Massobrio & Antognetti, MOS device modeling; Sakurai
//! & Newton, n-th power MOSFET model, IEEE TED 38 (1991), 887–894.

use crate::{dual::Dual, model::ModelParams};

const EPS_SI: f64 = 11.7 * 8.854_187_812_8e-12;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Channel {
    SquareLaw,
    PowerLaw {
        kv: f64,
        nv: f64,
        nc: f64,
        gamma1: f64,
        sigma: f64,
        lambda1: f64,
    },
    ChargeSheet {
        narrow: f64,
        auto_length: bool,
        neff: f64,
        critical: f64,
        exponent: f64,
        depth: f64,
        depletion: f64,
        velocity: f64,
    },
    SemiEmpirical {
        narrow: f64,
        theta: f64,
        depth: f64,
        diffusion: f64,
        depletion: f64,
        kappa: f64,
        eta: f64,
        velocity: f64,
    },
}

impl Channel {
    pub(crate) fn bind(level: f64, p: &ModelParams, cox: f64, nsub: Option<f64>) -> Self {
        if level == 1. {
            return Self::SquareLaw;
        }
        if level == 6. {
            return Self::PowerLaw {
                kv: p.get("kv", 2.),
                nv: p.get("nv", 0.5),
                nc: p.get("nc", 1.),
                gamma1: p.get("gamma1", 0.),
                sigma: p.get("sigma", 0.),
                lambda1: p.get("lambda1", 0.),
            };
        }
        let narrow = p.get("delta", 0.) * std::f64::consts::FRAC_PI_4 * EPS_SI / cox;
        let depletion = nsub.map_or(0., |n| (2. * EPS_SI / (crate::CHARGE * n * 1e6)).sqrt());
        if level == 2. {
            // Critical normal field (V/cm) converted to gate overdrive.
            Self::ChargeSheet {
                narrow,
                auto_length: p.opt(&["lambda"]).is_none(),
                neff: p.get("neff", 1.),
                critical: p.get("ucrit", 1e4) * 100. * EPS_SI / cox,
                exponent: p.get("uexp", 0.),
                depth: p.get("xj", 0.),
                depletion,
                velocity: p.get("vmax", 0.),
            }
        } else {
            Self::SemiEmpirical {
                narrow: 2. * narrow,
                theta: p.get("theta", 0.),
                depth: p.get("xj", 0.),
                diffusion: p.get("ld", 0.),
                depletion,
                kappa: p.get("kappa", 0.2),
                eta: p.get("eta", 0.) * 8.15e-22 / cox,
                velocity: p.get("vmax", 0.),
            }
        }
    }
    pub(crate) fn valid(&self) -> bool {
        let valid = |x: f64| x.is_finite() && x >= 0.;
        match *self {
            Self::SquareLaw => true,
            Self::PowerLaw {
                kv,
                nv,
                nc,
                gamma1,
                sigma,
                lambda1,
            } => {
                kv.is_finite()
                    && kv > 0.
                    && valid(nv)
                    && nc.is_finite()
                    && nc > 0.
                    && [gamma1, sigma, lambda1].into_iter().all(f64::is_finite)
            }
            Self::ChargeSheet {
                critical,
                neff,
                auto_length: _,
                exponent,
                narrow,
                depth,
                depletion,
                velocity,
            } => {
                neff.is_finite()
                    && neff > 0.
                    && critical > 0.
                    && [critical, exponent, narrow, depth, depletion, velocity]
                        .into_iter()
                        .all(valid)
            }
            Self::SemiEmpirical {
                narrow,
                theta,
                eta,
                velocity,
                depth,
                diffusion,
                depletion,
                kappa,
            } => {
                eta.is_finite()
                    && [narrow, theta, velocity, depth, diffusion, depletion, kappa]
                        .into_iter()
                        .all(valid)
            }
        }
    }
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::SquareLaw => "mos1",
            Self::PowerLaw { .. } => "mos6",
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
        body: Dual<N>,
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
            Self::SquareLaw | Self::PowerLaw { .. } => (0., 0.),
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
            Self::SemiEmpirical {
                depth,
                diffusion,
                depletion,
                ..
            } if depth > 0. && depletion > 0. => {
                // Project the curved depletion boundary on the channel. The
                // MOS3 fit gives lateral depletion width in units of XJ.
                let wp = root * (depletion / depth);
                let wc = 0.0631353 + wp * (0.8013292 - 0.01110777 * wp);
                let sine = wp / (1. + wp);
                let lateral = (wc * depth + diffusion) * (1. - sine.sq()).sqrt() - diffusion;
                lateral / length
            }
            _ => Dual::cst(0.),
        };
        let mut u = overdrive + root * sharing * gamma - root.sq() * narrow + vd * dibl;
        if let Self::PowerLaw { gamma1, sigma, .. } = self {
            u = u + body * (*gamma1) + vd * (*sigma);
        }
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
            Self::PowerLaw {
                kv,
                nv,
                nc,
                lambda1,
                ..
            } => {
                let sat = u.powf(nv) * kv;
                let peak = u.powf(nc) * beta;
                let v = clip(sat) / sat;
                let id = peak * v * (2. - v) * (1. + vd * (lambda - body * lambda1));
                (id, u.v, sat.v)
            }
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
                auto_length,
                neff,
                depletion,
                ..
            } => {
                // Local inversion charge vanishes when u = V + gamma*(sqrt(P+V)-sqrt(P)).
                let gamma = (1. - sharing) * gamma;
                let eta = 1. + narrow;
                let a = root * (2. * eta) + gamma;
                let dr = u * 2. / ((a.sq() + u * (4. * eta)).sqrt() + a);
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
                    v * (u - v * (body + 0.5 * eta))
                };
                let field = u - swing;
                let mobility_factor = if exponent != 0. && field.v > critical {
                    (field / critical).powf(-exponent)
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
                        let charge = u - sat * eta - gamma * (end - root);
                        let residual = integral(sat) - vc * charge;
                        let slope = charge + vc * (eta + gamma / (end * 2.));
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
                let loss = if auto_length && depletion > 0. && vd.v > sat.v {
                    // The voltage in the pinched-off region is E_sat*dL
                    // + q*N_eff*dL²/(2*epsilon). Use its positive root.
                    let alpha = depletion.powi(2) / neff;
                    let field = velocity / (mobility * mobility_factor);
                    drain_extension(vd - sat, alpha, field) / length
                } else {
                    vd * lambda
                };
                let id = integral(clip(sat)) * beta * mobility_factor / remaining_length(loss);
                (id * weak, gate_overdrive, sat.v)
            }
            Self::SemiEmpirical {
                theta,
                velocity,
                depletion,
                kappa,
                ..
            } => {
                let slope = if root.v > 0. {
                    (1. - sharing) * gamma / (root * 4.)
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
                let loss = if kappa <= 0. || depletion <= 0. {
                    Dual::cst(0.)
                } else if velocity == 0. {
                    // With no velocity limit, the measured triode shortening
                    // is quartic. Tangency to sqrt(Vd - Vsat + offset) fixes
                    // the offset to Vsat/8 at the join.
                    let offset = sat / 8.;
                    let scale = depletion * kappa.sqrt() / length;
                    if vd.v < sat.v {
                        scale * offset.sqrt() * (vd / sat).sq().sq()
                    } else {
                        scale * (vd - sat + offset).sqrt()
                    }
                } else if vd.v > sat.v {
                    // End field from current continuity at saturation,
                    // E = kappa * I_sat / (L * dI/dV at the knee).
                    // Substituting the saturation condition avoids subtraction
                    // of nearly equal terms in the left-hand derivative.
                    let field = field_voltage * (1. + field_voltage / sat) * (kappa / length);
                    drain_extension((vd - sat) * kappa, depletion.powi(2), field) / length
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

/// Positive root of dV = E*dL + dL²/alpha, rationalized near the knee.
fn drain_extension<const N: usize>(voltage: Dual<N>, alpha: f64, field: Dual<N>) -> Dual<N> {
    let half = field * (alpha * 0.5);
    let potential = voltage * alpha;
    potential / ((half.sq() + potential).sqrt() + half)
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
                Dual::cst(-0.04),
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
    fn depletion_length_and_explicit_zero_lambda() {
        let voltage = Dual::<1>::var(0.5, 0);
        let field = 2e6;
        let alpha = 3e-14;
        let loss = drain_extension(voltage, alpha, Dual::cst(field));
        assert!((field * loss.v + loss.v.powi(2) / alpha - voltage.v).abs() < 1e-14);
        assert!((loss.d[0] * (field + 2. * loss.v / alpha) - 1.).abs() < 1e-14);
        let bind = |extra: &[(&str, f64)]| {
            let p = ModelParams::new(
                &extra
                    .iter()
                    .map(|&(k, v)| (k.to_string(), v))
                    .collect::<Vec<_>>(),
            );
            Channel::bind(2., &p, 1e-3, Some(1e17))
        };
        let off = evaluate(&bind(&[("lambda", 0.)]), 0.8, 2.);
        let default = evaluate(&bind(&[]), 0.8, 2.);
        let higher_charge = evaluate(&bind(&[("neff", 3.)]), 0.8, 2.);
        assert!(default.v > higher_charge.v && higher_charge.v > off.v);
        assert!(default.d[1] > 0. && off.d[1] == 0.);
    }

    #[test]
    fn incremental_conductances_follow_each_channel_current() {
        for m in [
            channel(2., &[("vmax", 8e4), ("ucrit", 2e3), ("uexp", 0.4)]),
            channel(3., &[("theta", 0.2), ("vmax", 8e4)]),
            channel(
                6.,
                &[("kv", 0.7), ("nv", 0.85), ("nc", 1.4), ("sigma", 0.03)],
            ),
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
