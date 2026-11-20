//! Coupled magnetic, supply-current and resistive multiport equations.
//! Provenance: ngspice 44 user manual §§8.2.12, 8.2.14, 8.2.21 and 8.2.34. Original MNA stamps.

use super::{invalid, port_error, positive, signal::default_ports};
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, Slots, StampBuilder, StateRef, Unknowns, dual::Dual,
    volt,
};
use sansho_diag::Diagnostic;
use sansho_netlist::{CodePort, ModelCard};

type D = Dual<8>;
#[derive(Debug)]
enum Law {
    Winding(f64),
    Pot([f64; 2]),
    Switch {
        off: f64,
        on: f64,
        ro: f64,
        ri: f64,
        input: f64,
        log: bool,
    },
    Driver {
        offset: f64,
        gain: f64,
        r: [f64; 2],
        limit: [Option<f64>; 2],
        rail: f64,
        smooth: [f64; 2],
        domain: f64,
    },
}
#[derive(Debug)]
pub struct Multiport {
    name: String,
    law: Law,
    ports: Vec<[Node; 2]>,
    connected: Vec<bool>,
    branches: Vec<Node>,
}

impl Multiport {
    pub fn new(
        name: &str,
        raw: &[CodePort],
        m: &ModelCard,
        node: impl Fn(&str) -> Node,
    ) -> Result<Self, Diagnostic> {
        let k = m.kind.as_str();
        let p = ModelParams::new(&m.params);
        let (defaults, allowed): (&[&str], &[&[&str]]) = match k {
            "lcouple" => (&["hd", "hd"], &[&["hd", "h"], &["hd"]]),
            "pswitch" => (&["gd", "gd"], &[&["gd", "g"], &["gd"]]),
            "potentiometer" => (&["g", "g", "g"], &[&["g"], &["g"], &["g"]]),
            "ilimit" => (
                &["v", "g", "g", "g"],
                &[&["v", "vd"], &["g", "gd"], &["g", "gd"], &["g", "gd"]],
            ),
            _ => return Err(invalid("invalid multiport model")),
        };
        let ports = default_ports(raw, defaults)?;
        for (i, port) in ports.iter().enumerate() {
            if !allowed[i].contains(&port.modifier.as_str())
                || port.vector
                || port.nodes.is_empty() && !(k == "ilimit" && (i == 1 || i == 2))
            {
                return Err(port_error(format!("invalid {k} port {}", i + 1)));
            }
        }
        let law = match k {
            "lcouple" => Law::Winding(p.get("num_turns", 1.)),
            "pswitch" => {
                let off = p.get("cntl_off", 0.);
                let on = p.get("cntl_on", 1.);
                let ro = p.get("r_off", 1e12);
                let ri = p.get("r_on", 1.);
                let input = p.get("r_cntl_in", 1e12);
                if off == on || ro <= 0. || ri <= 0. || input <= 0. {
                    return Err(invalid("invalid pswitch bounds or resistance"));
                }
                Law::Switch {
                    off,
                    on,
                    ro,
                    ri,
                    input,
                    log: p.get("log", 1.) != 0.,
                }
            }
            "potentiometer" => {
                let pos = p.get("position", 0.5).clamp(1e-9, 1. - 1e-9);
                let r = p.get("r", 1e5);
                let exponent = p.get("log_multiplier", 1.);
                let f = if p.get("log", 0.) != 0. {
                    10f64.powf(-pos * exponent)
                } else {
                    pos
                };
                if r <= 0. || !(0. ..=1.).contains(&f) {
                    return Err(invalid("invalid potentiometer resistance or taper"));
                }
                Law::Pot([r * f, r * (1. - f)])
            }
            _ => {
                let r = [p.get("r_out_source", 1.), p.get("r_out_sink", 1.)];
                let limit = [p.opt(&["i_limit_source"]), p.opt(&["i_limit_sink"])];
                let smooth = [p.get("i_source_range", 1e-9), p.get("i_sink_range", 1e-9)];
                let domain = p.get("r_out_domain", 1e-9);
                let rail = p.get("v_pwr_range", 1e-6);
                if r.iter().any(|x| !(1e-9..=1e9).contains(x))
                    || limit.iter().flatten().any(|v| *v < 1e-12)
                    || smooth.iter().chain([&domain, &rail]).any(|v| *v < 1e-15)
                {
                    return Err(invalid(
                        "invalid ilimit resistance, current limit or smoothing range",
                    ));
                }
                Law::Driver {
                    offset: p.get("in_offset", 0.),
                    gain: p.get("gain", 1.),
                    r,
                    limit,
                    rail,
                    smooth,
                    domain,
                }
            }
        };
        if !p.unused().is_empty()
            || !m.arrays.is_empty()
            || m.params.iter().any(|(_, v)| !v.is_finite())
        {
            return Err(invalid(format!("unexpected {k} parameter")));
        }
        Ok(Self {
            name: name.into(),
            law,
            connected: ports.iter().map(|p| !p.nodes.is_empty()).collect(),
            branches: vec![None; ports.len()],
            ports: ports
                .iter()
                .map(|p| {
                    [
                        p.nodes.first().and_then(|n| node(n)),
                        p.nodes.get(1).and_then(|n| node(n)),
                    ]
                })
                .collect(),
        })
    }
    fn equations(&self, v: [D; 4], i: [D; 4]) -> ([D; 4], D) {
        let mut f = i;
        let mut q = D::cst(0.);
        match self.law {
            Law::Winding(n) => {
                f[0] = v[0];
                q = n * i[1];
                f[1] = v[1] - n * i[0];
            }
            Law::Pot([lower, upper]) => {
                f[0] = v[0] - v[1] - lower * i[0];
                f[1] = i[0] + i[1] + i[2];
                f[2] = v[2] - v[1] - upper * i[2];
            }
            Law::Switch {
                off,
                on,
                ro,
                ri,
                input,
                log,
            } => {
                let u = (v[0] - off) / (on - off);
                let t = if u.v <= 0.5 {
                    positive(u, 0.01)
                } else {
                    1. - positive(1. - u, 0.01)
                };
                let r = if log {
                    (ro.ln() + t * (ri.ln() - ro.ln())).exp()
                } else {
                    ro + t * (ri - ro)
                };
                f[0] = i[0] - v[0] / input;
                f[1] = i[1] - v[1] / r;
            }
            Law::Driver {
                offset,
                gain,
                r,
                limit,
                rail,
                smooth,
                domain,
            } => {
                let mut eq = gain * (v[0] + offset);
                if self.connected[1] && eq.v > v[1].v - rail {
                    eq = v[1] - positive(v[1] - eq, rail);
                }
                if self.connected[2] && eq.v < v[2].v + rail {
                    eq = v[2] + positive(eq - v[2], rail);
                }
                let delta = eq - v[3];
                let blend = if delta.v <= -domain {
                    D::cst(0.)
                } else if delta.v >= domain {
                    D::cst(1.)
                } else {
                    let t = (delta / domain + 1.) * 0.5;
                    t.sq() * (3. - 2. * t)
                };
                let mut drive = delta / (r[1] + blend * (r[0] - r[1]));
                if let Some(max) = limit[0]
                    && drive.v > max - smooth[0]
                {
                    drive = max - positive(max - drive, smooth[0]);
                }
                if let Some(max) = limit[1]
                    && drive.v < -max + smooth[1]
                {
                    drive = -max + positive(drive + max, smooth[1]);
                }
                // Blend the rail split without changing the sum of terminal currents.
                let [a, b] = [smooth[1], smooth[0]];
                let source = if drive.v <= -a {
                    D::cst(0.)
                } else if drive.v >= b {
                    drive
                } else if drive.v < 0. {
                    (drive + a).sq() * (0.5 * b / (a + b) / a)
                } else {
                    drive + (b - drive).sq() * (0.5 * a / (a + b) / b)
                };
                f[1] = i[1] - source;
                f[2] = i[2] - (drive - source);
                f[3] = i[3] + drive;
            }
        }
        (f, q)
    }
}
impl DeviceGroup for Multiport {
    fn type_name(&self) -> &'static str {
        "analog multiport"
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn len(&self) -> usize {
        1
    }
    fn is_linear(&self) -> bool {
        matches!(self.law, Law::Pot(_) | Law::Winding(_))
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (j, b) in self.branches.iter_mut().enumerate() {
            *b = Some(u.alloc_branch(&format!("{}:{j}", self.name)));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (&[a, n], &r) in self.ports.iter().zip(&self.branches) {
            b.f(a);
            b.f(n);
            b.f(r);
            b.g(a, r);
            b.g(n, r);
            for (&[p, m], &i) in self.ports.iter().zip(&self.branches) {
                b.g(r, p);
                b.g(r, m);
                b.g(r, i);
            }
        }
        if matches!(self.law, Law::Winding(_)) {
            b.tracked_q(self.branches[0]);
            b.c(self.branches[0], self.branches[1]);
        }
        Ok(())
    }
    fn eval(&self, _: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        let mut v = [D::cst(0.); 4];
        let mut i = v;
        for (j, (&[a, b], &r)) in self.ports.iter().zip(&self.branches).enumerate() {
            v[j] = D::var(volt(x, a) - volt(x, b), 2 * j);
            i[j] = D::var(volt(x, r), 2 * j + 1);
        }
        let (f, q) = self.equations(v, i);
        let mut g = 0;
        for j in 0..self.ports.len() {
            out.f[3 * j..3 * j + 3].copy_from_slice(&[i[j].v, -i[j].v, f[j].v]);
            out.g[g..g + 2].copy_from_slice(&[1., -1.]);
            g += 2;
            for k in 0..self.ports.len() {
                let d = f[j].d[2 * k];
                out.g[g..g + 3].copy_from_slice(&[d, -d, f[j].d[2 * k + 1]]);
                g += 3;
            }
        }
        if matches!(self.law, Law::Winding(_)) {
            out.q[0] = q.v;
            out.c[0] = q.d[3];
        }
    }
}
