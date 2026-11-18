//! Direct tunneling from gate to channel, diffusions and body.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, §§4.1–4.3 and Appendix A.7.

use super::{D, Size, channel::Bias, floor, params::Bin, softplus};
use crate::{ModelParams, junction::safe_exp};

#[derive(Debug)]
struct Barrier {
    scale: f64,
    exponent: f64,
    coeff: [f64; 3],
}
impl Barrier {
    fn current(&self, voltage: D, aux: D, oxide: D) -> D {
        let [a, b, c] = self.coeff;
        self.scale * voltage * aux * safe_exp(-self.exponent * (a - b * oxide) * (1. + c * oxide))
    }
}
#[derive(Debug)]
pub(super) struct Gate {
    channel_mode: u8,
    bulk_mode: bool,
    channel: Barrier,
    accumulation: Barrier,
    inversion: Barrier,
    edge: [Barrier; 2],
    slope: [f64; 3],
    eigbinv: f64,
    pigcd: Option<f64>,
    flat_sd: f64,
}
impl Gate {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        p: &ModelParams,
        bin: &Bin<'_>,
        sign: f64,
        tox: f64,
        l: f64,
        w: f64,
        vt: f64,
        flat_sd: f64,
    ) -> Result<Self, String> {
        let channel_mode = p.get("igcmod", 0.);
        let bulk_mode = p.get("igbmod", 0.);
        if ![0., 1., 2.].contains(&channel_mode) || ![0., 1.].contains(&bulk_mode) {
            return Err("unsupported gate tunneling selector".into());
        }
        let (a, b, default) = if sign > 0. {
            (4.97232e-7, 7.45669e11, [0.0136, 0.00171, 0.075])
        } else {
            (3.42537e-7, 1.16645e12, [0.0098, 0.000759, 0.03])
        };
        let ref_tox = p.get("toxref", 3e-9);
        let ntox = bin.get("ntox", 1.);
        let base = |suffix: &str, defaults: [f64; 3]| {
            std::array::from_fn(|k| {
                let prefix = ["aig", "big", "cig"][k];
                let def = if matches!(suffix, "s" | "d") {
                    bin.get(&format!("{prefix}sd"), defaults[k])
                } else {
                    defaults[k]
                };
                bin.get(&format!("{prefix}{suffix}"), def)
            })
        };
        let barrier = |a: f64, b: f64, length: f64, thick: f64, coeff: [f64; 3]| Barrier {
            scale: w * length * a * (ref_tox / thick).powf(ntox) / thick.powi(2),
            exponent: b * thick,
            coeff,
        };
        let edge_tox = tox * bin.get("poxedge", 1.);
        let dlcig = bin.get("dlcig", p.get("lint", 0.));
        let edge = [
            barrier(a, b, bin.get("dlcigd", dlcig), edge_tox, base("d", default)),
            barrier(a, b, dlcig, edge_tox, base("s", default)),
        ];
        let slope = [
            bin.get("nigc", 1.),
            bin.get("nigbacc", 1.),
            bin.get("nigbinv", 3.),
        ]
        .map(|n| n * vt);
        if (channel_mode != 0. && (slope[0] <= 0. || edge_tox <= 0.))
            || (bulk_mode != 0. && (slope[1] <= 0. || slope[2] <= 0.))
        {
            return Err("invalid tunneling slope or oxide thickness".into());
        }
        Ok(Self {
            channel_mode: channel_mode as u8,
            bulk_mode: bulk_mode != 0.,
            channel: barrier(a, b, l, tox, base("c", default)),
            accumulation: barrier(
                4.97232e-7,
                7.45669e11,
                l,
                tox,
                base("bacc", [9.49e-4, 0.00171, 0.075]),
            ),
            inversion: barrier(
                3.75956e-7,
                9.82222e11,
                l,
                tox,
                base("binv", [0.0111, 0.000949, 0.006]),
            ),
            edge,
            slope,
            eigbinv: bin.get("eigbinv", 1.1),
            pigcd: p.opt(&["pigcd"]).map(|v| bin.get("pigcd", v)),
            flat_sd,
        })
    }
}
impl Size {
    /// Currents leaving gate towards the forward-channel drain, source and body.
    pub(super) fn tunneling(&self, b: &Bias) -> [D; 3] {
        let g = &self.gate;
        let mut result = [D::cst(0.); 3];
        if g.channel_mode == 0 && !g.bulk_mode {
            return result;
        }
        let vgb = b.gate - b.bulk;
        let flat = self.cv.flat_zero;
        let delta = flat - vgb - 0.02;
        let acc = floor(
            0.5 * (delta + (delta.sq() + 0.08 * flat.abs().max(1e-30)).sqrt()),
            0.,
        );
        // Use the depletion solution of (7.62) to continue (4.2) into accumulation.
        let dep = vgb - flat + acc - b.over;
        let oxide = b.over + super::charges::depletion(dep, self.k1ox);
        let aux = |v: D, n: usize| g.slope[n] * softplus(v / g.slope[n]);
        if g.bulk_mode {
            result[2] = g.accumulation.current(vgb, aux(flat - vgb, 1), acc)
                + g.inversion.current(vgb, aux(oxide - g.eigbinv, 2), oxide);
        }
        if g.channel_mode != 0 {
            let vth = if g.channel_mode == 1 {
                D::cst(self.vth0)
            } else {
                b.threshold
            };
            let current = g.channel.current(b.gate, aux(b.gate - vth, 0), oxide);
            let drive = floor(b.over, 1e-12);
            let part = g.pigcd.map(D::cst).unwrap_or_else(|| {
                floor(
                    g.channel.exponent * (1. - 0.5 * b.vd / drive) / drive.sq(),
                    0.,
                )
            });
            let y = part * b.vd;
            let exp = safe_exp(-y);
            let den = y.sq() + 2e-4;
            result[0] = current * (1. - (1. + y) * exp + 1e-4) / den;
            result[1] = current * (y + exp - 1. + 1e-4) / den;
        }
        result
    }
    pub(super) fn edge_tunneling(&self, v: D, side: usize) -> D {
        let g = &self.gate;
        if g.channel_mode == 0 {
            return D::cst(0.);
        }
        let smooth = ((v - g.flat_sd).sq() + 1e-4).sqrt();
        g.edge[side].current(v, smooth, smooth)
    }
}
