//! Independent subtrees of a fixed-pivot column dependency graph.
//! Provenance: original implementation using the elimination forest of Liu (1990)
//! and the level scheduling principle of Anderson and Saad (1989).

use super::FactorColumn;
use crate::Scalar;
use rayon::prelude::*;

/// A prefix of the factorization has disjoint connected components in the
/// column DAG. Batch whole components into lanes, followed by a serial suffix.
#[derive(Debug, Default)]
pub(super) struct Schedule<T> {
    lanes: Vec<Lane<T>>,
    owner: Vec<usize>,
}

#[derive(Debug)]
struct Lane<T> {
    columns: Vec<usize>,
    work: Vec<T>,
}

fn root(parent: &mut [usize], mut j: usize) -> usize {
    while parent[j] != j {
        parent[j] = parent[parent[j]];
        j = parent[j];
    }
    j
}

impl<T: Scalar> Schedule<T> {
    pub(super) fn len(&self) -> usize {
        self.owner.len()
    }

    /// Check just the portion reconstructed by fresh pivot search. Numeric
    /// coefficients may change freely, but every edge must still be local.
    pub(super) fn valid(
        &self,
        start: usize,
        cols: &[FactorColumn<T::Cell>],
        u: &[(usize, T::Cell)],
    ) -> bool {
        (start..self.len()).all(|j| {
            u[cols[j].upper.clone()]
                .iter()
                .all(|(i, _)| self.owner[*i] == self.owner[j])
        })
    }

    pub(super) fn build(
        &mut self,
        cols: &[FactorColumn<T::Cell>],
        u: &[(usize, T::Cell)],
        workers: usize,
    ) {
        let n = cols.len();
        let mut parent: Vec<_> = (0..n).collect();
        let mut sizes = vec![1; n];
        // Bound the number of simultaneous writers of interleaved factor
        // columns. Each lane keeps one scatter vector for this block.
        let lanes = workers.min(12);
        let limit = (n / lanes.min(8)).max(64);
        let mut end = n;
        let mut marks = vec![usize::MAX; n];
        let mut children = Vec::new();
        for (j, c) in cols.iter().enumerate() {
            children.clear();
            for &(i, _) in &u[c.upper.clone()] {
                let child = root(&mut parent, i);
                if marks[child] != j {
                    marks[child] = j;
                    sizes[j] += sizes[child];
                    children.push(child);
                }
            }
            if sizes[j] > limit {
                end = j;
                break;
            }
            for &child in &children {
                parent[child] = j;
            }
        }
        let mut cost = vec![0; end];
        for (j, c) in cols[..end].iter().enumerate() {
            let r = root(&mut parent, j);
            cost[r] += 1
                + c.upper.len()
                + c.lower.len()
                + u[c.upper.clone()]
                    .iter()
                    .map(|(i, _)| cols[*i].lower.len())
                    .sum::<usize>();
        }
        let mut roots: Vec<_> = (0..end).filter(|&j| cost[j] > 0).collect();
        roots.sort_unstable_by_key(|&j| std::cmp::Reverse(cost[j]));
        let count = roots.len().min(lanes);
        if count < 2 || end < 256 {
            self.owner.clear();
            self.lanes.clear();
            return;
        }
        self.lanes.resize_with(count, || Lane {
            columns: Vec::new(),
            work: vec![T::zero(); n],
        });
        for lane in &mut self.lanes {
            lane.columns.clear();
        }
        self.owner.resize(end, 0);
        let mut loads = vec![0; count];
        for r in roots {
            if let Some((lane, load)) = loads.iter_mut().enumerate().min_by_key(|(_, v)| **v) {
                self.owner[r] = lane;
                *load += cost[r];
            }
        }
        for j in 0..end {
            let lane = self.owner[root(&mut parent, j)];
            self.owner[j] = lane;
            self.lanes[lane].columns.push(j);
        }
    }

    pub(super) fn run(&mut self, factor: impl Fn(usize, &mut [T]) -> bool + Sync) -> usize {
        let end = self.len();
        self.lanes
            .par_iter_mut()
            .map(|lane| {
                lane.work.fill(T::zero());
                lane.columns
                    .iter()
                    .copied()
                    .find(|&j| !factor(j, &mut lane.work))
                    .unwrap_or(end)
            })
            .min()
            .unwrap_or(end)
    }
}
