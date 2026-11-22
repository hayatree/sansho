//! Continuous and sampled transfer characteristics for analog code models.
//! Provenance: ngspice 44 user manual §§8.2.8–10, 8.2.13–15, 8.2.19–22 and 8.2.26–33.

use super::{Curve, D, Limits, invalid, port_error, positive};
use crate::{EvalCtx, Mode, ModelParams};
use sansho_diag::Diagnostic;
use sansho_netlist::{CodePort, ModelCard};

/// Model signatures supply differential defaults after the forward model reference resolves.
pub(super) fn default_ports(
    ports: &[CodePort],
    defaults: &[&str],
) -> Result<Vec<CodePort>, Diagnostic> {
    let mut parts = ports.iter();
    let mut out = Vec::new();
    for &default in defaults {
        let mut p = parts
            .next()
            .ok_or_else(|| port_error("missing code-model port"))?
            .clone();
        if p.modifier.is_empty() {
            p.modifier = default.into();
            if p.differential() && !p.vector && !p.nodes.is_empty() {
                let next = parts
                    .next()
                    .filter(|n| n.modifier.is_empty() && !n.vector && n.nodes.len() == 1)
                    .ok_or_else(|| port_error("missing negative differential terminal"))?;
                p.nodes.extend_from_slice(&next.nodes);
            }
        }
        out.push(p);
    }
    if parts.next().is_some() {
        return Err(port_error("extra code-model terminal"));
    }
    Ok(out)
}

pub(super) fn signal_ports(k: &str, ports: &[CodePort]) -> Result<Vec<CodePort>, Diagnostic> {
    let passive = matches!(k, "core" | "memristor" | "sidiode" | "zener");
    let defaults: &[&str] = match k {
        "aswitch" if ports.len() == 3 => &["v", "gd"],
        "multi_input_pwl" => &["vd", "vd"],
        "oneshot" => &["v", "v", "v", "v"],
        "delay" => &["v", "v", "v"],
        "pwlts" => &["v"],
        "table2d" => &["v", "v", "i"],
        "table3d" => &["v", "v", "v", "i"],
        _ if passive => &["gd"],
        _ => return Ok(ports.to_vec()),
    };
    let mut p = default_ports(ports, defaults)?;
    if passive {
        if !matches!(p[0].modifier.as_str(), "gd") && !(k == "core" && p[0].modifier == "g")
            || p[0].nodes.is_empty()
        {
            return Err(port_error("invalid passive port"));
        }
        let diff = p[0].differential();
        p[0].modifier = if diff { "vd" } else { "v" }.into();
        let mut output = p[0].clone();
        output.modifier = if diff { "id" } else { "i" }.into();
        p.push(output);
    } else if k == "multi_input_pwl"
        && (p
            .iter()
            .any(|p| !matches!(p.modifier.as_str(), "vd" | "id"))
            || p[0].nodes.len() < 4)
    {
        return Err(port_error(
            "multi_input_pwl needs at least two differential inputs",
        ));
    } else if k == "delay" || k == "oneshot" {
        if k == "delay" {
            if !matches!(p[0].modifier.as_str(), "v" | "vd" | "vnam")
                || !matches!(p[1].modifier.as_str(), "v" | "vd")
            {
                return Err(port_error(
                    "delay expects voltage output and voltage or named-source input",
                ));
            }
            p.swap(1, 2);
        }
        for port in &mut p[1..defaults.len() - 1] {
            if port.nodes.is_empty() {
                port.nodes.push("0".into());
                port.modifier = "v".into();
            }
        }
    }
    Ok(p)
}

#[derive(Debug)]
pub(super) enum Signal {
    Hyst {
        limits: Limits,
        shift: f64,
        low: f64,
        scale: f64,
    },
    Multi {
        curve: Curve,
        max: bool,
    },
    Time {
        curve: Curve,
        hold: bool,
    },
    Zener {
        vb: f64,
        ib: f64,
        avalanche_scale: f64,
        leakage: f64,
        saturation: f64,
        emission: f64,
        limited: bool,
    },
    Diode {
        on: f64,
        off: f64,
        reverse: f64,
        vf: f64,
        vr: f64,
        widths: [f64; 2],
        limits: [f64; 2],
    },
    Mem {
        range: [f64; 2],
        initial: f64,
        threshold: f64,
        alpha: f64,
        beta: f64,
    },
    Meter(f64),
    Pulse {
        width: Curve,
        trigger: f64,
        has_clear: bool,
        positive: bool,
        retrigger: bool,
        rails: [f64; 2],
        rise: f64,
        fall: f64,
        rd: f64,
        fd: f64,
    },
    Delay {
        fixed: f64,
        controlled: bool,
        lo: f64,
        hi: f64,
        capacity: usize,
    },
    Grid(super::tables::Grid),
    Frequency(super::tables::Frequency),
}

fn bound(x: D, lo: f64, hi: f64) -> D {
    if x.v < lo {
        D::cst(lo)
    } else if x.v > hi {
        D::cst(hi)
    } else {
        x
    }
}
impl Signal {
    pub(super) fn new(
        k: &str,
        p: &ModelParams,
        m: &ModelCard,
        raw_ports: &[CodePort],
        mut arr: impl FnMut(&'static str, Vec<f64>) -> Vec<f64>,
    ) -> Result<Self, Diagnostic> {
        let get = |s, d| p.get(s, d);
        Ok(match k {
            "hyst" | "core" => {
                let low = get("out_lower_limit", 0.);
                let high = get("out_upper_limit", 1.);
                let lo = get("in_low", 0.);
                let hi = get("in_high", 1.);
                let limits = Limits {
                    lo,
                    hi,
                    range: get("input_domain", 0.01),
                    fraction: get("fraction", 1.) != 0.,
                };
                let shift = get("hyst", 0.);
                if hi <= lo
                    || shift < 0.
                    || limits.range < 0.
                    || 2. * limits.width(D::cst(lo), D::cst(hi)).v > hi - lo
                {
                    return Err(invalid("invalid hysteresis thresholds or smoothing"));
                }
                Self::Hyst {
                    limits,
                    shift,
                    low,
                    scale: (high - low) / (hi - lo),
                }
            }
            "multi_input_pwl" => Self::Multi {
                curve: Curve::new(arr("x", vec![]), arr("y", vec![]), 0., false)?,
                max: get("model", 0.) != 0.,
            },
            "pwlts" => {
                let curve = Curve::new(
                    arr("x_array", vec![]),
                    arr("y_array", vec![]),
                    get("input_domain", 0.01),
                    get("fraction", 1.) != 0.,
                )?;
                if curve.x[0] < 0. {
                    return Err(invalid("negative pwlts time"));
                }
                Self::Time {
                    curve,
                    hold: get("limit", 0.) != 0.,
                }
            }
            "zener" => {
                let vb = get("v_breakdown", 0.);
                let ib = get("i_breakdown", 0.02);
                let rb = get("r_breakdown", 1.);
                // Reverse e-fold voltage measured across the resistance sweep.
                let avalanche_scale = ib * rb;
                let leakage = get("i_rev", 1e-6);
                let saturation = get("i_sat", 1e-12);
                let emission = get("n_forward", 1.);
                if !(1e-6..=1e6).contains(&vb)
                    || ib < 1e-9
                    || avalanche_scale <= 0.
                    || !avalanche_scale.is_finite()
                    || leakage < 0.
                    || saturation < 1e-15
                    || !(0.1..=10.).contains(&emission)
                {
                    return Err(invalid("invalid zener characteristic"));
                }
                Self::Zener {
                    vb,
                    ib,
                    avalanche_scale,
                    leakage,
                    saturation,
                    emission,
                    limited: get("limit_switch", 0.) != 0.,
                }
            }
            "sidiode" => {
                let on = get("ron", 1.);
                let off = get("roff", on);
                let r = get("rrev", 0.);
                let vf = get("vfwd", 0.);
                let vr = get("vrev", 1e30);
                let widths = [get("epsilon", 0.), get("revepsilon", 0.)];
                let limits = [get("ilimit", 1e30), get("revilimit", 1e30)];
                if on < 1e-6
                    || off < 1e-12
                    || r < 0.
                    || vf < 0.
                    || vr < 0.
                    || widths.iter().any(|&x| x < 0.)
                    || limits.iter().any(|&x| x < 1e-15)
                {
                    return Err(invalid("invalid sidiode parameters"));
                }
                Self::Diode {
                    on,
                    off,
                    reverse: if r == 0. { on } else { r },
                    vf,
                    vr,
                    widths,
                    limits,
                }
            }
            "memristor" => {
                let range = [get("rmin", 10.), get("rmax", 1e4)];
                let initial = get("rinit", 7000.);
                let threshold = get("vt", 0.);
                if range[0] <= 0. || !(range[0]..=range[1]).contains(&initial) || threshold < 0. {
                    return Err(invalid("invalid memristor resistance or threshold"));
                }
                Self::Mem {
                    range,
                    initial,
                    threshold,
                    alpha: get("alpha", 0.),
                    beta: get("beta", 1.),
                }
            }
            "cmeter" | "lmeter" => Self::Meter(get("gain", 1.)),
            "table2d" | "table3d" => Self::Grid(super::tables::Grid::new(m, p)?),
            "xfer" => Self::Frequency(super::tables::Frequency::new(m, p, arr("table", vec![]))?),
            "oneshot" => {
                let width = Curve::new(
                    arr("cntl_array", vec![0., 1.]),
                    arr("pw_array", vec![1e-6, 0.9999999]),
                    0.,
                    false,
                )?;
                let rise = get("rise_time", 1e-9);
                let fall = get("fall_time", 1e-9);
                let rd = get("rise_delay", 1e-9);
                let fd = get("fall_delay", 1e-9);
                if [rise, fall, rd, fd].iter().chain(&width.y).any(|&v| v < 0.) {
                    return Err(invalid("negative oneshot timing"));
                }
                Self::Pulse {
                    width,
                    has_clear: raw_ports.get(2).is_some_and(|p| !p.nodes.is_empty()),
                    trigger: get("clk_trig", 0.5),
                    positive: get("pos_edge_trig", 1.) != 0.,
                    retrigger: get("retrig", 0.) != 0.,
                    rails: [get("out_low", 0.), get("out_high", 1.)],
                    rise,
                    fall,
                    rd,
                    fd,
                }
            }
            "delay" => {
                let fixed = get("delay", 0.);
                let lo = get("delmin", 0.);
                let hi = get("delmax", 0.);
                let cap = get("buffer_size", 1024.);
                if fixed < 0.
                    || lo < 0.
                    || hi < lo
                    || cap.fract() != 0.
                    || !(1. ..=1048576.).contains(&cap)
                {
                    return Err(invalid("invalid delay bounds or history length"));
                }
                Self::Delay {
                    fixed,
                    controlled: get("has_delay_cnt", 0.) != 0.,
                    lo,
                    hi,
                    capacity: cap as usize,
                }
            }
            _ => return Err(invalid(format!("unknown signal model {k}"))),
        })
    }
    pub(super) fn state_len(&self) -> usize {
        match self {
            Self::Hyst { .. } | Self::Mem { .. } => 3,
            Self::Zener { limited: true, .. } => 1,
            Self::Pulse { .. } => 8,
            Self::Delay { capacity, .. } => 3 + 2 * capacity,
            _ => 0,
        }
    }
    pub(super) fn eval(
        &self,
        ctx: &EvalCtx,
        input: impl Fn(usize) -> D,
        n: usize,
        h: &[f64],
        save: Option<&mut [f64]>,
    ) -> D {
        let u = if n > 0 { input(0) } else { D::cst(0.) };
        match self {
            Self::Hyst {
                limits,
                shift,
                low,
                scale,
            } => {
                let up = limits.fixed(u - *shift);
                let down = limits.fixed(u + *shift);
                let v = if h[0] == 0. || h[1] <= up.v {
                    up
                } else if h[1] >= down.v {
                    down
                } else {
                    D::cst(h[1])
                };
                if let Some(s) = save {
                    s.copy_from_slice(&[1., v.v, ctx.time]);
                }
                *low + *scale * (v - limits.lo)
            }
            Self::Multi { curve, max } => {
                let selected = (1..n).fold(u, |a, i| {
                    let b = input(i);
                    if (*max && b.v > a.v) || (!max && b.v < a.v) {
                        b
                    } else {
                        a
                    }
                });
                curve.eval(selected)
            }
            Self::Time { curve, hold } => curve.eval(if *hold {
                bound(D::cst(ctx.time), curve.x[0], curve.x[curve.x.len() - 1])
            } else {
                D::cst(ctx.time)
            }),
            Self::Zener {
                vb,
                ib,
                avalanche_scale,
                leakage,
                saturation,
                emission,
                limited,
            } => {
                let v = D::var(if *limited { h[0] } else { u.v }, 0);
                let forward = crate::junction::ideal_current(
                    v,
                    *saturation,
                    crate::thermal_voltage(ctx.temp) * emission,
                );
                // Normalize the reverse exponential at both zero and the rated
                // voltage. Factor out exp(Vb/S) for arbitrarily narrow knees.
                let s = *avalanche_scale;
                let norm = -(-vb / s).exp_m1();
                let breakdown = if v.v >= 0. {
                    // Tangent continuation on the forward side of zero.
                    v * ((-vb / s).exp() / norm * ib / s)
                } else {
                    let rated = crate::junction::safe_exp((-v - *vb) / s);
                    rated * (v / s).exp_m1() / norm * *ib
                };
                let i = forward + v * (leakage / vb) + breakdown;
                i.v + (u - v.v) * i.d[0]
            }
            Self::Diode {
                on,
                off,
                reverse,
                vf,
                vr,
                widths,
                limits,
            } => {
                let i = u / *off + (1. / on - 1. / off) * positive(u - *vf, widths[0])
                    - (1. / reverse - 1. / off) * positive(-u - *vr, widths[1]);
                let limit = limits[usize::from(i.v < 0.)];
                let t = (i.v / limit).tanh();
                D {
                    v: limit * t,
                    d: [(1. - t * t) * i.d[0]],
                }
            }
            Self::Meter(v) => D::cst(*v),
            Self::Grid(grid) => grid.eval(input),
            Self::Frequency(_) => D::cst(0.),
            Self::Mem {
                range,
                initial,
                threshold,
                alpha,
                beta,
            } => {
                let r = if ctx.mode != Mode::Tran || h[0] == 0. {
                    D::cst(*initial)
                } else {
                    let rate = *beta * u + (alpha - beta) * bound(u, -threshold, *threshold);
                    bound(h[1] + (ctx.time - h[2]).max(0.) * rate, range[0], range[1])
                };
                if let Some(s) = save {
                    s.copy_from_slice(&[1., r.v, ctx.time]);
                }
                u / r
            }
            Self::Pulse {
                width,
                has_clear,
                trigger,
                positive,
                retrigger,
                rails,
                rise,
                fall,
                rd,
                fd,
            } => {
                let time = ctx.time;
                let clock = if *positive {
                    u.v >= *trigger
                } else {
                    u.v <= *trigger
                };
                let clear = *has_clear && input(2).v >= *trigger;
                let (mut start, mut end, mut from, mut peak) = (h[3], h[4], h[5], h[6]);
                let ramp = |t: f64, d: f64| if d == 0. { 1. } else { (t / d).clamp(0., 1.) };
                let shape = |start, end, from, peak| {
                    if time >= end {
                        peak * (1. - ramp(time - end, *fall))
                    } else if time < start {
                        from
                    } else {
                        from + (1. - from) * ramp(time - start, *rise)
                    }
                };
                let old = shape(start, end, from, peak);
                if ctx.mode == Mode::Tran && h[0] != 0. {
                    if clear && h[2] == 0. {
                        end = time;
                        peak = old;
                    } else if !clear
                        && clock
                        && h[1] == 0.
                        && (*retrigger || time >= end + fall || peak == 0.)
                    {
                        if old == 0. || time >= end {
                            start = time + rd;
                            from = old;
                        }
                        end = time + rd + rise + width.eval(input(1)).v.max(0.) + fd;
                        peak = 1.;
                    }
                }
                if let Some(s) = save {
                    s.copy_from_slice(&[
                        1.,
                        f64::from(clock),
                        f64::from(clear),
                        start,
                        end,
                        from,
                        peak,
                        time,
                    ]);
                }
                D::cst(rails[0] + (rails[1] - rails[0]) * shape(start, end, from, peak))
            }
            Self::Delay {
                fixed,
                controlled,
                lo,
                hi,
                capacity,
            } => {
                let lag = if *controlled {
                    *lo + (hi - lo) * bound(input(1), 0., 1.)
                } else {
                    D::cst(*fixed)
                };
                let count = h[0] as usize;
                let next = h[1] as usize;
                let at = |j| {
                    let i = (next + capacity - count + j) % capacity;
                    (h[3 + 2 * i], D::cst(h[4 + 2 * i]))
                };
                let t = ctx.time - lag;
                let v = if count == 0 || ctx.mode != Mode::Tran || lag.v == 0. {
                    u
                } else {
                    let mut lower = 0;
                    let mut upper = count;
                    while lower < upper {
                        let mid = lower + (upper - lower) / 2;
                        if at(mid).0 <= t.v {
                            lower = mid + 1;
                        } else {
                            upper = mid;
                        }
                    }
                    if lower == 0 {
                        at(0).1
                    } else {
                        let (a, va) = at(lower - 1);
                        let (b, vb) = if lower < count {
                            at(lower)
                        } else {
                            (ctx.time, u)
                        };
                        if b <= a {
                            va
                        } else {
                            va + (vb - va) * (t - a) / (b - a)
                        }
                    }
                };
                if let Some(s) = save {
                    s.copy_from_slice(h);
                    let append = count == 0 || ctx.time > at(count - 1).0;
                    let i = if append {
                        next
                    } else {
                        (next + capacity - 1) % capacity
                    };
                    s[3 + 2 * i] = ctx.time;
                    s[4 + 2 * i] = u.v;
                    s[2] = lag.v;
                    if append {
                        s[0] = (count + 1).min(*capacity) as f64;
                        s[1] = ((next + 1) % capacity) as f64;
                    }
                }
                v
            }
        }
    }
}
