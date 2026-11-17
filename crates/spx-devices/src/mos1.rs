//! MOSFET level 1 (Shichman–Hodges) with Meyer gate capacitances.
//!
//! Terminals: drain, gate, source, bulk. Constant series resistances `RD`, `RS` add internal
//! nodes. Equations are written for an n-channel device; p-channel devices flip all voltages
//! and currents. When `Vds < 0` the roles of drain and source are exchanged.
//!
//! - threshold `Vth = VTO + GAMMA·(√(PHI − Vbs) − √PHI)` (with the usual linear continuation for
//!   `Vbs > 0`)
//! - saturation (`Vds ≥ Vgs − Vth`): `Id = β/2·(Vgs − Vth)²·(1 + LAMBDA·Vds)`
//! - linear: `Id = β·Vds·(Vgs − Vth − Vds/2)·(1 + LAMBDA·Vds)`, `β = KP·W/(L − 2·LD)`
//! - bulk–drain / bulk–source junction diodes with depletion charge (`CBD`/`CBS` or
//!   `CJ·AD + CJSW·PD`)
//! - Meyer intrinsic gate capacitances plus overlap capacitances, integrated incrementally:
//!   `q = q_prev + ½(C(v) + C(v_prev))·(v − v_prev)` relative to the last accepted solution.
//!
//! Newton limiting of `Vgs` and `Vds` uses an original step-clamping rule (see
//! [`limit_step`]); junction voltages use [`crate::junction::limit_junction`].
//!
//! Provenance: original implementation of H. Shichman & D. A. Hodges, "Modeling and simulation
//! of insulated-gate field-effect transistor switching circuits", IEEE JSSC 3 (1968), and
//! J. E. Meyer, "MOS models and circuit simulation", RCA Review 32 (1971), in the
//! parameterization of Antognetti & Massobrio (1993), ch. 4. See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::model::ModelParams;
use crate::{
    DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    thermal_voltage, volt,
};

/// Permittivity of SiO2 (F/m).
const EPS_OX: f64 = 3.9 * 8.854_187_812_8e-12;

/// Level-1 model parameters.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct Mos1Model {
    /// +1 for NMOS, −1 for PMOS.
    pub polarity: f64,
    pub vto: f64,
    pub kp: f64,
    pub gamma: f64,
    pub phi: f64,
    pub lambda: f64,
    pub rd: f64,
    pub rs: f64,
    pub cbd: Option<f64>,
    pub cbs: Option<f64>,
    pub is: f64,
    pub js: f64,
    pub pb: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    pub cj: f64,
    pub mj: f64,
    pub cjsw: f64,
    pub mjsw: f64,
    pub fc: f64,
    pub ld: f64,
    /// Gate oxide capacitance per area (F/m²); 0 when `TOX` is not given.
    pub cox: f64,
}

impl Mos1Model {
    /// Build from `.model` parameters. Returns the model and the names of ignored parameters.
    pub fn from_params(p: &ModelParams, polarity: f64) -> (Self, Vec<String>) {
        let cox = p.get("tox", 0.0);
        let cox = if cox > 0.0 { EPS_OX / cox } else { 0.0 };
        let u0 = p.opt(&["u0", "uo"]).unwrap_or(600.0);
        let kp = p
            .opt(&["kp"])
            .unwrap_or(if cox > 0.0 { u0 * 1e-4 * cox } else { 2e-5 });
        let m = Self {
            polarity,
            vto: p.opt(&["vto", "vt0"]).unwrap_or(0.0),
            kp,
            gamma: p.get("gamma", 0.0),
            phi: p.get("phi", 0.6),
            lambda: p.get("lambda", 0.0),
            rd: p.get("rd", 0.0),
            rs: p.get("rs", 0.0),
            cbd: p.opt(&["cbd"]),
            cbs: p.opt(&["cbs"]),
            is: p.get("is", 1e-14),
            js: p.get("js", 0.0),
            pb: p.get("pb", 0.8),
            cgso: p.get("cgso", 0.0),
            cgdo: p.get("cgdo", 0.0),
            cgbo: p.get("cgbo", 0.0),
            cj: p.get("cj", 0.0),
            mj: p.get("mj", 0.5),
            cjsw: p.get("cjsw", 0.0),
            mjsw: p.get("mjsw", 0.5),
            fc: p.get("fc", 0.5),
            ld: p.get("ld", 0.0),
            cox,
        };
        p.accept(&["level", "kf", "af", "tnom"]);
        (m, p.unused())
    }
}

/// Instance geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mos1Geometry {
    /// Drawn channel length (m).
    pub l: f64,
    /// Channel width (m).
    pub w: f64,
    /// Drain area (m²).
    pub ad: f64,
    /// Source area (m²).
    pub as_: f64,
    /// Drain perimeter (m).
    pub pd: f64,
    /// Source perimeter (m).
    pub ps: f64,
}

impl Default for Mos1Geometry {
    fn default() -> Self {
        Self {
            l: 100e-6,
            w: 100e-6,
            ad: 0.0,
            as_: 0.0,
            pd: 0.0,
            ps: 0.0,
        }
    }
}

/// Per-instance derived constants.
#[derive(Debug, Clone)]
struct Derived {
    beta: f64,
    cox_total: f64,
    cgs_ov: f64,
    cgd_ov: f64,
    cgb_ov: f64,
    isd: f64,
    iss: f64,
    cbd: f64,
    cbs: f64,
    cbd_sw: f64,
    cbs_sw: f64,
}

/// All level-1 MOSFET instances.
#[derive(Debug, Default)]
pub struct Mos1s {
    names: Vec<String>,
    /// External d, g, s, b.
    ext: Vec<[Node; 4]>,
    /// Internal d', g, s', b.
    int: Vec<[Node; 4]>,
    off: Vec<bool>,
    model: Vec<Mos1Model>,
    derived: Vec<Derived>,
    at: Vec<SlotOffsets>,
}

/// Number of state values per instance: limited `vgs`, `vds`, `vbs`, then Meyer history
/// (`vgs`, `vgd`, `vgb`, `qgs`, `qgd`, `qgb`, `cgs`, `cgd`, `cgb`).
const NSTATE: usize = 12;

impl Mos1s {
    /// Add an instance with terminals `[d, g, s, b]`.
    pub fn push(
        &mut self,
        name: &str,
        nodes: [Node; 4],
        model: Mos1Model,
        geom: Mos1Geometry,
        off: bool,
    ) {
        let leff = geom.l - 2.0 * model.ld;
        let d = Derived {
            beta: model.kp * geom.w / leff,
            cox_total: model.cox * geom.w * leff,
            cgs_ov: model.cgso * geom.w,
            cgd_ov: model.cgdo * geom.w,
            cgb_ov: model.cgbo * leff,
            isd: if model.js > 0.0 && geom.ad > 0.0 {
                model.js * geom.ad
            } else {
                model.is
            },
            iss: if model.js > 0.0 && geom.as_ > 0.0 {
                model.js * geom.as_
            } else {
                model.is
            },
            cbd: model.cbd.unwrap_or(model.cj * geom.ad),
            cbs: model.cbs.unwrap_or(model.cj * geom.as_),
            cbd_sw: model.cjsw * geom.pd,
            cbs_sw: model.cjsw * geom.ps,
        };
        self.names.push(name.to_string());
        self.ext.push(nodes);
        self.int.push(nodes);
        self.off.push(off);
        self.model.push(model);
        self.derived.push(d);
    }

    /// Validate geometry; returns an error message for non-physical values.
    pub fn check_geometry(model: &Mos1Model, geom: &Mos1Geometry) -> Option<String> {
        if geom.w <= 0.0 {
            Some(format!("W must be positive (got {})", geom.w))
        } else if geom.l - 2.0 * model.ld <= 0.0 {
            Some(format!(
                "effective channel length L - 2*LD must be positive (L = {}, LD = {})",
                geom.l, model.ld
            ))
        } else {
            None
        }
    }
}

/// Clamp a Newton step of a FET control voltage. The allowed step grows with the distance
/// from the threshold region so that strongly-on devices are not slowed down.
fn limit_step(vnew: f64, vold: f64, scale: f64) -> (f64, bool) {
    let dmax = 0.5 + 0.5 * scale.abs();
    let d = vnew - vold;
    if d.abs() > dmax {
        (vold + dmax.copysign(d), true)
    } else {
        (vnew, false)
    }
}

/// Meyer intrinsic capacitances `(cgs, cgd, cgb)` for an n-normalized device in normal mode.
fn meyer(vgst: f64, vds: f64, phi: f64, cox: f64) -> (f64, f64, f64) {
    let two_thirds = 2.0 / 3.0 * cox;
    if vgst <= -phi {
        (0.0, 0.0, cox)
    } else if vgst <= -phi / 2.0 {
        (0.0, 0.0, -vgst / phi * cox)
    } else if vgst <= 0.0 {
        (
            two_thirds * (1.0 + 2.0 * vgst / phi),
            0.0,
            -vgst / phi * cox,
        )
    } else if vgst <= vds {
        (two_thirds, 0.0, 0.0)
    } else {
        let den = 2.0 * vgst - vds;
        (
            two_thirds * (1.0 - ((vgst - vds) / den).powi(2)),
            two_thirds * (1.0 - (vgst / den).powi(2)),
            0.0,
        )
    }
}

impl DeviceGroup for Mos1s {
    fn type_name(&self) -> &'static str {
        "mos1"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        NSTATE * self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for i in 0..self.len() {
            let m = &self.model[i];
            if m.rd > 0.0 {
                self.int[i][0] = Some(u.alloc_internal_node(&self.names[i], "d"));
            }
            if m.rs > 0.0 {
                self.int[i][2] = Some(u.alloc_internal_node(&self.names[i], "s"));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.at.clear();
        for i in 0..self.len() {
            self.at.push(b.counts());
            let m = &self.model[i];
            if m.rd > 0.0 {
                b.resistor(self.ext[i][0], self.int[i][0]);
            }
            if m.rs > 0.0 {
                b.resistor(self.ext[i][2], self.int[i][2]);
            }
            b.core(&self.int[i]);
        }
        Ok(())
    }
    #[allow(clippy::many_single_char_names)]
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let vt = thermal_voltage(ctx.temp);
        for i in 0..self.len() {
            let m = &self.model[i];
            let dv = &self.derived[i];
            let mut at = self.at[i];
            if m.rd > 0.0 {
                out.resistor(
                    &mut at,
                    1.0 / m.rd,
                    volt(x, self.ext[i][0]),
                    volt(x, self.int[i][0]),
                );
            }
            if m.rs > 0.0 {
                out.resistor(
                    &mut at,
                    1.0 / m.rs,
                    volt(x, self.ext[i][2]),
                    volt(x, self.int[i][2]),
                );
            }
            let act = self.int[i].map(|n| volt(x, n));
            let [vd, vg, vs, vb] = act;
            let sgn = m.polarity;
            let vto = sgn * m.vto;
            let s = &mut st.current[NSTATE * i..NSTATE * (i + 1)];
            let acc = &st.accepted[NSTATE * i..NSTATE * (i + 1)];

            // Limited control voltages (n-normalized).
            let (vgs_a, vds_a, vbs_a) = (sgn * (vg - vs), sgn * (vd - vs), sgn * (vb - vs));
            let (vgs, vds, vbs) = if ctx.init_junctions {
                if self.off[i] {
                    (0.0, 0.0, 0.0)
                } else {
                    (vto + 0.1, 0.1, -1.0)
                }
            } else if ctx.limit {
                let (vgs, l1) = limit_step(vgs_a, s[0], s[0] - vto);
                let (vds, l2) = limit_step(vds_a, s[1], s[1]);
                let vc = vcrit(dv.iss.max(dv.isd), vt);
                let (vbs, l3) = limit_junction(vbs_a, s[2], vt, vc);
                // Also keep the bulk-drain junction from jumping into strong forward bias.
                let (vbd, l4) = limit_junction(vbs - vds, s[2] - s[1], vt, vc);
                let vds = if l4 { vbs - vbd } else { vds };
                if l1 || l2 || l3 || l4 {
                    out.noncon = true;
                }
                (vgs, vds, vbs)
            } else {
                (vgs_a, vds_a, vbs_a)
            };
            s[0] = vgs;
            s[1] = vds;
            s[2] = vbs;
            let lin = [vs + sgn * vds, vs + sgn * vgs, vs, vs + sgn * vbs];
            let [d, g, so, b] = [0, 1, 2, 3].map(|k| Dual::<4>::var(lin[k], k));
            let vgs = (g - so) * sgn;
            let vds = (d - so) * sgn;
            let vbs = (b - so) * sgn;
            let vbd = vbs - vds;

            // Channel current (normal mode uses s as source, reverse mode uses d).
            let reverse = vds.v < 0.0;
            let (vgs_e, vds_e, vbs_e) = if reverse {
                (vgs - vds, -vds, vbd)
            } else {
                (vgs, vds, vbs)
            };
            let sqrt_phi = m.phi.sqrt();
            let sarg = if vbs_e.v <= 0.0 {
                (-vbs_e + m.phi).sqrt()
            } else {
                let lin_cont = -(vbs_e / (2.0 * sqrt_phi)) + sqrt_phi;
                if lin_cont.v > 0.0 {
                    lin_cont
                } else {
                    Dual::cst(0.0)
                }
            };
            let vth = (sarg - sqrt_phi) * m.gamma + vto;
            let vgst = vgs_e - vth;
            let ids = if vgst.v <= 0.0 {
                Dual::cst(0.0)
            } else {
                let clm = vds_e * m.lambda + 1.0;
                if vgst.v <= vds_e.v {
                    vgst.sq() * (0.5 * dv.beta) * clm
                } else {
                    vds_e * (vgst - vds_e * 0.5) * dv.beta * clm
                }
            };
            // Current flowing from drain to source (n-normalized).
            let ids = if reverse { -ids } else { ids };

            // Bulk junctions (current flowing from bulk into drain / source).
            let ibd = ideal_current(vbd, dv.isd, vt) + vbd * ctx.gmin;
            let ibs = ideal_current(vbs, dv.iss, vt) + vbs * ctx.gmin;
            let qbd = depletion_charge(vbd, dv.cbd, m.pb, m.mj, m.fc)
                + depletion_charge(vbd, dv.cbd_sw, m.pb, m.mjsw, m.fc);
            let qbs = depletion_charge(vbs, dv.cbs, m.pb, m.mj, m.fc)
                + depletion_charge(vbs, dv.cbs_sw, m.pb, m.mjsw, m.fc);

            // Meyer gate capacitances, incrementally integrated from the accepted state.
            let (mut cgs, mut cgd, cgb) = meyer(vgst.v, vds_e.v, m.phi, dv.cox_total);
            if reverse {
                std::mem::swap(&mut cgs, &mut cgd);
            }
            let caps = [cgs + dv.cgs_ov, cgd + dv.cgd_ov, cgb + dv.cgb_ov];
            let vgd = vgs - vds;
            let vgb = vgs - vbs;
            let ctrl = [vgs, vgd, vgb];
            let mut qg = [Dual::<4>::cst(0.0); 3];
            for k in 0..3 {
                let c_avg = 0.5 * (caps[k] + acc[9 + k]);
                qg[k] = (ctrl[k] - acc[3 + k]) * c_avg + acc[6 + k];
                s[3 + k] = ctrl[k].v;
                s[6 + k] = qg[k].v;
                s[9 + k] = caps[k];
            }
            let [qgs, qgd, qgb] = qg;

            let i_d = (ids - ibd) * sgn;
            let i_s = (-ids - ibs) * sgn;
            let i_b = (ibd + ibs) * sgn;
            let i_t = [i_d, Dual::cst(0.0), i_s, i_b];
            let q_t = [
                (-qgd - qbd) * sgn,
                (qgs + qgd + qgb) * sgn,
                (-qgs - qbs) * sgn,
                (qbd + qbs - qgb) * sgn,
            ];
            out.core(&mut at, &i_t, &q_t, &act, &lin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_ids(vgs: f64, vds: f64, polarity: f64) -> f64 {
        let p = ModelParams::new(&[
            ("vto".into(), 0.7 * polarity),
            ("kp".into(), 1e-4),
            ("is".into(), 0.0),
        ]);
        let (model, _) = Mos1Model::from_params(&p, polarity);
        let geom = Mos1Geometry {
            w: 10e-6,
            l: 1e-6,
            ..Mos1Geometry::default()
        };
        let mut m = Mos1s::default();
        m.push("m1", [Some(0), Some(1), None, None], model, geom, false);
        let u = Unknowns::with_nodes(["d", "g"]);
        let mut b = StampBuilder::new(&u);
        m.setup(&mut b).unwrap();
        let l = b.finish();
        let mut s = Slots::for_layout(&l);
        let ctx = EvalCtx {
            limit: false,
            gmin: 0.0,
            ..EvalCtx::dc()
        };
        let mut cur = [0.0; NSTATE];
        m.eval(
            &ctx,
            &[vds, vgs],
            StateRef {
                accepted: &[0.0; NSTATE],
                current: &mut cur,
            },
            &mut s,
        );
        s.f[0]
    }

    #[test]
    fn square_law_regions() {
        let beta = 1e-4 * 10.0;
        // Saturation: beta/2 * (1.5 - 0.7)^2.
        let sat = eval_ids(1.5, 3.0, 1.0);
        assert!((sat - beta / 2.0 * 0.64).abs() < 1e-15);
        // Linear: beta * 0.2 * (0.8 - 0.1).
        let lin = eval_ids(1.5, 0.2, 1.0);
        assert!((lin - beta * 0.2 * 0.7).abs() < 1e-15);
        // Off below threshold.
        assert!(eval_ids(0.5, 1.0, 1.0).abs() < 1e-20);
        // Reverse mode: swapping drain and source negates the current.
        assert!((eval_ids(1.5 - 3.0, -3.0, 1.0) + eval_ids(1.5, 3.0, 1.0)).abs() < 1e-15);
        // PMOS mirrors NMOS.
        let p = eval_ids(-1.5, -3.0, -1.0);
        assert!((p + sat).abs() < 1e-15);
    }

    #[test]
    fn meyer_is_continuous() {
        let (phi, cox) = (0.6, 1.0);
        let e = 1e-9;
        for v in [-phi, -phi / 2.0, 0.0] {
            let a = meyer(v - e, 1.0, phi, cox);
            let b = meyer(v + e, 1.0, phi, cox);
            assert!(
                (a.0 - b.0).abs() < 1e-6 && (a.2 - b.2).abs() < 1e-6,
                "at {v}"
            );
        }
        // Linear/saturation boundary.
        let a = meyer(1.0, 1.0 - e, phi, cox);
        let b = meyer(1.0, 1.0 + e, phi, cox);
        assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
    }
}
