//! BSIM4 channel, trap, gate and resistive network noise.
//! Provenance: UC Berkeley BSIM4 4.8.2 Technical Manual, noise models and Appendix A.10.
use super::{D, Instance, Size, channel::Bias};
use crate::dual::Dual;
use crate::{CHARGE, ModelParams, NoiseSource, volt};
use spx_diag::Diagnostic;

#[derive(Debug)]
pub(super) struct Parameters {
    thermal: f64,
    flicker: f64,
    ntnoi: f64,
    partition: [f64; 4],
    traps: [f64; 3],
    kf: f64,
    af: f64,
    ef: f64,
    em: f64,
    lintnoi: f64,
}
impl Parameters {
    pub fn new(p: &ModelParams, sign: f64) -> Self {
        p.accept(&["tnoic", "rnoic"]);
        Self {
            thermal: p.get("tnoimod", 0.),
            flicker: p.get("fnoimod", 1.),
            ntnoi: p.get("ntnoi", 1.),
            partition: [
                p.get("rnoia", 0.577),
                p.get("rnoib", 0.5164),
                p.get("tnoia", 1.5),
                p.get("tnoib", 3.5),
            ],
            traps: [
                p.get("noia", if sign > 0. { 6.25e41 } else { 6.188e40 }),
                p.get("noib", if sign > 0. { 3.125e26 } else { 1.5e25 }),
                p.get("noic", 8.75),
            ],
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            ef: p.get("ef", 1.),
            em: p.get("em", 4.1e7),
            lintnoi: p.get("lintnoi", 0.),
        }
    }
    /// Strong-inversion trap integral and channel-length modulation, blended
    /// with the weak-inversion limit by their harmonic combination.
    fn flicker(&self, m: &Size, b: &Bias<3>, id: f64, vds: f64) -> f64 {
        let length = m.l - 2. * self.lintnoi;
        if self.flicker == 0. {
            return self.kf * id.powf(self.af) / (m.coxe * m.l * m.l);
        }
        if id == 0. {
            return 0.;
        }
        let [a, bt, c] = self.traps;
        let kt = m.vt * CHARGE;
        let width = b.width * m.nf;
        let xdep = m.xdep0 * (1. - b.cv.bulk.v / m.phi).sqrt();
        let nstar = m.vt * (m.coxe + super::EPS_SI / xdep + m.p.cit) / CHARGE;
        let ns = m.coxe * b.over.v / CHARGE;
        let nd = ns * (1. - b.abulk * b.vd.v / (b.over.v + 2. * m.vt)).max(0.);
        let trap = a * ((ns + nstar) / (nd + nstar)).ln()
            + bt * (ns - nd)
            + 0.5 * c * (ns - nd) * (ns + nd);
        let strong = CHARGE.powi(2) * kt * id * b.mobility * trap
            / (1e10 * b.abulk * m.coxe * length.powi(2));
        let esat = 2. * m.vsat / b.mobility;
        let delta = if self.em > 0. {
            m.litl * ((((vds - b.vd.v) / m.litl + self.em) / esat).max(1.)).ln()
        } else {
            0.
        };
        let end = kt * id.powi(2) * delta * (a + bt * nd + c * nd * nd)
            / (1e10 * length.powi(2) * width * (nd + nstar).powi(2));
        let weak = a * kt * id.powi(2) / (1e10 * width * m.l * nstar.powi(2));
        let strong = (strong + end).max(0.);
        if strong + weak > 0. {
            strong * weak / (strong + weak)
        } else {
            0.
        }
    }
}
impl Instance {
    /// Eliminate the ideal series noise voltage (10.11) when the source
    /// diffusion is shorted. A unit AC shift of this device terminal drives
    /// the corresponding column of its G + jωC stamp, even at circuit ground.
    fn shorted_source_noise(&self, x: &[f64], source: usize, gmin: f64, power: f64) -> NoiseSource {
        let m = &self.model.size;
        let voltages = self.inner.map(|n| Dual::<1>::cst(volt(x, n)));
        let mut drive = voltages;
        drive[source].d[0] = 1.;
        let (currents, charges, bias) = m.terminal_equations(drive);
        let mut generator = NoiseSource::branch(&self.name, None, None, power, 0.);
        let mut row = |node: crate::Node, g: f64, c: f64| {
            if let Some(index) = node {
                generator.rows.push((index, g, c));
            }
        };
        for i in 0..4 {
            row(self.inner[i], currents[i].d[0], charges[i].d[0]);
        }

        let overlap = self.overlap_nodes();
        drive[1] = Dual::cst(volt(x, overlap[1]));
        for (node, q) in overlap.into_iter().zip(m.overlaps(drive)) {
            row(node, 0., q.d[0]);
        }
        let body = self.body[source / 2];
        let vj = (volt(x, body) - drive[source]) * m.sign;
        let (i, q) = m.junction[source / 2].eval(vj, gmin);
        let scale = m.sign * m.mult;
        row(body, scale * i.d[0], scale * q.d[0]);
        row(self.inner[source], -scale * i.d[0], -scale * q.d[0]);
        if self.model.network.gate_mode >= 2 {
            let delta = volt(x, self.middle) - voltages[1].v;
            let dg = self.gate_conductance(&bias).d[0] * m.mult * delta;
            row(self.middle, dg, 0.);
            row(self.inner[1], -dg, 0.);
        }
        generator
    }

    pub(super) fn noise(
        &self,
        x: &[f64],
        gmin: f64,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        let m = &self.model.size;
        let p = &m.noise;
        if !matches!(p.flicker, 0. | 1.)
            || !matches!(p.thermal, 0. | 1.)
            || (p.flicker == 1. && m.l <= 2. * p.lintnoi)
        {
            return Err(Diagnostic::error(
                "E-ANALYSIS",
                format!(
                    "{} requires BSIM4 noise selectors 0 or 1 and a positive noise length",
                    self.name
                ),
            ));
        }
        let v = self.inner.map(|n| volt(x, n) * m.sign);
        let (d, s) = if v[0] >= v[2] { (0, 2) } else { (2, 0) };
        let vg = v[1] - v[s];
        let vd = v[d] - v[s];
        let vb = v[3] - v[s];
        let (id, _, b) = m.channel(Dual::<3>::var(vg, 0), Dual::var(vd, 1), Dual::var(vb, 2));
        let kt = m.vt * CHARGE;
        let thermal = 4. * kt * m.mult;
        let esat_l = 2. * m.vsat * m.l / b.mobility;
        let field = (b.over.v / esat_l).powi(2) * m.l;
        let [ra, rb, ta, tb] = p.partition;
        let beta = ra * (1. + ta * field);
        let theta = rb * (1. + tb * field);
        let conductance = if p.thermal == 0. {
            // Integrate the inversion charge along the gradual channel. Use
            // the CV geometry even when charge partitioning has been disabled.
            let abulk = b.cv.abulk0.v * m.cv.bulk_scale;
            let over = b.over.v;
            let sat = over / abulk;
            let drop = abulk * super::smooth_min(D::cst(sat), D::cst(vd), 0.02).v;
            let mean = over - 0.5 * drop;
            let density = if mean > 0. {
                mean + drop * drop / (12. * mean)
            } else {
                0.
            };
            let [l, w] = m.cv.dimensions;
            let muq = b.mobility * m.centroid_cap(b.over).v * l * w * m.nf * density;
            p.ntnoi * muq / (m.l.powi(2) + muq * b.rds / m.nf)
        } else {
            let [gm, gd, gb] = id.d;
            let gch = b.id_per_volt.v.max(1e-38);
            ((beta * (gm + gb) + gd).powi(2) - (theta * (gm + gb + gd)).powi(2)) / gch
        };
        if p.thermal == 1. && self.inner[s] == self.nodes[s] {
            // The terminal Jacobian includes M copies, whose independent series
            // voltages average down in power by M.
            let voltage_psd = 4. * kt * theta.powi(2) / (m.mult * b.id_per_volt.v.max(1e-38));
            out.push(self.shorted_source_noise(x, s, gmin, voltage_psd));
        }
        let mut emit = |a, b, power, exponent| {
            out.push(NoiseSource::branch(&self.name, a, b, power, exponent))
        };
        emit(self.inner[d], self.inner[s], thermal * conductance, 0.);
        emit(
            self.inner[d],
            self.inner[s],
            m.mult * p.flicker(m, &b, id.v.abs(), vd),
            p.ef,
        );
        let [igd, igs, igb] = m.tunneling(&b);
        for (k, current) in [(d, igd.v), (s, igs.v), (3, igb.v)] {
            let edge = if k == 3 {
                0.
            } else {
                m.edge_tunneling(D::cst(v[1] - v[k]), k / 2).v
            };
            emit(
                self.inner[1],
                self.inner[k],
                2. * CHARGE * m.mult * (current + edge).abs(),
                0.,
            );
        }
        for k in [0, 2] {
            if self.inner[k] != self.nodes[k] {
                let v = self.series_nodes(k / 2).map(|n| volt(x, n));
                let mut g = -self.series(k / 2, v)[0].d[1] / m.mult;
                if p.thermal == 1. && k == s {
                    g *= 1. + theta.powi(2) * g / b.id_per_volt.v.max(1e-38);
                }
                emit(self.nodes[k], self.inner[k], thermal * g, 0.);
            }
        }
        let net = &self.model.network;
        if net.body.is_some() {
            for ((a, b), g) in self.body_branches().into_iter().zip(net.body_noise) {
                emit(a, b, thermal * g, 0.);
            }
        }
        if net.gate_mode > 0 {
            let r = net.gate.max(1e-6);
            let (node, g) = match net.gate_mode {
                3 => (self.middle, net.gate.max(0.) / r.powi(2)),
                2 => (
                    self.inner[1],
                    net.gate * self.gate_conductance(&b).v.powi(2),
                ),
                _ => (self.inner[1], net.gate.max(0.) / r.powi(2)),
            };
            emit(self.nodes[1], node, thermal * g, 0.);
        }
        Ok(())
    }
}
