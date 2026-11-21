//! Intrinsic CIDER electrode G, C and Y measurements.
//! Provenance: ngspice 44 manual §§26.10, 26.17–26.18 and small-signal Dirichlet elimination.

use sansho_devices::{Slots, cider::Numerical};
use sansho_diag::Diagnostic;
use sansho_sparse::{PatternBuilder, SerialLu, c64};

pub(crate) fn element(kind: &str, key: &str) -> Option<(u8, usize, usize)> {
    let count = match kind {
        "numd" => 2,
        "nbjt" => 3,
        _ => return None,
    };
    let &[p, i, j] = key.as_bytes() else {
        return None;
    };
    if matches!(p, b'g' | b'c' | b'y')
        && (b'1'..b'1' + count).contains(&i)
        && (b'1'..b'1' + count).contains(&j)
    {
        Some((p, usize::from(i - b'1'), usize::from(j - b'1')))
    } else {
        None
    }
}

pub(crate) struct PortSolve {
    ports: usize,
    size: usize,
    lu: SerialLu<c64>,
    interior: Vec<(usize, usize)>,
    electrode: Vec<(usize, usize, usize)>,
    fixed: Vec<usize>,
    values: Vec<c64>,
}
impl PortSolve {
    pub fn new(device: &Numerical) -> Result<Self, Diagnostic> {
        let (ports, size, layout) = device.isolated_layout();
        let mut builder = PatternBuilder::new(size);
        let mut interior = Vec::new();
        let mut electrode = Vec::new();
        for (slot, &(r, c)) in layout.g.iter().enumerate() {
            if let (Some(row), Some(col)) = (r, c) {
                if row < ports {
                    electrode.push((slot, row, col))
                } else {
                    interior.push((slot, builder.add(row, col)))
                }
            }
        }
        let mut fixed = (0..ports).map(|i| builder.add(i, i)).collect::<Vec<_>>();
        let (pattern, map) = builder.build();
        for (_, i) in &mut interior {
            *i = map[*i]
        }
        for i in &mut fixed {
            *i = map[*i]
        }
        Ok(Self {
            ports,
            size,
            lu: SerialLu::new(&pattern).map_err(crate::small_signal::solver)?,
            interior,
            electrode,
            fixed,
            values: vec![c64::default(); pattern.nnz()],
        })
    }
    pub fn current(
        &self,
        row: usize,
        slots: &Slots,
        layout: &sansho_devices::StampLayout,
        solution: &[Vec<(f64, f64)>],
        index: usize,
        frequency: f64,
    ) -> (f64, f64) {
        let mut current = c64::default();
        let omega = std::f64::consts::TAU * frequency;
        for &(slot, port, _) in &self.electrode {
            if row == port
                && let Some(k) = layout.g[slot].1
            {
                let (re, im) = solution[k][index];
                current += c64::new(slots.g[slot], omega * slots.c[slot]) * c64::new(re, im);
            }
        }
        (current.re, current.im)
    }
    /// Impose a unit voltage on each independent terminal and collect its reaction currents.
    pub fn matrix(&mut self, slots: &Slots, frequency: f64) -> Result<[[c64; 3]; 3], Diagnostic> {
        let omega = std::f64::consts::TAU * frequency;
        let y = |k: usize| c64::new(slots.g[k], omega * slots.c[k]);
        self.values.fill(c64::default());
        for &(slot, pos) in &self.interior {
            self.values[pos] += y(slot)
        }
        for &pos in &self.fixed {
            self.values[pos] = 1.0.into()
        }
        self.lu
            .factor(&self.values)
            .map_err(crate::small_signal::solver)?;
        let mut result = [[c64::default(); 3]; 3];
        for j in 0..self.ports {
            let mut v = vec![c64::default(); self.size];
            v[j] = 1.0.into();
            self.lu.solve(&mut v).map_err(crate::small_signal::solver)?;
            for &(slot, row, col) in &self.electrode {
                result[row][j] += y(slot) * v[col]
            }
        }
        Ok(result)
    }
}

pub(crate) fn component(y: c64, kind: u8, freq: f64) -> (f64, f64) {
    match kind {
        b'g' => (y.re, 0.),
        b'c' => (y.im / (std::f64::consts::TAU * freq), 0.),
        _ => (y.re, y.im),
    }
}
