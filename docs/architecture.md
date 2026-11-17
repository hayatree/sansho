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
- Writing our own Verilog-A compiler (we load `.osdi` files produced by OpenVAF)
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
| `spx-devices` | `DeviceGroup` trait. Built-ins (R/L/C/K/V/I/E/F/G/H/switches/diode/BJT (GP)/MOS1/later BSIM4) | — |
| `spx-osdi` | OSDI 0.3/0.4 loader (independently implemented from the spec); exposes `.osdi` models as `DeviceGroup` | libloading |
| `spx-engine` | MNA assembly, Newton, homotopy, time integration, analyses | rayon |
| `spx-output` | Columnar waveform store, ngspice raw writer (ASCII/binary), `.meas` evaluation | — |
| `spx-api` | Session / Job API, structured results, diagnostics, determinism hash | — |
| `spx-cli` | `spx run deck.cir`, `--json`, ngspice-like batch mode | clap |
| `spx-mcp` | MCP server (stdio / HTTP) | Rust MCP SDK (license to be checked) |
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
2. `stamp_map` stores, in CSR-like form, "the ordered list of slots summed into matrix value k".
3. The summation can be parallelized per matrix value k (each k is independent; its summation order
   is fixed).

Results are therefore bit-identical regardless of thread count or scheduling. Extra memory is about
8 bytes × number of contributions.

---

## 5. Numerical engine

### 5.1 Nonlinear solve (Newton)

- Residual `F(x) = f(x) + α·q(x) − r_hist − b` (α and r_hist supplied by the integrator), Jacobian
  `J = G + α·C`.
- SPICE-compatible convergence tests:
  - voltage unknowns: `|Δx| ≤ reltol·max(|x|,|x_prev|) + vntol`
  - current unknowns: `... + abstol`
  - plus per-device current convergence checks (SPICE `noncon` equivalent).
- Limiting: device-side `limit()` (pnjlim/fetlim) and global damping.
- **Device bypass**: skip evaluation of instances whose terminal voltages changed less than a
  threshold (optional, off by default → enabled after accuracy validation).

### 5.2 DC operating point fallback chain

```
plain Newton → gmin stepping (diagonal gmin / junction gmin) → source stepping
 → pseudo-transient → on failure: structured diagnostic (§7.3)
```

Every attempt is recorded in the diagnostic so agents can see where it got stuck.

### 5.3 Transient analysis

- Integration methods behind an `Integrator` trait:
  - Initial: **trapezoidal + BDF2 (Gear2)** — SPICE-compatible defaults and the accuracy baseline.
  - Later: TR-BDF2, variable-order BDF (1–6); Rosenbrock-type (W-methods) as research track.
- Step control: charge-based LTE (preserving SPICE `trtol` / `chgtol` semantics), breakpoint
  management (source corners, `.tran` tstop/tmax), reduced step after breakpoints, step ÷8 on Newton
  failure.
- Initial conditions: OP → `.ic` / `UIC`, `.nodeset`.

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

`refactor` checks pivot stability and re-pivots when a pivot falls below a threshold. This choice is
also made deterministically.

---

## 7. Agent-native design

### 7.1 API (`spx-api`)

```rust
let mut s = Session::new(SessionOptions::default());
let ckt  = s.load_netlist(text, LoadOpts::ngspice())?;  // or s.load_ir(json)
let job  = s.run(&ckt, Analysis::Tran { .. })?;           // sync or async (JobHandle)
let v    = job.signal("v(out)")?;                         // columnar waveform
let m    = job.measure("rise_time v(out) 10% 90%")?;      // structured scalar
s.alter(&ckt, "M1", "w", 2e-6)?;                          // incremental change → rerun reusing symbolic
```

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

- Pre-run lint: floating nodes, no DC path, V loops, unused models, suspicious units
  (e.g. `1M` vs `1Meg`), etc.
- Error codes are stable IDs, each with documentation.

### 7.4 MCP server (`spx-mcp`)

| Tool | Purpose |
|---|---|
| `validate_netlist` | Parse + elaborate + lint only. Fast |
| `estimate_cost` | Unknowns, device count, estimated time and memory |
| `simulate` | Run an analysis, return a summary |
| `get_waveform` | Fetch signals over a range with a point budget |
| `measure` | Evaluate `.meas`-style measurements after the fact |
| `sweep` / `monte_carlo` | Async jobs (parallel layer L0) |
| `alter_and_rerun` | Rerun with parameter deltas |
| `explain_failure` | Diagnostic details and remedies |
| `export_deck` | Export ngspice / Xyce decks (for cross-checking) |

Guarantees fast startup (single static binary) and determinism (same input → same hash → same
result).

---

## 8. Parallelism and GPU

| Layer | Target | Mechanism | When |
|---|---|---|---|
| L0 | Independent jobs (sweeps, corners, MC) | Share `CompiledCircuit`, one `SimState` per job, rayon | M2 |
| L1 | Device evaluation within a run | Parallel over group × chunk (D3/D4) | M3 |
| L2 | Matrix assembly within a run | Parallel reduction per value index | M3 |
| L3 | LU within a run | Parallel BTF blocks, parallel refactor | M5 |
| G1 | GPU batch | Same topology × N instances (MC): device eval on GPU; LU as CPU batch or cuDSS uniform batch | M5 |
| G2 | GPU-resident single run | Assembly through LU entirely on GPU. Research track | TBD |

- The thread pool can be injected by the caller (to avoid clashing with agent-side parallelism).
- Parallelism is automatically suppressed for small circuits (thresholds set by benchmarks).

---

## 9. Accuracy assurance and verification (`spx-verify`)

- **Oracles**: ngspice (BSD; installed and run in CI) as primary, Xyce as secondary — invoked only as
  external processes, no code imported. Circuits with analytic solutions (RC, RLC, linear networks)
  are compared against exact values.
- **Metric** (Xyce `xyce_verify` style): `e_i = (y_ref − y)/(reltol·|y_ref| + abstol)`; pass if
  RMS ≤ 1. OP per node; AC magnitude and phase; TRAN with time interpolation; `.meas` scalars.
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
- **Output**: ngspice raw format (readable by Xschem and other existing tools).
- **Models**: built-ins start at Level 1. BSIM4 is ported from the Berkeley distribution into a
  separate crate (`spx-model-bsim4`, under its original license; see §11). Everything else is loaded
  via OSDI. PDKs are not bundled.

---

## 11. Legal and provenance (design constraints)

- **Allowed references**: papers, textbooks, public specifications (OSDI spec, general knowledge of
  SPICE syntax), BSD-licensed parts of ngspice (when ported, keep notices and list them in `NOTICE`).
- **Forbidden references**: source of Xyce / VACASK / Gnucap / OpenVAF / Qucs; LGPL/MPL parts of
  ngspice; manual text of commercial tools.
- Each module starts with a `// Provenance:` comment listing its references; these are collected in
  `docs/PROVENANCE.md`.
- `cargo-deny` restricts dependency licenses. Models with conditional licenses (BSIM4, etc.) live in
  separate crates and are excluded from default builds (feature flag).

---

## 12. Milestones

| M | Scope | Exit criteria |
|---|---|---|
| M0 | Repository foundation: workspace, licenses, CI, cargo-deny, PROVENANCE | CI green |
| M1 | Linear circuits: basic parser, elaboration, MNA, OP/DC/AC/TRAN (trapezoidal), faer LU, raw output | RC/RLC match analytic solutions and ngspice |
| M2 | Nonlinear: diode, BJT (GP), MOS1, Newton + limiting, gmin/source stepping, BDF2, LTE step control, `.meas`, L0 parallelism, JSON output | Small-circuit benchmarks within RMS error ≤ 1 vs ngspice |
| M3 | Agent: Session API, diagnostics, lint, MCP server, deterministic L1/L2 parallelism | Design loop runs end-to-end via MCP |
| M4 | OSDI loader and PDKs: IHP SG13G2 (PSP103, ...), SKY130 (ported BSIM4 or OSDI), broader ngspice dialect | PDK testbenches match ngspice |
| M5 | Performance: own KLU-style solver, bypass, parallel LU, GPU batch (G1) | ≥ 1.x× ngspice on C6288 single-core; further gains multi-core |
| M6 | Extensions: noise, sensitivity, PSS/HB, HSPICE dialect, Python | — |

---

## 13. Open questions

1. Final project name (trademark and crates.io checks)
2. MSRV (proposal: latest stable − 2)
3. Choice of Rust MCP SDK (license and maturity)
4. SKY130 BSIM4: port and build in, or OSDI only? (license conditions vs performance)
5. First GPU backend: wgpu (portability) or CUDA (performance)
6. Spilling the waveform store to disk for large transients (Arrow / Parquet?)
