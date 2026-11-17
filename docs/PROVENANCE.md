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
| Forward-mode automatic differentiation | `spx-devices/src/dual.rs` | Griewank & Walther, *Evaluating Derivatives* (SIAM, 2008) |
| pn-junction current, depletion charge with `fc` linearization, junction limiting, `vcrit` | `spx-devices/src/junction.rs` | Nagel (1975); Antognetti & Massobrio (1993), ch. 1 |
| Diode (level 1) | `spx-devices/src/diode.rs` | Antognetti & Massobrio (1993), ch. 1; breakdown as a single exponential (own simplification) |
| BJT Gummel–Poon | `spx-devices/src/bjt.rs` | Gummel & Poon (1970); Getreu (1976); Antognetti & Massobrio (1993), ch. 2 |
| MOSFET level 1, Meyer capacitances | `spx-devices/src/mos1.rs` | Shichman & Hodges (1968); Meyer (1971); Antognetti & Massobrio (1993), ch. 4; FET step limiting is original |
| Gmin stepping, source stepping | `spx-engine/src/analysis.rs` | Standard continuation methods (e.g. Nagel 1975; Vlach & Singhal, *Computer Methods for Circuit Analysis and Design*, 1983); adaptive schedules are original |
| LTE step control, variable-step BDF2 | `spx-engine/src/analysis.rs` | Nagel (1975); Brayton, Gustavson & Hachtel, "A new efficient algorithm for solving differential-algebraic systems using implicit BDF", Proc. IEEE 60 (1972) |
| `.meas` evaluation | `spx-netlist/src/meas.rs`, `spx-engine/src/measure.rs` | Original; card syntax as commonly documented for SPICE-family simulators |
| Parallel job map (L0) | `spx-engine/src/parallel.rs` | Original |
| ASCII raw reader, verification metric | `spx-output/src/raw_reader.rs`, `spx-verify` | Original; public rawfile layout |

Reference simulators (ngspice) are only ever executed as external programs on our own decks
(`tools/ngspice`, `spx-verify`); their sources are not read.

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
- H. K. Gummel, H. C. Poon, "An integral charge control model of bipolar transistors", Bell Syst.
  Tech. J. 49, 1970.
- H. Shichman, D. A. Hodges, "Modeling and simulation of insulated-gate field-effect transistor
  switching circuits", IEEE J. Solid-State Circuits 3, 1968.
- J. E. Meyer, "MOS models and circuit simulation", RCA Review 32, 1971.
- P. Antognetti, G. Massobrio (eds.), *Semiconductor Device Modeling with SPICE*, 2nd ed.,
  McGraw-Hill, 1993.
