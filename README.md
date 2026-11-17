# rs-spice (working name)

A fast, agent-native SPICE circuit simulator written in Rust.

> **Status:** pre-alpha. Nothing works yet. See [`docs/architecture.md`](docs/architecture.md).

## Goals

- **Accuracy** on par with ngspice and Xyce, preserving SPICE tolerance semantics
- **Speed**: single-core performance at least on par with ngspice + KLU, plus multithreading
- **Agent native**: structured JSON I/O, machine-readable diagnostics, deterministic results,
  session API and a built-in MCP server
- **Parallel and GPU**: bit-deterministic multithreading; GPU batch evaluation for sweeps and Monte Carlo
- **Compatibility**: ngspice-dialect netlists, open PDKs (SKY130, GF180MCU, IHP SG13G2),
  Verilog-A models via OSDI

## Repository layout

| Path | Contents |
|---|---|
| `crates/spx-netlist` | Netlist lexer and parser |
| `crates/spx-sparse` | Sparse linear solvers |
| `crates/spx-devices` | Device model interface and built-in models |
| `crates/spx-engine` | MNA assembly, Newton, analyses |
| `crates/spx-cli` | `spx` command-line tool |
| `docs/` | Design documents and provenance records |

## Contributing

Please read [`CONTRIBUTING.md`](CONTRIBUTING.md) first. This project follows a strict
clean-room policy regarding copyleft sources.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
