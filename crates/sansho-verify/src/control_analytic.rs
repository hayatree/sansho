//! Independent closed-form references for the seven original R10 circuits.
//! Provenance: Ohm's law, first-order RC transfer, and the classical MOS1 triode equation.

use std::{collections::BTreeMap, fmt::Write};

/// In-memory comparison inputs. These never populate the ngspice cache.
#[derive(Default)]
pub(crate) struct Analytic {
    pub log: String,
    pub files: BTreeMap<String, Vec<u8>>,
}
impl Analytic {
    pub fn for_deck(text: &str) -> Option<Self> {
        let mut a = Self::default();
        match text {
            include_str!("../../../verify/decks/r10_alter_meas.cir") => {
                for r in [1., 2., 3.] {
                    writeln!(a.log, "endpoint = {}", 2. * r / (1. + r)).ok()?
                }
            }
            include_str!("../../../verify/decks/r10_foreach_temp.cir") => {
                for t in [0., 27., 80.] {
                    writeln!(a.log, "divider = {}", 1. / (2. + 0.004 * (t - 27.))).ok()?
                }
            }
            include_str!("../../../verify/decks/r10_alterparam.cir") => {
                for r in [1., 2., 4.] {
                    writeln!(a.log, "output = {}", 1. / (1. + r)).ok()?
                }
            }
            include_str!("../../../verify/decks/r10_mos_model.cir") => {
                // Vds = 1 V, W/L = 5. Both endpoints are in the triode region.
                for vt in [0.4, 0.8] {
                    writeln!(a.log, "supply = {}", -5e-4 * (2. - vt - 0.5)).ok()?
                }
            }
            include_str!("../../../verify/decks/r10_ac_vectors.cir") => {
                let mut table = String::new();
                for i in 0..=10 {
                    let f = 1000. + f64::from(i) * 0.1;
                    let x = std::f64::consts::TAU * 1e-5 * f;
                    let gain = -10. * (1. + x * x).log10();
                    let slope = -20. * x * x / (f * (1. + x * x) * std::f64::consts::LN_10);
                    writeln!(table, "{f} {gain} {} {slope}", -x.atan()).ok()?;
                }
                a.files.insert("response.dat".into(), table.into_bytes());
            }
            include_str!("../../../verify/decks/r10_linearize.cir") => {
                a.raw(
                    "uniform.raw",
                    "Transient Analysis",
                    "time time",
                    "v(out) voltage",
                    (0..41).map(|i| {
                        let t = f64::from(i) * 25e-9;
                        (t, t * 250000.)
                    }),
                )?;
            }
            include_str!("../../../verify/decks/r10_setplot.cir") => {
                a.raw(
                    "delta.raw",
                    "DC transfer characteristic",
                    "v-sweep voltage",
                    "delta notype",
                    (0..5).map(|i| {
                        let v = f64::from(i) * 0.25;
                        (v, v * (0.75 - 0.5))
                    }),
                )?;
            }
            _ => return None,
        }
        Some(a)
    }
    fn raw(
        &mut self,
        file: &str,
        kind: &str,
        axis: &str,
        signal: &str,
        points: impl ExactSizeIterator<Item = (f64, f64)>,
    ) -> Option<()> {
        let mut text = format!(
            "Title: closed-form circuit values\nPlotname: {kind}\nFlags: real\nNo. Variables: 2\nNo. Points: {}\nVariables:\n0 {axis}\n1 {signal}\nValues:\n",
            points.len()
        );
        for (i, (x, y)) in points.enumerate() {
            writeln!(text, "{i} {x} {y}").ok()?
        }
        self.files.insert(file.into(), text.into_bytes());
        Some(())
    }
}
