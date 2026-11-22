# Instructions for AI coding agents (Codex and others)

Sansho is a clean-room, Apache-2.0 OR MIT licensed SPICE simulator in Rust.
Read `CONTRIBUTING.md` and `docs/architecture.md` before making changes.

## Hard rules

1. **Never read, fetch, clone, search or reproduce source code from copyleft or
   non-permissive projects**: Xyce, VACASK, Gnucap, OpenVAF(-reloaded), Qucs/Qucs-S/QucsatorRF,
   ADMS, SuiteSparse/KLU, LGPL/MPL parts of ngspice, JaimeHW/RSpice, or any GPL/LGPL/AGPL/MPL
   code. Do not reproduce such code from memory either. Implement from papers and specifications.
2. Porting BSD/MIT/Apache code is allowed only together with updates to `NOTICE` and
   `docs/PROVENANCE.md` in the same change.
3. Every new source file begins with a module doc comment that includes a `Provenance:` line.
4. Do not add dependencies that `deny.toml` would reject. Do not vendor PDK files.
5. All code, comments, docs and commit messages are in English.

## Engineering rules

- Library code must not panic (`unwrap`, `expect`, `panic!` are denied by clippy); return
  `Result` with structured diagnostics.
- Keep results bit-deterministic regardless of thread count (see architecture D4).
- Before finishing: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

## Verilog-A work

See `docs/verilog-a.md`. Implement only from the Accellera Verilog-AMS LRM and general compiler
literature. Never consult OpenVAF, ADMS, VACASK, Gnucap, Xyce, Qucs or OSDI headers.
