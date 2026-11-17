//! `spx` command-line interface.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

fn main() {
    println!("spx {}", env!("CARGO_PKG_VERSION"));
}
