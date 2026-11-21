//! Quasi-static channel and charge for extracted BSIM1 and BSIM2 cards.
//! Provenance: Sheu et al., IEEE JSSC 22 (1987), 558–566; Jeng, ERL M90/90;
//! ngspice 44 manual §§7.6.3, 27.6.6–7. See docs/r14-models.md for approximations.

use crate::{ModelParams, dual::Dual};

/// A bias coefficient and its first-order body/drain sensitivities.
#[derive(Debug, Clone, PartialEq)]
struct BiasFit([f64; 3]);

impl BiasFit {
    fn at<const N: usize>(&self, b: Dual<N>, d: Dual<N>) -> Dual<N> {
        self.0[0] + self.0[1] * b + self.0[2] * d
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EarlyBsim {
    second: bool,
    drain_fit: [BiasFit; 3],
    transition: [f64; 2],
    k2: f64,
    eta: BiasFit,
    mobility: BiasFit,
    high_mobility: BiasFit,
    ua: BiasFit,
    ub: BiasFit,
    velocity: BiasFit,
    slope: BiasFit,
    offset: BiasFit,
    hot_a: BiasFit,
    hot_b: BiasFit,
    vdd: f64,
    vgg: f64,
    vbb: f64,
    extraction_vt: f64,
    zero_drain_charge: bool,
}

impl EarlyBsim {
    pub(crate) fn bind(p: &ModelParams, second: bool, l: f64, w: f64) -> Self {
        // R14 paired geometries resolve inverse dimensions in micrometres.
        let bin = |key: &str| binned(p, key, l, w, 0.);
        let fit = |base: &str, body: &str, drain: Option<&str>| {
            BiasFit([bin(base), bin(body), drain.map_or(0., bin)])
        };
        let mu = p.get(if second { "mu0" } else { "muz" }, 0.);
        let high = if second { "mus0" } else { "mus" };
        let ms = binned(p, high, l, w, if second { 500. } else { 0. });
        let zero = BiasFit([0.; 3]);
        Self {
            second,
            // Jeng, M90/90: coefficients of the tanh and field-polynomial
            // mobility terms have independent gate and body sensitivities.
            drain_fit: if second {
                [
                    BiasFit([binned(p, "mu20", l, w, 1.5), bin("mu2b"), bin("mu2g")]),
                    BiasFit([binned(p, "mu30", l, w, 10.), bin("mu3b"), bin("mu3g")]),
                    fit("mu40", "mu4b", Some("mu4g")),
                ]
            } else {
                std::array::from_fn(|_| zero.clone())
            },
            transition: if second {
                [
                    binned(p, "vglow", l, w, -0.15),
                    binned(p, "vghigh", l, w, 0.2),
                ]
            } else {
                [0.; 2]
            },
            k2: bin("k2"),
            eta: if second {
                fit("eta0", "etab", None)
            } else {
                fit("eta", "x2e", Some("x3e"))
            },
            mobility: BiasFit([mu, bin(if second { "mu0b" } else { "x2mz" }), 0.]),
            high_mobility: BiasFit([
                ms,
                bin(if second { "musb" } else { "x2ms" }),
                if second { 0. } else { bin("x3ms") },
            ]),
            ua: if second {
                BiasFit([binned(p, "ua0", l, w, 0.2), bin("uab"), 0.])
            } else {
                fit("u0", "x2u0", None)
            },
            ub: if second {
                fit("ub0", "ubb", None)
            } else {
                zero.clone()
            },
            velocity: if second {
                BiasFit([binned(p, "u10", l, w, 0.1), bin("u1b"), bin("u1d")])
            } else {
                fit("u1", "x2u1", Some("x3u1"))
            },
            slope: BiasFit([
                binned(p, "n0", l, w, if second { 1.4 } else { 0. }),
                binned(p, "nb", l, w, if second { 0.5 } else { 0. }),
                bin("nd"),
            ]),
            offset: if second {
                BiasFit([binned(p, "vof0", l, w, 1.8), bin("vofb"), bin("vofd")])
            } else {
                zero.clone()
            },
            hot_a: if second {
                fit("ai0", "aib", None)
            } else {
                zero.clone()
            },
            hot_b: if second {
                fit("bi0", "bib", None)
            } else {
                zero
            },
            vdd: p.get("vdd", if second { 5. } else { 0. }).abs(),
            vgg: if second { p.get("vgg", 5.) } else { 0. },
            vbb: if second { p.get("vbb", 5.) } else { 0. },
            extraction_vt: (p.get("temp", 27.) + 273.15) / 11600.,
            zero_drain_charge: {
                p.accept(&["xpart"]);
                !second
            },
        }
    }

    pub(crate) fn valid(&self) -> bool {
        (self.vdd > 0. || !self.second)
            && self.vdd.is_finite()
            && self.extraction_vt.is_finite()
            && self.extraction_vt > 0.
            && self.vbb.is_finite()
            && self.vgg.is_finite()
            && self.mobility.0[0] > 0.
            && self.k2.is_finite()
            && (!self.second
                || (self.transition.iter().all(|v| v.is_finite())
                    && self.transition[0] < self.transition[1]))
            && self
                .drain_fit
                .iter()
                .all(|fit| fit.0.iter().all(|v| v.is_finite()))
            && [
                &self.eta,
                &self.mobility,
                &self.high_mobility,
                &self.ua,
                &self.ub,
                &self.velocity,
                &self.slope,
                &self.offset,
                &self.hot_a,
                &self.hot_b,
            ]
            .iter()
            .all(|fit| fit.0.iter().all(|v| v.is_finite()))
    }

    /// Gate overdrive includes flat band, surface potential and K1 at the source.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bias<const N: usize>(
        &self,
        gate: Dual<N>,
        flatband: f64,
        ds: Dual<N>,
        bs: Dual<N>,
        phi: f64,
        k1: f64,
        nominal_beta: f64,
        length: f64,
        operating_vt: f64,
    ) -> Bias<N> {
        let limit = |v: Dual<N>, ceiling| if v.v > ceiling { Dual::cst(ceiling) } else { v };
        let (gs, bs, ds) = if self.second {
            (
                limit(gate, 2. * self.vgg),
                -limit(-bs, -2. * self.vbb),
                limit(ds, 2. * self.vdd),
            )
        } else {
            (gate, -positive(-bs), ds)
        };
        let (surface, root) = if bs.v <= 0. {
            let surface = phi - bs;
            (surface, surface.sqrt())
        } else {
            (phi / (1. + bs / phi), phi.sqrt() / (1. + bs / (2. * phi)))
        };
        let drain_offset = if self.second { ds } else { ds - self.vdd };
        let eta = self.eta.at(bs, drain_offset);
        let eta = if self.second { eta } else { positive(eta) };
        let u = gs - flatband - phi - k1 * root + self.k2 * surface + eta * ds;
        let bulk = 1. + (0.5 * k1) / root * (1. - 1. / (1.744 + 0.8364 * surface));
        let vt = if self.second {
            self.extraction_vt
        } else {
            operating_vt
        };
        let body_slope = if self.second { 1. / root } else { bs };
        let n = self.slope.at(body_slope, ds);
        let diffusion = -(-ds / vt).exp_m1();
        let mut drive = positive(u);
        let mut weak_current = None;
        if self.second && self.slope.0[0] > 0. && n.v != 0. {
            let [lo, hi] = self.transition;
            let slope = n * vt;
            let offset = self.offset.at(bs, ds);
            let weak =
                |gate: Dual<N>| crate::junction::safe_exp(0.5 * (offset + gate / slope)) * vt;
            if u.v <= lo {
                let amplitude = weak(u);
                weak_current = Some(amplitude.sq() * diffusion);
                drive = (2. * bulk).sqrt() * amplitude;
            } else if u.v < hi {
                let width = hi - lo;
                let t = (u - lo) / width;
                let low = if ds.v > 0. {
                    (2. * bulk * diffusion).sqrt() * weak(Dual::cst(lo))
                } else {
                    Dual::cst(0.)
                };
                drive = positive(
                    (1. - t).sq() * ((1. + 2. * t) * low + t * width * low / (2. * slope))
                        + t.sq() * ((3. - 2. * t) * hi - (1. - t) * width),
                );
            }
        }
        let ua = self.ua.at(bs, ds);
        let ub = self.ub.at(bs, ds);
        let speed = if self.second {
            self.velocity.at(bs, Dual::cst(0.))
        } else {
            self.velocity.at(bs, drain_offset) / (length * 1e6)
        };
        let fields = |drive: Dual<N>| {
            let minimum = if self.second { 0.2 } else { 1. };
            let vertical = 1. + ua * drive + ub * drive.sq();
            let vertical = if vertical.v < minimum {
                Dual::cst(minimum)
            } else {
                vertical
            };
            let field = if self.second {
                positive(speed) / vertical
            } else {
                positive(speed)
            };
            let long = drive / bulk;
            let sat = 2. * long / (1. + (1. + 2. * field * long).sqrt());
            (vertical, field, sat)
        };
        let (vertical, field, sat) = fields(drive);
        let v = if ds.v < sat.v { ds } else { sat };
        let low = self.mobility.at(bs, ds);
        let high = self.high_mobility.at(bs, Dual::cst(0.));
        let mobility = if self.second {
            let [shape, linear, quadratic] = self.drain_fit.each_ref().map(|f| f.at(bs, gs));
            let [mu0, mu_body, _] = self.mobility.0;
            let [mus, mut ms_body, _] = self.high_mobility.0;
            let mus = mus.max(1.01 * mu0);
            if self.vbb != 0. && mus + ms_body * self.vbb < mu0 + mu_body * self.vbb {
                ms_body = mu_body + (mu0 - mus) / self.vbb;
            }
            let high = mus + ms_body * bs;
            let polynomial = |d: Dual<N>| d * (linear - d * quadratic);
            let residual = high - low - polynomial(Dual::cst(self.vdd));
            let knee = if sat.v > 0. {
                tanh(shape * ds / sat)
            } else {
                Dual::cst(0.)
            };
            positive(low + residual * knee + polynomial(ds))
        } else {
            // BSIM1: quadratic Hermite interpolation through the two extracted
            // mobilities and specified endpoint slope X3MS. Continue the tangent.
            let slope = self.high_mobility.0[2].max(0.);
            if self.vdd > 0. && ds.v <= self.vdd {
                let fraction = ds / self.vdd;
                positive(
                    low + fraction
                        * ((2. - fraction) * (high - low) + (fraction - 1.) * (slope * self.vdd)),
                )
            } else {
                positive(high + slope * (ds - self.vdd))
            }
        };
        let denominator = if self.second && ds.v < sat.v {
            let reduction = 1. - self.velocity.0[2] * (1. - ds / sat).sq();
            let combined = vertical + ds * speed * reduction;
            if combined.v >= 0.5 {
                combined
            } else {
                Dual::cst(0.5)
            }
        } else {
            vertical * (1. + field * v)
        };
        let mut id = nominal_beta * mobility / self.mobility.0[0] * v * (drive - 0.5 * bulk * v)
            / denominator;
        if let Some(diffusion) = weak_current {
            id = nominal_beta * mobility / self.mobility.0[0] * diffusion;
        } else if !self.second {
            let slope = if n.v < 0.5 { Dual::cst(0.5) } else { n };
            let exponent = 1.8 + u / (slope * vt);
            // The harmonic limit avoids overflow at large positive gate bias.
            let sub = if exponent.v <= 0. {
                let e = diffusion * exponent.exp();
                e / (4.5 + e)
            } else if ds.v > 0. {
                diffusion / (diffusion + 4.5 * (-exponent).exp())
            } else {
                diffusion * crate::junction::safe_exp(exponent) / 4.5
            };
            id += 4.5 * nominal_beta * vt * vt * sub;
        }
        // Jeng: hot-carrier enhancement beyond pinch-off, with fitted AI/BI.
        let extra = ds - sat;
        if self.second && extra.v > 0. && self.hot_a.0 != [0.; 3] {
            id = id * (1. + self.hot_a.at(bs, ds) * (-self.hot_b.at(bs, ds) / extra).exp());
        }
        // The DC transition interpolates current. Mobile sheet charge starts
        // at electrostatic inversion; a diffusion current can flow below it.
        let (charge_drive, partition, bulk_charge, depletion_share) = if self.second {
            let charge_drive = positive(u);
            let flat_gate = flatband + phi - surface;
            let threshold = gs - u;
            let span = positive(threshold - flat_gate);
            let swing = u + span;
            // Integrate Cgb/Cox = (1 - swing/span)^2 between flat band
            // and threshold. Its value and slope join both outer regions.
            let depleted = if swing.v <= 0. {
                swing
            } else if u.v < 0. {
                let x = swing / span;
                swing * (1. - x + x.sq() / 3.)
            } else {
                span / 3.
            };
            // Use the strong-inversion saturation field for charge even
            // when the DC spline is active.
            let (_, _, sat) = fields(charge_drive);
            (
                charge_drive,
                charge_fraction(ds, sat),
                depleted,
                Dual::cst(0.),
            )
        } else {
            // ERL M85/85, section 4.2. Pin the surface potential at
            // inversion; transport barrier lowering does not move this join.
            let onset = flatband + phi + k1 * root;
            let drive = positive(gs - onset);
            let swing = gs - bs - flatband;
            let dep = if swing.v <= 0. {
                swing
            } else if gs.v >= onset.v {
                k1 * root
            } else if k1 != 0. {
                2. * k1 * swing / ((k1 * k1 + 4. * swing).sqrt() + k1)
            } else {
                Dual::cst(0.)
            };
            (
                drive,
                charge_fraction(ds, drive / bulk),
                dep,
                1. - 1. / bulk,
            )
        };
        Bias {
            id,
            overdrive: u.v,
            sat: sat.v,
            drive: charge_drive,
            partition,
            bulk_charge,
            zero_drain_charge: self.zero_drain_charge,
            depletion_share,
        }
    }
}

pub(crate) fn binned(p: &ModelParams, key: &str, l: f64, w: f64, default: f64) -> f64 {
    p.get(key, default)
        + p.get(&format!("l{key}"), 0.) / (l * 1e6)
        + p.get(&format!("w{key}"), 0.) / (w * 1e6)
}

fn tanh<const N: usize>(x: Dual<N>) -> Dual<N> {
    let value = x.v.tanh();
    Dual {
        v: value,
        d: x.d.map(|g| g * (1. - value * value)),
    }
}

fn positive<const N: usize>(v: Dual<N>) -> Dual<N> {
    if v.v > 0. { v } else { Dual::cst(0.) }
}

fn charge_fraction<const N: usize>(drain: Dual<N>, pinch: Dual<N>) -> Dual<N> {
    if drain.v < pinch.v {
        drain / pinch
    } else {
        Dual::cst(1.)
    }
}

pub(crate) struct Bias<const N: usize> {
    pub id: Dual<N>,
    pub overdrive: f64,
    pub sat: f64,
    depletion_share: Dual<N>,
    drive: Dual<N>,
    bulk_charge: Dual<N>,
    partition: Dual<N>,
    zero_drain_charge: bool,
}

impl<const N: usize> Bias<N> {
    /// The bulk shares the channel potential drop in the BSIM1 sheet integral.
    pub fn depletion(&self, inversion: Dual<N>, cox: f64) -> Dual<N> {
        self.bulk_charge * cox + self.depletion_share * (cox * self.drive - inversion)
    }

    /// Magnitudes of channel charge assigned to source and drain.
    pub fn charges(&self, cox: f64) -> (Dual<N>, Dual<N>) {
        if self.drive.v <= f64::MIN_POSITIVE {
            return (Dual::cst(0.), Dual::cst(0.));
        }
        // Sheu et al.: integrate the local inversion sheet and its first moment.
        // With r = A Vd/U, dx/d(V/Vd) = (1-r V/Vd)/(1-r/2).
        let r = self.partition;
        let den = 1. - 0.5 * r;
        let scale = cox * self.drive;
        let total = scale * (1. - r + r.sq() / 3.) / den;
        let drain = if self.zero_drain_charge {
            // Eq. 4.41, factored to give zero slope at pinch-off.
            0.5 * scale * (1. - r).sq() / den
        } else {
            total * (0.5 - r * (5. / 6. - r * (0.5 - 0.1 * r)))
                / ((1. - 0.5 * r) * (1. - r + r.sq() / 3.))
        };
        (total - drain, drain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sheet_charge_moments_and_saturation_partition() {
        for r in [0., 0.35, 1.] {
            let b = Bias {
                id: Dual::<1>::cst(0.),
                depletion_share: Dual::cst(0.),
                overdrive: 1.,
                sat: 1.,
                drive: Dual::var(1., 0),
                bulk_charge: Dual::cst(0.),
                partition: Dual::cst(r),
                zero_drain_charge: false,
            };
            let (s, d) = b.charges(1.);
            // Independently integrate charge versus normalized channel distance.
            let mut total = 0.;
            let mut moment = 0.;
            let n = 32768;
            for i in 0..n {
                let x = (f64::from(i) + 0.5) / f64::from(n);
                let sheet = (1. - x * r * (2. - r)).sqrt();
                total += sheet / f64::from(n);
                moment += x * sheet / f64::from(n);
            }
            assert!((s.v + d.v - total).abs() < 2e-8 && (d.v - moment).abs() < 2e-8);
            if r == 1. {
                assert!((d.v / (s.v + d.v) - 0.4).abs() < 1e-14);
                assert_eq!(
                    Bias {
                        zero_drain_charge: true,
                        ..b
                    }
                    .charges(1.)
                    .1
                    .v,
                    0.
                );
            }
        }
    }
}
