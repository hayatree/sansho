# Architecture Design (v0.1 draft)

- Date: 2026-11-17
- Status: Draft
- Background: private OSS landscape survey (not included in the repository)

> **Working name:** The survey found `JaimeHW/RSpice` (non-commercial license). To avoid a name
> collision, this document uses the provisional crate prefix `spx-`. The final name will be chosen
> after trademark and crates.io checks.

---

## 1. Goals and non-goals

### 1.1 Goals (in priority order)

1. **Accuracy** on par with ngspice / Xyce (quantitative criteria in §9). Preserve SPICE tolerance
   semantics (reltol / abstol / vntol / chgtol).
2. **Speed**: at least ngspice+KLU on a single core; clearly faster with multiple threads.
3. **Agent native**: structured I/O, machine-readable diagnostics, deterministic results, a session
   API and MCP as first-class engine features.
4. **Parallelism**: (a) across independent runs (sweeps / corners / Monte Carlo) and (b) within a
   single run. Both **bit-deterministic**.
5. **GPU**: first, batched runs of an identical topology (MC / corners). Single-run GPU comes later.
6. **Compatibility**: ngspice-dialect netlists and open PDKs (SKY130 / GF180MCU / IHP SG13G2).
7. **License**: Apache-2.0 OR MIT, developed clean-room (§11).

### 1.2 Non-goals (for now)

- GUI / schematic editor (use Xschem etc.)
- fast-SPICE approximations that trade accuracy (table models, partitioned approximations)
- Mixed-signal digital (XSPICE equivalent), device simulation (CIDER equivalent)

---

## 2. Core design decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Represent every device in **DAE form** `d/dt q(x) + f(x) = b(t)` | Integrator-agnostic (BDF / TR / TR-BDF2 / Rosenbrock are swappable). AC falls out as `G + jωC`; maps directly to HB / PSS / sensitivity. Matches the OSDI interface (resistive/reactive residuals and Jacobians). |
| D2 | **Fix the matrix pattern at elaboration** and precompute each device's stamp targets as indices into the value array | No runtime hashing or insertion. Symbolic factorization (ordering) is reusable. Suits GPU offload. |
| D3 | Store devices as **SoA batches per model type** | Batch evaluation via SIMD, threads, or GPU. Dynamic dispatch happens per group, not per instance. |
| D4 | **Deterministic reduction**: devices write contributions to slot arrays, which are summed into the matrix in a fixed order | Bit-identical results regardless of thread count (agent reproducibility, stable regression tests). |
| D5 | Linear solver behind a trait. Initial backend: faer. Target: our own KLU-style solver (BTF + AMD + Gilbert–Peierls + pivot-reuse refactor) | Avoids linking KLU (LGPL). Circuit-specific refactorization is key to speed. |
| D6 | **The library is the product**; CLI / MCP / Python are thin shells | Agent-native requirements (sessions, structured results) are met at the library API level. |
| D7 | f64 everywhere; Complex<f64> for AC. Options may loosen tolerances but never internal precision | Accuracy is the top priority. |
| D8 | The library never panics; everything returns `Result<_, Diagnostic>` | Agents can handle failures structurally. |

---

## 3. System overview

```
           ┌─────────────┐  ┌────────────┐  ┌────────────┐
 Front     │  spx-cli    │  │  spx-mcp   │  │  spx-py    │  (thin shells)
           └──────┬──────┘  └─────┬──────┘  └─────┬──────┘
                  └───────────────┼───────────────┘
           ┌──────────────────────▼───────────────────────┐
 API       │ spx-api : Session / Job / Result / Diagnostic │
           └──────┬───────────────────────────┬───────────┘
           ┌──────▼──────┐             ┌──────▼──────┐
 Input     │ spx-netlist │  ───AST───▶ │ spx-elab    │ ──▶ Circuit IR
           │ (parse)     │             │ (flatten,   │
           └─────────────┘             │  params)    │
                                       └──────┬──────┘
           ┌──────────────────────────────────▼───────────┐
 Engine    │ spx-engine : MNA assembly / Newton / analyses │
           │   OP · DC sweep · AC · TRAN · (Noise · PSS/HB)│
           └───┬──────────────┬───────────────┬───────────┘
           ┌───▼────────┐ ┌───▼──────────┐ ┌──▼──────────┐
 Kernel    │ spx-devices│ │ spx-sparse   │ │ spx-output  │
           │ builtin +  │ │ LU / order / │ │ waveform,   │
           │ spx-osdi   │ │ solve        │ │ raw, .meas  │
           └───┬────────┘ └──────────────┘ └─────────────┘
           ┌───▼────────┐
 Accel     │ spx-gpu    │ (batched device eval, optional feature)
           └────────────┘
           spx-verify : regression / accuracy harness (runs ngspice/Xyce externally as oracles)
```

### 3.1 Crates

| Crate | Responsibility | Main external deps (all permissive) |
|---|---|---|
| `spx-netlist` | Lexing / parsing. Lossless AST with line/column spans. `.include/.lib/.param/.func/.subckt/.model`, expression evaluation | none (hand-written parser) |
| `spx-elab` | Subcircuit flattening, parameter resolution, model binning, node numbering, topology checks (floating nodes, V loops, I cutsets) | — |
| `spx-ir` | Post-elaboration circuit IR. JSON schema via serde (also accepted as agent input) | serde, schemars |
| `spx-sparse` | CSC pattern, AMD/COLAMD, BTF, LU (real & complex), refactor, triangular solves, `LinearSolver` trait | faer (initial backend) |
| `spx-devices` | `DeviceGroup` trait. Built-ins (R/L/C/K/V/I/E/F/G/H/switches/diode/BJT (GP)/MOS1/BSIM4) | — |
| `spx-va` | Clean-room Verilog-A analog subset, binding specialization, register interpretation and optional native evaluation | Cranelift (optional) |
| `spx-osdi` | OSDI 0.3/0.4 loader (independently implemented from the spec); exposes `.osdi` models as `DeviceGroup` | libloading |
| `spx-engine` | MNA assembly, Newton, homotopy, time integration, analyses | rayon |
| `spx-output` | Columnar waveform store, ngspice raw writer (ASCII/binary), `.meas` evaluation | — |
| `spx-api` | Session / Job API, structured results, diagnostics, determinism hash | rayon |
| `spx-cli` | `spx run deck.cir`, `--json`, ngspice-like batch mode | clap |
| `spx-mcp` | MCP server (stdio; HTTP later) | serde_json (no SDK) |
| `spx-py` | Python bindings | pyo3 |
| `spx-gpu` | Batched device evaluation (feature flag) | wgpu / cudarc (dynamic loading) |
| `spx-verify` | Regression suites, oracle comparison, report generation | — |

---

## 4. Data model

### 4.1 Pipeline

```
text ──parse──▶ AST(spans) ──elaborate──▶ Circuit IR ──compile──▶ CompiledCircuit
                                              ▲                        │
                                   JSON (agent input)           SimState (per job)
```

- **Circuit IR**: node table, device instances (model reference + resolved parameters), analysis
  specs. Immutable and hashable.
- **CompiledCircuit**: immutable, shared across jobs (`Arc`).
  - `unknowns`: node voltages + branch currents (V sources, inductors, ...). Each unknown carries a
    kind (voltage / current) used to apply tolerances.
  - `pattern`: CSC structure (union of the G and C patterns).
  - `groups: Vec<Box<dyn DeviceGroup>>`: per-model-type batches.
  - `stamp_map`: group contribution slots → matrix value indices / RHS indices.
  - `symbolic`: ordering and BTF blocks (cached after the first factorization).
- **SimState**: mutable, one per job. Solution history, device state (q history, previous voltages
  for limiting), numeric factorization.

### 4.2 `DeviceGroup` trait (sketch)

```rust
pub trait DeviceGroup: Send + Sync {
    /// Elaboration: declare required unknowns (internal nodes, etc.) and nonzero positions.
    fn setup(&mut self, b: &mut SetupBuilder) -> Result<(), Diagnostic>;

    /// Evaluate f, q and their Jacobians G = ∂f/∂x, C = ∂q/∂x at x, writing into this group's
    /// private slot buffer (never directly into the shared matrix; see D4).
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: &mut GroupState, out: &mut Slots);

    /// Newton voltage limiting (pnjlim/fetlim equivalents). Returns true if x_new was modified.
    fn limit(&self, x_new: &mut [f64], x_old: &[f64], st: &GroupState) -> bool { false }

    /// Charge/flux states to be integrated, exposed for LTE estimation.
    fn charges(&self, st: &GroupState) -> &[f64];

    /// Noise sources (added later).
    fn noise(&self, ...) {}
}
```

- Built-in models are SoA (one `Vec<f64>` per parameter) and evaluated in parallel over instance
  chunks.
- OSDI models: `OsdiGroup` reads the `.osdi` descriptor and implements the same trait.

### 4.3 Deterministic assembly (D4)

1. Each group writes contributions into `Slots { g: [f64], c: [f64], f: [f64], q: [f64] }`. Slot
   positions within a group are fixed.
2. The gather map records source slots in their original accumulation order. Arrays with up to
   2,048 destinations use a per-group scatter table. Larger arrays use a CSR-style table per destination.
3. The summation can be parallelized per matrix value k (each k is independent; its summation order
   is fixed).

Results are therefore bit-identical regardless of thread count or scheduling. Extra memory is about
8 bytes × number of contributions.

Elaboration fixes the chunk size at 1,024 for linear devices, 64 for built-in nonlinear devices
and two or six for interpreted VA devices, with the larger size at 32 instances. Native VA
circuits use two below eight instances, four from eight to 31, and eight from 32 upwards. Each
instance has reusable local inputs and an evaluator workspace. Scalar evaluation writes directly
into the final stamp slots. Native transient circuits with at least eight VA devices can pair equal
specializations within a chunk. Their outputs are deinterleaved first. Device evaluation uses Rayon when the circuit
has at least eight VA instances or 256 nonlinear instances. Every value of `G`, `C`, `f`, `q`
and `b` is gathered in its original source order. The VA assembly test compares the bits of every
time point and signal on one, two and five threads.

### 4.4 Native Verilog-A evaluation (VA-5b)

The optional `jit` feature on spx-cli, spx-verify, spx-engine and spx-devices
forwards to spx-va. The scalar specializer lowers into the same register program
used by the interpreter. Cranelift compiles the bias-dependent equations into a
host function with one scalar SSA value per live partial. A compiled module owns
its finalized code and specialization cache. The device adapter prepares kernels
in parallel for distinct bindings during elaboration. Unix JIT builds can restore
validated typed modules, specialized register programs and relocatable native code
from a per-user disk cache. Long straight-line kernels can span two functions
with per-instance scratch. Each instance supplies its own voltages, retained history,
limiter state and output slots. This preserves the ordered gather in D4.

For a single native transient, the standalone run pool uses at most the larger
of the device-group count and the number of gather chunks. An embedding that
supplies a Rayon pool retains control of its workers. Section 13 of
[the Verilog-A design](verilog-a.md#13-va-5b-native-evaluation) covers the ABI,
backend selection and exceptional-value replay. Section 14 covers cache identity and
publication. Section 16 describes two-lane native circuit evaluation and timing measurements.

---

## 5. Numerical engine

### 5.1 Nonlinear solve (Newton)

- Residual `F(x) = f(x) + α·q(x) − r_hist − b` (α and r_hist supplied by the integrator), Jacobian
  `J = G + α·C`.
- SPICE-compatible convergence tests:
  - voltage unknowns: `|Δx| ≤ reltol·max(|x|,|x_prev|) + vntol`
  - current unknowns: `... + abstol`
  - plus per-device current convergence checks (SPICE `noncon` equivalent).
- Limiting: device-side. Devices linearize around a limited point `x̃` and return
  `f(x̃) + G·(x − x̃)` so the Newton step stays consistent; limiting marks the iteration as not
  converged (`Slots::noncon`). Junctions use Nagel's logarithmic limiting; FET control voltages use
  an original step clamp.
- Device Jacobians come from forward-mode dual numbers (`spx-devices/src/dual.rs`): model
  equations are written once and their derivatives are exact.
- Device state (`StateRef`): an `accepted` copy (last accepted solution, read-only during a solve)
  and a `current` copy (per iteration). Used for limiting history and incremental charge models
  (MOS1 Meyer capacitances).
- **Device bypass**: skip evaluation of instances whose terminal voltages changed less than a
  threshold (optional, off by default → enabled after accuracy validation).

### 5.2 DC operating point fallback chain

```
plain Newton → gmin stepping (diagonal gmin / junction gmin) → source stepping
 → pseudo-transient → on failure: structured diagnostic (§7.3)
```

Every attempt is recorded in the diagnostic so agents can see where it got stuck.

Implemented in M2: plain Newton, diagonal gmin stepping (adaptive factor, ending with a solve at
zero shunt), adaptive source stepping. Junction gmin is always present. Pseudo-transient is
planned.

### 5.3 Transient analysis

- Integration methods behind an `Integrator` trait:
  - Initial: **trapezoidal + BDF2 (Gear2)** — SPICE-compatible defaults and the accuracy baseline.
  - Later: TR-BDF2, variable-order BDF (1–6); Rosenbrock-type (W-methods) as research track.
- Step control: charge-based LTE (preserving SPICE `trtol` / `chgtol` semantics), breakpoint
  management (source corners, `.tran` tstop/tmax), reduced step after breakpoints, step ÷8 on Newton
  failure.
- Initial conditions: OP → `.ic` / `UIC`, `.nodeset` (planned).

Implemented in M2: trapezoidal (default) and variable-step BDF2 (`.options method=gear`);
backward Euler for the first steps after t = 0 and after every breakpoint (history is reset there);
LTE from divided differences of the assembled charge vector with SPICE `trtol`/`chgtol`/`reltol`
semantics; rejected steps shrink by at least 10 %, accepted steps grow by at most 2×.

### 5.4 Small-signal analyses

- **AC**: solve `G + jωC` at the OP with complex LU, sharing the symbolic factorization (same
  pattern).
- **Noise**: adjoint method (later).
- **Sensitivity**: adjoint DC/AC sensitivities (later; useful for agent-driven optimization loops).

### 5.5 Periodic steady state (later)

Shooting-method PSS and HB. Thanks to the DAE form (D1), devices need no additional code.

---

## 6. Linear solver (`spx-sparse`)

```rust
pub trait LinearSolver<T> {
    fn analyze(&mut self, pattern: &CscPattern) -> Result<()>;        // ordering, BTF, symbolic
    fn factor(&mut self, values: &[T]) -> Result<()>;                 // first time: partial pivoting
    fn refactor(&mut self, values: &[T]) -> Result<RefactorOutcome>;  // reuse pivots; fall back to factor if unstable
    fn solve(&self, rhs: &mut [T]) -> Result<()>;
}
```

| Stage | Implementation | Notes |
|---|---|---|
| S0 | faer sparse LU | Get running quickly; also the accuracy reference |
| S1 | Own KLU-style solver: BTF → per-block AMD → Gilbert–Peierls left-looking LU → pivot-reuse refactor | Implemented from papers (Davis & Palamadai 2010; Gilbert & Peierls 1988). SuiteSparse source is not consulted |
| S2 | Parallelism across BTF blocks; level-scheduled parallel refactor | Techniques from NICSLU/CKTSO and Basker papers |
| S3 | Iterative methods for large power grids (preconditioned GMRES/CG); GPU (cuDSS as an optional plugin) | Optional |

Real Newton solves reuse the symbolic ordering and simplicial pivot sequence. After a pivoted
factorization, the solver records scatter offsets and ordered updates reachable from the active
CSC entries. Activating a previously omitted entry triggers fresh pivoting and expands the
support used for subsequent plans. Both solves traverse the same flat factor storage.
Each subsequent factorization checks the pivot against the remaining column entries and monitors
upper-factor growth. Ties, tiny pivots or excessive growth trigger fresh pivoting. The general
backend handles dense and supernodal patterns and complex solves.

---

## 7. Agent-native design

### 7.1 API (`spx-api`)

```rust
let mut s = Session::new(SessionOptions::default());
let c   = s.validate(text);                       // parse + elaborate + lint, nothing stored
let c   = s.load(text)?;                          // → circuit_id, input_hash, stats, warnings
let job = s.simulate(&c.circuit_id, None)?;       // or Some(&["tran 1n 1u".into()])
let w   = s.waveform(&job.job_id, "v(out)", None, Reduce::Auto, None, None, 200)?;
let m   = s.measure(&job.job_id, &["tran tr trig v(out) val=0.1 rise=1 targ v(out) val=0.9 rise=1".into()])?;
s.alter(&c.circuit_id, &[Change { target: "m1".into(), param: "w".into(), value: 2e-6 }])?;
let t   = s.sweep(&c.circuit_id, "rd", "value", &[5e3, 1e4, 2e4], &[])?;   // parallel (L0)
let x   = s.explain(&job.job_id)?;                // diagnostics + code documentation
let d   = s.export(&c.circuit_id)?;               // canonical deck, runs in ngspice
```

Alterations edit the netlist AST (instance values, device parameters, model parameters,
`.options`) and are re-elaborated on the next run; reusing the symbolic factorization across
alterations is planned with the own sparse solver (M5).

### 7.2 Output principles

- **Summary first**: default results are scalars (OP voltages, `.meas`), statistics and warnings.
  Waveforms are returned only on request, with optional downsampling (e.g. LTTB).
- Every result carries `run_id`, input hash, engine version and the full option set
  (reproducibility).
- Numbers carry units (`{"value":1.2e-9,"unit":"s"}`).

### 7.3 Diagnostics

```json
{
  "code": "E-CONV-OP-003",
  "severity": "error",
  "message": "DC operating point did not converge after gmin and source stepping",
  "span": {"file": "amp.cir", "line": 42, "col": 1},
  "culprits": [{"kind": "node", "name": "net17", "residual": 3.2e-3},
               {"kind": "device", "name": "M5", "reason": "Vgs oscillating"}],
  "attempts": ["newton", "gmin_stepping", "source_stepping"],
  "suggestions": [{"action": "add .nodeset v(net17)=0.9", "confidence": "medium"},
                  {"action": "check floating gate on M5"}]
}
```

- Pre-run lint: floating nodes, no DC path, V loops, unused models defined in the main file (once per source definition), suspicious units
  (e.g. `1M` vs `1Meg`), implausible values, default MOSFET geometry, `.meas` signals that do not
  exist (with "did you mean" suggestions), suspicious `.tran` steps.
- Error codes are stable IDs, each with documentation: the registry in
  `spx-diag/src/codes.rs` generates [`diagnostics.md`](diagnostics.md), and a test fails when a code
  is used without being registered.
- Convergence failures report the non-converged unknowns with their last value and update ratio,
  the devices attached to the worst node, the strategies tried, and suggestions (implemented in M3).

### 7.4 MCP server (`spx-mcp`)

| Tool | Purpose |
|---|---|
| `validate_netlist` | Parse + elaborate + lint only. Fast |
| `load_netlist` | Store a netlist in the session, return a `circuit_id` |
| `estimate_cost` | Unknowns, device count, estimated points, time and memory |
| `simulate` | Run analyses (optionally overriding the cards), return a summary |
| `get_waveform` | Fetch a signal over a range with a point budget (LTTB) |
| `measure` | Evaluate `.meas`-style measurements after the fact |
| `alter_and_rerun` | Rerun with parameter deltas |
| `sweep` | Parameter sweep run in parallel (L0), table of measurements |
| `explain_failure` | Diagnostic details and remedies, by job or by code |
| `export_deck` | Export the canonical ngspice deck (for cross-checking) |

The server implements the MCP stdio transport (JSON-RPC 2.0, one message per line, protocol
revisions 2024-11-05 through 2025-11-25) directly on `serde_json`, without an SDK: the protocol
surface needed is small, and this keeps startup fast and the dependency tree permissive. Tool
results carry both `structuredContent` and the same JSON as text. Request errors (unknown ids, bad
arguments) are tool results with `isError: true` and structured diagnostics; only protocol errors
use JSON-RPC errors. `monte_carlo` and an HTTP transport are planned.

Guarantees fast startup (single static binary) and determinism (same input → same hash → same
result). The M3 exit criterion is tested end to end in `crates/spx-mcp/tests/stdio.rs`.

---

## 8. Parallelism and GPU

| Layer | Target | Mechanism | When |
|---|---|---|---|
| L0 | Independent jobs (analyses of one netlist, batches of netlists) | Shared compiled circuit, one workspace per job, `std::thread::scope` with index-ordered results | M2 (done) |
| L1 | Device evaluation within a run | Parallel over group × chunk (D3/D4) | M3 (done) |
| L2 | Matrix assembly within a run | Parallel reduction per value index | M3 (done) |
| L3 | LU within a run | Parallel BTF blocks, parallel refactor | M5 |
| G1 | GPU batch | Same topology × N instances (MC): device eval on GPU; LU as CPU batch or cuDSS uniform batch | M5 |
| G2 | GPU-resident single run | Assembly through LU entirely on GPU. Research track | TBD |

- All levels run on a rayon pool. `run_with` uses up to `threads` workers.
  `run_in_current_pool` uses the pool the caller installed (to avoid clashing with agent-side
  parallelism), and a `Session` keeps one pool for its lifetime.
- L1 and L2 use the circuit-size thresholds in §4. The [performance report](performance.md) describes the VA benchmarks and timings.
- Measured (M3, 1001-stage MOS1 ring oscillator, 1595 Newton iterations): assembly 466 ms on 1
  thread, 219 ms on 4, 194 ms on 8; LU (serial until M5) ≈ 145 ms. Results are bit-identical.

---

## 9. Accuracy assurance and verification (`spx-verify`)

- **Oracles**: ngspice (BSD; installed and run in CI) as primary, Xyce as secondary — invoked only as
  external processes, no code imported. Circuits with analytic solutions (RC, RLC, linear networks)
  are compared against exact values.
- **Metric** (implemented in M2): per variable `max|y − y_ref| / (rtol·max|y_ref| + atol)`, i.e.
  the worst error relative to the reference full scale; pass if ≤ 1. TRAN results are linearly
  interpolated onto the reference time points, AC compares complex values, `.meas` scalars are
  compared relatively. Tolerances can be overridden per deck with `* verify:` comments. An
  RMS-based variant (`xyce_verify` style) can be added for long waveforms.
- **Running**: `cargo run -p spx-verify` runs every deck in `verify/decks` with spx and with
  ngspice in Docker (`tools/ngspice`); `--cached` reuses reference results.
- **Tiers**:
  1. Device level (I-V / C-V curves; Jacobian checked against finite differences)
  2. Small circuits
  3. Benchmarks (Quarles, CircuitSim90-style, C6288, ...)
  4. PDK testbenches
- **Reporting**: per corpus, counts of pass / fail / unsupported / oracle disagreement, published by CI.
- **Convergence check**: error must decrease monotonically as tolerances tighten.
- **Performance benchmarks**: continuous comparison against ngspice on fixed hardware (no comparison
  against commercial tools).

---

## 10. Compatibility

- **Netlists**: ngspice dialect first (PDKs depend on it), then the main parts of the HSPICE dialect;
  Spectre dialect later. The dialect is chosen via `LoadOpts`; after elaboration everything is the
  common IR.
- **Implemented ngspice dialect (M4, part 1)** — `spx-netlist/src/preproc.rs`, `expr.rs`:
  - `.include` / `.inc`, `.lib file section` with `.lib name` … `.endl` sections, resolved
    relative to the including file through a `SourceLoader` (`FsLoader` for files; `NoFiles` for
    text-only input such as MCP `netlist`, where includes are rejected with `E-PARSE-INCLUDE`).
  - `.param` (lazy, order-independent, cycle detection), `{expr}` / `'expr'` values, bare
    expressions after `=`, `.func`, built-in math functions, ternary and logical operators.
  - `.subckt name ports [params:] k=v` / `.ends`, `X` instances with parameter overrides, nesting,
    local models, `.global`. Flattened names follow SPICE3 conventions: node `x1.n`, element
    `m.x1.m1`, branch `i(v.x1.v1)`.
  - Model binning (`nch.1`, `nch.2`, … selected by `lmin <= l < lmax`, `wmin <= w < wmax`). Unlike
    ngspice, binning also works for level-1 models.
  - `.ic` (nodes held through 1e10 S during the transient operating point), `.nodeset` (held, then
    released), `.tran … uic`.
  - Nested `.if`, `.elseif`, `.else` and `.endif` select branches during flattening. The
    expression sees global parameters or the current subcircuit instance's overrides and
    defaults. Only the selected body loads includes and elaborates definitions and devices.
    Conditions also work inside `.lib` sections. Paths resolve from the containing file.
  - `temper` in a parameter expression is the circuit temperature in °C. Typical-corner
    `agauss(nominal, variation, sigma)` returns nominal when variation or sigma is zero.
    Active random variation reports a diagnostic. Unknown simulator options produce warnings.
  - `.hdl` loads VA modules independently of model-card order. An `N` instance matches the
    module type case-insensitively and accepts numeric and quoted parameter expressions.
    The simulator scales parallel copies at the terminals when it supplies an implicit
    multiplier. See the binding rules in [Verilog-A §11](verilog-a.md#11-va-4b-production-circuits).
  - Not yet: `.option scale`, `B` sources, `K` coupling, switches, `.temp` scaling,
    instance `ic=`.
- `.save v(node) i(vsrc)` selects exported raw and API vectors across analysis cards. Multiple
  cards accumulate selections. The independent scale and circuit ordering are preserved, and
  `.save all` exports all vectors. Both deck measurements and later session measurements have
  access to the complete traces. Raw files use the ngspice format readable by Xschem.
- Built-in MOS levels 1 and 54 use the same nonlinear slot assembly. Level 54 specializes
  the silicon model at binding time and differentiates terminal current and charge during
  evaluation. Its optional resistance networks allocate internal unknowns during setup.
  The parser binds selected geometry bins in the lexical scope of each instance.
  The coverage and SKY130 dialect rules are in [the BSIM4 model notes](bsim4.md).
  Production Verilog-A models are compiled by spx-va.

---

## 11. Legal and provenance (design constraints)

- **Allowed references**: papers, textbooks, public specifications (OSDI spec, general knowledge of
  SPICE syntax), BSD-licensed parts of ngspice (when ported, keep notices and list them in `NOTICE`).
- **Forbidden references**: source of Xyce / VACASK / Gnucap / OpenVAF / Qucs; LGPL/MPL parts of
  ngspice; manual text of commercial tools.
- Each module starts with a `// Provenance:` comment listing its references; these are collected in
  `docs/PROVENANCE.md`.
- `cargo-deny` restricts dependency licenses. The built-in BSIM4 equations are original Rust derived from the published manual. The SKY130 PDK is supplied externally.

---

## 12. Milestones

| M | Scope | Exit criteria |
|---|---|---|
| M0 | Repository foundation: workspace, licenses, CI, cargo-deny, PROVENANCE | CI green |
| M1 | Linear circuits: basic parser, elaboration, MNA, OP/DC/AC/TRAN (trapezoidal), faer LU, raw output | RC/RLC match analytic solutions and ngspice |
| M2 | Nonlinear: diode, BJT (GP), MOS1, Newton + limiting, gmin/source stepping, BDF2, LTE step control, `.meas`, L0 parallelism, JSON output | Small-circuit benchmarks within RMS error ≤ 1 vs ngspice (done: `verify/decks`) |
| M3 | Agent: Session API, diagnostics, lint, MCP server, deterministic L1/L2 parallelism | Design loop runs end-to-end via MCP (done: `spx-mcp/tests/stdio.rs`) |
| M4 | OSDI loader and PDKs: IHP SG13G2 (PSP103, ...), SKY130 (built-in level 54), broader ngspice dialect | PDK testbenches match ngspice (dialect part done: `verify/decks/{subckt_ring,lib_include,nodeset_latch,uic_rlc}`) |
| M5 | Performance: own KLU-style solver, bypass, parallel LU, GPU batch (G1) | ≥ 1.x× ngspice on C6288 single-core; further gains multi-core |
| M6 | Extensions: noise, sensitivity, PSS/HB, HSPICE dialect, Python | — |

---

## 13. Open questions

1. Final project name (trademark and crates.io checks)
2. MSRV (proposal: latest stable − 2)
3. ~~Choice of Rust MCP SDK~~ — resolved in M3: own implementation of the stdio transport (see §7.4)
4. ~~SKY130 BSIM4 delivery~~ — level 54 is a clean-room built-in in `spx-devices`.
5. First GPU backend: wgpu (portability) or CUDA (performance)
6. Spilling the waveform store to disk for large transients (Arrow / Parquet?)
