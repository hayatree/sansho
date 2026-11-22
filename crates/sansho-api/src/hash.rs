//! Stable 64-bit hashes for reproducibility metadata.
//!
//! Provenance: original implementation of the FNV-1a hash as published by Fowler, Noll and Vo
//! (public domain; see the IETF draft `draft-eastlake-fnv`). See `docs/PROVENANCE.md`.

use sansho_output::{Data, SimResults};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a hasher.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Self(OFFSET)
    }

    pub(crate) fn bytes(&mut self, b: &[u8]) -> &mut Self {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(PRIME);
        }
        self
    }

    pub(crate) fn f64(&mut self, v: f64) -> &mut Self {
        self.bytes(&v.to_bits().to_le_bytes())
    }

    pub(crate) fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Hash of a canonical deck text.
pub(crate) fn text(s: &str) -> String {
    Fnv::new().bytes(s.as_bytes()).hex()
}

/// Canonical deck plus retained HDL source text; no-HDL decks keep their existing hash.
pub(crate) fn netlist(nl: &sansho_netlist::Netlist) -> String {
    let deck = sansho_netlist::write(nl);
    if nl.hdl.is_empty() && nl.parameter_source.is_none() {
        return text(&deck);
    }
    let mut hash = Fnv::new();
    hash.bytes(deck.as_bytes());
    if let Some(source) = &nl.parameter_source {
        hash.bytes(&(source.len() as u64).to_le_bytes())
            .bytes(source.as_bytes());
    }
    for h in &nl.hdl {
        hash.bytes(&(h.text.len() as u64).to_le_bytes())
            .bytes(h.text.as_bytes());
    }
    hash.hex()
}

/// Bit-exact hash of all numeric results (independent of thread count by construction).
pub(crate) fn results(r: &SimResults) -> String {
    let mut h = Fnv::new();
    for a in &r.analyses {
        h.bytes(a.kind.plot_name().as_bytes());
        for &v in &a.scale_values {
            h.f64(v);
        }
        for v in &a.variables {
            h.bytes(v.name.as_bytes());
        }
        for (name, changes) in &a.events {
            h.bytes(name.as_bytes());
            for (t, s) in changes {
                h.f64(*t).bytes(s.as_bytes());
            }
        }
        match &a.data {
            Data::Real { columns } => {
                for c in columns {
                    for &v in c {
                        h.f64(v);
                    }
                }
            }
            Data::Complex { columns } => {
                for c in columns {
                    for &(re, im) in c {
                        h.f64(re).f64(im);
                    }
                }
            }
        }
    }
    for table in &r.prints {
        h.bytes(table.analysis.plot_name().as_bytes());
        for (name, column) in table.headings.iter().zip(&table.columns) {
            h.bytes(name.as_bytes());
            for &v in column {
                h.f64(v);
            }
        }
    }
    for m in &r.measures {
        h.bytes(m.name.as_bytes()).f64(m.value.unwrap_or(f64::NAN));
    }
    h.hex()
}

/// Include control-only jobs (e.g. a seed and print in the const plot) in the result identity.
pub(crate) fn control(result: &str, t: &crate::control::Transcript) -> String {
    let mut h = Fnv::new();
    h.bytes(result.as_bytes());
    for line in t
        .lines
        .iter()
        .chain(&t.plots)
        .chain(&t.written_files)
        .chain(std::iter::once(&t.current_plot))
    {
        h.bytes(&(line.len() as u64).to_le_bytes())
            .bytes(line.as_bytes());
    }
    for table in &t.prints {
        for (name, column) in table.expressions.iter().zip(&table.columns) {
            h.bytes(name.as_bytes())
                .bytes(&(column.len() as u64).to_le_bytes());
            for &(re, im) in column {
                h.f64(re).f64(im);
            }
        }
    }
    h.hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(text(""), "cbf29ce484222325");
        assert_eq!(text("a"), "af63dc4c8601ec8c");
        assert_eq!(text("foobar"), "85944171f73967e8");
    }
}
