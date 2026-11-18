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
| Verilog-A preprocessing, lexical grammar, AST and parser | `spx-va/src` | Original implementation from knowledge of Accellera Verilog-AMS LRM 2.4 analog subset; recursive descent and precedence climbing from general compiler techniques |
| Verilog-A built-in disciplines and constants | `spx-va/src/preproc.rs` | Original declarations from LRM nature/discipline semantics; mathematical constants, SI defining constants and CODATA 2022 values |
| Verilog-A test models | `spx-va/tests` | Original resistor, junction and simplified square-law MOS examples; no third-party models |
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
| Parallel jobs (L0), chunked device evaluation (L1), ordered gather assembly (L2) | `spx-engine/src/parallel.rs`, `circuit.rs`, `solve.rs` | Original design (`docs/architecture.md` D4); `rayon` crate (MIT/Apache-2.0) as a library dependency |
| Diagnostic code registry, lint checks, "did you mean" (Levenshtein distance) | `spx-diag/src/codes.rs`, `spx-engine/src/lint.rs`, `spx-netlist/src/parser.rs` | Original; edit distance as in V. I. Levenshtein (1966), textbook dynamic programming |
| Parameter expressions, `.param`, `.func` | `spx-netlist/src/expr.rs` | Original recursive-descent parser; operator set and functions as commonly documented for SPICE parameter expressions (ngspice user manual, used as documentation only) |
| `.include`/`.lib`, subcircuit flattening, model binning, `.ic`/`.nodeset`/UIC | `spx-netlist/src/preproc.rs`, `parser.rs`, `spx-engine/src/analysis.rs` | Nagel (1975) input format; hierarchical naming and library sections as documented in the ngspice user manual; node clamping is a standard Norton-equivalent technique |
| Netlist writer | `spx-netlist/src/writer.rs` | Original; SPICE input format (Nagel 1975) |
| Session / job API, summaries, alterations | `spx-api` | Original |
| Input / result hashes | `spx-api/src/hash.rs` | FNV-1a by G. Fowler, L. C. Noll, K.-P. Vo (public domain; IETF draft-eastlake-fnv) |
| Waveform downsampling (LTTB) | `spx-api/src/waveform.rs` | S. Steinarsson, *Downsampling Time Series for Visual Representation*, MSc thesis, University of Iceland, 2013 |
| MCP server | `spx-mcp` | Model Context Protocol specification (modelcontextprotocol.io, revisions 2024-11-05 – 2025-11-25) and JSON-RPC 2.0 specification; no SDK code used |
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


### Verilog-A VA-2

- `spx-va/src/sema.rs` and `ir.rs`: original semantic analysis and resolved typed-tree lowering
  from the Accellera Verilog-AMS LRM 2.4 and general compiler techniques. No external compiler
  implementation was consulted or ported.
- `spx-va/src/dual.rs` and `eval.rs`: original dynamic forward-mode differentiation from the
  chain rule and general AD literature; DAE current/charge stamping from elementary modified
  nodal analysis. Built-in derivatives are elementary analytic derivatives.
- `spx-va/src/limit.rs`: original logarithmic junction limiter based on the published junction
  voltage limiting idea in L. W. Nagel, *SPICE2*, ERL Memo M520 (1975). The field-effect voltage
  step clamp is an original heuristic, not a port of another simulator's limiter.
- `spx-va/tests/evaluator.rs`: models and test expectations authored for this change from
  elementary resistor, exponential junction and square-law charge equations; central finite
  differences provide independent derivative checks. No PDK files are included.

VA-2 was implemented without network access, new dependencies, external source code, generated
model code, or interface headers. There is no third-party code port requiring a NOTICE addition.


### Verilog-A VA-3

| Component | Files | Provenance |
|---|---|---|
| Limiting linearization tangent | `spx-va/src/dual.rs`, `eval.rs` | Original chain-rule AD implementation; first-order SPICE linearization from Nagel (1975) |
| Simulator adapter and dense stamping | `spx-devices/src/va.rs`, `lib.rs`; `spx-engine/src/circuit.rs` | Original repository DAE/MNA integration, Ho, Ruehli & Brennan (1975), Accellera Verilog-AMS LRM 2.4 |
| HDL cards, N instances and hierarchy | `spx-netlist/src` | Original parser/writer work from the requested syntax and existing source-loader/flattening interfaces |
| Models and integration tests | `spx-engine/tests/verilog_a.rs`, `tests/va/*`; `examples/va/diode.va`, `examples/va_diode.cir`; `spx-api/tests/session.rs` | Authored resistor/capacitor and junction equations from repository built-ins, Nagel (1975), Antognetti & Massobrio (1993); analytic RC response |

Implemented without network access, external source implementations, interface headers, PDK
files or new third-party dependencies. No ported code requires a NOTICE addition.

### Verilog-A VA-4a

| Component | Files | Provenance |
|---|---|---|
| Built-in headers | `spx-va/src/headers/constants.vams`, `disciplines.vams` | Original declarations from Accellera Verilog-AMS LRM 2.4. Math values follow the named mathematical identities. Physical values follow the SI definitions and CODATA 2022. |
| Binding and branch collapse | `spx-va/src/eval/topology.rs`, `sema.rs`, `eval.rs`, `spx-engine/src/circuit.rs` | Original constant propagation, union-find and MNA stamping from general compiler and circuit literature. Port current probes use an ideal series voltage source. Integration introduces a DAE state. |
| Collapse fixture | `spx-va/tests/models/collapse.va` | Authored from Ohm's law and the Shockley diode equation. |
| Local model verification | `spx-va/tests/production.rs`, `spx-engine/tests/verilog_a.rs` | Original finite-difference harness and DC transfer test. Third-party models are external test inputs. |

### Verilog-A VA-5

The register compiler in `spx-va/src/eval/program.rs` and `eval/program/` is an original
implementation of forward constant propagation, control-flow joins, local value numbering
and backwards liveness analysis from general compiler literature (Aho et al., *Compilers*).
The dense gradient rows, scratch arena and scalar chain-rule factors in `dual.rs` and
`math.rs` use forward-mode AD as described by Griewank and Walther, *Evaluating Derivatives*.
The 21-stage benchmark in `benches/psp_ring21.cir` extends the project's authored CMOS ring.

### Verilog-A VA-5b

The native backend in `crates/spx-va/src/eval/program/jit.rs` and its submodules
is original work based on the project's register evaluator, the Accellera
Verilog-AMS LRM 2.4, SSA construction and forward-mode AD (Aho et al.,
*Compilers*, Griewank and Walther, *Evaluating Derivatives*). Finiteness checks
use IEEE 754 arithmetic. Native math shims use the repository's scalar math
and limiter implementations.

Cranelift codegen, frontend, jit, module and native 0.137.2 are optional Cargo
dependencies under Apache-2.0 WITH LLVM-exception. The five packaged crates
each contain LICENSE and ship no NOTICE file. The backend uses their public
APIs. No Cranelift source was ported into the repository, and NOTICE requires
no additional attribution for this change. Local IHP model files and circuit
cards are external test inputs.

### Verilog-A startup and persistent compilation

The VA artifact store, include manifest, binding scheduler and benchmark drivers
are original repository work. The store uses SHA-256, per-key initialization,
exclusive file creation and atomic POSIX rename. The native entry uses the
public Cranelift 0.137.2 code buffer, relocation and JIT module APIs. Math
callbacks and limiters come from spx. The straight-line partition in `jit/split.rs` uses original SSA
liveness and register handoff code from general compiler literature. No
external simulator code, headers,
model text or precompiled models are part of the change.

The optional JIT build uses the locally cached hmac-sha256 1.1.15 (ISC) for
SHA-256 and rustix 1.1.6 (permissive Apache/MIT alternatives) for operations
relative to an open directory descriptor. Typed compiler data uses the
workspace's serde and serde_json packages. These are Cargo dependencies.
No code from these packages was ported. The offline license, ban and source
checks use the workspace deny.toml.

### Verilog-A optimization round 2

Circuit-specific stamp projection, forward tangent pairing, SSA edge copies,
backward derivative liveness and finiteness propagation are original implementations
based on the chain rule, SSA construction and data-flow analysis. The implementation
uses the existing Cranelift public IR and frontend APIs. The charge-history optimization
uses exact equality of the simulator inputs to a side-effect-free evaluation. The
existing Newton update, voltage limiting and integration equations are preserved.
The timing probes use Rust and Python monotonic clocks. This round adds no dependencies.

### Verilog-A optimization round 3

The Newton numeric refactorization, scatter schedule, pivot tests and triangular solves
are original repository code. The algorithmic sources are J. R. Gilbert and T. Peierls,
*Sparse Partial Pivoting in Time Proportional to Arithmetic Operations* (1988), and
T. A. Davis, *Direct Methods for Sparse Linear Systems* (2006). Symbolic ordering and
fresh partial pivoting use the existing faer dependency through its public Rust API.

The two-instance evaluator uses standard SSA value numbering, postdominators and
masked SIMD execution through Cranelift's public IR and host-ISA APIs. Scalar math
uses the existing spx shims. Instance pairing, ordered stamp assembly, local model
card reuse and the timing probes are original work. The IHP models served as test
inputs. Dependency and licensing inventories are unchanged.

### Verilog-A optimization round 4

The numeric-support traversal and flat column update lists extend the Gilbert–Peierls
implementation described above. The ordering experiment implements the pivot cost
from H. M. Markowitz, *The Elimination Form of the Inverse and Its Application to
Linear Programming* (1957), with a column-relative numerical threshold. It uses
matrices assembled by this simulator. The guarded SIMD specialization uses speculative
constant propagation, SSA liveness and replay through the existing scalar evaluator.
The block sampler uses Cranelift's public block map, POSIX interval timers and Linux
signal contexts. All implementations are original. The model files served as inputs,
and no new dependencies were introduced.

### Verilog-A optimization round 5

The packed literal pool, constant folding of chain-rule factors, per-instance
binding reuse and analysis worker limits are original implementations using
the existing simulator and Cranelift public APIs. The direct-pivot schedule
specializes the existing Gilbert–Peierls refactorization. The cache checksum
accelerator implements FIPS PUB 180-4, sections 5 and 6.2, using the SHA,
SSSE3 and SSE4.1 instruction semantics exposed by Rust's x86-64 intrinsics.
The portable path uses the existing hmac-sha256 dependency. No third-party
source or model text was incorporated.
