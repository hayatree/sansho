# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions before 1.0 may change public interfaces.

## [0.1.0] - 2026-11-20

First public release, distributed on GitHub.

### Analyses

- Compute operating points, nested DC and temperature sweeps, complex AC responses and adaptive transients with trapezoidal or Gear integration.
- Extract small-signal noise spectra and integrals, DC transfer and port resistances, DC/AC sensitivities, Fourier harmonics and finite poles and zeros for small lumped circuits.
- Run seeded Monte Carlo batches, summarize measurements and replay an individual draw.

### Devices and models

- Simulate passive and mutually coupled elements, independent and controlled sources, behavioral sources, switches and lossless transmission lines.
- Use junction diodes, Gummel–Poon BJTs, JFETs, classical MOS levels 1–3 and built-in quasi-static BSIM3v3.3 and BSIM4.8.2 models.
- Load the Verilog-A analog subset from source with a register interpreter and optional native JIT.

### Netlist dialect

- Expand parameterized subcircuits, library sections, model bins, functions and conditional includes. Set initial conditions, nodesets and simulator tolerances.
- Select signals with `.save` and extract event times, derivatives, windowed aggregates and derived scalars with `.meas`. Export ASCII or binary raw plots.

### Agent interfaces

- Run, check and batch netlists from the CLI with JSON results and structured diagnostics.
- Use the Rust session API or stdio MCP server to simulate, inspect waveforms, measure, alter parameters, sweep and export decks.

### Performance

- Reuse the matrix structure, apply block-triangular sparse LU and parallelize device evaluation and numeric refactorization with fixed accumulation order.
- Share topology across Monte Carlo samples. The performance notes report wall times, solver work and waveform error on large circuits. [Measurements](https://github.com/hayatree/sansho/blob/v0.1.0/docs/performance.md)

### Verification

- Compare original device, analysis, SKY130 and Verilog-A decks against black-box ngspice output, alongside analytic and worker-count reproducibility tests.
- Report known numerical differences separately and record compact-model behavior with one-variable probes. [Verification guide](https://github.com/hayatree/sansho/blob/v0.1.0/verify/README.md)

[0.1.0]: https://github.com/hayatree/sansho/releases/tag/v0.1.0
