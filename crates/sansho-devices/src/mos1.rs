//! Classical MOSFET terminal and charge assembly, with a Shichman–Hodges level-one channel.
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

use sansho_diag::Diagnostic;

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::model::ModelParams;
use crate::{
    DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    thermal_voltage, volt,
};

/// Permittivity of SiO2 (F/m).
const EPS_OX: f64 = 3.9 * 8.854_187_812_8e-12;

/// Parameters shared by the classical MOS terminal and charge models.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct Mos1Model {
    /// Prepared operating temperature in kelvin.
    pub temperature: Option<f64>,
    copies: f64,
    channel: crate::mos23::Channel,
    mobility: f64,
    surface_states: f64,
    sheet: [Option<f64>; 2],
    width_reduction: f64,
    pub kf: f64,
    pub af: f64,
    pub tnom: Option<f64>,

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
    /// Group index for the three classical channel models.
    pub fn level_index(&self) -> usize {
        match self.channel {
            crate::mos23::Channel::SquareLaw => 0,
            crate::mos23::Channel::ChargeSheet { .. } => 1,
            crate::mos23::Channel::SemiEmpirical { .. } => 2,
        }
    }

    /// Apply sheet resistance only at ends without an explicit model resistance.
    pub fn apply_squares(&mut self, inst: &ModelParams) {
        if let Some(sheet) = self.sheet[0] {
            self.rd = sheet * inst.get("nrd", 1.);
        }
        if let Some(sheet) = self.sheet[1] {
            self.rs = sheet * inst.get("nrs", 1.);
        }
    }

    /// Combine parallel copies with identical terminal voltages and geometry.
    pub fn multiply(&mut self, copies: f64) {
        self.copies *= copies;
        self.kp *= copies;
        self.is *= copies;
        self.js *= copies;
        self.rd /= copies;
        self.rs /= copies;
        self.cbd = self.cbd.map(|v| v * copies);
        self.cbs = self.cbs.map(|v| v * copies);
        self.cj *= copies;
        self.cjsw *= copies;
        self.cgso *= copies;
        self.cgdo *= copies;
        self.cgbo *= copies;
        self.cox *= copies;
    }
    pub fn at_temperature(&mut self, t: crate::temperature::Temperature) {
        self.temperature = Some(t.kelvin);
        if t.kelvin == t.nominal {
            return;
        }
        let phi = t.potential(self.phi);
        self.vto += -0.5 * t.gap_change()
            + self.polarity
                * (0.5 * (phi - self.phi) + self.gamma * (phi.sqrt() - self.phi.sqrt()));
        self.phi = phi;
        self.kp /= t.ratio().powf(1.5);
        self.mobility /= t.ratio().powf(1.5);
        self.is *= t.mos_saturation();
        self.js *= t.mos_saturation();
        let bottom = t.capacitance(self.pb, self.mj);
        self.cbd = self.cbd.map(|c| c * bottom);
        self.cbs = self.cbs.map(|c| c * bottom);
        self.cj *= bottom;
        self.cjsw *= t.capacitance(self.pb, self.mjsw);
        self.pb = t.potential(self.pb);
    }
    /// Build from `.model` parameters. Returns the model and the names of ignored parameters.
    pub fn from_params(p: &ModelParams, polarity: f64) -> (Self, Vec<String>) {
        Self::from_params_at(p, polarity, 27.)
    }
    pub fn from_params_at(
        p: &ModelParams,
        polarity: f64,
        default_tnom: f64,
    ) -> (Self, Vec<String>) {
        let level = p.get("level", 1.);
        let cox = p.get("tox", if level == 1. { 0. } else { 1e-7 });
        let cox = if cox > 0.0 { EPS_OX / cox } else { 0.0 };
        let u0 = p.opt(&["u0", "uo"]).unwrap_or(600.0);
        let kp = p
            .opt(&["kp"])
            .unwrap_or(if cox > 0.0 { u0 * 1e-4 * cox } else { 2e-5 });
        let nsub = if level == 1. { None } else { p.opt(&["nsub"]) };
        let tnom = p.opt(&["tnom"]);
        let nominal = tnom.unwrap_or(default_tnom) + 273.15;
        let vt = thermal_voltage(nominal);
        let gap = crate::temperature::gap(nominal);
        let ni = 1.45e10
            * (nominal / crate::REFTEMP).powf(1.5)
            * (crate::temperature::gap(crate::REFTEMP) / (2. * thermal_voltage(crate::REFTEMP))
                - gap / (2. * vt))
                .exp();
        let phi = p
            .opt(&["phi"])
            .unwrap_or_else(|| nsub.map_or(0.6, |n| (2. * vt * (n / ni).ln()).max(0.1)));
        let gamma = p
            .opt(&["gamma"])
            .unwrap_or_else(|| nsub.map_or(0., |n| crate::mos23::body_coefficient(n, cox)));
        let vto = p.opt(&["vto", "vt0"]).unwrap_or_else(|| {
            nsub.map_or(0., |_| {
                let gate = p.get("tpg", 1.);
                let work = if gate == 0. {
                    -0.05 - gap * 0.5 - polarity * phi * 0.5
                } else {
                    -polarity * (gate * gap + phi) * 0.5
                };
                work - crate::CHARGE * p.get("nss", 0.) * 1e4 / cox
                    + polarity * (gamma * phi.sqrt() + phi)
            })
        });
        let rd = p.opt(&["rd"]);
        let rs = p.opt(&["rs"]);
        let sheet = if level == 1. { None } else { p.opt(&["rsh"]) };
        let m = Self {
            channel: crate::mos23::Channel::bind(level, p, cox, nsub),
            sheet: [rd, rs].map(|r| sheet.filter(|_| r.is_none())),
            width_reduction: if level == 3. { p.get("wd", 0.) } else { 0. },
            mobility: u0 * 1e-4,
            surface_states: if level == 1. {
                0.
            } else {
                crate::CHARGE * 1e4 * p.get("nfs", 0.) / cox
            },
            temperature: None,
            copies: 1.,
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            tnom,

            polarity,
            vto,
            kp,
            gamma,
            phi,
            lambda: if level == 3. { 0. } else { p.get("lambda", 0.) },
            rd: rd.unwrap_or(0.),
            rs: rs.unwrap_or(0.),
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
        // Binning ranges are used when the model is selected (`sansho-netlist`).
        p.accept(&["level", "kf", "af", "tnom", "lmin", "lmax", "wmin", "wmax"]);
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
    width: f64,
    length: f64,
    noise_gate_cap: f64,
    has_storage: bool,
    sqrt_phi: f64,
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

impl Derived {
    fn has_capacitance(&self) -> bool {
        [
            self.cox_total,
            self.cgs_ov,
            self.cgd_ov,
            self.cgb_ov,
            self.cbd,
            self.cbs,
            self.cbd_sw,
            self.cbs_sw,
        ]
        .iter()
        .any(|&c| c != 0.)
    }
}

/// A batch of MOSFET instances with the same classical channel model.
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
        let width = geom.w - 2. * model.width_reduction;
        let mut d = Derived {
            width,
            length: leff,
            noise_gate_cap: width
                * leff
                * if model.cox > 0. {
                    model.cox
                } else {
                    model.copies * EPS_OX / 1e-7
                },
            has_storage: false,
            sqrt_phi: model.phi.sqrt(),
            beta: model.kp * width / leff,
            cox_total: model.cox * width * leff,
            cgs_ov: model.cgso * width,
            cgd_ov: model.cgdo * width,
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
        d.has_storage = d.has_capacitance();
        self.names.push(name.to_string());
        self.ext.push(nodes);
        self.int.push(nodes);
        self.off.push(off);
        self.model.push(model);
        self.derived.push(d);
    }

    /// Validate geometry; returns an error message for non-physical values.
    pub fn check_geometry(model: &Mos1Model, geom: &Mos1Geometry) -> Option<String> {
        if model.level_index() != 0
            && (![
                model.cox,
                model.phi,
                model.pb,
                model.mobility,
                geom.w,
                geom.l,
            ]
            .iter()
            .all(|v| v.is_finite() && *v > 0.)
                || ![
                    model.gamma,
                    model.kp,
                    model.rd,
                    model.rs,
                    model.width_reduction,
                    model.surface_states,
                ]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.)
                || !model.vto.is_finite()
                || !model.channel.valid()
                || !(0. ..1.).contains(&model.fc))
        {
            return Some("invalid classical MOS channel, junction or geometry parameter".into());
        }
        if geom.w - 2. * model.width_reduction <= 0.0 {
            Some(format!(
                "effective W must be positive (drawn width {})",
                geom.w
            ))
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
pub(crate) fn limit_step(vnew: f64, vold: f64, scale: f64) -> (f64, bool) {
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
    fn noise(
        &self,
        ctx: &EvalCtx,
        _: &[f64],
        slots: &Slots,
        out: &mut Vec<crate::NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            let m = &self.model[i];
            let temp = m.temperature.unwrap_or(ctx.temp);
            let thermal = 4. * crate::BOLTZMANN * temp;
            let mut at = self.at[i];
            let mut emit = |p, n, power, exp| {
                out.push(crate::NoiseSource::branch(&self.names[i], p, n, power, exp))
            };
            let [d, _, s, _] = self.int[i];
            for (k, r) in [(0, m.rd), (2, m.rs)] {
                if r > 0. {
                    emit(self.ext[i][k], self.int[i][k], thermal / r, 0.);
                    at.g += 4;
                }
            }
            let gm = slots.g[at.g + 1].abs();
            emit(d, s, thermal * (2. / 3.) * gm, 0.);
            if m.kf != 0. {
                // Gate-referred KF / (Cgate f^AF) becomes drain current
                // power via gm². gm includes all parallel copies.
                emit(
                    d,
                    s,
                    m.kf * gm.powi(2) / self.derived[i].noise_gate_cap,
                    m.af,
                );
            }
        }
        Ok(())
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }
    fn type_name(&self) -> &'static str {
        self.model.first().map_or("mos1", |m| m.channel.name())
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
            if self.derived[i].has_storage {
                for _ in 0..5 {
                    b.tracked_q(None);
                }
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        self.evaluate::<false>(ctx, x, st, out);
    }
    fn sample_charges(
        &self,
        x: &[f64],
        st: &mut StateRef<'_>,
        out: &mut Slots,
        _: &mut crate::bsim4::CoreCache,
    ) -> bool {
        // The converged charge and Meyer history depend only on voltage and
        // the accepted state. Temperature and gmin enter the IV equations.
        let sample = EvalCtx {
            limit: false,
            ..EvalCtx::dc()
        };
        self.evaluate::<true>(
            &sample,
            x,
            StateRef {
                accepted: st.accepted,
                current: st.current,
            },
            out,
        );
        true
    }
}
impl Mos1s {
    #[allow(clippy::many_single_char_names)]
    fn evaluate<const CHARGES_ONLY: bool>(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        st: StateRef<'_>,
        out: &mut Slots,
    ) {
        let vt = thermal_voltage(ctx.temp);
        let common_is = self.derived.first().map_or(0., |d| d.iss.max(d.isd));
        let common_vcrit = if ctx.limit { vcrit(common_is, vt) } else { 0. };
        for i in 0..self.len() {
            let m = &self.model[i];
            let dv = &self.derived[i];
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
            let mut at = self.at[i];
            if !CHARGES_ONLY && m.rd > 0.0 {
                out.resistor(
                    &mut at,
                    1.0 / m.rd,
                    volt(x, self.ext[i][0]),
                    volt(x, self.int[i][0]),
                );
            }
            if !CHARGES_ONLY && m.rs > 0.0 {
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
                let peak = dv.iss.max(dv.isd);
                let vc = if m.temperature.is_none_or(|t| t == ctx.temp)
                    && peak.to_bits() == common_is.to_bits()
                {
                    common_vcrit
                } else {
                    vcrit(peak, vt)
                };
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
            let sqrt_phi = dv.sqrt_phi;
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
            let (ids, vgst, vdsat) = m.channel.evaluate(
                vgs_e - vth,
                vds_e,
                sarg,
                m.gamma,
                dv.beta,
                m.lambda,
                dv.width,
                dv.length,
                m.mobility,
                m.surface_states,
                vt,
            );
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
            let (mut cgs, mut cgd, cgb) = meyer(
                if vgst > 0. { vdsat } else { vgst },
                vds_e.v,
                m.phi,
                dv.cox_total,
            );
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
            if CHARGES_ONLY {
                let delta: [f64; 4] = std::array::from_fn(|k| act[k] - lin[k]);
                for (dst, q) in out.q[at.q..at.q + 4].iter_mut().zip(q_t) {
                    *dst = q.v;
                    for (d, v) in q.d.into_iter().zip(delta) {
                        *dst += d * v;
                    }
                }
                at.q += 4;
            } else {
                out.core(&mut at, &i_t, &q_t, &act, &lin);
            }
            if dv.has_storage {
                out.q[at.q..at.q + 5].copy_from_slice(&[qgs.v, qgd.v, qgb.v, qbd.v, qbs.v]);
            }
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
    fn flicker_current_density_in_square_amperes_per_hz() {
        let params = ModelParams::new(&[
            ("tox".into(), 17.26566623496e-9),
            ("ld".into(), 0.5e-6),
            ("kf".into(), 8e-26),
            ("af".into(), 1.5),
        ]);
        let (model, _) = Mos1Model::from_params(&params, 1.);
        let geom = Mos1Geometry {
            w: 5e-6,
            l: 3e-6,
            ..Default::default()
        };
        let mut mos = Mos1s::default();
        mos.push("m1", [Some(0), Some(1), None, None], model, geom, false);
        let u = Unknowns::with_nodes(["d", "g"]);
        let mut builder = StampBuilder::new(&u);
        mos.setup(&mut builder).unwrap();
        let mut slots = Slots::for_layout(&builder.finish());
        slots.g[1] = 2e-3; // 2 mS at the operating point
        let mut sources = Vec::new();
        mos.noise(&EvalCtx::dc(), &[1., 1.], &slots, &mut sources)
            .unwrap();
        // Cox = 0.002 F/m², W Leff = 5 µm × (3 − 2 × 0.5) µm, Cg = 2e-14 F.
        // At 100 Hz, Sg = 8e-26 / (2e-14 × 100^1.5) = 4e-15 V²/Hz.
        // Multiplying by (2 mS)² gives 1.6e-20 A²/Hz.
        let flicker = sources.iter().find(|n| n.exponent == 1.5).unwrap();
        assert!((flicker.spectral_power(100.) / 1.6e-20 - 1.).abs() < 1e-12);
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
    fn converged_sample_matches_charge_slots_and_meyer_history() {
        let params = ModelParams::new(&[
            ("tox".into(), 8e-9),
            ("cgso".into(), 1e-10),
            ("cbd".into(), 2e-15),
            ("rd".into(), 1.),
        ]);
        for polarity in [1., -1.] {
            let (model, _) = Mos1Model::from_params(&params, polarity);
            let mut group = Mos1s::default();
            group.push(
                "m1",
                [Some(0), Some(1), Some(2), None],
                model,
                Default::default(),
                false,
            );
            let mut u = Unknowns::with_nodes(["d", "g", "s"]);
            group.alloc_unknowns(&mut u);
            let mut builder = StampBuilder::new(&u);
            group.setup(&mut builder).unwrap();
            let layout = builder.finish();
            let mut full = Slots::for_layout(&layout);
            let mut sample = Slots::for_layout(&layout);
            let mut accepted = [0.; NSTATE];
            for drain in [-0.9, 0.01, 0.7] {
                let x = [drain, 0.83, 0.13, drain + 0.01].map(|v| v * polarity);
                let mut a = accepted;
                let mut b = accepted;
                group.eval(
                    &EvalCtx {
                        limit: false,
                        temp: 345.,
                        ..EvalCtx::dc()
                    },
                    &x,
                    StateRef {
                        accepted: &accepted,
                        current: &mut a,
                    },
                    &mut full,
                );
                assert!(group.sample_charges(
                    &x,
                    &mut StateRef {
                        accepted: &accepted,
                        current: &mut b
                    },
                    &mut sample,
                    &mut Default::default()
                ));
                assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
                assert!(
                    full.q
                        .iter()
                        .zip(&sample.q)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
                accepted = a;
            }
        }
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
