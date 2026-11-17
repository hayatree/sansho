//! Shared pn-junction physics: ideal-diode current, depletion charge, Newton limiting.
//!
//! Provenance: original implementation of textbook equations. The junction-voltage limiting
//! strategy and the critical voltage follow L. W. Nagel, *SPICE2* (UCB ERL M520, 1975); the
//! depletion charge with forward-bias linearization above `fc·vj` follows the standard
//! formulation in P. Antognetti & G. Massobrio, *Semiconductor Device Modeling with SPICE*
//! (2nd ed., 1993). See `docs/PROVENANCE.md`.

use crate::dual::Dual;

/// Critical voltage above which junction limiting is applied: the voltage at which the diode
/// curve has its minimum radius of curvature, `nVt·ln(nVt / (√2·Is))`.
pub fn vcrit(is: f64, nvt: f64) -> f64 {
    nvt * (nvt / (std::f64::consts::SQRT_2 * is)).ln()
}

/// Limit a new junction voltage `vnew` given the previous iterate `vold`.
///
/// Above `vcrit`, large forward steps are replaced by a logarithmic step so that the
/// exponential current cannot overflow and Newton does not overshoot. Returns the limited
/// voltage and whether limiting was applied.
pub fn limit_junction(vnew: f64, vold: f64, nvt: f64, vcrit: f64) -> (f64, bool) {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * nvt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / nvt;
            if arg > 0.0 {
                (vold + nvt * arg.ln(), true)
            } else {
                (vcrit, true)
            }
        } else {
            (nvt * (vnew / nvt).ln(), true)
        }
    } else {
        (vnew, false)
    }
}

/// Exponential with linear continuation above `x = 80` to avoid overflow during early Newton
/// iterations.
pub fn safe_exp<const N: usize>(x: Dual<N>) -> Dual<N> {
    const XMAX: f64 = 80.0;
    if x.v <= XMAX {
        x.exp()
    } else {
        let e = XMAX.exp();
        (x - XMAX) * e + e
    }
}

/// Ideal junction current `is·(exp(v/nvt) − 1)`.
pub fn ideal_current<const N: usize>(v: Dual<N>, is: f64, nvt: f64) -> Dual<N> {
    (safe_exp(v / nvt) - 1.0) * is
}

/// Depletion-layer charge of a junction with zero-bias capacitance `cj0`, built-in potential
/// `vj`, grading coefficient `m` and forward-bias coefficient `fc`.
///
/// For `v < fc·vj` the charge integrates `cj0 / (1 − v/vj)^m`; above it the capacitance is
/// continued linearly, which keeps the charge finite at and beyond `vj`.
pub fn depletion_charge<const N: usize>(v: Dual<N>, cj0: f64, vj: f64, m: f64, fc: f64) -> Dual<N> {
    if cj0 == 0.0 {
        return Dual::cst(0.0);
    }
    let vfc = fc * vj;
    if v.v < vfc {
        let arg = 1.0 - v / vj;
        if (m - 1.0).abs() < 1e-12 {
            -(arg.ln()) * (cj0 * vj)
        } else {
            (1.0 - arg.powf(1.0 - m)) * (cj0 * vj / (1.0 - m))
        }
    } else {
        // Charge at vfc plus the integral of the linearized capacitance
        // c(v) = cj0 / (1-fc)^(1+m) · (1 - fc(1+m) + m·v/vj).
        let q_fc = if (m - 1.0).abs() < 1e-12 {
            -(1.0 - fc).ln() * cj0 * vj
        } else {
            cj0 * vj / (1.0 - m) * (1.0 - (1.0 - fc).powf(1.0 - m))
        };
        let f2 = (1.0 - fc).powf(1.0 + m);
        let f3 = 1.0 - fc * (1.0 + m);
        let dv = v - vfc;
        let quad = (v * v - vfc * vfc) * (m / (2.0 * vj));
        (dv * f3 + quad) * (cj0 / f2) + q_fc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depletion_charge_is_continuous_with_matching_capacitance() {
        let (cj0, vj, m, fc) = (1e-12, 0.8, 0.4, 0.5);
        let vfc = fc * vj;
        let below = depletion_charge(Dual::<1>::var(vfc - 1e-9, 0), cj0, vj, m, fc);
        let above = depletion_charge(Dual::<1>::var(vfc + 1e-9, 0), cj0, vj, m, fc);
        assert!((below.v - above.v).abs() < 1e-20);
        assert!((below.d[0] - above.d[0]).abs() < 1e-20);
        // Capacitance at zero bias equals cj0.
        let z = depletion_charge(Dual::<1>::var(0.0, 0), cj0, vj, m, fc);
        assert!((z.d[0] - cj0).abs() < 1e-24);
    }

    #[test]
    fn limiting_reduces_large_forward_steps() {
        let nvt = 0.025852;
        let vc = vcrit(1e-14, nvt);
        let (v, lim) = limit_junction(5.0, 0.6, nvt, vc);
        assert!(lim && v < 0.8 && v > 0.6);
        let (v, lim) = limit_junction(0.3, 0.29, nvt, vc);
        assert!(!lim && v == 0.3);
    }
}
