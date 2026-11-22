//! Doping relations for the BSIM3v3.3 and BSIM4.8.2 body-effect defaults.
//! Provenance: UC Berkeley BSIM3v3.3 and BSIM4.8.2 manuals, threshold model
//! and parameter-dependence equations for GAMMA1, GAMMA2 and VBX.

// Doping in cm^-3, Cox in F/m^2 and XT in metres. The compact-model
// coefficients retain the numerical convention used in parameter extraction.
const GAMMA_FACTOR: f64 = 5.753e-12;

pub(super) fn gamma(doping: f64, cox: f64) -> f64 {
    GAMMA_FACTOR * doping.sqrt() / cox
}

pub(super) fn doping(gamma: f64, cox: f64) -> f64 {
    (gamma * cox / GAMMA_FACTOR).powi(2)
}

pub(super) fn vbx(phi: f64, doping: f64, xt: f64) -> f64 {
    phi - 7.7348e-4 * doping * xt.powi(2)
}
