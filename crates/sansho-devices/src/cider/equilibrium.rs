//! Equilibrium Poisson–Boltzmann initialization on a rectangular box mesh.
//! Provenance: Selberherr (1984); diagonally preconditioned conjugate gradients for the
//! symmetric positive definite Newton correction with fixed electrode potentials.

use super::{CHARGE, Edge, Vertex, invalid};
use sansho_diag::Diagnostic;

pub(super) fn planar(p: &mut [Vertex], edges: &[Edge], vt: f64) -> Result<(), Diagnostic> {
    let free = p
        .iter()
        .map(|v| v.active && v.port.is_none())
        .collect::<Vec<_>>();
    let mut laplace = vec![0.; p.len()];
    let mut coupling = Vec::new();
    for e in edges {
        let [i, j] = e.ends;
        let w = e.dielectric / e.length;
        laplace[i] += w;
        laplace[j] += w;
        if free[i] && free[j] {
            coupling.push((i, j, w));
        }
    }
    for _ in 0..160 {
        let mut diag = vec![1.; p.len()];
        let mut rhs = vec![0.; p.len()];
        for (i, v) in p.iter().enumerate().filter(|(i, _)| free[*i]) {
            let n = v.ni * (v.psi0 / vt).exp();
            let h = v.ni * (-v.psi0 / vt).exp();
            let q = CHARGE * v.volume;
            diag[i] = laplace[i] + q * (n + h) / vt;
            rhs[i] = q * (h - n + v.net) + v.fixed;
        }
        for e in edges {
            let [i, j] = e.ends;
            let flux = e.dielectric / e.length * (p[j].psi0 - p[i].psi0);
            if free[i] {
                rhs[i] += flux;
            }
            if free[j] {
                rhs[j] -= flux;
            }
        }
        let step = cg(&diag, &coupling, rhs)?;
        let max = step.iter().map(|v| v.abs()).fold(0., f64::max);
        let damping = (4. * vt / max).min(1.);
        for (pt, dx) in p.iter_mut().zip(step) {
            pt.psi0 += dx * damping;
        }
        if max < 1e-12 {
            return Ok(());
        }
    }
    Err(invalid("2-D equilibrium Poisson solve did not converge"))
}

fn cg(
    diag: &[f64],
    edges: &[(usize, usize, f64)],
    mut residual: Vec<f64>,
) -> Result<Vec<f64>, Diagnostic> {
    let mut step = vec![0.; diag.len()];
    let mut direction: Vec<_> = residual.iter().zip(diag).map(|(r, d)| r / d).collect();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let mut norm = dot(&residual, &direction);
    let target = norm * 1e-24;
    let mut product = step.clone();
    for _ in 0..2 * diag.len() {
        if norm <= target && norm.is_finite() {
            return Ok(step);
        }
        for ((v, d), s) in product.iter_mut().zip(diag).zip(&direction) {
            *v = d * s;
        }
        for &(i, j, w) in edges {
            product[i] -= w * direction[j];
            product[j] -= w * direction[i];
        }
        let alpha = norm / dot(&direction, &product);
        if !alpha.is_finite() {
            break;
        }
        for i in 0..step.len() {
            step[i] += alpha * direction[i];
            residual[i] -= alpha * product[i];
            product[i] = residual[i] / diag[i];
        }
        let next = dot(&residual, &product);
        for (v, z) in direction.iter_mut().zip(&product) {
            *v = z + next / norm * *v;
        }
        norm = next;
    }
    Err(invalid("2-D equilibrium linear solve failed"))
}
