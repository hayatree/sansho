//! Bound statistical expressions and reproducible parameter sampling.
//! Provenance: original implementation from the ngspice user manual (statistical
//! functions), Steele, Lea & Flood (2014), and the Box–Muller transform.

use crate::expr::{Expr, Funcs, eval};

/// A counter identifies a run, including run zero.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub seed: u64,
    pub index: u64,
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

pub(crate) fn identity(parent: u64, name: &str) -> u64 {
    mix(name.bytes().fold(parent ^ 0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    }))
}

impl Sample {
    fn uniform(self, site: u64, lane: u64) -> f64 {
        let counter = mix(mix(self.seed) ^ self.index);
        let bits = mix(mix(counter ^ site).wrapping_add(lane)) >> 12;
        (bits as f64 + 0.5) * (1. / (1u64 << 52) as f64)
    }
}

pub(crate) fn statistical(name: &str, n: usize) -> bool {
    matches!(
        (name, n),
        ("agauss" | "gauss", 3) | ("aunif" | "unif" | "limit", 2)
    )
}

/// Lazily materialized unit variates shared by every use of a lexical call site.
pub(crate) struct Draws {
    run: Sample,
    values: std::cell::RefCell<std::collections::HashMap<(u64, u8), f64>>,
}

impl Draws {
    pub fn new(run: Sample) -> Self {
        Self {
            run,
            values: Default::default(),
        }
    }
    fn variate(&self, site: u64, name: &str) -> f64 {
        let family = match name {
            "gauss" | "agauss" => 0,
            "limit" => 1,
            _ => 2,
        };
        *self
            .values
            .borrow_mut()
            .entry((site, family))
            .or_insert_with(|| {
                let u = self.run.uniform(site, 0);
                match name {
                    "gauss" | "agauss" => {
                        (-2. * u.ln()).sqrt()
                            * (std::f64::consts::TAU * self.run.uniform(site, 1)).cos()
                    }
                    "limit" => {
                        if u < 0.5 {
                            -1.
                        } else {
                            1.
                        }
                    }
                    _ => 2. * u - 1.,
                }
            })
    }
}

pub(crate) fn draw(name: &str, a: &[f64], site: u64, run: Option<&Draws>) -> Result<f64, String> {
    if !statistical(name, a.len()) {
        return Err(format!("invalid arguments to {name}"));
    }
    let Some(run) = run else { return Ok(a[0]) };
    let spread = if matches!(name, "unif" | "gauss") {
        a[0] * a[1]
    } else {
        a[1]
    };
    if spread == 0. {
        return Ok(a[0]);
    }
    let mut z = run.variate(site, name);
    if matches!(name, "gauss" | "agauss") {
        if a[2] <= 0. || !a[2].is_finite() {
            return Err("Gaussian sigma divisor must be positive and finite".into());
        }
        z /= a[2];
    }
    Ok(a[0] + spread * z)
}

/// Inline lexical names, fold constants and assign keys by expression-tree position.
pub(crate) fn bind(
    e: &Expr,
    var: &dyn Fn(&str) -> Result<Expr, String>,
    funcs: &Funcs,
    site: u64,
    depth: usize,
) -> Result<Expr, String> {
    if depth > 128 {
        return Err("statistical expression nests too deeply".into());
    }
    let child = |x: &Expr, i: u64| bind(x, var, funcs, mix(site.wrapping_add(i)), depth + 1);
    let bound = match e {
        Expr::Var(n) => {
            return var(n).or_else(|err| {
                if n == "pi" {
                    Ok(Expr::Num(std::f64::consts::PI))
                } else {
                    Err(err)
                }
            });
        }
        Expr::Num(_) | Expr::Draw(..) => return Ok(e.clone()),
        Expr::Unary(op, x) => Expr::Unary(*op, Box::new(child(x, 1)?)),
        Expr::Binary(op, l, r) => Expr::Binary(*op, Box::new(child(l, 1)?), Box::new(child(r, 2)?)),
        Expr::Cond(c, a, b) => {
            let cond = child(c, 1)?;
            if let Expr::Num(c) = cond {
                return if c != 0. { child(a, 2) } else { child(b, 3) };
            }
            Expr::Cond(
                Box::new(cond),
                Box::new(child(a, 2)?),
                Box::new(child(b, 3)?),
            )
        }
        Expr::Call(name, args) => {
            let args = args
                .iter()
                .enumerate()
                .map(|(i, a)| child(a, i as u64 + 1))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(f) = funcs.get(name) {
                if f.args.len() != args.len() {
                    return Err(format!("invalid argument count for {name}"));
                }
                return bind(
                    &f.body,
                    &|n| match f.args.iter().position(|a| a == n) {
                        Some(i) => Ok(args[i].clone()),
                        None => var(n),
                    },
                    funcs,
                    identity(site, name),
                    depth + 1,
                );
            }
            if statistical(name, args.len()) {
                if matches!(args[1], Expr::Num(0.)) {
                    return Ok(args[0].clone());
                }
                return Ok(Expr::Draw(site, name.clone(), args));
            }
            Expr::Call(name.clone(), args)
        }
    };
    let literal = |x: &Expr| matches!(x, Expr::Num(_));
    let constant = match &bound {
        Expr::Unary(_, x) => literal(x),
        Expr::Binary(_, l, r) => literal(l) && literal(r),
        Expr::Call(_, a) => a.iter().all(literal),
        _ => false,
    };
    if constant {
        eval(&bound, &|n| Err(n.into()), funcs).map(Expr::Num)
    } else {
        Ok(bound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_spreads_and_sigma_divisor() {
        let draws = Draws::new(Sample {
            seed: 47,
            index: 29,
        });
        let run = Some(&draws);
        for (relative, absolute) in [("gauss", "agauss"), ("unif", "aunif")] {
            let n = if relative == "gauss" { 3 } else { 2 };
            let rel = [8., 0.25, 2.];
            let abs = [8., 2., 2.];
            assert_eq!(
                draw(relative, &rel[..n], 47, run),
                draw(absolute, &abs[..n], 47, run)
            );
        }
        let a = draw("agauss", &[0., 1., 1.], 1, run).unwrap();
        assert_eq!(a / 3., draw("agauss", &[0., 1., 3.], 1, run).unwrap());
        let x = draw("limit", &[8., 2.], 2, run).unwrap();
        assert!(x == 6. || x == 10.);
        assert_eq!(draw("limit", &[8., 2.], 1, None).unwrap(), 8.);
    }
}
