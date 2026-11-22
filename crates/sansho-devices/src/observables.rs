//! Intrinsic FET operating-point currents, conductances and MOS charge derivatives.
//! Provenance: BSIM3v3.3 and BSIM4.8.2 technical manuals, terminal-charge derivatives.

use crate::dual::Dual;

/// Total terminal currents include junction conduction. Small-signal gm and
/// gds are derivatives of the intrinsic channel at the operating bias.
pub(crate) fn three_terminal_fet(
    values: &mut Vec<(&'static str, f64)>,
    channel: Dual<3>,
    [id, ig, is]: [f64; 3],
) {
    values.extend([
        ("id", id),
        ("ig", ig),
        ("is", is),
        ("gm", channel.d[1]),
        ("gds", channel.d[0]),
    ]);
}

pub(crate) fn mos(
    values: &mut Vec<(&'static str, f64)>,
    ids: Dual<4>,
    mut charge: [Dual<4>; 4],
    mult: f64,
    threshold: f64,
    sat: f64,
    reverse: bool,
) {
    values.extend([
        ("id", ids.v * mult),
        ("gm", ids.d[1] * mult),
        ("gds", ids.d[0] * mult),
        ("gmbs", ids.d[3] * mult),
        ("vth", threshold),
        ("vdsat", sat),
    ]);
    if reverse {
        charge.swap(0, 2);
    }
    for (r, q) in charge.into_iter().enumerate() {
        let mut grad = q.d;
        grad[2] = -grad.iter().sum::<f64>();
        if reverse {
            grad.swap(0, 2);
        }
        terminal_charge(values, r, q.v * mult, grad.map(|g| g * mult));
    }
}

pub(crate) fn terminal_charge(
    values: &mut Vec<(&'static str, f64)>,
    row: usize,
    charge: f64,
    gradient: [f64; 4],
) {
    const Q: [&str; 4] = ["qd", "qg", "qs", "qb"];
    const C: [[&str; 4]; 4] = [
        ["cdd", "cdg", "cds", "cdb"],
        ["cgd", "cgg", "cgs", "cgb"],
        ["csd", "csg", "css", "csb"],
        ["cbd", "cbg", "cbs", "cbb"],
    ];
    values.push((Q[row], charge));
    values.extend(C[row].into_iter().zip(gradient));
}
