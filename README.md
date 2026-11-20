# Sansho

A fast, agent-native SPICE circuit simulator in Rust

Sansho (山椒) takes its name from Japanese pepper. It runs analog circuit netlists from the command line, from Rust through a session API, or from an agent through the Model Context Protocol (MCP).

**Status.** v0.1.0 is the first public release. Interfaces, file formats and model coverage may change before v1.0.

## Highlights

- **Large circuits.** In the documented 16-bit MOS1 multiplier transient benchmark, Sansho took 8.307 s with five workers, including setup, profiling and ASCII waveform output. The separately measured ngspice 44.2 default run took 82 s. Performance depends on the circuit. On the 1,000-stage BSIM4 inverter chain in the same table, the times were 0.349 s and 0.33 s, respectively. [Benchmark conditions and results](docs/performance.md#m5-2-wall-time-and-profile)
- **Reproducible parallel runs.** Fixed device accumulation order makes results bit-identical across worker counts on the same build and platform. In the Monte Carlo validation, scalar JSON matched byte for byte with one and two workers on each of three decks of 200 samples. A seed and run index reproduce an individual draw. [Architecture](docs/architecture.md) · [Monte Carlo measurements](docs/monte-carlo.md#m5-5-validation-and-cost)
- **Agent access.** The stdio MCP server can validate a deck, estimate its cost, simulate it and return measurements with units. An agent can request individual waveforms, alter parameters or run a sweep in the same session. Simulation jobs carry input and result hashes. [Diagnostics](docs/diagnostics.md)
- **Compact models.** Built-in quasi-static models cover BSIM3v3.3 and silicon BSIM4.8.2. The Verilog-A analog subset supports user-supplied models through `.hdl` and `N` instances, with a register interpreter and optional Cranelift JIT. [BSIM3 coverage](docs/bsim3.md) · [BSIM4 coverage](docs/bsim4.md) · [Verilog-A coverage](docs/verilog-a.md)

## Install

Download the archive for your system, check it against `SHA256SUMS`, extract it, and put `sansho` and `sansho-mcp` on your PATH. Archives target Linux (x86-64 and Arm64), macOS (Apple silicon) and Windows (x86-64). Binaries are distributed through [GitHub Releases](https://github.com/hayatree/sansho/releases).

To build from source, use Rust 1.98 or later. These commands install the CLI and MCP server from the release tag with the locked dependencies and JIT enabled. crates.io publication is deferred.

```sh
cargo install --git https://github.com/hayatree/sansho sansho-cli --tag v0.1.0 --locked --features jit
cargo install --git https://github.com/hayatree/sansho sansho-mcp --tag v0.1.0 --locked --features jit
```

## Quick start

Save this voltage divider as `divider.cir`. The operating point should give 0.5 V at `out`.

```spice
Voltage divider
V1 in 0 1
R1 in out 1k
R2 out 0 1k
.op
.end
```

```sh
sansho check divider.cir
sansho run divider.cir
sansho run divider.cir -r divider.raw --raw-format binary
sansho run divider.cir --json
```

For an MCP client with a JSON configuration, register the executable as a stdio server. Use its absolute path if the client's PATH differs from your shell's.

```json
{
  "mcpServers": {
    "sansho": { "command": "sansho-mcp", "args": [] }
  }
}
```

Ask the client to validate the divider, load it, simulate the operating point and report the voltage at `out`. Paths resolve on the server. Self-contained decks can also be passed as netlist text.

## Supported features

| Area | Available in v0.1 |
| --- | --- |
| Analyses | Operating point, nested DC and temperature sweeps, AC, adaptive trapezoidal and Gear transients, noise, transfer function, DC/AC sensitivity, Fourier and small lumped-circuit pole-zero analysis |
| Devices | R, L, C, mutual inductance, independent and controlled sources, behavioral and polynomial sources, switches, lossless transmission lines, junction diodes, Gummel–Poon BJTs, JFETs and MOS models |
| Netlists and output | Includes, library sections, parameterized subcircuits, functions, conditional expansion, model bins, initial conditions, `.save`, `.meas`, ASCII/binary raw plots and JSON |
| Statistical runs | Seeded Monte Carlo batches with per-instance mismatch, shared parameter draws, measurement statistics and single-run replay |

Support varies by model selector and analysis. The compatibility table distinguishes implemented forms, partial coverage and gaps such as Nutmeg control scripts. [ngspice compatibility](docs/compatibility.md) · [Analysis cards and outputs](docs/analyses.md)

## Accuracy and verification

Analytic tests check equations and derivatives. Original verification decks compare waveforms and measurements against ngspice as a black-box reference. The verifier reports tolerance scores and distinguishes passing comparisons from documented differences, such as E-table interpolation near a knot and the RC resistor's AC sensitivity vector. [Verification method and decks](verify/README.md)

For extracted BSIM3, BSIM4 and SPICE3 diode cards, Sansho follows Berkeley model behavior as observed through controlled ngspice experiments when it differs from the printed equations. Each probe establishes behavior for the conditions tested. Ambiguous results remain documented. [Compact-model compatibility policy](docs/compatibility.md#compact-model-compatibility)

## Clean-room development and PDKs

Sansho implements published equations, papers and public language specifications. Contributors do not consult copyleft simulator source or copy reference decks. Permissively licensed ports require attribution in both the notice and provenance record. [Contributing](CONTRIBUTING.md) · [Provenance](docs/PROVENANCE.md)

PDKs are installed separately. The SKY130 verification decks use the TT library from a local Volare installation of sky130A. The verification guide gives the expected directory layout for the checkout. [SKY130 setup](verify/README.md#built-in-bsim4-and-sky130)

## Roadmap

Planned work includes GPU batch evaluation for Monte Carlo and corners. M6 covers periodic steady-state and harmonic balance (PSS/HB), the HSPICE dialect and Python bindings. [Architecture and roadmap](docs/architecture.md)

## License

Sansho is available under the [Apache License 2.0](LICENSE-APACHE) or the [MIT License](LICENSE-MIT), at your option. Binary distributions include the notices for their dependencies. [Third-party licenses](THIRD-PARTY-LICENSES.md)

The name SPICE originated at the University of California, Berkeley. Sansho is unaffiliated with Berkeley, ngspice and simulator vendors. Trademarks belong to their respective owners. [Notice](NOTICE)
