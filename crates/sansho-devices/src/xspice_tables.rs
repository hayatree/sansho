//! Sampled transfer functions on rectilinear grids and complex frequency tables.
//! Provenance: ngspice 44 user manual §§8.2.19, 8.2.30–31 and Newton divided differences.

use super::{D, invalid};
use crate::ModelParams;
use sansho_diag::Diagnostic;
use sansho_netlist::ModelCard;

#[derive(Debug)]
pub(super) struct Grid {
    axes: Vec<Vec<f64>>,
    data: Vec<f64>,
    strides: Vec<usize>,
    order: usize,
    offset: f64,
    gain: f64,
}
impl Grid {
    pub(super) fn new(m: &ModelCard, p: &ModelParams) -> Result<Self, Diagnostic> {
        let dim = if m.kind == "table2d" { 2 } else { 3 };
        let rows = &m
            .table
            .as_ref()
            .ok_or_else(|| invalid("missing grid file"))?
            .rows;
        let mut words = rows.iter().flatten().copied();
        let sizes = words.by_ref().take(dim).collect::<Vec<_>>();
        if sizes.len() != dim
            || sizes
                .iter()
                .any(|v| v.fract() != 0. || *v < 2. || *v > usize::MAX as f64)
        {
            return Err(invalid("invalid grid dimensions"));
        }
        let mut stride = 1usize;
        let mut axes = Vec::new();
        let mut strides = Vec::new();
        for size in sizes {
            let axis = words.by_ref().take(size as usize).collect::<Vec<_>>();
            if axis.len() != size as usize || axis.windows(2).any(|v| v[0] >= v[1]) {
                return Err(invalid("incomplete or unordered grid axis"));
            }
            strides.push(stride);
            stride = stride
                .checked_mul(axis.len())
                .ok_or_else(|| invalid("grid size overflow"))?;
            axes.push(axis);
        }
        let data = words.collect::<Vec<_>>();
        let order = p.get("order", 3.);
        if data.len() != stride || order < 1. || order.fract() != 0. {
            return Err(invalid("grid size or interpolation order is invalid"));
        }
        p.accept(&["verbose"]);
        Ok(Self {
            axes,
            data,
            strides,
            order: order as usize,
            offset: p.get("offset", 0.),
            gain: p.get("gain", 1.),
        })
    }
    // Select an ENO stencil by extending toward the smaller divided difference.
    fn slope(&self, axis: usize, base: usize, cell: usize, u: f64) -> f64 {
        let x = &self.axes[axis];
        let step = self.strides[axis];
        let count = self.order.max(2).min(x.len());
        let diff = |lo: usize, hi: usize| {
            let mut ys = (lo..=hi)
                .map(|i| self.data[base + i * step])
                .collect::<Vec<_>>();
            for d in 1..ys.len() {
                for j in (d..ys.len()).rev() {
                    ys[j] = (ys[j] - ys[j - 1]) / (x[lo + j] - x[lo + j - d]);
                }
            }
            ys
        };
        let (mut lo, mut hi) = (cell, cell + 1);
        while hi - lo + 1 < count {
            if hi + 1 == x.len()
                || lo > 0
                    && diff(lo - 1, hi).last().map(|d| d.abs())
                        <= diff(lo, hi + 1).last().map(|d| d.abs())
            {
                lo -= 1;
            } else {
                hi += 1;
            }
        }
        let ys = diff(lo, hi);
        let mut v = ys[ys.len() - 1];
        let mut d = 0.;
        for j in (0..ys.len() - 1).rev() {
            d = d * (u - x[lo + j]) + v;
            v = v * (u - x[lo + j]) + ys[j];
        }
        d
    }
    pub(super) fn eval(&self, input: impl Fn(usize) -> D) -> D {
        let dim = self.axes.len();
        let mut cell = [0; 3];
        let mut frac = [0.; 3];
        for (i, x) in self.axes.iter().enumerate() {
            cell[i] = x
                .partition_point(|v| *v <= input(i).v)
                .saturating_sub(1)
                .min(x.len() - 2);
            frac[i] = (input(i).v - x[cell[i]]) / (x[cell[i] + 1] - x[cell[i]]);
        }
        let mut out = D::cst(0.);
        for mask in 0..1usize << dim {
            let mut idx = 0;
            let mut weight = 1.;
            for i in 0..dim {
                let bit = (mask >> i) & 1;
                idx += (cell[i] + bit) * self.strides[i];
                weight *= if bit == 0 { 1. - frac[i] } else { frac[i] };
            }
            out.v += weight * self.data[idx];
            for i in 0..dim {
                if input(i).d[0] == 0. || mask & (1 << i) != 0 {
                    continue;
                }
                let other = (0..dim)
                    .filter(|j| *j != i)
                    .map(|j| {
                        if mask & (1 << j) == 0 {
                            1. - frac[j]
                        } else {
                            frac[j]
                        }
                    })
                    .product::<f64>();
                out.d[0] += input(i).d[0]
                    * other
                    * self.slope(i, idx - cell[i] * self.strides[i], cell[i], input(i).v);
            }
        }
        self.offset + self.gain * out
    }
}

#[derive(Debug)]
pub(super) struct Frequency(pub Vec<[f64; 3]>);
impl Frequency {
    pub(super) fn new(
        m: &ModelCard,
        p: &ModelParams,
        mut values: Vec<f64>,
    ) -> Result<Self, Diagnostic> {
        let mut unit = 1.;
        let (mut ri, mut db) = (false, true);
        if let Some(table) = &m.table {
            if !values.is_empty() {
                return Err(invalid("xfer accepts either file or table"));
            }
            unit = 1e9;
            for line in &table.records {
                let line = line.split('!').next().unwrap_or("").trim();
                if let Some(options) = line.strip_prefix('#') {
                    for opt in options.split_whitespace() {
                        match opt.to_ascii_lowercase().as_str() {
                            "hz" => unit = 1.,
                            "khz" => unit = 1e3,
                            "mhz" => unit = 1e6,
                            "ghz" => unit = 1e9,
                            "ri" => {
                                ri = true;
                                db = false
                            }
                            "ma" => {
                                ri = false;
                                db = false
                            }
                            "db" => {
                                ri = false;
                                db = true
                            }
                            _ => {}
                        }
                    }
                } else {
                    for field in line.split_whitespace() {
                        values.push(
                            field
                                .parse::<f64>()
                                .map_err(|_| invalid("invalid Touchstone value"))?,
                        );
                    }
                }
            }
        }
        let span = p.get("span", 3.);
        let offset = p.get("offset", 1.);
        if span.fract() != 0.
            || offset.fract() != 0.
            || span < 3.
            || offset < 1.
            || offset + 1. >= span
            || span > values.len() as f64
            || !values.len().is_multiple_of(span as usize)
        {
            return Err(invalid(
                "xfer needs complete frequency rows with a selected value pair",
            ));
        }
        ri = p.get("r_i", f64::from(ri)) != 0.;
        db = p.get("db", f64::from(db)) != 0.;
        let rad = p.get("rad", 0.) != 0.;
        let j = offset as usize;
        let mut points = Vec::new();
        for row in values.chunks_exact(span as usize) {
            let (a, b) = (row[j], row[j + 1]);
            let (re, im) = if ri {
                (a, b)
            } else {
                let mag = if db { 10f64.powf(a / 20.) } else { a };
                let angle = if rad { b } else { b.to_radians() };
                (mag * angle.cos(), mag * angle.sin())
            };
            let point = [unit * row[0], re, im];
            if point.iter().any(|v| !v.is_finite())
                || point[0] < 0.
                || points
                    .last()
                    .is_some_and(|prev: &[f64; 3]| prev[0] >= point[0])
            {
                return Err(invalid("nonfinite or unordered xfer table"));
            }
            points.push(point);
        }
        Ok(Self(points))
    }
}
