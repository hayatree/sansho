# rs-spice (working name)

A fast, agent-native SPICE circuit simulator written in Rust.

> **Status:** pre-alpha. R, C, L, V, I, E, F, G, H, diodes, BJTs (Gummel–Poon) and level-1
> MOSFETs; `.op`, `.dc`, `.ac`, `.tran` (trapezoidal or BDF2 with LTE control), `.meas`, `.model`,
> `.options`. Results are checked against ngspice (`verify/`). See
> [`docs/architecture.md`](docs/architecture.md).

```sh
cargo run -p spx-cli -- run examples/rc_lowpass.cir            # summary
cargo run -p spx-cli -- run examples/rc_lowpass.cir -r out.raw  # ngspice-compatible raw file
cargo run -p spx-cli -- run examples/rc_lowpass.cir --json      # machine-readable output
cargo run -p spx-cli -- check examples/rc_lowpass.cir           # parse + elaborate only
cargo run -p spx-cli -- batch a.cir b.cir -j 8                   # parallel runs, JSON lines
cargo run -p spx-verify                                          # compare with ngspice (Docker)
```

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
| `crates/spx-diag` | Structured diagnostics |
| `crates/spx-netlist` | Netlist lexer and parser |
| `crates/spx-sparse` | Sparse linear solvers |
| `crates/spx-devices` | Device model interface and built-in models |
| `crates/spx-engine` | MNA assembly, Newton, analyses |
| `crates/spx-output` | Result types, raw writer |
| `crates/spx-cli` | `spx` command-line tool |
| `crates/spx-verify` | Comparison against a reference simulator |
| `tools/ngspice` | ngspice in Docker, used as a black-box reference |
| `verify/decks` | Verification circuits |
| `examples/` | Example netlists |
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
