//! Scratch-backed forward-mode differentiation for the general evaluator.
//! Provenance: original implementation from the chain rule and general AD literature.

/// Each live tangent owns an immutable row for the duration of the evaluation.
/// Constants (including values retained from a previous call) need no row.
#[derive(Debug, Default)]
pub(crate) struct Gradients {
    pub width: usize,
    used: usize,
    floor: usize,
    collect_after: usize,
    retained: Vec<f64>,
    data: Vec<f64>,
}

impl Gradients {
    pub fn reset(&mut self, width: usize) {
        self.width = width;
        self.used = 0;
        self.floor = 0;
        self.collect_after = Self::COLLECT_INTERVAL;
    }

    const COLLECT_INTERVAL: usize = 262144;

    // Rows belonging to suspended callers include expression temporaries.
    // Keep their offsets stable when reclaiming storage inside a function.
    pub fn enter(&mut self) -> usize {
        std::mem::replace(&mut self.floor, self.used)
    }
    pub fn leave(&mut self, floor: usize) {
        self.floor = floor;
    }
    pub fn needs_collection(&self) -> bool {
        self.used > self.collect_after && self.used - self.floor > Self::COLLECT_INTERVAL
    }
    pub fn retain(&mut self, dual: &mut Dual) {
        if let Some(r) = dual.row
            && r >= self.floor
        {
            dual.row = Some(self.floor + self.retained.len());
            self.retained
                .extend_from_slice(&self.data[r..r + self.width]);
        }
    }
    pub fn finish_collection(&mut self) {
        self.used = self.floor + self.retained.len();
        if self.data.len() < self.used {
            self.data.resize(self.used, 0.)
        }
        self.data[self.floor..self.used].copy_from_slice(&self.retained);
        self.retained.clear();
        self.collect_after = self.used + Self::COLLECT_INTERVAL;
    }
    fn allocate(&mut self) -> usize {
        let start = self.used;
        self.used += self.width;
        if self.used > self.data.len() {
            self.data.resize(self.used, 0.);
        }
        start
    }

    pub fn row(&self, dual: Dual) -> &[f64] {
        dual.row.map_or(&[], |r| &self.data[r..r + self.width])
    }
}

#[inline]
pub(crate) fn scaled(partial: f64, slope: f64) -> f64 {
    if partial == 0. { 0. } else { partial * slope }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Dual {
    pub v: f64,
    pub t: f64,
    row: Option<usize>,
}
impl Dual {
    pub fn constant(v: f64) -> Self {
        Self {
            v,
            t: 0.,
            row: None,
        }
    }

    pub fn variable(v: f64, i: usize, g: &mut Gradients) -> Self {
        let row = g.allocate();
        g.data[row..row + g.width].fill(0.);
        g.data[row + i] = 1.;
        Self {
            v,
            t: 0.,
            row: Some(row),
        }
    }
    pub fn map(self, v: f64, s: f64, g: &mut Gradients) -> Self {
        let row = self.row.map(|r| {
            let dst = g.allocate();
            let (src, out) = g.data.split_at_mut(dst);
            for (d, &a) in out[..g.width].iter_mut().zip(&src[r..]) {
                *d = scaled(a, s);
            }
            dst
        });
        Self {
            v,
            t: scaled(self.t, s),
            row,
        }
    }
    pub fn combine(self, b: Self, v: f64, sa: f64, sb: f64, g: &mut Gradients) -> Self {
        let row = match (self.row, b.row) {
            (Some(a), Some(b)) => {
                let dst = g.allocate();
                let (src, out) = g.data.split_at_mut(dst);
                for ((d, &a), &b) in out[..g.width].iter_mut().zip(&src[a..]).zip(&src[b..]) {
                    *d = scaled(a, sa) + scaled(b, sb);
                }
                Some(dst)
            }
            (Some(_), None) => self.map(v, sa, g).row,
            (None, Some(_)) => b.map(v, sb, g).row,
            _ => None,
        };
        Self {
            v,
            t: scaled(self.t, sa) + scaled(b.t, sb),
            row,
        }
    }
    pub fn add(self, b: Self, g: &mut Gradients) -> Self {
        self.combine(b, self.v + b.v, 1., 1., g)
    }
    pub fn sub(self, b: Self, g: &mut Gradients) -> Self {
        self.combine(b, self.v - b.v, 1., -1., g)
    }
    pub fn mul(self, b: Self, g: &mut Gradients) -> Self {
        self.combine(b, self.v * b.v, b.v, self.v, g)
    }
    pub fn div(self, b: Self, g: &mut Gradients) -> Self {
        self.combine(b, self.v / b.v, 1. / b.v, -self.v / (b.v * b.v), g)
    }
}
