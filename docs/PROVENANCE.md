# Provenance Record

This file records where each significant algorithm and each piece of non-original code comes
from. It exists to demonstrate that the project is an independent, clean-room implementation.

Rules: see [`CONTRIBUTING.md`](../CONTRIBUTING.md) §1–2. Add an entry in the same pull request
that introduces the code.

## Third-party code

| Path | Origin | License | Notes |
|---|---|---|---|
| — | — | — | No third-party code yet |

The generated [binary license inventory](../THIRD-PARTY-LICENSES.md) records
Cargo dependencies separately, including additional notices supplied by a crate.

## Module coverage

The paths below include all nested modules and their tests under `crates/`. Each
source header identifies its specific origin. The algorithm records below give
the references for numerical methods and model equations.

| Crate | Origin and scope |
| --- | --- |
| `sansho-diag` | Original structured diagnostic types, codes, suggestions and registry tests. |
| `sansho-netlist` | Original SPICE frontend, expression and measurement parsers, preprocessing, statistical bindings and writer. |
| `sansho-devices` | Original model equations, automatic differentiation and terminal stamps from the device references below. The Verilog-A adapter uses the local compiler. |
| `sansho-sparse` | Original matrix layout, ordering, refactorization, scheduling and solver adapters. |
| `sansho-engine` | Original circuit elaboration, analyses, nonlinear solve, parallel assembly, lint, measurements and statistical runs. |
| `sansho-output` | Original results, waveform serialization and raw reader/writer from the public SPICE raw layout. |
| `sansho-va` | Original analog compiler, interpreter, JIT lowering, private cache, fingerprint and tests. Standard headers are locally authored declarations from the LRM and published constants. |
| `sansho-api` | Original Rust sessions, job management, hashes, summaries, alterations and waveform queries. |
| `sansho-cli` | Original argument handling, reporting, batch execution, raw/JSON export and process-level tests. |
| `sansho-mcp` | Original stdio transport, tool schemas, session adapter and protocol tests from the MCP and JSON-RPC specifications. |
| `sansho-verify` | Original raw comparison, interpolation, reference-process drivers and harness tests. |

The examples, verification circuits, model cards, include wrappers, black-box
probe drivers and benchmark generators are original project fixtures. External
PDKs and production Verilog-A models are supplied locally. The CI scripts,
Docker recipe, release workflow and Cargo metadata notice generator are project
tooling. Upstream legal texts in the notice inventory retain their attribution.

## Algorithms and references

| Area | Implementation | References |
|---|---|---|
| Verilog-A preprocessing, lexical grammar, AST and parser | `sansho-va/src` | Original implementation from knowledge of Accellera Verilog-AMS LRM 2.4 analog subset; recursive descent and precedence climbing from general compiler techniques |
| Verilog-A built-in disciplines and constants | `sansho-va/src/preproc.rs` | Original declarations from LRM nature/discipline semantics; mathematical constants, SI defining constants and CODATA 2022 values |
| Verilog-A test models | `sansho-va/tests` | Original resistor, junction and simplified square-law MOS examples; no third-party models |
| Netlist syntax, number scale factors | `sansho-netlist` | Nagel (1975), SPICE2 input format |
| MNA stamps for R, C, L, V, I, E, F, G, H | `sansho-devices/src/linear.rs`, `sources.rs` | Ho, Ruehli & Brennan (1975) |
| PULSE / SIN / EXP / PWL waveforms and defaults | `sansho-devices/src/waveform.rs` | Nagel (1975); commonly documented SPICE3 defaults |
| DAE form `dq/dt + f = b`, deterministic slot assembly | `sansho-devices`, `sansho-engine/src/solve.rs` | Original design (`docs/architecture.md` D1, D4) |
| Newton iteration, SPICE convergence test | `sansho-engine/src/solve.rs` | Nagel (1975) |
| Backward Euler / trapezoidal integration, breakpoint handling | `sansho-engine/src/analysis.rs` | Nagel (1975); standard numerical ODE texts. Device-local charge error estimates and the internal RC startup bound are original applications of divided differences and linearized KCL. |
| Topology checks (DC path, voltage-source loops) | `sansho-engine/src/circuit.rs` | Ho, Ruehli & Brennan (1975); union-find (textbook) |
| ASCII raw file layout | `sansho-output` | Publicly documented SPICE3/ngspice rawfile format |
| Large circuit BTF and real LU | `sansho-sparse/src/btf.rs` | Original alternating-path maximum transversal from Duff (1981), iterative strongly connected components from Tarjan (1972), and left-looking sparse elimination from Gilbert & Peierls (1988). Block triangular solves and threshold pivot reuse follow the algorithm descriptions in Davis & Palamadai Natarajan, *Algorithm 907*, ACM TOMS (2010). |
| Large circuit decks | `benches/large/gen.py` | Original row-wise unsigned multiplication, CMOS gates and RC lattice. The ring scales the repository's `big_ring` test circuit. The BSIM4 cards come from `verify/bsim4-decks/b4_inverter_tran.cir`. |
| BSIM4 intrinsic bypass | `sansho-devices/src/bsim4/mod.rs` | Original reuse of current and charge linearizations, following device bypass in Nagel's SPICE2 thesis (1975). |
| Quotient-graph ordering | `sansho-sparse/src/ordering.rs` | Original symmetric approximate degree implementation from Amestoy, Davis & Duff, “An Approximate Minimum Degree Ordering Algorithm” (1996), and George & Liu, “The Evolution of the Minimum Degree Ordering Algorithm” (1989). Breadth-first level separators follow George & Liu, *Computer Solution of Large Sparse Positive Definite Systems* (1981). |
| Real and complex sparse LU | `sansho-sparse/src/{btf,serial,complex}.rs` | Original Gilbert & Peierls (1988) elimination, fixed-pivot panel updates and triangular solves with elementary Cartesian complex arithmetic. |
| Dense generalized eigenvalues | `sansho-sparse/src/qz.rs` | Original Hessenberg–triangular reduction and double-shift real QZ from Golub & Van Loan, *Matrix Computations*, and Moler & Stewart, “An Algorithm for Generalized Matrix Eigenvalue Problems” (1973). |
| Forward-mode automatic differentiation | `sansho-devices/src/dual.rs` | Griewank & Walther, *Evaluating Derivatives* (SIAM, 2008) |
| pn-junction current, depletion charge with `fc` linearization, junction limiting, `vcrit` | `sansho-devices/src/junction.rs` | Nagel (1975); Antognetti & Massobrio (1993), ch. 1 |
| Diode (level 1) | `sansho-devices/src/diode.rs` | Massobrio & Antognetti (1993), ch. 1, and ngspice user manual §7.2.2 (reverse bias, adjusted avalanche knee and forward high injection) |
| VDMOS electrical core and integrated gate charge | `sansho-devices/src/vdmos.rs` | Shichman and Hodges (1968), ngspice user manual, VDMOS section. Independent integration of the joined Miller-capacitance curve. Default capacitances, smooth overdrive and mobility denominator checked against the R8 black-box sweeps. |
| MESFET channel and effective gate potential | `sansho-devices/src/jfet.rs` | H. Statz et al., “GaAs FET Device and Circuit Simulation in SPICE,” IEEE TED 34 (1987), 160–169. Incremental rectangular integration from accepted biases, checked against the 44.2 loaded AC gain and transient. |
| BJT epitaxial collector | `sansho-devices/src/bjt.rs` | G. M. Kull et al., “A Unified Circuit Model for Bipolar Transistors Including Quasi-Saturation Effects,” IEEE TED 32 (1985). Conductivity modulation and storage. The high-field denominator and vertical reverse-diffusion area were reconstructed from original R8 VO, GAMMA and loaded RCO sweeps against ngspice 44.2. [Measurements](../verify/probes/README.md#r8b-epitaxial-collector-and-urc) |
| LTRA transfer and history convolution | `sansho-devices/src/ltra.rs`, `sansho-engine/src/analysis.rs` | Roychowdhury and Pederson, DAC (1991), distributed lossy interconnect convolution. Independent inverse transforms, spectral/image sums and quadrature. |
| URC graded ladder and diode temperature coefficients | `sansho-devices/src/urc.rs`, `diode.rs` | ngspice user manual, uniform distributed RC and junction diode sections. The R8 FMAX and explicit-N black-box sweeps resolve count truncation and inverse-area access resistance. The diode TCV sign and units derive from the maintainer’s paired 100 °C measurement. The four TBV runs and their warnings establish that 44.2 ignores both parameters. See [distributed lines](distributed-lines.md) and [semiconductor coverage](r8-semiconductors.md). |
| BJT Gummel–Poon | `sansho-devices/src/bjt.rs` | Gummel & Poon (1970); Getreu (1976); Antognetti & Massobrio (1993), ch. 2 |
| Silicon MOSFET levels 8 and 49 | `sansho-devices/src/bsim3/` | Original derivation from UC Berkeley, *BSIM3v3.3 Manual* (2005), chapters 2–5 and 8–9, Appendices A, B and D. The R5 decks and numerical tests use original parameter choices. [Equation choices and coverage](bsim3.md) |
| Silicon MOSFET level 54, parameter catalog and device tests | `sansho-devices/src/bsim4/` | Original Rust from UC Berkeley, *BSIM4 4.8.2 Technical Manual*, ch. 1–15 and Appendix A, and published update notes (CMC/ECL-2.0 documentation). The local notes contain an embedded implementation excerpt encountered during document inspection. Implementation files and excerpts were not used to derive the Rust equations. |
| SKY130 nominal library binding | `sansho-netlist/src/{preproc,parser,expr}.rs` | Original implementation. Local Apache-2.0 SKY130 model cards informed syntax and the maintainer's ngspice 44.2 waveforms supplied black-box comparisons. |
| MOSFET level 1, Meyer capacitances | `sansho-devices/src/mos1.rs` | Shichman & Hodges (1968); Meyer (1971); Antognetti & Massobrio (1993), ch. 4; FET step limiting is original |
| Gmin stepping, source stepping | `sansho-engine/src/analysis.rs` | Standard continuation methods (e.g. Nagel 1975; Vlach & Singhal, *Computer Methods for Circuit Analysis and Design*, 1983); adaptive schedules are original |
| LTE step control, variable-step BDF2 | `sansho-engine/src/analysis.rs` | Nagel (1975); Brayton, Gustavson & Hachtel, "A new efficient algorithm for solving differential-algebraic systems using implicit BDF", Proc. IEEE 60 (1972) |
| `.meas` evaluation | `sansho-netlist/src/meas.rs`, `sansho-engine/src/measure.rs` | Original; card syntax as commonly documented for SPICE-family simulators |
| Parallel jobs (L0), chunked device evaluation (L1), ordered gather assembly (L2) | `sansho-engine/src/parallel.rs`, `circuit.rs`, `solve.rs` | Original design (`docs/architecture.md` D4); `rayon` crate (MIT/Apache-2.0) as a library dependency |
| Diagnostic code registry, lint checks, "did you mean" (Levenshtein distance) | `sansho-diag/src/codes.rs`, `sansho-engine/src/lint.rs`, `sansho-netlist/src/parser.rs` | Original; edit distance as in V. I. Levenshtein (1966), textbook dynamic programming |
| Parameter expressions, `.param`, `.func` | `sansho-netlist/src/expr.rs` | Original recursive-descent parser; operator set and functions as commonly documented for SPICE parameter expressions (ngspice user manual, used as documentation only) |
| `.include`/`.lib`, subcircuit flattening, model binning, `.ic`/`.nodeset`/UIC | `sansho-netlist/src/preproc.rs`, `parser.rs`, `sansho-engine/src/analysis.rs` | Nagel (1975) input format; hierarchical naming and library sections as documented in the ngspice user manual; node clamping is a standard Norton-equivalent technique |
| Netlist writer | `sansho-netlist/src/writer.rs` | Original; SPICE input format (Nagel 1975) |
| Session / job API, summaries, alterations | `sansho-api` | Original |
| Input / result hashes | `sansho-api/src/hash.rs` | FNV-1a by G. Fowler, L. C. Noll, K.-P. Vo (public domain; IETF draft-eastlake-fnv) |
| Waveform downsampling (LTTB) | `sansho-api/src/waveform.rs` | S. Steinarsson, *Downsampling Time Series for Visual Representation*, MSc thesis, University of Iceland, 2013 |
| MCP server | `sansho-mcp` | Model Context Protocol specification (modelcontextprotocol.io, revisions 2024-11-05 – 2025-11-25) and JSON-RPC 2.0 specification; no SDK code used |
| ASCII raw reader, verification metric | `sansho-output/src/raw_reader.rs`, `sansho-verify` | Original; public rawfile layout |

Reference simulators (ngspice) are only ever executed as external programs on our own decks
(`tools/ngspice`, `sansho-verify`); their sources are not read.

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

- `sansho-va/src/sema.rs` and `ir.rs`: original semantic analysis and resolved typed-tree lowering
  from the Accellera Verilog-AMS LRM 2.4 and general compiler techniques. No external compiler
  implementation was consulted or ported.
- `sansho-va/src/dual.rs` and `eval.rs`: original dynamic forward-mode differentiation from the
  chain rule and general AD literature; DAE current/charge stamping from elementary modified
  nodal analysis. Built-in derivatives are elementary analytic derivatives.
- `sansho-va/src/limit.rs`: original logarithmic junction limiter based on the published junction
  voltage limiting idea in L. W. Nagel, *SPICE2*, ERL Memo M520 (1975). The field-effect voltage
  step clamp is an original heuristic, not a port of another simulator's limiter.
- `sansho-va/tests/evaluator.rs`: models and test expectations authored for this change from
  elementary resistor, exponential junction and square-law charge equations; central finite
  differences provide independent derivative checks. No PDK files are included.

VA-2 was implemented without network access, new dependencies, external source code, generated
model code, or interface headers. There is no third-party code port requiring a NOTICE addition.


### Verilog-A VA-3

| Component | Files | Provenance |
|---|---|---|
| Limiting linearization tangent | `sansho-va/src/dual.rs`, `eval.rs` | Original chain-rule AD implementation; first-order SPICE linearization from Nagel (1975) |
| Simulator adapter and dense stamping | `sansho-devices/src/va.rs`, `lib.rs`; `sansho-engine/src/circuit.rs` | Original repository DAE/MNA integration, Ho, Ruehli & Brennan (1975), Accellera Verilog-AMS LRM 2.4 |
| HDL cards, N instances and hierarchy | `sansho-netlist/src` | Original parser/writer work from the requested syntax and existing source-loader/flattening interfaces |
| Models and integration tests | `sansho-engine/tests/verilog_a.rs`, `tests/va/*`; `examples/va/diode.va`, `examples/va_diode.cir`; `sansho-api/tests/session.rs` | Authored resistor/capacitor and junction equations from repository built-ins, Nagel (1975), Antognetti & Massobrio (1993); analytic RC response |

Implemented without network access, external source implementations, interface headers, PDK
files or new third-party dependencies. No ported code requires a NOTICE addition.

### Verilog-A VA-4a

| Component | Files | Provenance |
|---|---|---|
| Built-in headers | `sansho-va/src/headers/constants.vams`, `disciplines.vams` | Original declarations from Accellera Verilog-AMS LRM 2.4. Math values follow the named mathematical identities. Physical values follow the SI definitions and CODATA 2022. |
| Binding and branch collapse | `sansho-va/src/eval/topology.rs`, `sema.rs`, `eval.rs`, `sansho-engine/src/circuit.rs` | Original constant propagation, union-find and MNA stamping from general compiler and circuit literature. Port current probes use an ideal series voltage source. Integration introduces a DAE state. |
| Collapse fixture | `sansho-va/tests/models/collapse.va` | Authored from Ohm's law and the Shockley diode equation. |
| Local model verification | `sansho-va/tests/production.rs`, `sansho-engine/tests/verilog_a.rs` | Original finite-difference harness and DC transfer test. Third-party models are external test inputs. |

### Verilog-A VA-5

The register compiler in `sansho-va/src/eval/program.rs` and `eval/program/` is an original
implementation of forward constant propagation, control-flow joins, local value numbering
and backwards liveness analysis from general compiler literature (Aho et al., *Compilers*).
The dense gradient rows, scratch arena and scalar chain-rule factors in `dual.rs` and
`math.rs` use forward-mode AD as described by Griewank and Walther, *Evaluating Derivatives*.
The 21-stage benchmark in `benches/psp_ring21.cir` extends the project's authored CMOS ring.

### Verilog-A VA-5b

The native backend in `crates/sansho-va/src/eval/program/jit.rs` and its submodules
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
callbacks and limiters come from Sansho. The straight-line partition in `jit/split.rs` uses original SSA
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
T. A. Davis, *Direct Methods for Sparse Linear Systems* (2006).

The two-instance evaluator uses standard SSA value numbering, postdominators and
masked SIMD execution through Cranelift's public IR and host-ISA APIs. Scalar math
uses the existing Sansho shims. Instance pairing, ordered stamp assembly, local model
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

### Verilog-A optimization round 6

The ideal-source constraint follows the modified nodal formulation of Ho,
Ruehli and Brennan (1975). Its use in the final elimination column follows
from the left-looking LU and triangular solves already implemented here.
The packed lowering uses the forward-mode chain rule with a zero seed for
a constrained voltage. The implementation, work-count instrumentation and
static stack-operand counter are original. The counter reads Sansho's emitted
machine code through Linux procfs and disassembles it with the system
objdump. Third-party models served only as simulator test inputs.

### M4 final verification and general time derivatives

The derivative operator lowering, genvar state allocation and analytic regression
circuits are original work from Accellera Verilog-AMS LRM 2.4, the repository's
modified nodal equations and the forward-mode chain rule. BSIM-CMG 111.2.1
(ECL-2.0) served as external compiler input. The known-difference directive and
the SKY130 terminal-waveform replay are original verification work using local
black-box results. R6 revised the original equation 14.5 choice using the cached ngspice 44.2 KU0 control and layout sweeps.

### M5-3 single-worker BSIM4

The terminal stamp and charge sampler use the BSIM4 4.8.2 manual equations
already implemented in this repository. The derivative reduction follows
translation invariance and the forward chain rule. Unequally spaced Newton
divided differences supply the transient predictor. The block ordering
implements minimum-degree elimination on a symmetric graph as described in
Davis, *Direct Methods for Sparse Linear Systems* (2006). The implementations
and performance probes are original.

### M5-4 parallel fixed-pivot LU

The BTF column dependency schedule and cached symbolic reach in
`sansho-sparse/src/btf.rs` and `btf/schedule.rs` are original implementations.
The numerical algorithm follows Gilbert and Peierls (1988) and Davis and
Palamadai Natarajan, Algorithm 907 (2010). The dependency forest and
independent work scheduling use the principles described by Liu (1990) and
Anderson and Saad (1989). The discarded separator-ordering experiment used
George (1973). The tile gather, charge-history buffer rotation, parallel LTE
and determinism fixtures extend the repository's existing algorithms. The
MOS1 charge-only specialization uses this repository's existing charge and
linearization equations. The sparse crate uses the workspace's existing
Rayon library.

### M5-5 statistical analysis

The netlist sampler, lexical binding plan and CPU batch runner are original Rust implementations. Parameter scopes and the absolute, relative, normal, uniform and two-point distributions follow public ngspice user documentation on `.param` and statistical functions. The external count selects repeated statistical instances of one deck, following the public SPICE Monte Carlo analysis convention.

The integer finalizer follows Steele, Lea and Flood, *Fast Splittable Pseudorandom Number Generators* (OOPSLA 2014). Stable string identities use FNV-1a arithmetic. Indexing the random function by independent run and call-site counters follows the design principle in Salmon et al., *Parallel Random Numbers: As Easy as 1, 2, 3* (SC 2011). The normal transform follows Box and Muller, *A Note on the Generation of Random Normal Deviates* (1958). Sample variance uses Welford's online recurrence. The independent sample-variance ratio uses the standard F distribution and the incomplete beta continued fraction.

All three validation circuits and their reference drivers were authored for this repository. The SKY130 deck references the locally installed Volare model files. The numeric solvers share an immutable symbolic ordering across runs.

### R1 ngspice-dialect devices and temperature

The element cards and control quantities follow the public ngspice user manual
descriptions of coupled inductors, nonlinear dependent sources, switches,
temperature options and transient initial conditions. The coupled flux and
controlled-source stamps follow Ho, Ruehli and Brennan, *The Modified Nodal
Approach to Network Analysis* (1975). The expression evaluator extends the
repository's forward-mode dual-number implementation. The switch configuration
iteration and crossing-time interpolation are original implementations.

The temperature translations in `temperature.rs`, `diode.rs`, `bjt.rs` and
`mos1.rs` follow the silicon junction, Gummel–Poon and level-1 MOS models
in Massobrio and Antognetti, *Semiconductor Device Modeling with SPICE* (1993).
The BSIM4 equations retain their existing provenance. The R1 verification
circuits and analytic regression tests were written for this project.

### R2 small-signal and Fourier analyses

The netlist syntax, analysis terminology and conventional vector names follow the
public ngspice user manual descriptions of NOISE, TF, SENS, FOUR and PZ. The
adjoint noise formulation follows Rohrer, Nagel, Meyer and Weber, “Computationally
Efficient Electronic-Circuit Noise Calculations” (1971). The DC sensitivity
contraction follows Director and Rohrer (1969). The MNA transfer and bordered
pole-zero pencils follow the network equations in Vlach and Singhal, *Computer
Methods for Circuit Analysis and Design*. The Fourier implementation uses the
real-signal discrete Fourier transform. The original dense QZ kernel and its references appear in the algorithm table above.

The built-in noise sources use the Johnson–Nyquist, shot and empirical flicker
laws, the Berkeley BSIM4 4.8.2 Technical Manual chapter 10 and Appendix A.10,
and the Accellera Verilog-AMS 2.4 LRM small-signal noise functions. The device
adapters, source projection, parameter perturbations, integration, tests and
R2 verification circuits are original implementations.

MOS1 oxide thickness, effective geometry and KF/AF parameters follow the
[ngspice 44 user manual, §7.6.2.5](https://ngspice.sourceforge.io/docs/ngspice-44-manual.pdf#page=159).
The table lists the parameters and defaults. The gate-area power law and
conversion through gm² are also given in Bucher et al., *The EPFL-EKV MOSFET
Model Equations for Simulation*, eq. 94. The choice of gate-referred form for
the R2 deck was checked against the maintainer's 44.2 simulation output.
[Gate-area noise equation](https://ngspice.sourceforge.io/external-documents/models/ekv_v262.pdf)

The BSIM4 source-side partition and flicker normalization follow the
[Berkeley BSIM4 4.8.2 Technical Manual](https://bsim.berkeley.edu/models/bsim4/),
equations 10.2–14 and Appendix A.10. The zero-resistance reduction uses
the device's own current and charge derivatives. The verifier matches complex
pole and zero multisets by minimizing the largest normalized distance using
bipartite augmenting paths.

### R3 lines, JFETs, classical MOS channels and polynomials

The two traveling-wave equations in `sansho-devices/src/line.rs` follow F. H.
Branin, “Transient Analysis of Lossless Transmission Lines” (1967). Port
stamps, accepted-step history, corner propagation, interpolation and the
complex matrix correction are original Rust implementations.

The level-one JFET uses the channel and depletion-junction description in
Massobrio and Antognetti, *Semiconductor Device Modeling with SPICE*. The
classical MOS channel reference is A. Vladimirescu and S. Liu, *The Simulation
of MOS Integrated Circuits Using SPICE2*, UCB/ERL M80/7 (1980). The level-two
current integrates inversion charge along the channel. The level-three current
uses the bulk-charge slope and field-dependent mobility. Both reuse the
existing Shichman–Hodges terminal conventions and Meyer charge implementation.
Implemented terms and coverage limits are in [compatibility.md](compatibility.md).

The J, T and POLY cards use the published ngspice element and model syntax.
Graded polynomial enumeration, directional differentiation, numeric tests
and the R3 decks were written for this project. No third-party device
implementation or reference deck was imported.

### R4 device and analysis changes

The source and analysis card syntax, measurement operators, parameter names and
raw plot conventions follow the public ngspice user documentation. Sweep execution,
column serialization, measurement expression binding, interpolation, device stamps
and the small verification decks are original implementations. Cached 44.2 outputs
provide the comparisons recorded in the verification notes.

The MOS2 and MOS3 channel reference is Vladimirescu and Liu, UCB/ERL M80/7
(1980), with the classical MOS treatment in Massobrio and Antognetti. The
level-two velocity boundary equates integrated channel current and drain-end
inversion charge times saturation velocity. Beyond saturation the positive root
of the depletion voltage drop gives the channel extension. The level-three model
uses the lateral junction depletion estimate, charge sharing and a finite drain
field at the saturation knee.

The bipolar charge-control model follows Gummel and Poon (1970), Getreu,
*Modeling the Bipolar Transistor*, and Massobrio and Antognetti. The transient
forward excess phase follows the second-order transport delay approximation of
Weil and McNamee (1978). The frequency-domain stamp uses the phase of the delay
directly. Diode recombination, high injection, breakdown and perimeter charge use
Massobrio and Antognetti, chapter 1 (especially §§1.1, 1.8.1 and 1.9.1), and
the ngspice manual, §7.2.2 (area/perimeter scaling, Algorithm 7.1 and equations
7.1–7.3). The modulation waveforms follow §§4.1.5–4.1.6 of the same manual. The stamp derivatives use the repository's dual numbers.


### R6 compact-model compatibility

The NOIC default, KU0 mobility response, BSIM3 gate–bulk overlap default, accumulation-centroid doping and area-only junction current derive from black-box comparisons with ngspice 44.2 and independently authored Sansho control decks. The device notes give the measured quantities and scope. The original noise and breakdown probes establish the BSIM3 channel-density endpoints, weak/strong flicker blend, EM switch and capMod 3 weak-inversion smoothing, and the SPICE3 diode bottom-current knee match, sidewall prefactor and low-IBV clamp. [BSIM3](bsim3.md#verification-status) · [BSIM4](bsim4.md#mobility-stress) · [Probe measurements](../verify/probes/README.md)

## R7 netlist and output support

The syntax and device quantities target the ngspice user manual sections on element cards, independent sources, DC and transient analysis, output variables, batch commands and simulator options. The R7 implementation reuses Sansho's existing clean-room compact-model equations and analytic derivatives. The R7b body-effect defaults use the BSIM3v3.3 and BSIM4.8.2 manual doping relations for GAMMA1/2 and VBX. The default-card sweeps and the UIC grid replay use the maintainer’s ngspice 44.2 raw files. No model source was imported.

| Component | Files | Basis |
|---|---|---|
| Device output sampling and UIC | `sansho-engine/src/probes.rs`, `analysis.rs`, `sansho-devices/src/observables.rs` and the respective device groups | Device-local charge histories, MNA current balance, existing BSIM3v3.3 and BSIM4.8.2 terminal charge equations. |
| Behavioral passives | Netlist preprocessing and parsing, `sansho-devices/src/behavioral.rs` | Differential definitions of capacitance and inductance, Kirchhoff’s current law and forward differentiation. |
| Transient randomness | `sansho-devices/src/random.rs`, source and waveform modules | Original indexed integer mixer, Box and Muller (1958), exponential waiting times, octave-band synthesis, and Hörmann (1993), “The transformed rejection method for generating Poisson random variables.” |
| Output cards, options and tests | Netlist, output, CLI and engine crates, `sansho-engine/tests/outputs.rs`, MCP stdio tests and `verify/decks/r7_*.cir` | Original circuits and implementation. Tests use circuit laws, finite differences, deterministic replay and distribution properties. |

## R9 RF, distortion and device equations

| Component | Basis |
|---|---|
| RF (`sansho-engine/src/rf.rs`) | K. Kurokawa, “Power Waves and the Scattering Matrix,” IEEE Transactions on Microwave Theory and Techniques 13 (1965), 194–202. Two-port noise factor and optimum reflection coefficient follow the noise-wave formalism in D. M. Pozar, *Microwave Engineering*. Original matrix solves and covariance calculation. SP plot layout, Norton noise correlation, 1 V circuit traces and noise normalization were checked against maintainer-supplied 44.2 black-box outputs. The R9 matched-pad sweep separates circuit-temperature normalization from a fixed reference by varying the pad and circuit temperature together. |
| Distortion (engine and device `distortion.rs`) | P. Wambacq and W. Sansen, *Distortion Analysis of Analog Integrated Circuits*, Kluwer, 1998. Mixed analytic partials use forward differentiation as in A. Griewank and A. Walther, *Evaluating Derivatives*, SIAM, 2008. The local current and charge laws come from the Sansho diode, Gummel–Poon and Meyer MOS1 implementations and their cited literature. |
| MOS6 | T. Sakurai and A. R. Newton, “A Simple MOSFET Model for Circuit Analysis,” IEEE Transactions on Electron Devices 38 (1991), 887–894, and their alpha-power-law model, IEEE Journal of Solid-State Circuits 25 (1990), 584–594. |
| VDMOS | Existing clean-room square-law, junction and Miller equations. Original R9 through R9d circuits and maintainer-supplied 44.2 traces. Inverting the saturation and triode laws identifies the gate intercept and drain resistance. Heat injection, case steps, clamp currents and paired transfers identify thermal storage, the default ambient boundary, mobility and source access loss. Open and clamped case sweeps resolve both RTHCA terms. At a fixed 30 K rise the access branch isolates the hot RD multiplier. The disabled-channel sweep measures the RB heat contribution. [Probe evidence](../verify/probes/README.md#r9d-thermal-probe-results) |
| Cards, results and fixtures | ngspice user manual is the syntax and analysis compatibility target. All R9 circuits, tests and Rust implementations were written for Sansho. No device implementation was imported. |
