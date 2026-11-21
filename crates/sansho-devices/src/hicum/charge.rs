//! Junction storage and generalized integral charge control.
//! Provenance: Schröter and Chakravorty, Compact Hierarchical Bipolar
//! Transistor Modeling with HICUM (2010), and the graded-junction law.
//! R16b terminal measurements identify the field rounding and storage knee.

use super::{D, Model, floor};

// Integral of C0*(1-v/Vd)^(-z), with tangent extensions for a finite
// forward capacitance and fully depleted collector. Return charge and C/C0.
pub(super) fn depletion(v: D, potential: D, z: f64, ceiling: f64, punch: f64, vt: D) -> (D, D) {
    if z <= 0. {
        return (v, D::cst(1.));
    }
    let log_knee = -ceiling.max(1.).ln() / z;
    let upper = -potential * log_knee.exp_m1();
    let width = 2. * std::f64::consts::LN_2 * vt;
    // A graded primitive J and its slope, both continued at the forward knee.
    let limited = |bias: D| {
        let distance = upper - bias;
        let blend = positive_part(distance, width);
        let slope = blend / (distance.sq() + width.sq()).sqrt();
        // Retain shallow-grading precision near the forward knee.
        let log_arg = (log_knee.exp() + blend / potential).ln();
        (log_arg, bias - upper + blend, slope)
    };
    let primitive = |grading: f64, (log_arg, extension, slope): (D, D, D)| {
        let integral = if (grading - 1.).abs() < 1e-10 {
            -potential * log_arg
        } else {
            -potential * ((1. - grading) * log_arg).exp_m1() / (1. - grading)
        };
        let maximum = ceiling.max(1.).powf(grading / z);
        (
            integral + maximum * extension,
            maximum + ((-grading * log_arg).exp() - maximum) * slope,
        )
    };
    let at_v = limited(v);
    // The nominal 100 V no-reach-through card uses the ordinary graded
    // junction, also resolved by the forward BC capacitance sweep.
    if punch <= 0. || punch >= 100. {
        return primitive(z, at_v);
    }

    // Split the primitive at the smoothly limited reach-through voltage.
    // The second power law has grading z/4 and the same C at the join.
    let knee = potential - punch;
    let delta = v - knee;
    let span = 4. * vt - 0.1 * knee;
    let (split, weight) = if span.v <= 0. {
        if delta.v > 0. {
            (v, D::cst(1.))
        } else {
            (knee, D::cst(0.))
        }
    } else if delta.v > 0. {
        let tail = (-delta / span).exp();
        (v + span * tail.ln_1p(), 1. / (1. + tail))
    } else {
        let tail = (delta / span).exp();
        (knee + span * tail.ln_1p(), tail / (1. + tail))
    };
    let scale = (potential / punch).powf(0.75 * z);
    let at_split = limited(split);
    let (qn, cn) = primitive(z, at_split);
    let (qp, cp) = primitive(z / 4., at_split);
    let (qv, cv) = primitive(z / 4., at_v);
    let offset = if knee.v > 0. {
        let at_knee = limited(knee);
        scale * primitive(z / 4., at_knee).0 - primitive(z, at_knee).0
    } else {
        D::cst(0.)
    };
    (
        offset + qn + scale * (qv - qp),
        weight * (cn - scale * cp) + scale * cv,
    )
}

/// Algebraic positive part, with the conjugate form on the negative side.
pub(super) fn positive_part(x: D, width: D) -> D {
    if width.v == 0. {
        return floor(x, 0.);
    }
    let hyp = (x.sq() + width.sq()).sqrt();
    if x.v >= 0. {
        (hyp + x) * 0.5
    } else {
        0.5 * width.sq() / (hyp - x)
    }
}

pub(super) fn base_charge(depletion: D, equilibrium: D) -> D {
    let minimum = 0.05 * equilibrium;
    minimum + positive_part(depletion - minimum, 2. * std::f64::consts::LN_2 * minimum)
}

pub(super) struct Storage {
    pub forward: D,
    pub reverse: D,
    pub qf: D,
    pub qr: D,
    pub excess: D,
    pub transit_time: D,
}

// GICCR: j_f,j_r are transport numerators divided by QP0. The common
// denominator is total weighted hole charge in units of QP0. For a linear
// transit law the positive quadratic root is exact. At high injection we
// solve the monotone charge balance in log(width); implicit differentiation
// gives the AC and Newton derivatives independently of iteration count.
pub(super) fn transport(
    m: &Model,
    base: D,
    jf: D,
    jr: D,
    tau: D,
    critical: D,
    temperature_ratio: D,
) -> Storage {
    let charge = |current: D, critical: D| {
        let over = current / critical;
        let emitter_time = if m.tef0 > 0. {
            m.tef0 * temperature_ratio * floor(over, 1e-30).powf(m.gtfe)
        } else {
            D::cst(0.)
        };
        let emitter = current * emitter_time / (1. + m.gtfe);
        // Base push-out and collector storage grow beyond the Kirk knee.
        // The squared fractional overdrive tends to unity at high injection.
        let (high, high_time) = if m.thcs > 0. && over.v > 0. {
            let z = over - 1.;
            let hyp = (z.sq() + m.ahc * over.sq()).sqrt();
            let norm = 1. + (1. + m.ahc).sqrt();
            let fraction = if z.v < 0. {
                m.ahc * over / ((hyp - z) * norm)
            } else {
                (hyp + z) / (over * norm)
            };
            let linear = 0.5 * (m.latb + m.latl) * m.fthc;
            let quadratic = m.latb * m.latl * m.fthc / 3.;
            let spread = 1. + fraction * (linear + quadratic * fraction);
            let storage_time = m.thcs * fraction.sq() * spread;
            let derivative = if hyp.v > 0. {
                storage_time
                    + m.thcs * fraction.sq() / hyp
                        * (2. + fraction * (3. * linear + 4. * quadratic * fraction))
            } else {
                storage_time
            };
            (current * storage_time, derivative)
        } else {
            (D::cst(0.), D::cst(0.))
        };
        (emitter, high, tau + emitter_time + high_time)
    };
    let mobile = (m.hf0 * tau * jf + m.tr * jr) / m.qp0;
    let width = (base + (base.sq() + 4. * mobile).sqrt()) * 0.5;
    let width = if jf.v > 0. && (m.tef0 > 0. || m.thcs > 0.) {
        let weighted = |i, k| {
            let (e, h, _) = charge(i, k);
            (m.hfe * e + ((1. - m.fthc) * m.hf0 + m.fthc * m.hfc) * h) / m.qp0
        };
        let mut logw = width.v.ln();
        for _ in 0..48 {
            let w = logw.exp();
            let i = D::var(jf.v / w, 0);
            let extra = weighted(i, D::cst(critical.v));
            let rhs = base.v + mobile.v / w + extra.v;
            let residual = logw - rhs.ln();
            if residual.abs() < 2e-14 {
                break;
            }
            logw -= residual / (1. + (mobile.v / w + i.v * extra.d[0]) / rhs);
        }
        let w = logw.exp();
        let i = jf / w;
        let residual = base + mobile / w + weighted(i, critical) - w;
        let slope =
            1. + (mobile.v / w + i.v * weighted(D::var(i.v, 0), D::cst(critical.v)).d[0]) / w;
        D {
            v: w,
            d: residual.d.map(|x| x / slope),
        }
    } else {
        width
    };
    let forward = jf / width;
    let reverse = jr / width;
    let (emitter, high, transit_time) = charge(forward, critical);
    Storage {
        forward,
        reverse,
        qf: tau * forward + emitter + high,
        qr: m.tr * reverse,
        excess: high,
        transit_time,
    }
}
