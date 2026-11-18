//! Model and instance parameter lookup with defaults and unused-parameter reporting.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use std::cell::Cell;

/// Description of one parameter accepted by a model (used for documentation and agents).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Lower-case parameter name.
    pub name: &'static str,
    /// Default value.
    pub default: f64,
    /// Short description including the unit.
    pub doc: &'static str,
}

/// A list of `name = value` pairs that tracks which entries were consumed.
#[derive(Debug, Default)]
pub struct ModelParams {
    entries: Vec<(String, f64, Cell<bool>)>,
    // Large compact-model cards are queried hundreds of times during elaboration.
    sorted: Vec<usize>,
}

impl ModelParams {
    /// Wrap parameter pairs (names are expected in lower case). Later entries override earlier
    /// ones.
    pub fn new(pairs: &[(String, f64)]) -> Self {
        let entries: Vec<_> = pairs
            .iter()
            .map(|(k, v)| (k.clone(), *v, Cell::new(false)))
            .collect();
        let mut sorted = Vec::new();
        if entries.len() > 16 {
            sorted.extend(0..entries.len());
            sorted.sort_by(|&a, &b| entries[a].0.cmp(&entries[b].0));
        }
        Self { entries, sorted }
    }

    fn lookup(&self, names: &[&str]) -> Option<f64> {
        if !self.sorted.is_empty() {
            let mut last = None;
            for &key in names {
                let start = self
                    .sorted
                    .partition_point(|&i| self.entries[i].0.as_str() < key);
                for &i in &self.sorted[start..] {
                    let (k, _, used) = &self.entries[i];
                    if k != key {
                        break;
                    }
                    used.set(true);
                    last = Some(i.max(last.unwrap_or(0)));
                }
            }
            return last.map(|i| self.entries[i].1);
        }
        let mut found = None;
        for (k, v, used) in &self.entries {
            if names.contains(&k.as_str()) {
                used.set(true);
                found = Some(*v);
            }
        }
        found
    }

    /// Value of the first matching name (aliases allowed), if given.
    pub fn opt(&self, names: &[&str]) -> Option<f64> {
        self.lookup(names)
    }

    /// Value of `name` or `default`.
    pub fn get(&self, name: &str, default: f64) -> f64 {
        self.lookup(&[name]).unwrap_or(default)
    }

    /// Mark parameters as accepted without using them (e.g. noise parameters while no noise
    /// analysis exists). Returns the names that were present.
    pub fn accept(&self, names: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for (k, _, used) in &self.entries {
            if names.contains(&k.as_str()) {
                used.set(true);
                out.push(k.clone());
            }
        }
        out
    }

    /// Names of parameters that were never consumed.
    pub fn unused(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, _, u)| !u.get())
            .map(|(k, _, _)| k.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_usage() {
        let p = ModelParams::new(&[("is".into(), 1e-15), ("foo".into(), 1.0), ("n".into(), 2.0)]);
        assert_eq!(p.get("is", 1e-14), 1e-15);
        assert_eq!(p.get("rs", 0.0), 0.0);
        assert_eq!(p.opt(&["n", "nf"]), Some(2.0));
        assert_eq!(p.unused(), vec!["foo".to_string()]);
    }
}
