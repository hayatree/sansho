//! Fixed sparsity patterns in compressed sparse column (CSC) form.
//!
//! Provenance: original work (standard CSC layout). See `docs/PROVENANCE.md`.

/// Collects matrix entry positions (duplicates allowed) and builds a [`CscPattern`].
#[derive(Debug, Clone)]
pub struct PatternBuilder {
    n: usize,
    entries: Vec<(usize, usize)>,
}

impl PatternBuilder {
    /// Create a builder for an `n × n` matrix. The diagonal is always included so that
    /// diagonal regularization (e.g. gmin) never changes the pattern.
    pub fn new(n: usize) -> Self {
        Self {
            n,
            entries: Vec::new(),
        }
    }

    /// Matrix dimension.
    pub fn dim(&self) -> usize {
        self.n
    }

    /// Register an entry at `(row, col)` and return its request id. Ids are dense and assigned
    /// in call order.
    pub fn add(&mut self, row: usize, col: usize) -> usize {
        self.entries.push((row, col));
        self.entries.len() - 1
    }

    /// Build the pattern. Returns the pattern and, for each request id, the index into the
    /// value array of the corresponding (deduplicated) entry.
    pub fn build(self) -> (CscPattern, Vec<usize>) {
        let n = self.n;
        let mut all: Vec<(usize, usize)> = self
            .entries
            .iter()
            .copied()
            .chain((0..n).map(|i| (i, i)))
            .collect();
        // Column-major order.
        all.sort_unstable_by_key(|&(r, c)| (c, r));
        all.dedup();

        let mut col_ptr = vec![0usize; n + 1];
        let mut row_idx = Vec::with_capacity(all.len());
        for &(r, c) in &all {
            col_ptr[c + 1] += 1;
            row_idx.push(r);
        }
        for c in 0..n {
            col_ptr[c + 1] += col_ptr[c];
        }
        let mut implicit = vec![true; n];
        for &(r, c) in &self.entries {
            if r == c {
                implicit[r] = false;
            }
        }
        let pattern = CscPattern {
            implicit_diagonals: (0..n).filter(|&j| implicit[j]).collect(),
            n,
            col_ptr,
            row_idx,
        };
        let map = self
            .entries
            .iter()
            .map(|&(r, c)| pattern.find(r, c).unwrap_or(usize::MAX))
            .collect();
        (pattern, map)
    }
}

/// An immutable `n × n` CSC sparsity pattern with sorted row indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CscPattern {
    pub(crate) implicit_diagonals: Vec<usize>,
    n: usize,
    col_ptr: Vec<usize>,
    row_idx: Vec<usize>,
}

impl CscPattern {
    /// Matrix dimension.
    pub fn dim(&self) -> usize {
        self.n
    }

    /// Number of stored entries.
    pub fn nnz(&self) -> usize {
        self.row_idx.len()
    }

    /// Column pointers (length `n + 1`).
    pub fn col_ptr(&self) -> &[usize] {
        &self.col_ptr
    }

    /// Row indices (length `nnz`), sorted within each column.
    pub fn row_idx(&self) -> &[usize] {
        &self.row_idx
    }

    /// Value index of entry `(row, col)`, if present.
    pub fn find(&self, row: usize, col: usize) -> Option<usize> {
        let start = *self.col_ptr.get(col)?;
        let end = *self.col_ptr.get(col + 1)?;
        self.row_idx[start..end]
            .binary_search(&row)
            .ok()
            .map(|k| start + k)
    }

    /// Value index of diagonal entry `i` (always present).
    pub fn diag(&self, i: usize) -> Option<usize> {
        self.find(i, i)
    }

    /// Compute `y = A x` for values in pattern order.
    pub fn mul_vec(&self, values: &[f64], x: &[f64], y: &mut [f64]) {
        y.iter_mut().for_each(|v| *v = 0.0);
        for c in 0..self.n {
            for k in self.col_ptr[c]..self.col_ptr[c + 1] {
                y[self.row_idx[k]] += values[k] * x[c];
            }
        }
    }
}
