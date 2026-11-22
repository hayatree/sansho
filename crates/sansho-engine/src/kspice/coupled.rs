//! Transient propagation with non-diagonal modal loss.
//! Provenance: original symmetric spatial splitting of Paul's coupled telegrapher equations.

use super::matrix::{Matrix, scattering};
use sansho_sparse::c64;
use std::collections::VecDeque;

/// Accepted samples. All paths in a bank use the same time grid.
#[derive(Default)]
struct Samples(VecDeque<(f64, Vec<f64>)>);

impl Samples {
    fn push(&mut self, t: f64, values: Vec<f64>, span: f64) {
        self.0.push_back((t, values));
        while self.0.get(1).is_some_and(|v| v.0 <= t - span) {
            self.0.pop_front();
        }
    }

    fn at(&self, t: f64, index: usize) -> f64 {
        let next = self.0.partition_point(|v| v.0 <= t);
        if next == 0 {
            return 0.;
        }
        let (a, left) = &self.0[next - 1];
        let Some((b, right)) = self.0.get(next) else {
            return left[index];
        };
        left[index] + (t - a) / (b - a) * (right[index] - left[index])
    }
}

pub(super) struct Coupled {
    cells: usize,
    flight: Vec<f64>,
    longest: f64,
    scatter: Matrix<f64>,
    // Direct transmission to each cell centre, and across the whole line.
    gain: Vec<Vec<f64>>,
    ends: Samples,
    scattered: Samples,
}

impl Coupled {
    pub fn new(r: &Matrix<f64>, g: &Matrix<f64>, flight: Vec<f64>, step: f64) -> Self {
        let longest = flight.iter().copied().fold(0., f64::max);
        // Resolve the flight time on the requested grid, with bounded spatial storage.
        let cells = (longest / step).ceil().clamp(1., 256.) as usize;
        let section =
            |m: &Matrix<f64>| Matrix::build(m.n, |i, j| c64::from(m[(i, j)] / cells as f64));
        let s = scattering(&section(r), &section(g));
        let scatter = Matrix::build(s.n, |i, j| s[(i, j)].re);
        let n = flight.len();
        let mut gain = vec![vec![1.; n]; cells + 1];
        for k in 1..=cells {
            for j in 0..n {
                gain[k][j] = gain[k - 1][j] * scatter[(n + j, j)];
            }
        }
        Self {
            cells,
            flight,
            longest,
            scatter,
            gain,
            ends: Samples::default(),
            scattered: Samples::default(),
        }
    }

    pub fn step_limit(&self) -> f64 {
        self.flight.iter().copied().fold(f64::INFINITY, f64::min) / (2 * self.cells) as f64
    }

    /// Incoming characteristic at an exterior port. Follow the straight wave
    /// from its launch sample so interpolating at each junction cannot smear it.
    pub fn incoming(&self, t: f64, side: usize, j: usize) -> f64 {
        let n = self.flight.len();
        let delay = self.flight[j];
        let edge = if side == 0 { 0 } else { self.cells - 1 };
        self.gain[self.cells][j] * self.ends.at(t - delay, (1 - side) * n + j)
            + self.scattered.at(
                t - delay / (2 * self.cells) as f64,
                (2 * edge + side) * n + j,
            )
    }

    pub fn accept(&mut self, t: f64, outgoing: Vec<f64>) {
        let n = self.flight.len();
        let mut tails = vec![0.; self.cells * 2 * n];
        // Both arrays are bounded by the eight-conductor card limit.
        let mut incident = [0.; 16];
        let mut straight = [0.; 16];
        for k in 0..self.cells {
            for side in 0..2 {
                let hops = if side == 0 { k } else { self.cells - 1 - k };
                for (j, &delay) in self.flight.iter().enumerate() {
                    let col = side * n + j;
                    let transit = delay / self.cells as f64;
                    straight[col] =
                        self.gain[hops][j] * self.ends.at(t - (hops as f64 + 0.5) * transit, col);
                    let tail = if hops == 0 {
                        0.
                    } else {
                        let upstream = if side == 0 { k - 1 } else { k + 1 };
                        self.scattered
                            .at(t - transit, (2 * upstream + 1 - side) * n + j)
                    };
                    incident[col] = straight[col] + tail;
                }
            }
            for row in 0..2 * n {
                let through = (row + n) % (2 * n);
                let wave: f64 = self.scatter.data[row * 2 * n..(row + 1) * 2 * n]
                    .iter()
                    .zip(incident)
                    .map(|(s, x)| s * x)
                    .sum();
                tails[2 * k * n + row] = wave - self.scatter[(row, through)] * straight[through];
            }
        }
        self.scattered
            .push(t, tails, self.longest / self.cells as f64);
        self.ends.push(t, outgoing, self.longest);
    }
}
