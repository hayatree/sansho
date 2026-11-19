//! Temperature translation of nominal SPICE semiconductor model parameters.
//! Provenance: original implementation from Massobrio & Antognetti,
//! *Semiconductor Device Modeling with SPICE*, silicon junction and MOS models.

use crate::{REFTEMP, thermal_voltage};

/// Absolute operating and nominal temperatures (kelvin).
#[derive(Debug, Clone, Copy)]
pub struct Temperature {
    pub kelvin: f64,
    pub nominal: f64,
}

fn gap(t: f64) -> f64 {
    1.16 - 7.02e-4 * t * t / (t + 1108.)
}

impl Temperature {
    pub fn ratio(self) -> f64 {
        self.kelvin / self.nominal
    }
    /// Log of the saturation-current multiplier at ideality one.
    pub fn saturation(self, eg: f64, xti: f64) -> f64 {
        xti * self.ratio().ln() + eg * (self.ratio() - 1.) / thermal_voltage(self.kelvin)
    }
    pub fn potential(self, v: f64) -> f64 {
        self.ratio() * v + gap(self.kelvin)
            - self.ratio() * gap(self.nominal)
            - 3. * thermal_voltage(self.kelvin) * self.ratio().ln()
    }
    /// Translate the zero-bias depletion capacitance via its 300.15 K value.
    pub fn capacitance(self, potential: f64, grading: f64) -> f64 {
        let vref = Self {
            kelvin: REFTEMP,
            nominal: self.nominal,
        }
        .potential(potential);
        let factor = |t: f64, v: f64| 1. + grading * (4e-4 * (t - REFTEMP) - (v / vref - 1.));
        factor(self.kelvin, self.potential(potential)) / factor(self.nominal, potential)
    }
    pub fn gap_change(self) -> f64 {
        gap(self.kelvin) - gap(self.nominal)
    }
    pub fn mos_saturation(self) -> f64 {
        (gap(self.nominal) / thermal_voltage(self.nominal)
            - gap(self.kelvin) / thermal_voltage(self.kelvin))
        .exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_between_two_nonreference_temperatures_is_reversible() {
        let hot = Temperature {
            nominal: 285.,
            kelvin: 350.,
        };
        let cold = Temperature {
            nominal: hot.kelvin,
            kelvin: hot.nominal,
        };
        let vj = 0.78;
        let m = 0.38;
        assert!((cold.potential(hot.potential(vj)) - vj).abs() < 1e-14);
        assert!(
            (hot.capacitance(vj, m) * cold.capacitance(hot.potential(vj), m) - 1.).abs() < 1e-14
        );
        assert!((hot.saturation(1.11, 3.) + cold.saturation(1.11, 3.)).abs() < 1e-13);
    }
}
