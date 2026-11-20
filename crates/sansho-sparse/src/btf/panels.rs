//! Fixed-pivot, multiple-column Gilbert–Peierls updates for large filled blocks.
//! Provenance: original grouped implementation of the column operations in Gilbert & Peierls (1988).

use super::{FactorValue, Gp, TOL};
use crate::Scalar;

// Independent target columns share the traversal of each earlier L column.
// Rows of the local accumulator are contiguous for vector arithmetic.
pub(super) const WIDTH: usize = 8;
const ABSENT: usize = usize::MAX;

#[derive(Debug)]
struct Update {
    pivot: usize,
    upper: [usize; WIDTH],
}

#[derive(Debug)]
pub(super) struct Panel<T> {
    // External L rows packed across the eight constituent columns.
    lower: Vec<(usize, [T; WIDTH])>,
    internal_lower: [[T; WIDTH]; WIDTH],
    upper: Vec<(usize, [T; WIDTH])>,
    donors: Vec<(usize, std::ops::Range<usize>)>,
    pub(super) start: usize,
    updates: Vec<Update>,
    internal: [[usize; WIDTH]; WIDTH],
}

#[derive(Debug, Default)]
pub(super) struct Panels<T> {
    pub(super) groups: Vec<Panel<T>>,
    work: Vec<[T; WIDTH]>,
}

impl<T: Scalar> Panels<T> {
    pub(super) fn new(gp: &mut Gp<T>) -> Self {
        let mut groups = Vec::new();
        let mut owner = vec![ABSENT; gp.columns.len()];
        let mut start = 0;
        while start + WIDTH <= gp.columns.len() {
            let cols = &gp.columns[start..start + WIDTH];
            // Concentrate on thick fronts. For a fundamental supernode the
            // strict L column lengths decrease by one at every pivot.
            let len = cols[0].lower.len();
            if len < 16 || !(1..WIDTH).all(|i| cols[i].lower.len() + i == len) {
                start += 1;
                continue;
            }
            // In a supernode each successive L column omits the previous
            // pivot and has exactly the same rows below the panel.
            let end = start + WIDTH;
            for c in cols {
                gp.lower[c.lower.clone()].sort_unstable_by_key(|(r, _)| gp.pivot_at[*r]);
            }
            let mut lower = Vec::new();
            for (r, _) in &gp.lower[cols[0].lower.start + WIDTH - 1..cols[0].lower.end] {
                lower.push((*r, [T::zero(); WIDTH]));
            }
            let same = cols.iter().enumerate().all(|(lane, c)| {
                let entries = &gp.lower[c.lower.clone()];
                let k = WIDTH - 1 - lane;
                entries[..k]
                    .iter()
                    .zip(&cols[lane + 1..])
                    .all(|((r, _), next)| *r == next.pivot)
                    && entries[k..]
                        .iter()
                        .zip(&lower)
                        .all(|((r, _), (s, _))| r == s)
            });
            if !same {
                start += 1;
                continue;
            }
            let upper_entries = &gp.upper;
            let mut slots = cols
                .iter()
                .enumerate()
                .flat_map(|(lane, c)| c.upper.clone().map(move |k| (upper_entries[k].0, lane, k)))
                .filter(|&(pivot, _, _)| pivot < start)
                .collect::<Vec<_>>();
            slots.sort_unstable();
            let mut updates = Vec::new();
            for entries in slots.chunk_by(|a, b| a.0 == b.0) {
                let mut upper = [ABSENT; WIDTH];
                for &(_, lane, k) in entries {
                    upper[lane] = k;
                }
                updates.push(Update {
                    pivot: entries[0].0,
                    upper,
                });
            }
            let mut internal = [[ABSENT; WIDTH]; WIDTH];
            for (lane, c) in cols.iter().enumerate() {
                for k in c
                    .upper
                    .clone()
                    .rev()
                    .take_while(|&k| gp.upper[k].0 >= start)
                {
                    internal[gp.upper[k].0 - start][lane] = k;
                }
            }
            let mut donors = Vec::new();
            let mut offset = 0;
            for entries in updates.chunk_by(|a, b| owner[a.pivot] == owner[b.pivot]) {
                donors.push((owner[entries[0].pivot], offset..offset + entries.len()));
                offset += entries.len();
            }
            owner[start..end].fill(groups.len());
            let mut internal_lower = [[T::zero(); WIDTH]; WIDTH];
            for (lane, c) in cols.iter().enumerate() {
                let (inside, outside) = gp.lower[c.lower.clone()].split_at(WIDTH - 1 - lane);
                for (i, (_, l)) in inside.iter().enumerate() {
                    internal_lower[lane + 1 + i][lane] = l.get();
                }
                for ((_, row), (_, l)) in lower.iter_mut().zip(outside) {
                    row[lane] = l.get();
                }
            }
            let upper = updates
                .iter()
                .map(|u| {
                    (
                        gp.columns[u.pivot].pivot,
                        u.upper.map(|k| {
                            if k == ABSENT {
                                T::zero()
                            } else {
                                gp.upper[k].1.get()
                            }
                        }),
                    )
                })
                .collect();
            groups.push(Panel {
                start,
                lower,
                internal_lower,
                upper,
                donors,
                updates,
                internal,
            });
            start += WIDTH;
        }
        let work = if groups.is_empty() {
            Vec::new()
        } else {
            vec![[T::zero(); WIDTH]; gp.columns.len()]
        };
        Self { groups, work }
    }

    pub(super) fn refactor(&mut self, gp: &mut Gp<T>, values: &[T]) -> usize {
        gp.work.fill(T::zero());
        let mut start = 0;
        for k in 0..self.groups.len() {
            let (previous, next) = self.groups.split_at_mut(k);
            let group = &mut next[0];
            if let Err(bad) = gp.refactor_range(start..group.start, values) {
                return bad;
            }
            if !group.refactor(gp, values, &mut self.work, previous) {
                // Partial writes are confined to this panel. Rebuild its
                // reach and reselect pivots from the beginning of the panel.
                return group.start;
            }
            start = group.start + WIDTH;
        }
        gp.refactor_range(start..gp.columns.len(), values)
            .err()
            .unwrap_or(gp.columns.len())
    }
}
impl<T: Scalar> Panel<T> {
    pub(super) fn forward(&self, cols: &[super::FactorColumn<T::Cell>], rhs: &mut [T]) {
        let mut x = [T::zero(); WIDTH];
        for (i, c) in cols.iter().enumerate() {
            let mut v = rhs[c.pivot];
            for (&l, &u) in self.internal_lower[i].iter().zip(&x[..i]) {
                v -= l * u;
            }
            x[i] = v;
            rhs[c.pivot] = v;
        }
        for (r, l) in &self.lower {
            let mut v = rhs[*r];
            for (&l, &x) in l.iter().zip(&x) {
                v -= l * x;
            }
            rhs[*r] = v;
        }
    }
    pub(super) fn backward(
        &self,
        cols: &[super::FactorColumn<T::Cell>],
        upper: &[(usize, T::Cell)],
        rhs: &mut [T],
    ) {
        let mut x = [T::zero(); WIDTH];
        for (i, c) in cols.iter().enumerate().rev() {
            let mut v = rhs[c.pivot];
            for j in (i + 1..WIDTH).rev() {
                let k = self.internal[i][j];
                if k != ABSENT {
                    v -= upper[k].1.get() * x[j];
                }
            }
            x[i] = v / c.diagonal.get();
            rhs[c.pivot] = x[i];
        }
        for (r, u) in &self.upper {
            let mut v = rhs[*r];
            for (&u, &x) in u.iter().zip(&x).rev() {
                v -= u * x;
            }
            rhs[*r] = v;
        }
    }
    fn refactor(
        &mut self,
        gp: &mut Gp<T>,
        values: &[T],
        work: &mut [[T; WIDTH]],
        previous: &[Self],
    ) -> bool {
        let end = self.start + WIDTH;
        for (lane, &col) in gp.symbolic.order[self.start..end].iter().enumerate() {
            for &(r, k) in &gp.symbolic.input[col] {
                work[r][lane] = values[k];
            }
        }
        for (donor, range) in &self.donors {
            let updates = self.updates[range.clone()]
                .iter()
                .zip(&mut self.upper[range.clone()]);
            if *donor != ABSENT {
                let donor = &previous[*donor];
                let mut u = [[T::zero(); WIDTH]; WIDTH];
                for (k, c) in gp.columns[donor.start..donor.start + WIDTH]
                    .iter()
                    .enumerate()
                {
                    u[k] = std::mem::replace(&mut work[c.pivot], [T::zero(); WIDTH]);
                    for i in 0..k {
                        let factor = donor.internal_lower[k][i];
                        for j in 0..WIDTH {
                            u[k][j] -= factor * u[i][j];
                        }
                    }
                }
                for (update, (_, packed)) in updates {
                    let row = &u[update.pivot - donor.start];
                    *packed = *row;
                    for (&k, &x) in update.upper.iter().zip(row) {
                        if k != ABSENT {
                            if !x.is_finite_value() {
                                return false;
                            }
                            gp.upper[k].1.store_exclusive(x);
                        }
                    }
                }
                for (r, l) in &donor.lower {
                    let mut row = work[*r];
                    for (&l, u) in l.iter().zip(&u) {
                        for j in 0..WIDTH {
                            row[j] -= l * u[j];
                        }
                    }
                    work[*r] = row;
                }
                continue;
            }
            for (u, (_, packed)) in updates {
                let c = &gp.columns[u.pivot];
                let v = std::mem::replace(&mut work[c.pivot], [T::zero(); WIDTH]);
                *packed = v;
                for (&k, &v) in u.upper.iter().zip(&v) {
                    if k != ABSENT {
                        if !v.is_finite_value() {
                            return false;
                        }
                        gp.upper[k].1.store_exclusive(v);
                    }
                }
                for (r, l) in &mut gp.lower[c.lower.clone()] {
                    let l = l.load_exclusive();
                    let row = &mut work[*r];
                    for lane in 0..WIDTH {
                        row[lane] -= l * v[lane];
                    }
                }
            }
        }
        for (lane, c) in gp.columns[self.start..end].iter_mut().enumerate() {
            let pivot = std::mem::replace(&mut work[c.pivot], [T::zero(); WIDTH]);
            let diag = pivot[lane];
            if !diag.magnitude().is_normal() {
                return false;
            }
            c.diagonal.store_exclusive(diag);
            for (i, (r, l)) in gp.lower[c.lower.clone()].iter_mut().enumerate() {
                let row = &mut work[*r];
                let value = std::mem::take(&mut row[lane]);
                if !value.is_finite_value() || value.magnitude() * TOL > diag.magnitude() {
                    return false;
                }
                let factor = value / diag;
                l.store_exclusive(factor);
                if lane + 1 + i < WIDTH {
                    self.internal_lower[lane + 1 + i][lane] = factor;
                } else {
                    self.lower[i + lane + 1 - WIDTH].1[lane] = factor;
                }
                for k in lane + 1..WIDTH {
                    row[k] -= factor * pivot[k];
                }
            }
            // The upper entries internal to the panel belong to later columns.
            for (other, &v) in pivot.iter().enumerate().skip(lane + 1) {
                if !v.is_finite_value() {
                    return false;
                }
                let k = self.internal[lane][other];
                if k != ABSENT {
                    gp.upper[k].1.store_exclusive(v);
                }
            }
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::BatchOrdering;
    #[test]
    fn packed_updates_and_rejected_pivots_match_scalar_elimination() {
        let n = 65;
        let input: Vec<Vec<_>> = (0..n)
            .map(|j| (0..n).map(|r| (r, j * n + r)).collect())
            .collect();
        let order = BatchOrdering::block(input);
        let mut gp = Gp::<f64>::new(order.clone(), false);
        let mut reference = Gp::<f64>::new(order, false);
        let mut a = vec![0.; n * n];
        for j in 0..n {
            for r in 0..n {
                a[j * n + r] = if j == r {
                    10.
                } else {
                    ((r + 7 * j) % 11) as f64 * 0.125
                };
            }
        }
        gp.factor_from(0, &a).unwrap();
        reference.factor_from(0, &a).unwrap();
        let mut panels = Panels::new(&mut gp);
        assert!(panels.groups.len() >= 2);
        for j in 0..n {
            a[j * n + j] += 0.5;
        }
        assert_eq!(panels.refactor(&mut gp, &a), n);
        assert_eq!(reference.refactor(&a), n);
        for (a, b) in gp.upper.iter().zip(&reference.upper) {
            assert_eq!(a.1.get().to_bits(), b.1.get().to_bits());
        }
        let mut x: Vec<_> = (0..n).map(|j| j as f64 - 3.).collect();
        let mut y = x.clone();
        gp.solve(&mut x);
        reference.solve(&mut y);
        assert_eq!(x, y);
        let bad = gp.symbolic.order[9];
        a[bad * n + bad] = 0.;
        a[bad * n + gp.columns[15].pivot] = 1000.;
        let valid = panels.refactor(&mut gp, &a);
        assert!(valid <= 9);
        gp.factor_from(valid, &a).unwrap();
        let mut rhs: Vec<_> = (0..n)
            .map(|r| (0..n).map(|j| a[j * n + r] * j as f64).sum())
            .collect();
        gp.solve(&mut rhs);
        assert!(
            rhs.iter()
                .enumerate()
                .all(|(i, v)| (i as f64 - v).abs() < 1e-10)
        );
    }
}
