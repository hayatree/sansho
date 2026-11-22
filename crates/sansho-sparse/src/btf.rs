//! Block triangular real LU for sparse circuit Jacobians.
//!
//! Provenance: original implementation from Duff (1981), Tarjan (1972),
//! Gilbert and Peierls (1988), and the Algorithm 907 paper (2010).
//! Block ordering follows Amestoy, Davis & Duff (1996).

use crate::{BatchOrdering, Scalar, c64};
use crate::{CscPattern, SolverError};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;
use std::sync::Arc;

mod panels;
mod schedule;

const LARGE_BLOCK: usize = 2048;

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Disjoint task-owned factor entries. Relaxed loads and stores let the
/// scheduler borrow the arrays safely. Rayon joins publish completed lanes.
#[derive(Debug, Default)]
pub struct Value(AtomicU64);

pub trait FactorValue<T>: Default + std::fmt::Debug + Send + Sync {
    fn new(v: T) -> Self;
    fn store_exclusive(&mut self, v: T);
    fn load_exclusive(&mut self) -> T;
    fn get(&self) -> T;
    fn set(&self, v: T);
}

impl FactorValue<c64> for [Value; 2] {
    fn new(v: c64) -> Self {
        [Value::new(v.re), Value::new(v.im)]
    }
    fn store_exclusive(&mut self, v: c64) {
        self[0].store_exclusive(v.re);
        self[1].store_exclusive(v.im);
    }
    fn load_exclusive(&mut self) -> c64 {
        c64::new(self[0].load_exclusive(), self[1].load_exclusive())
    }
    fn get(&self) -> c64 {
        c64::new(self[0].get(), self[1].get())
    }
    fn set(&self, v: c64) {
        self[0].set(v.re);
        self[1].set(v.im);
    }
}

/// Single-precision factor entry for mixed-precision refinement.
#[derive(Debug, Default)]
pub struct Value32(AtomicU32);

impl FactorValue<f32> for Value32 {
    fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    fn store_exclusive(&mut self, v: f32) {
        *self.0.get_mut() = v.to_bits();
    }
    fn load_exclusive(&mut self) -> f32 {
        f32::from_bits(*self.0.get_mut())
    }
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

impl FactorValue<f64> for Value {
    fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    fn store_exclusive(&mut self, v: f64) {
        *self.0.get_mut() = v.to_bits();
    }
    fn load_exclusive(&mut self) -> f64 {
        f64::from_bits(*self.0.get_mut())
    }
    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

const EMPTY: usize = usize::MAX;
const TOL: f64 = 0.1;

/// Match columns to rows. The common MNA case needs only short alternating
/// paths from unmatched branch columns through the initial diagonal matching.
fn transversal(graph: &[Vec<usize>]) -> Option<Vec<usize>> {
    let n = graph.len();
    let mut owner = vec![EMPTY; n];
    for (j, rows) in graph.iter().enumerate() {
        if rows.binary_search(&j).is_ok() {
            owner[j] = j;
        }
    }
    let mut marks = vec![EMPTY; n];
    let mut stack = Vec::new();
    for start in 0..n {
        if graph[start].binary_search(&start).is_ok() {
            continue;
        }
        marks[start] = start;
        stack.clear();
        stack.push((start, 0, EMPTY));
        while let Some((col, next, _)) = stack.last_mut() {
            let Some(&row) = graph[*col].get(*next) else {
                stack.pop();
                continue;
            };
            *next += 1;
            let old = owner[row];
            if old == EMPTY {
                let mut free = row;
                while let Some((col, _, via)) = stack.pop() {
                    owner[free] = col;
                    free = via;
                }
                break;
            }
            if marks[old] != start {
                marks[old] = start;
                stack.push((old, 0, row));
            }
        }
    }
    owner.iter().all(|&v| v != EMPTY).then_some(owner)
}

/// Iterative Tarjan traversal. Components leave the active stack sink first.
fn components(graph: &[Vec<usize>], owner: &[usize]) -> Vec<Vec<usize>> {
    let n = graph.len();
    let mut index = vec![EMPTY; n];
    let mut low = vec![0; n];
    let mut active = vec![false; n];
    let mut pending = Vec::new();
    let mut path = Vec::new();
    let mut blocks = Vec::new();
    let mut tick = 0;
    for root in 0..n {
        if index[root] != EMPTY {
            continue;
        }
        path.push((root, 0));
        while let Some((v, next)) = path.last_mut() {
            let v = *v;
            if *next == 0 {
                index[v] = tick;
                low[v] = tick;
                tick += 1;
                active[v] = true;
                pending.push(v);
            }
            if let Some(&row) = graph[v].get(*next) {
                *next += 1;
                let w = owner[row];
                if index[w] == EMPTY {
                    path.push((w, 0));
                } else if active[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                path.pop();
                if let Some(&(parent, _)) = path.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut block = Vec::new();
                    while let Some(w) = pending.pop() {
                        active[w] = false;
                        block.push(w);
                        if w == v {
                            break;
                        }
                    }
                    block.sort_unstable();
                    blocks.push(block);
                }
            }
        }
    }
    blocks.reverse();
    blocks
}

#[derive(Debug)]
struct Block {
    cols: Vec<usize>,
    rows: Vec<usize>,
    lu: Gp,
    outgoing: Vec<Coupling>,
}

#[derive(Debug)]
struct Coupling {
    row: usize,
    col: usize,
    source: usize,
    value: f64,
}

#[derive(Debug)]
pub(crate) struct Btf {
    blocks: Vec<Block>,
    omitted: Vec<usize>,
    nnz: usize,
    factored: bool,
    solution: Vec<f64>,
    rhs: Vec<f64>,
}

impl Btf {
    pub(crate) fn new(p: &CscPattern, force: bool) -> Result<Option<Self>, SolverError> {
        let n = p.dim();
        let mut graph = Vec::with_capacity(n);
        for j in 0..n {
            graph.push(p.row_idx()[p.col_ptr()[j]..p.col_ptr()[j + 1]].to_vec());
        }
        // PatternBuilder includes diagonals even for equations that never
        // stamp them (e.g. ideal source branch currents).
        for &j in &p.implicit_diagonals {
            graph[j].retain(|&r| r != j);
        }
        let Some(owner) = transversal(&graph) else {
            return Ok(None);
        };
        let parts = components(&graph, &owner);
        let largest = parts.iter().map(Vec::len).max().unwrap_or(0);
        // A grid with only a low-degree supply branch gains little from BTF.
        // Factor the complete pattern in that case.
        let outside: usize = parts
            .iter()
            .filter(|b| b.len() < largest)
            .flatten()
            .map(|&c| graph[c].len())
            .sum();
        if !force && (parts.len() < 2 || (largest > n / 2 && outside < p.nnz() / 50)) {
            return Ok(None);
        }
        if std::env::var("SANSHO_PROFILE").as_deref() == Ok("1") {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-btf {} {largest} {n}",
                parts.len()
            );
        }
        let mut row_of = vec![0; n];
        for (row, &col) in owner.iter().enumerate() {
            row_of[col] = row;
        }
        let mut part = vec![0; n];
        let mut local = vec![0; n];
        for (b, cols) in parts.iter().enumerate() {
            for (k, &col) in cols.iter().enumerate() {
                part[col] = b;
                local[col] = k;
            }
        }
        let mut blocks = Vec::with_capacity(parts.len());
        let mut omitted = Vec::new();
        for (b, cols) in parts.into_iter().enumerate() {
            let mut outgoing = Vec::new();
            let mut input = Vec::with_capacity(cols.len());
            for &col in &cols {
                let mut entries = Vec::new();
                for k in p.col_ptr()[col]..p.col_ptr()[col + 1] {
                    let r = p.row_idx()[k];
                    let c = owner[r];
                    match part[c].cmp(&b) {
                        std::cmp::Ordering::Equal => entries.push((local[c], k)),
                        std::cmp::Ordering::Greater => outgoing.push(Coupling {
                            row: r,
                            col,
                            source: k,
                            value: 0.,
                        }),
                        std::cmp::Ordering::Less => omitted.push(k),
                    }
                }
                input.push(entries);
            }
            let rows = cols.iter().map(|&c| row_of[c]).collect();
            blocks.push(Block {
                cols,
                rows,
                outgoing,
                lu: Gp::new(BatchOrdering::block(input), true),
            });
        }
        Ok(Some(Self {
            blocks,
            omitted,
            nnz: p.nnz(),
            factored: false,
            solution: vec![0.; n],
            rhs: vec![0.; largest],
        }))
    }

    pub(crate) fn supports(&self, values: &[f64]) -> bool {
        values.len() == self.nnz && self.omitted.iter().all(|&k| values[k] == 0.)
    }

    pub(crate) fn factor(&mut self, values: &[f64]) -> Result<bool, SolverError> {
        if values.iter().any(|v| !v.is_finite()) {
            return Err(SolverError::Backend("non-finite matrix entry".into()));
        }
        let mut reused = true;
        for b in &mut self.blocks {
            let valid = b.lu.refactor(values);
            if valid < b.cols.len() {
                b.lu.factor_from(valid, values)
                    .map_err(|col| SolverError::Singular {
                        column: Some(b.cols[col]),
                    })?;
                reused = false;
            }
            for entry in &mut b.outgoing {
                entry.value = values[entry.source];
            }
        }
        if !self.factored && std::env::var_os("SANSHO_PROFILE_MATRIX").is_some() {
            let fill: usize = self.blocks.iter().map(|b| b.lu.nnz()).sum();
            use std::io::Write;
            let coupling: usize = self.blocks.iter().map(|b| b.outgoing.len()).sum();
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-matrix A={} LU={fill} coupling={coupling}",
                self.nnz
            );
        }
        self.factored = true;
        Ok(reused)
    }

    pub(crate) fn solve(&mut self, rhs: &mut [f64]) -> Result<(), SolverError> {
        for b in &mut self.blocks {
            let local = &mut self.rhs[..b.cols.len()];
            for (x, &r) in local.iter_mut().zip(&b.rows) {
                *x = rhs[r];
            }
            b.lu.solve(local);
            for (&c, &v) in b.cols.iter().zip(local.iter()) {
                self.solution[c] = v;
            }
            for entry in &b.outgoing {
                rhs[entry.row] -= entry.value * self.solution[entry.col];
            }
        }
        rhs.copy_from_slice(&self.solution);
        if rhs.iter().all(|x| x.is_finite()) {
            Ok(())
        } else {
            Err(SolverError::Singular { column: None })
        }
    }
}

#[derive(Debug, Default)]
struct FactorColumn<C = Value> {
    pivot: usize,
    diagonal: C,
    lower: Range<usize>,
    upper: Range<usize>,
}

#[derive(Debug)]
pub(crate) struct Gp<T: Scalar = f64> {
    pub(crate) symbolic: Arc<BatchOrdering>,
    columns: Vec<FactorColumn<T::Cell>>,
    lower: Vec<(usize, T::Cell)>,
    upper: Vec<(usize, T::Cell)>,
    schedule: schedule::Schedule<T>,
    panels: Option<panels::Panels<T>>,
    workers: usize,
    work: Vec<T>,
    pivot_at: Vec<usize>,
    seen: Vec<usize>,
    queue: BinaryHeap<Reverse<usize>>,
    candidates: Vec<usize>,
    traces: Vec<Reach>,
    attempts: usize,
    failures: usize,
    cache_reach: bool,
}

/// Structural reach of a fresh search. An unchanged predecessor supplies
/// the same pivot and the same lower rows in discovery order.
#[derive(Debug, Default)]
struct Reach {
    parents: Vec<(usize, usize)>,
    candidates: Vec<usize>,
    pivot: Option<usize>,
    version: usize,
}

impl<T: Scalar> Gp<T> {
    pub(crate) fn new(symbolic: Arc<BatchOrdering>, parallel: bool) -> Self {
        let n = symbolic.dimension();
        Self {
            symbolic,
            columns: Vec::with_capacity(n),
            upper: vec![],
            schedule: Default::default(),
            panels: None,
            workers: if parallel && n >= LARGE_BLOCK && rayon::current_thread_index().is_some() {
                rayon::current_num_threads()
            } else {
                1
            },
            lower: vec![],
            work: vec![T::zero(); n],
            pivot_at: vec![EMPTY; n],
            seen: vec![EMPTY; n],
            queue: BinaryHeap::new(),
            candidates: Vec::new(),
            traces: Vec::new(),
            attempts: 0,
            failures: 0,
            cache_reach: false,
        }
    }

    // Scatter each column, then visit only reachable earlier pivots in
    // elimination order. Store L using original row labels so pivot changes
    // never require relabelling already factored columns.
    pub(crate) fn factor_from(&mut self, valid: usize, values: &[T]) -> Result<(), usize> {
        self.panels = None;
        // A rejected pivot invalidates its own column and its successors.
        // Earlier accepted pivots and updates already factor the new matrix.
        let first = self.columns.is_empty() && self.symbolic.order.len() >= 256;
        if self.symbolic.order.len() >= LARGE_BLOCK {
            self.failures += 1;
            if self.cache_reach && self.traces.is_empty() {
                self.traces
                    .resize_with(self.symbolic.order.len(), Reach::default);
            }
        }
        self.columns.truncate(valid);
        self.lower
            .truncate(self.columns.last().map_or(0, |c| c.lower.end));
        self.upper
            .truncate(self.columns.last().map_or(0, |c| c.upper.end));
        let pivot_at = &mut self.pivot_at;
        pivot_at.fill(EMPTY);
        for (j, c) in self.columns.iter().enumerate() {
            pivot_at[c.pivot] = j;
        }
        let seen = &mut self.seen;
        seen.fill(EMPTY);
        let queue = &mut self.queue;
        queue.clear();
        let candidates = &mut self.candidates;
        for (j, &col) in self.symbolic.order.iter().enumerate().skip(valid) {
            candidates.clear();
            let u = self.upper.len();
            let cached = self.traces.get(j).is_some_and(|t| {
                t.pivot.is_some()
                    && t.parents.iter().all(|&(i, v)| self.traces[i].version == v)
                    && t.candidates.iter().all(|&r| pivot_at[r] == EMPTY)
            });
            if cached {
                let t = &self.traces[j];
                candidates.extend_from_slice(&t.candidates);
                for &r in candidates.iter() {
                    self.work[r] = T::zero();
                    seen[r] = j;
                }
                for &(i, _) in &t.parents {
                    self.work[self.columns[i].pivot] = T::zero();
                }
                for &(r, k) in &self.symbolic.input[col] {
                    self.work[r] = values[k];
                }
                for &(i, _) in &t.parents {
                    let prev = &self.columns[i];
                    let v = self.work[prev.pivot];
                    if !v.is_finite_value() {
                        return Err(col);
                    }
                    self.upper.push((i, T::Cell::new(v)));
                    for (r, l) in &self.lower[prev.lower.clone()] {
                        self.work[*r] -= l.get() * v;
                    }
                }
            } else {
                let mut visit =
                    |r: usize, queue: &mut BinaryHeap<Reverse<usize>>, rows: &mut Vec<usize>| {
                        if seen[r] != j {
                            seen[r] = j;
                            if pivot_at[r] == EMPTY {
                                rows.push(r);
                            } else {
                                queue.push(Reverse(pivot_at[r]));
                            }
                            true
                        } else {
                            false
                        }
                    };
                for &(r, k) in &self.symbolic.input[col] {
                    visit(r, queue, candidates);
                    self.work[r] = values[k];
                }
                while let Some(Reverse(i)) = queue.pop() {
                    let prev = &self.columns[i];
                    let v = self.work[prev.pivot];
                    if !v.is_finite_value() {
                        return Err(col);
                    }
                    self.upper.push((i, T::Cell::new(v)));
                    for (r, l) in &self.lower[prev.lower.clone()] {
                        let r = *r;
                        if visit(r, queue, candidates) {
                            self.work[r] = T::zero();
                        }
                        self.work[r] -= l.get() * v;
                    }
                }
            }
            let best = candidates
                .iter()
                .copied()
                .max_by_key(|&r| self.work[r].magnitude().to_bits())
                .ok_or(col)?;
            let max = self.work[best].magnitude();
            if !max.is_normal() {
                return Err(col);
            }
            let pivot = if pivot_at[col] == EMPTY
                && seen[col] == j
                && self.work[col].magnitude() >= TOL * max
            {
                col
            } else {
                best
            };
            let diagonal = self.work[pivot];
            let l = self.lower.len();
            for &r in candidates.iter() {
                if r != pivot {
                    self.lower.push((r, T::Cell::new(self.work[r] / diagonal)));
                }
            }
            if !self.traces.is_empty() {
                let (prior, rest) = self.traces.split_at_mut(j);
                let t = &mut rest[0];
                if !cached {
                    t.parents.clear();
                    t.parents
                        .extend(self.upper[u..].iter().map(|(i, _)| (*i, prior[*i].version)));
                }
                if t.pivot != Some(pivot) || (!cached && t.candidates != *candidates) {
                    t.pivot = Some(pivot);
                    t.version += 1;
                    if !cached {
                        t.candidates.clone_from(candidates);
                    }
                }
            }
            pivot_at[pivot] = j;
            self.columns.push(FactorColumn {
                pivot,
                diagonal: T::Cell::new(diagonal),
                lower: l..self.lower.len(),
                upper: u..self.upper.len(),
            });
        }
        if first && std::env::var("SANSHO_PROFILE_MATRIX").as_deref() == Ok("shape") {
            let n = self.columns.len();
            let tail = 64;
            let start = n - tail;
            let mut nz = tail;
            for c in &self.columns[start..] {
                nz += self.lower[c.lower.clone()]
                    .iter()
                    .filter(|(r, _)| self.pivot_at[*r] >= start)
                    .count();
                nz += self.upper[c.upper.clone()]
                    .iter()
                    .filter(|(r, _)| *r >= start)
                    .count();
            }
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-block-shape n={n} factors={} tail={tail} tail_entries={nz}",
                n + self.lower.len() + self.upper.len()
            );
        }
        if self.columns.len() >= 16_384 && self.lower.len() > 16 * self.columns.len() {
            self.panels = Some(panels::Panels::new(self));
        }
        if self.panels.is_none()
            && self.workers > 1
            && (self.schedule.len() == 0 || !self.schedule.valid(valid, &self.columns, &self.upper))
        {
            self.schedule
                .build(&self.columns, &self.upper, self.workers);
        }
        Ok(())
    }

    pub(crate) fn refactor(&mut self, values: &[T]) -> usize {
        if self.symbolic.order.len() >= LARGE_BLOCK {
            // Repivot-heavy blocks amortize the additional symbolic records.
            if self.attempts == 128 {
                self.cache_reach = self.failures >= 8;
                if !self.cache_reach {
                    self.traces = Vec::new();
                }
                self.attempts = 0;
                self.failures = 0;
            }
            self.attempts += 1;
        }
        if self.columns.len() != self.symbolic.order.len() {
            return 0;
        }
        if let Some(mut panels) = self.panels.take() {
            let valid = panels.refactor(self, values);
            self.panels = Some(panels);
            return valid;
        }
        let end = self.schedule.len();
        if end == 0 || rayon::current_thread_index().is_none() || rayon::current_num_threads() == 1
        {
            return self.serial_refactor(0, values);
        }
        let columns = &self.columns;
        let lower = &self.lower;
        let upper = &self.upper;
        let input = &self.symbolic.input;
        let order = &self.symbolic.order;
        let factor = |j: usize, work: &mut [T]| {
            let c = &columns[j];
            for &(r, k) in &input[order[j]] {
                work[r] = values[k];
            }
            for (i, u) in &upper[c.upper.clone()] {
                let prev = &columns[*i];
                let v = std::mem::take(&mut work[prev.pivot]);
                if !v.is_finite_value() {
                    return false;
                }
                u.set(v);
                if v != T::zero() {
                    for (r, l) in &lower[prev.lower.clone()] {
                        work[*r] -= l.get() * v;
                    }
                }
            }
            let diag = std::mem::take(&mut work[c.pivot]);
            if !diag.magnitude().is_normal() {
                return false;
            }
            for (r, l) in &lower[c.lower.clone()] {
                let value = std::mem::take(&mut work[*r]);
                if !value.is_finite_value() || value.magnitude() * TOL > diag.magnitude() {
                    return false;
                }
                l.set(value / diag);
            }
            c.diagonal.set(diag);
            true
        };
        let first = self.schedule.run(factor);
        if first < end {
            return first;
        }
        self.serial_refactor(end, values)
    }

    // The serial suffix has exclusive ownership. Borrow the cells mutably so
    // LLVM can use ordinary loads and stores throughout the elimination.
    fn serial_refactor(&mut self, start: usize, values: &[T]) -> usize {
        self.work.fill(T::zero());
        self.refactor_range(start..self.columns.len(), values)
            .err()
            .unwrap_or(self.columns.len())
    }

    fn refactor_range(&mut self, range: Range<usize>, values: &[T]) -> Result<(), usize> {
        for j in range {
            let col = self.symbolic.order[j];
            let c = &self.columns[j];
            for &(r, k) in &self.symbolic.input[col] {
                self.work[r] = values[k];
            }
            for (i, u) in &mut self.upper[c.upper.clone()] {
                let prev = &self.columns[*i];
                let v = std::mem::take(&mut self.work[prev.pivot]);
                if !v.is_finite_value() {
                    return Err(j);
                }
                u.store_exclusive(v);
                if v != T::zero() {
                    for (r, l) in &mut self.lower[prev.lower.clone()] {
                        self.work[*r] -= l.load_exclusive() * v;
                    }
                }
            }
            let diag = std::mem::take(&mut self.work[c.pivot]);
            if !diag.magnitude().is_normal() {
                return Err(j);
            }
            for (r, l) in &mut self.lower[c.lower.clone()] {
                let v = std::mem::take(&mut self.work[*r]);
                if !v.is_finite_value() || v.magnitude() * TOL > diag.magnitude() {
                    return Err(j);
                }
                l.store_exclusive(v / diag);
            }
            self.columns[j].diagonal.store_exclusive(diag);
        }
        Ok(())
    }
    pub(crate) fn nnz(&self) -> usize {
        self.columns.len() + self.lower.len() + self.upper.len()
    }

    pub(crate) fn solve_transpose(&mut self, rhs: &mut [T]) {
        for (c, &col) in self.columns.iter().zip(&self.symbolic.order) {
            let mut v = rhs[col];
            for (i, u) in &self.upper[c.upper.clone()] {
                v -= u.get() * self.work[self.columns[*i].pivot];
            }
            self.work[c.pivot] = v / c.diagonal.get();
        }
        for c in self.columns.iter().rev() {
            let mut v = self.work[c.pivot];
            for (r, l) in &self.lower[c.lower.clone()] {
                v -= l.get() * self.work[*r];
            }
            self.work[c.pivot] = v;
        }
        rhs.copy_from_slice(&self.work);
    }
    pub(crate) fn solve(&mut self, rhs: &mut [T]) {
        Self::triangular(
            &self.columns,
            &self.lower,
            &self.upper,
            &self.symbolic.order,
            self.panels.as_ref().map_or(&[], |p| &p.groups),
            rhs,
            &mut self.work,
        );
    }
    pub(crate) fn solve_shared(&self, rhs: &mut [T]) {
        let mut work = vec![T::zero(); rhs.len()];
        Self::triangular(
            &self.columns,
            &self.lower,
            &self.upper,
            &self.symbolic.order,
            self.panels.as_ref().map_or(&[], |p| &p.groups),
            rhs,
            &mut work,
        );
    }
    fn triangular(
        columns: &[FactorColumn<T::Cell>],
        lower: &[(usize, T::Cell)],
        upper: &[(usize, T::Cell)],
        order: &[usize],
        groups: &[panels::Panel<T>],
        rhs: &mut [T],
        work: &mut [T],
    ) {
        let mut start = 0;
        for panel in groups {
            forward(&columns[start..panel.start], lower, rhs);
            start = panel.start + panels::WIDTH;
            panel.forward(&columns[panel.start..start], rhs);
        }
        forward(&columns[start..], lower, rhs);
        let mut end = columns.len();
        for panel in groups.iter().rev() {
            let start = panel.start + panels::WIDTH;
            backward(columns, upper, order, start..end, rhs, work);
            panel.backward(&columns[panel.start..start], upper, rhs);
            for (c, &col) in columns[panel.start..start]
                .iter()
                .zip(&order[panel.start..start])
            {
                work[col] = rhs[c.pivot];
            }
            end = panel.start;
        }
        backward(columns, upper, order, 0..end, rhs, work);
        rhs.copy_from_slice(work);
    }
}

fn forward<T: Scalar>(cols: &[FactorColumn<T::Cell>], lower: &[(usize, T::Cell)], rhs: &mut [T]) {
    for c in cols {
        let x = rhs[c.pivot];
        if x != T::zero() {
            for (r, l) in &lower[c.lower.clone()] {
                rhs[*r] -= l.get() * x;
            }
        }
    }
}
fn backward<T: Scalar>(
    cols: &[FactorColumn<T::Cell>],
    upper: &[(usize, T::Cell)],
    order: &[usize],
    range: Range<usize>,
    rhs: &mut [T],
    work: &mut [T],
) {
    for (c, &col) in cols[range.clone()].iter().zip(&order[range]).rev() {
        let x = rhs[c.pivot] / c.diagonal.get();
        work[col] = x;
        for (i, u) in &upper[c.upper.clone()] {
            rhs[cols[*i].pivot] -= u.get() * x;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PatternBuilder;

    #[test]
    fn parallel_refactor_matches_every_serial_factor_and_solve_bit() {
        // A resistively coupled ladder and a sparse random block-banded matrix.
        // The cyclic strong entries force row exchanges through previously
        // independent subtrees; signed zeros exercise the zero-update skip.
        let n = 2048;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        for random in [false, true] {
            let mut rng = 31u32;
            let mut next = || {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                rng
            };
            let mut input = Vec::new();
            let mut count = 0;
            for j in 0..n {
                let mut rows = vec![j, j ^ 1];
                if j % 32 != 31 {
                    rows.push(j + 1);
                } else {
                    rows.push(n - 1);
                }
                if j % 32 != 0 {
                    rows.push(j - 1);
                }
                if j == n - 1 {
                    rows.extend((31..n).step_by(32));
                }
                if random {
                    for _ in 0..3 {
                        rows.push((j / 16) * 16 + next() as usize % 16);
                    }
                }
                rows.sort_unstable();
                rows.dedup();
                input.push(
                    rows.into_iter()
                        .map(|r| {
                            count += 1;
                            (r, count - 1)
                        })
                        .collect(),
                );
            }
            let mut serial = Gp::new(BatchOrdering::block(input.clone()), false);
            // Compare with a fresh reach traversal as well as serial refactors.
            let mut parallel = pool.install(|| Gp::new(BatchOrdering::block(input.clone()), true));
            parallel.cache_reach = true;
            let mut values = vec![0.; count];
            let mut refactored = false;
            let mut repivoted = false;
            for step in 0..8 {
                for (j, col) in input.iter().enumerate() {
                    for &(r, k) in col {
                        let exchange = (2..5).contains(&step) && j % 64 < 32;
                        let pivot = if exchange { j ^ 1 } else { j };
                        values[k] = if r == pivot {
                            8.
                        } else if (j + k + step) % 5 == 0 {
                            -0.
                        } else {
                            ((next() % 100) as f64 - 49.) / 400.
                        };
                    }
                }
                let first = serial.refactor(&values);
                let other = pool.install(|| parallel.refactor(&values));
                assert_eq!(first, other);
                refactored |= first == n;
                repivoted |= step > 0 && first < n;
                if first < n {
                    serial.factor_from(first, &values).unwrap();
                    pool.install(|| parallel.factor_from(other, &values))
                        .unwrap();
                }
                assert!(parallel.schedule.len() > 0);
                for (a, b) in serial.columns.iter().zip(&parallel.columns) {
                    assert_eq!(
                        (a.pivot, a.diagonal.get().to_bits(), &a.lower, &a.upper),
                        (b.pivot, b.diagonal.get().to_bits(), &b.lower, &b.upper)
                    );
                }
                let entries = |factor: &[(usize, Value)]| {
                    factor
                        .iter()
                        .map(|(i, v)| (*i, v.get().to_bits()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(entries(&serial.lower), entries(&parallel.lower));
                assert_eq!(entries(&serial.upper), entries(&parallel.upper));
                let mut rhs: Vec<_> = (0..n).map(|i| (i % 23) as f64 - 11.).collect();
                let mut other = rhs.clone();
                serial.solve(&mut rhs);
                parallel.solve(&mut other);
                assert!(
                    rhs.iter()
                        .zip(other)
                        .all(|(x, y)| x.to_bits() == y.to_bits())
                );
            }
            assert!(refactored && repivoted);
        }
    }

    #[test]
    fn alternating_path_and_disconnected_singular_graph() {
        // Column 0 must move off its diagonal to make room for column 2.
        let graph = vec![vec![0, 1], vec![1, 2], vec![0]];
        let owner = transversal(&graph).unwrap();
        assert_eq!(owner, [2, 0, 1]);
        assert!(transversal(&[vec![0], vec![0]]).is_none());
    }

    #[test]
    fn block_solve_refactor_and_numeric_repivot() {
        // The final equation constrains the first voltage, as in a grounded
        // source. Two strongly connected pairs have a one-way coupling, and
        // the supply KCL row collects currents from both.
        let mut builder = PatternBuilder::new(6);
        for (r, c) in [(0, 5), (5, 0), (0, 0), (0, 2), (0, 4), (1, 0), (3, 2)] {
            builder.add(r, c);
        }
        for base in [1, 3] {
            for r in base..base + 2 {
                for c in base..base + 2 {
                    builder.add(r, c);
                }
            }
        }
        let (pattern, _) = builder.build();
        let mut btf = Btf::new(&pattern, true).unwrap().unwrap();
        assert_eq!(btf.blocks.len(), 4);
        let expected = [2., -0.25, 1.5, 0.75, -0.8, 0.125];
        let mut values = vec![0.; pattern.nnz()];
        for (k, v) in values.iter_mut().enumerate() {
            *v = 0.1 + (k + 1) as f64 / 23.;
        }
        values[pattern.diag(5).unwrap()] = 0.;
        let change = pattern.diag(3).unwrap();
        for (diag, reused) in [(3., false), (2.5, true), (1e-12, false), (1e-12, true)] {
            values[change] = diag;
            let mut rhs = [0.; 6];
            pattern.mul_vec(&values, &expected, &mut rhs);
            assert_eq!(btf.factor(&values).unwrap(), reused);
            btf.solve(&mut rhs).unwrap();
            for (x, y) in rhs.iter().zip(expected) {
                assert!((x - y).abs() < 1e-12);
            }
        }
        values[pattern.diag(5).unwrap()] = 0.5;
        assert!(!btf.supports(&values));
        values.fill(0.);
        assert!(btf.factor(&values).is_err());
    }

    #[test]
    fn reordered_block_preserves_fill_across_pivot_changes() {
        let n = 29;
        let mut builder = PatternBuilder::new(n);
        for col in 0..n {
            for offset in [0, 1, 4, 11] {
                builder.add((col + offset) % n, col);
            }
        }
        let (pattern, _) = builder.build();
        let mut btf = Btf::new(&pattern, true).unwrap().unwrap();
        assert_eq!(btf.blocks.len(), 1);
        let solution: Vec<_> = (0..n).map(|i| (i % 9) as f64 - 4.5).collect();
        let mut values = vec![0.; pattern.nnz()];
        // Changing the dominant cyclic permutation forces row pivots to move.
        // Each matrix is a scaled permutation plus a bounded perturbation.
        for offset in [0, 4, 11, 1, 0] {
            for col in 0..n {
                let dominant = (col + offset) % n;
                let entries = pattern.col_ptr()[col]..pattern.col_ptr()[col + 1];
                for (value, &row) in values[entries.clone()]
                    .iter_mut()
                    .zip(&pattern.row_idx()[entries])
                {
                    *value = if row == dominant {
                        16.
                    } else {
                        (row % 7) as f64 / 20. - 0.15
                    };
                }
            }
            btf.factor(&values).unwrap();
            assert!(btf.factor(&values).unwrap());
            let mut rhs = vec![0.; n];
            pattern.mul_vec(&values, &solution, &mut rhs);
            btf.solve(&mut rhs).unwrap();
            for (computed, expected) in rhs.iter().zip(&solution) {
                assert!((computed - expected).abs() < 1e-12);
            }
        }
    }
}
