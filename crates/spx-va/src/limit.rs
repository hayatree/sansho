//! Original junction and field-effect voltage step limiters.
//! Provenance: logarithmic junction limiting idea in Nagel, SPICE2 (1975); original step clamp.
pub(crate) fn pnjlim(new: f64, old: f64, vt: f64, vcrit: f64) -> f64 {
    if vt <= 0. {
        return new;
    }
    if new > vcrit && (new - old).abs() > 2. * vt {
        if old > 0. {
            let t = 1. + (new - old) / vt;
            if t > 0. { old + vt * t.ln() } else { vcrit }
        } else if new > 0. {
            vt * (new / vt).ln()
        } else {
            new
        }
    } else {
        new
    }
}
pub(crate) fn fetlim(new: f64, old: f64, threshold: f64) -> f64 {
    if !new.is_finite() || !old.is_finite() || !threshold.is_finite() {
        return new;
    }
    let step = 0.5 + 0.5 * (old - threshold).abs();
    new.clamp(old - step, old + step)
}
