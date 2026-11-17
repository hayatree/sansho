# Provenance Record

This file records where each significant algorithm and each piece of non-original code comes
from. It exists to demonstrate that the project is an independent, clean-room implementation.

Rules: see [`CONTRIBUTING.md`](../CONTRIBUTING.md) §1–2. Add an entry in the same pull request
that introduces the code.

## Third-party code

| Path | Origin | License | Notes |
|---|---|---|---|
| — | — | — | No third-party code yet |

## Algorithms and references

| Area | Implementation | References |
|---|---|---|
| Netlist syntax, number scale factors | `spx-netlist` | Nagel (1975), SPICE2 input format |
| MNA stamps for R, C, L, V, I, E, F, G, H | `spx-devices/src/linear.rs`, `sources.rs` | Ho, Ruehli & Brennan (1975) |
| PULSE / SIN / EXP / PWL waveforms and defaults | `spx-devices/src/waveform.rs` | Nagel (1975); commonly documented SPICE3 defaults |
| DAE form `dq/dt + f = b`, deterministic slot assembly | `spx-devices`, `spx-engine/src/solve.rs` | Original design (`docs/architecture.md` D1, D4) |
| Newton iteration, SPICE convergence test | `spx-engine/src/solve.rs` | Nagel (1975) |
| Backward Euler / trapezoidal integration, breakpoint handling | `spx-engine/src/analysis.rs` | Nagel (1975); standard numerical ODE texts |
| Topology checks (DC path, voltage-source loops) | `spx-engine/src/circuit.rs` | Ho, Ruehli & Brennan (1975); union-find (textbook) |
| ASCII raw file layout | `spx-output` | Publicly documented SPICE3/ngspice rawfile format |
| Sparse LU | `spx-sparse/src/faer_backend.rs` | `faer` crate (MIT), used as a library dependency |

### Planned references (for upcoming work)

- L. W. Nagel, *SPICE2: A Computer Program to Simulate Semiconductor Circuits*, UC Berkeley
  ERL Memo M520, 1975.
- C.-W. Ho, A. E. Ruehli, P. A. Brennan, "The modified nodal approach to network analysis",
  IEEE Trans. Circuits and Systems, 1975.
- J. R. Gilbert, T. Peierls, "Sparse partial pivoting in time proportional to arithmetic
  operations", SIAM J. Sci. Stat. Comput., 1988.
- T. A. Davis, E. Palamadai Natarajan, "Algorithm 907: KLU, a direct sparse solver for circuit
  simulation problems", ACM TOMS, 2010.
- P. R. Amestoy, T. A. Davis, I. S. Duff, "An approximate minimum degree ordering algorithm",
  SIAM J. Matrix Anal. Appl., 1996.
- OSDI specification (interface description only; implemented independently).
