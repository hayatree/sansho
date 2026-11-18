# Verilog-A compact-model compiler (`spx-va`)

VA-5 specializes compact-model equations to register instructions. Section 11 describes the reference circuits, §12 measures the register interpreter and §13 measures the optional Cranelift backend.

## 1. Purpose and constraints

PDK transistor models (BSIM4 for SKY130, PSP103 for IHP SG13G2, ...) are distributed as Verilog-A
source. spx compiles that source itself instead of loading OSDI shared objects, so that no
copyleft or all-rights-reserved interface code is involved.

**Clean-room rules (binding):**

- The only language reference is the *Accellera Verilog-AMS Language Reference Manual* (2.4,
  analog subset) and general compiler literature (dragon book, SSA papers, AD literature).
- Never read, fetch or reproduce code or headers from OpenVAF / OpenVAF-reloaded, ADMS, VACASK,
  Gnucap, Xyce, Qucs, ngspice's OSDI or ADMS-generated code, or `osdi.h`. Do not reproduce them
  from memory either.
- OpenVAF and ngspice may be **executed** as external black-box tools (Docker) to produce
  reference results; their sources are never consulted.
- Model `.va` files are user-supplied; PDK model sources are not committed. Tests use small models
  written from scratch in this repository.

## 2. Pipeline

```
.va text ─▶ preprocessor ─▶ lexer ─▶ parser ─▶ AST ─▶ semantic analysis ─▶ IR ─▶ evaluator
            (`define,          (tokens with     (modules,        (names, types,       (SSA-like      (interpreter with
             `include,          spans)           disciplines,     branches, params,    straight-line  forward-mode AD;
             `ifdef)                             analog block)    contributions)       + control flow) JIT later)
```

Crate layout (`crates/spx-va`, depends only on `spx-diag`):

| Module | Content |
|---|---|
| `preproc.rs` | `` `define `` (with arguments), `` `undef ``, `` `ifdef/`ifndef/`elsif/`else/`endif ``, `` `include `` through a loader trait, `` `resetall ``/`` `timescale `` ignored, line mapping for spans |
| `lexer.rs` | Tokens: identifiers (incl. escaped `\foo `), system identifiers (`$vt`), numbers (integer, real, SI suffixes `T G M K k m u n p f a`), strings, operators, model/instance placement from `(* ... *)` attributes |
| `ast.rs` | Syntax tree (see §3) |
| `parser.rs` | Recursive descent with precedence climbing for expressions |
| `sema.rs` | Symbol resolution, type checks, branch/port mapping, linear `ddt` validation |
| `ir.rs`, `eval.rs`, `dual.rs`, `math.rs`, `limit.rs` | Resolved typed tree, general interpreter, scratch-backed gradients, shared math, binding and limiting |
| `eval/program.rs`, `eval/program/` | Binding specialization, register lowering, liveness, constant folding and execution |
| `eval/topology.rs` | Binding-time constant propagation, node equivalence and compact unknown numbering |
| `spx-devices/src/va.rs` | `DeviceGroup` adapter for compiled modules |

## 3. Supported language subset (target: BSIM4, PSP103, BSIM-CMG, HICUM/L2 sources)

- `module name(ports); ... endmodule`, port directions (`inout`, `input`, `output`), discipline
  declarations of ports and internal nets (`electrical`), `ground`.
- Includes resolve next to the including file first. Built-in `disciplines.vams` and
  `constants.vams` also answer to the legacy names `discipline.h` and `constants.h`.
  The headers declare electrical, thermal, magnetic and mechanical disciplines and define
  the LRM math and physical constants. Physical values use SI definitions and CODATA 2022.
- Function-like macros accept multiline argument lists. Conditional compilation supports
  nested `ifdef`, `ifndef`, `elsif`, `else` and `undef`. The preprocessor defines
  `__VAMS_ENABLE__` and `__VAMS_COMPACT_MODELING__`.
- `parameter real|integer name = expr [from|exclude ranges];`, `localparam`, string parameters,
  `aliasparam`.
- Variables: `real`, `integer` (scalars and arrays), `genvar`.
- Named branches may share endpoints. `branch (<a>) name;` names the current probe into a port.
- Analog functions support input, output and inout arguments with copy-back.
- Statements: blocking assignment, `if/else`, `case/default`, `for`, `while`, `repeat`, named
  blocks `begin : name` with local declarations, contributions `I(a,b) <+ e`, `V(a,b) <+ e`,
  indirect contributions are out of scope, `@(initial_step)` / `@(initial_model)` /
  `@(initial_instance)` / `@(final_step)` event blocks, and system tasks including
  `$strobe`, `$display`, `$warning`, `$error`, `$fatal`, `$finish`, `$bound_step` and `$discontinuity`.
- Expressions: full operator set with precedence of the LRM, ternary, built-in math
  (`exp ln log sqrt pow abs min max floor ceil sin cos tan asin acos atan atan2 sinh cosh tanh
  asinh acosh atanh hypot limexp`), access functions `V(n)`, `V(a,b)`, `I(...)`, `I(<a>)` port
  flows, `ddt`, `ddx`, `idt(expr, initial_condition)`, `$temperature`, `$vt`, `$vt(T)`, `$abstime`,
  `$mfactor`, `$param_given` / `$given`, `$port_connected`, `$simparam("name", default)`, `$limit`,
  `analysis("...")`, noise functions (`white_noise`, `flicker_noise`, `noise_table`) parsed and
  evaluated as 0 (no noise analysis yet).

Anything else produces a structured diagnostic `E-VA-UNSUPPORTED` with a span; never a panic.

### AST shape (guidance)

```rust
pub struct SourceFile { pub modules: Vec<Module>, pub natures: ..., pub disciplines: ... }
pub struct Module { pub name: Ident, pub ports: Vec<Ident>, pub items: Vec<ModuleItem>, pub span: Span }
pub enum ModuleItem { PortDecl{..}, NetDecl{discipline, names}, Param(ParamDecl), Var(VarDecl),
                      Branch{..}, Analog(Stmt), Function(AnalogFunction), AliasParam{..} }
pub enum Stmt { Block{name, decls, stmts}, Assign{lhs, rhs}, Contrib{access, branch, rhs},
                If{..}, Case{..}, For{..}, While{..}, Repeat{..}, Event{event, body},
                SysTask{name, args}, Empty }
pub enum Expr { Int(i64), Real(f64), Str(String), Ident(Ident), Index(..), Unary{op, e},
                Binary{op, l, r}, Ternary{..}, Call{name, args}, SysCall{name, args},
                Access{nature, a, b} }
```

All nodes carry `spx_diag::Span` (line, column, file index into the preprocessor's file list).

## 4. Evaluation model

- One compiled module = one device type. Unknowns are terminal voltages plus internal-node
  voltages (and branch currents for `V() <+` on branches that need them).
- The analog block is lowered once per module to an IR whose values carry a value and a gradient
  with respect to the instance's unknowns (dynamic-length forward-mode AD). Each evaluation
  produces, per node, the resistive current `f` and the reactive charge `q` (arguments of `ddt`)
  with exact Jacobians, which map directly onto spx's `G`, `C`, `f`, `q` slots (dense core stamp
  over the instance's nodes).
- Parameter defaults are evaluated at binding. See §8 for the initialization event contract.
- `$limit(V(a,b), "pnjlim", vt, vcrit)` uses an independently written logarithmic limiter based
  on Nagel (1975) and sets `noncon`. `fetlim` uses an original bounded voltage step.
- Each binding determines its own unknowns before the simulator allocates a matrix. See §10.
- Later: JIT through Cranelift (Apache-2.0) behind a feature flag.

## 5. Netlist integration (phase 3)

```spice
.hdl "diode.va"                       * HSPICE/Xyce-style: load a Verilog-A file
.model dmod mydiode is=1e-14 n=1.0    * model type = module name
N1 a k dmod                           * ngspice OSDI-style instance letter `N`
```

## 6. Verification

1. Unit tests per stage (preprocessor, lexer, parser, sema, AD against finite differences).
2. Verilog-A re-implementations of spx built-ins (resistor, capacitor, diode with the same
   equations) must match the built-in devices to rounding.
3. Black-box comparison against ngspice + OpenVAF in Docker on models written in this repository,
   then on user-supplied PDK models (not committed).

## 7. Phases

| Phase | Scope | Exit criterion |
|---|---|---|
| VA-1 (done) | Preprocessor, lexer, parser, AST, diagnostics | Parses hand-written test models covering §3; robust errors |
| VA-2 (done) | Semantic analysis, IR, AD interpreter | Evaluates test models; Jacobians match finite differences |
| VA-3 (done) | Device integration, `.hdl`, `N` instances | VA diode/resistor match built-ins; engine tests pass |
| VA-4a (done) | Compact-model coverage, parameter kinds, per-instance collapse | IHP model smoke and Jacobian tests, PSP103 engine DC sweep |
| VA-4b (done) | Black-box comparison with ngspice + OpenVAF | 10/10 IHP decks pass; with tight Newton tolerances PSP103 I-V matches to ~1e-14 relative |
| VA-5 (interpreter implemented) | Binding specialization, register execution and parallel assembly | Five-stage ring analysis takes 6.9× the OSDI reference time. The 2× target remains open. |
| VA-5b | Optional Cranelift CPU backend with scalar forward AD | Native and interpreted IHP vectors agree bit for bit. See §13 for the remaining performance gap. |

## 8. VA-2 library API and execution contract

`parse_va(text, loader)` produces a `SourceFile`; `compile_module(&source, name)` resolves and
lowers one module to an immutable `CompiledModule`. `None` selects the last module. Compilation
reports `E-VA-SEMA` for invalid names/types/arity/assignments and `E-VA-UNSUPPORTED` for constructs
outside this phase, with source spans. Public frontend and evaluator types have rustdoc comments.
No new dependencies or external source implementations are used.

`compiled.bind(&[(name, value)])` evaluates defaults in declaration order, applies overrides,
checks ranges, resolves aliases and records explicit supply for `$param_given`. `bind_instance`
applies overrides to a model binding, retaining explicitly supplied model values and recomputing
unsupplied dependent defaults. Integer parameters round to nearest (ties away from zero); integer
variable assignments truncate toward zero. String parameters retain their source defaults and
are usable in comparisons and case selection; the numeric binding API cannot override strings.
Multiple `from` ranges form a union; every `exclude` removes values from that union. Local
parameters cannot be externally overridden. Invalid bindings report `E-VA-PARAM`.

`compiled.eval(&bound, &input, state, &mut output)` provides a convenience per-binding instance
workspace. `eval_with_scratch(&bound, &input, state, &mut scratch, &mut output)` permits multiple
instances or threads to share immutable bindings with separate `Scratch::default()` workspaces.
`Bound` and `CompiledModule` are Send + Sync. Reusing a scratch with a different binding resets
its retained variables. Scratch also owns the specialized instructions, reusable value and gradient
rows, and storage for evaluation of general procedural constructs.
Treat the public compiled metadata as read-only.

The module numbers ports, internal nodes (including integral states), then branch currents.
Use `bound.layout().unknown_len()` for evaluation vectors and `representatives()` to interpret their rows. Ground ports retain an unused,
zero residual row/column to preserve terminal order; their input voltage is ignored. Unused
internal nets are omitted. Branch orientation follows its first contribution, and reversed
accesses change sign. Each voltage branch injects its unknown current once, while its equation
is `V(a,b) - sum(contributions)`; the charge channel of the sum has a negative sign in that row.
The evaluator clears outputs each call, validates input/state shapes and stops on a runtime
failure through `EvalOutput.error`. Loop execution has a total 100,000-step budget.

Supply at least `state_len()` entries in both `StateIo` slices. Limiting reads the previous
Newton value from `current` (falling back to `accepted` for a non-finite history entry), then
writes the new value into `current`, including when limiting is disabled. The simulator must
copy accepted state into current on rollback. Limiting retains the incoming voltage gradient:
its Jacobian is the device linearization at the limited point, rather than a derivative of the
limiting algorithm. A separate forward-mode tangent accumulates the displacement from each
limited argument and propagates through the model equations. Residuals include that tangent,
so `f = i(v_lim) + g(v_lim)*(v_actual-v_lim)` (and likewise for charge), while the Jacobians
remain evaluated at the limited point. `noncon` is set for a change larger than `1e-14 * (1 + abs(vnew))`.
User limiters are called as `(vnew, vold, extra_args...)`. `fetlim` clamps a step to
`0.5 + 0.5 * abs(vold - threshold)` volts. `limexp` equals `exp` through 80, with the tangent
line continued above 80. Piecewise derivatives follow the active arm (first argument at min/max
ties); `abs(0)` and `hypot(0,0)` have zero gradient.

`initial_model` and `initial_instance` run on the first evaluation of a binding in a workspace.
The caller signals `initial_step` and `final_step` explicitly.
Module variables retain numeric values between calls, while their old gradients are cleared.
Block locals have per-instance static storage; function locals initialize on entry. `$temperature` is kelvin, `$vt` uses
`k/q = 8.617333262145179e-5 V/K`, and `$simparam("gmin", default)` falls back to `input.gmin`.
`analysis("ic")` follows `initial_step`; `static` is synonymous with `dc`. Informational system
tasks and `$warning` are ignored. Error/finish/fatal tasks populate `output.error`.

### Current limitations

- `ddt` is supported only in contribution expressions, linearly with parameter-only scaling.
  Dynamic scaling, nested derivatives and reactive temporaries produce compile diagnostics.
- `ddx(e,V(n))` returns the first gradient component with **zero gradient of its own**;
  second derivatives and differentiation with respect to `V(a,b)` are unsupported.
- The two-argument `idt` allocates an integral state. DC enforces its initial condition.
  Transient and AC stamp the state in the charge vector and its negative input in the current vector.
  Integrals without an initial condition, resettable integrals, cross/timer events, recursion
  and block-local parameters are unsupported.
- Arrays are one-dimensional with parameter-only bounds and at most 100,001 elements;
  array function arguments and their initializers are unsupported. Function nesting and
  semantic expression nesting are capped at 128. Integer arithmetic uses f64 storage, so
  integers beyond the exact f64 range are not represented exactly.
- Noise generators return zero. Reaching a `$rdist_*` call produces a spanned
  `E-VA-UNSUPPORTED` diagnostic. Informational tasks, `$bound_step` and `$discontinuity` have
  no simulator side effects.

The authored evaluator regression suite checks analytic resistor stamps, diode and nonlinear
MOS-like charge Jacobians against central finite differences at relative tolerance `1e-6`,
voltage-source signs/orientation, limiting state, ranges, integer arithmetic, arrays/control
flow/functions, environment queries, initialization, runtime failure paths and spanned errors.


## 9. VA-3 simulator integration

Load a deck with `spx run examples/va_diode.cir` or `Session::load_file`. Both quoted
and unquoted `.hdl path.va` cards use the existing netlist source loader, relative to the
including deck. Text-only parsing with `NoFiles` rejects them with `E-PARSE-INCLUDE`.
The netlist retains the resolved HDL path and text. External Verilog-A includes resolve
relative to the HDL file (and nested include files) when that path identifies a real file;
synthetic HDL paths in the engine use the built-in headers and their aliases.

Each file is compiled once during elaboration, all its modules are cached, and compiled modules
are shared across analysis circuits. Duplicate module names are errors. Bindings and retained
interpreter variables belong to individual instances and analyses. Fixed chunks of two VA instances,
dense private stamps and ordered gather preserve thread-count determinism.

`.model name module parameter=value ...` selects a loaded module. Module and numeric parameter
names at the SPICE boundary are case-insensitive; original VA source identifiers retain their
language semantics. Instance overrides follow model overrides. `Nname nodes... model
parameter=value ...` connects terminals in module port order; ground is supported. Subcircuits
rename N terminals and local model references. Internal nodes appear as `v(n1#net)`, branch
currents as `i(n1#branch)`. Wrong port counts and unknown modules report `E-ELAB-PARAM`.
Invalid parameter bindings report `E-VA-PARAM` at the offending model or instance card.
Ordinary runtime failures report `E-VA-EVAL`.

Instance numeric parameters support session alterations and group rebinding; a VA parameter
named `dc` can be swept with the engine's existing `.dc instance start stop step` interface.
VA devices inject no independent AC excitation. They contribute their conductance and charge
Jacobians to small-signal assembly. No implicit gmin conductance is added: a model can use
`$simparam("gmin", default)` explicitly.

The simulator classifies every VA instance as nonlinear. Topology lint treats all terminals as DC-connected. AC performs a DC operating-point
solve and then exposes `analysis("ac")` during linearization. Initial-step and final-step events
can execute on multiple Newton evaluations at their respective analysis points. Includes are read from
the filesystem at elaboration; exported decks retain HDL paths and require those files.
Rollback restores limiter state; retained procedural variables are not separately snapshotted.
Models should recompute voltage-dependent quantities on every evaluation.

Integration tests cover built-in diode agreement (IS, N, RS, depletion charge and transit time),
resistor/capacitor OP/DC/AC/transient behavior, analytic RC response, hierarchical instances,
parameter overrides, initialization, runtime errors and bit-identical 1/4-thread results.
No network, external code, interface headers or new third-party dependencies were used.

## 10. VA-4a bindings and production-model tests

The strict VA binding API `bind_model` accepts model values and the circuit's nominal temperature (°C)
and gmin (S). It reserves parameters marked `(* type="instance" *)` for instance binding. The SPICE adapter also accepts model-card defaults for these parameters (§11). A parameter
with `(* type="model" *)` belongs on the model card. Unannotated model parameters also
accept instance overrides. Aliases inherit placement. Name lookup ignores ASCII case,
rejects ambiguous spellings and suggests the closest declared name for unknown parameters.
In modules without a declared multiplier, instance parameter `m` defaults to one and
scales the value returned by `$mfactor`. A module can query omitted trailing terminals
with `$port_connected`. An explicitly connected ground terminal counts as connected.

At binding, a traversal follows parameter-determined branches and scalar assignments.
An active zero-potential branch with no competing contribution or current probe merges its
endpoints. Ground takes priority, followed by ports, then internal nodes. A branch with only
flow contributions needs no extra current unknown unless the model reads its current.
Conditions that depend on bias keep their switch-branch equation. The traversal conservatively
retains equations when loops, events or function results prevent proof. Instances can have
different layouts even within one device group. Circuit elaboration merges external nets
before stamping other devices and reports their voltages under each original net name.
An instance targeted by an in-place DC parameter sweep retains its full possible layout.

Run `cargo test -p spx-va --test production -- --nocapture` for the local model suite.
The default root is `va-models/ihp` relative to the checkout. Set `SPX_VA_MODELS` to an
alternative directory containing the model subdirectories. Each test prints a skip note
when its entry file is absent. The suite covers PSP103, its thermal and NQS variants,
JUNCAP200, VBIC 1.3, r3_cmc and mosvar. It checks every entry of both dense Jacobians by
central differences at a DC bias point, with relative tolerance `1e-5`, absolute floors
of `1e-12` for conductance and `1e-23` for capacitance, and a cancellation allowance.
It prints preprocessing-through-lowering time and cold and subsequent evaluator times.

The local PSP source normally declares `PSP103VA`. SPICE model-card lookup accepts a
trailing `_va` spelling for a loaded module ending in `va`, with exact matches taking
priority. The smoke suite also selects the source configurations declaring `PSP103_VA`
and three-terminal `vbic13_va`. The engine test sweeps a 1 µm by 1 µm NMOS at 1.2 V
drain bias and checks convergence and monotonic drain current.

## 11. VA-4b production circuits

The ten circuits in `verify/va-decks` run through the CLI and library simulation
paths. They exercise SG13G2 NMOS and PMOS characteristics, inverter DC and delay,
a five-stage ring, common-source AC gain, HBT collector and base currents, and a
poly resistor at 60 °C. The decks select typical corners from the local IHP
libraries. The bundled inputs in this directory are original test circuits and
a two-directive wrapper selecting the five-terminal configuration of VBIC.

The IHP HBT library selects ngspice's native VBIC through a five-terminal
`Q` instance and an NPN level-9 model card. For this form spx binds the
loaded `vbic13_5t` module, sets the transistor polarity, and connects the
substrate and thermal terminals. The three ngspice safe-operating-area voltage
limits are advisory and have no effect on the equations. The reference deck
retains the library's native level-9 device. The PSP and resistor reference
circuits use the loaded OSDI devices.

### SPICE instance multiplicity

For an N instance, `m` and `mult` address the module's declared multiplier
when it declares either spelling or an alias. When both are declared, each
spelling addresses its own declaration. The model controls the scaling in
this case and the intrinsic `$mfactor` is one. Geometry parameters such as
`nf` pass through to the declared VA parameter.

When neither multiplier is declared, positive `m` (also spelled `mult`)
defaults to one and supplies `$mfactor`. The simulator sums terminal
currents and charges over that many parallel copies, including their Newton
and small-signal Jacobians. Internal branch currents and voltages describe
one copy. SPICE model cards may supply instance defaults, including via an
alias. An explicit N-line value overrides the corresponding default.

### Running and comparing

From the checkout root, with the files listed in each deck header installed locally:

```sh
cargo run -p spx-cli -- run verify/va-decks/psp_nmos_idvg.cir
cargo test -p spx-engine --test verilog_a ihp_corner_transfer_and_inverter_switching
cargo run --release -p spx-verify -- verify/va-decks
cargo run --release -p spx-verify -- --cached verify/va-decks
```

The local NMOS reaches 401 µA at 1.2 V on both gate and drain. The loaded
inverter delay is 33.2 ps and the ring period is 557 ps. Across these typical
corner DC decks, the existing continuation strategy took 2.00–3.06 Newton
iterations per point on average, including the initial bias solve. Each point
converged on its first solve. The CLI prints the counts, also available as
`RunOutput::dc_stats`. These runs use the VA-4a limiter and gmin handling.

The typical-corner
netlist evaluator supports disabled statistical variation in `agauss`.
Active random mismatch and Monte Carlo corners remain unsupported.

### 11.x Reference comparison results (2026-11-18)

`cargo run --release -p spx-verify -- verify/va-decks` passes 10/10 decks against ngspice 44.2 with
models compiled by OpenVAF-Reloaded v24.0.2mob (both run as black boxes in Docker). At the default
tolerances (`reltol=1e-3`) the median relative drain-current difference of the PSP103 sweeps is
below 1e-11; isolated points differ by up to 1e-3 because the two simulators stop Newton at
different iterates. With `.option reltol=1e-7 abstol=1e-15 vntol=1e-9` on both sides the maximum
relative difference is 2.4e-11 (Id-Vg) and 7.9e-15 (Id-Vd), i.e. the model evaluation agrees to
rounding.

## 12. VA-5 evaluation and performance

On the first call, the general evaluator initializes the instance, including retained
variables and startup events. On the next call, the scalar compiler specializes the typed
tree for the binding, compact unknown layout, temperature and multiplier. Changing the
temperature or input multiplier rebuilds the program. A new binding resets the workspace.

The compiler resolves parameter reads, folds math, prunes constant branches, inlines scalar
functions and unrolls small loops with known bounds. Each assignment gets a register.
At a dynamic branch the executed arm supplies the value at the join. The compiler shares
pure arithmetic within basic blocks and removes unused computations. Live-in analysis
identifies the final variable values that a later invocation may read before writing them.
Voltage, current, simulator parameter callbacks, time, analysis, event flags and stateful
operators retain their runtime semantics. The program preserves contribution order.

Register values and gradient rows are preallocated in scratch. Only registers that can carry
a derivative get rows, with one column per bound unknown and a final column for the limiting
displacement. The charge channel can often be folded to zero. Runtime checks protect this
optimization at exceptional floating-point inputs. If a check fails, the evaluator uses the
full charge equations, reusing the simulator query results from the guarded call.

Specialization is conservative. Bindings with remaining array accesses, dynamic loops,
unsupported scalar instructions or reachable error tasks use the general interpreter. That
interpreter keeps numeric derivatives in a reusable arena, including across nested function
calls. It can reclaim temporary rows during long loops.

### Release measurements

Each wall time is the median of three launches of `target/release/spx run <deck>` on an AMD
Ryzen AI Max+ 395 with the default thread pool and local IHP inputs. The before binary was
built from `4441f0f`. Wall time includes input, compilation, binding, analysis and CLI output.
The eval count includes converged-point evaluations outside Newton.

| PSP deck | Before (s) | VA-5 (s) | Points | Newton iterations | Instance evals |
|---|---:|---:|---:|---:|---:|
| psp_cs_ac | 0.095 | 0.100 | 81 | 4 (OP) | 5 |
| psp_inverter_dc | 0.351 | 0.125 | 121 | 370 | 740 |
| psp_inverter_tran | 0.577 | 0.133 | 219 | 479 | 1,396 |
| psp_nmos_idvd | 0.201 | 0.116 | 151 | 317 | 317 |
| psp_nmos_idvd_1v2 | 0.202 | 0.102 | 151 | 319 | 319 |
| psp_nmos_idvg | 0.203 | 0.108 | 121 | 317 | 317 |
| psp_pmos_idvg | 0.197 | 0.112 | 121 | 313 | 313 |
| psp_ring5 | 4.410 | 0.248 | 307 | 917 | 12,240 |
| benches/psp_ring21 | 22.604 | 0.468 | 307 | 917 | 51,408 |

Separate timing probes around transient analysis, the L1 device loop and LU gave the following
wall-time breakdown. Each column is the median of three runs. L1 includes scheduling and slot
writes. The remaining time includes ordered matrix assembly, integration and solver bookkeeping.

| Transient | Analysis (ms) | Device evaluation (ms) | LU factor and solve (ms) | Accepted steps | Rejected | Assemblies |
|---|---:|---:|---:|---:|---:|---:|
| Inverter | 29.5 | 28.1 | 0.8 | 218 | 0 | 698 |
| Ring, 5 stages | 131.4 | 116.2 | 10.3 | 306 | 0 | 1,224 |
| Ring, 21 stages | 290.2 | 228.0 | 32.2 | 306 | 0 | 1,224 |

The five-stage ring takes **6.9×** the maintainer's 19 ms ngspice+OSDI analysis time.
Its 4 ps maximum step requires at least 300 advances over 1.2 ns. spx takes 306 and the
cached reference has 307 advances (308 points including time zero). The inverter reference
has 220 points versus 219 in spx. These runs follow the same step sequence as VA-4b.
The cached raw files contain no Newton counts. There is no cached 21-stage reference or
local executable available for an independent OSDI timing of the larger benchmark.

In a single-worker ring run, time inside the VA adapter divided by instance calls fell
from **358 to 20.9 µs** on ring5. Ring21 averaged 24.4 µs per call. The lighter six-unknown
standalone PSP103 smoke bias took 7.6 µs warm in release and 153 µs in debug. The smoke
test reports initialization, specialization and the median of 11 warm evaluations separately.

With all vectors selected, the CLI results for the ten IHP decks and the 21-stage benchmark
matched VA-4b, including every returned floating-point bit. The PSP103 smoke test also checks
the cold and warm residual and Jacobian bits at its bias point. The cached reference suites
passed 14/14 standard and 10/10 IHP decks.

## 13. VA-5b native evaluation

Build with `--features jit` to select Cranelift. The feature defaults off.
In a JIT build, `.option va_jit=0`, `SimOptions::va_jit = false` or the process
environment setting `SPX_VA_JIT=0` selects the interpreter. Library clients can
choose the interpreter independently for each workspace with
`Scratch::set_jit(false)`.

**Preparation and ownership.** Elaboration prepares a native kernel before the
transient timer starts. A standalone evaluator prepares it on demand. The
specialization key includes the bits of the bound parameter values, string
parameters, explicit-supply flags, terminal connectivity, compact layout,
temperature and multiplier. The module cache serializes compilation per key,
including failed attempts. Equal instance bindings share the same code.
The first evaluation of each instance still runs startup events in the general
interpreter. Subsequent calls read and update per-instance numeric history.
Scratch holds a weak reference to its kernel. The last module owner releases
the executable pages, even when a binding or scratch survives it.

**Native function.** A C-layout call frame carries pointers to unknowns,
hoisted constants, retained variables, accepted/current limiter history and
the four dense output buffers. Flags and simulator context are call-local.
The return word carries validity and nonconvergence bits. The last tangent
component is the limiting displacement and is added at each residual stamp.
Unknown derivatives become scalar SSA values. Missing components use the
structural zero, and conditional transfers become native blocks. The lowerer
uses ordinary multiply and add instructions in interpreter order. Rust
C-ABI shims evaluate transcendental functions and limiters using the same
implementations as the interpreter. Cranelift's optional e-graph optimizer
increased spills and code size on the rings, so the backend uses the optimized
register program with that pass disabled. Only `jit/abi.rs` permits unsafe Rust
for calling finalized functions and releasing executable pages.

**Floating-point guards.** The full lowering implements the interpreter's
zero-incoming-partial rule, including at nonfinite slopes. Stateless programs
whose derivatives only reach additive stamps can also use a guarded lowering.
It merges both signs of zero in the charge channel at joins and omits per-lane
zero tests in derivative products. Accumulation into dense output rows starts
at positive zero. An otherwise suppressed zero times a nonfinite slope poisons
the computed derivative until it dies on an untaken path or reaches a guard or
output. The compiler propagates finiteness obligations backward through sums,
products and control-flow joins. A finite sum of guarded terms certifies their
finiteness. Overflow of that sum conservatively triggers replay. Stamps are
checked before accepting the result. A failed guard replays the exact register
program with the original simulator query results. Stateful and `ddx`
programs keep exact signed components throughout.

**Fallback.** Numeric case, if and ternary expressions compile to control flow.
The scalar specializer unrolls loops with known bounds up to 64 iterations.
Dynamic loops, deeper dynamic case chains, remaining arrays and string
operations, reachable error tasks and unsupported scalar instructions use the
existing evaluator for the affected binding. A backend or specialization
failure emits the informational diagnostic `I-VA-JIT`. Other bindings and
modules continue using their selected backend. Exceptional-value replay keeps
the affected workspace on the interpreter until its specialization changes.

**Reproducing verification.** Run from the checkout root with the local IHP
inputs installed as described in §11.

```sh
cargo test --offline
cargo test --offline --features jit -p spx-va
cargo test --offline --features jit -p spx-engine
cargo clippy --offline --all-targets --all-features -- -D warnings
cargo run --offline --release --features jit -p spx-verify -- --cached
cargo run --offline --release --features jit -p spx-verify -- --cached verify/va-decks
cargo test --offline --release --features jit -p spx-va --test production -- --nocapture --test-threads=1
SPX_VA_JIT=0 cargo test --offline --release --features jit -p spx-va --test production -- --nocapture --test-threads=1
```

The model comparison uses six evaluations at each of three bindings and compares
the bits of every residual, Jacobian and limiter-state entry, as well as
`noncon`. The engine comparison clears the deck's save restriction and compares
all real and complex vector components and the independent variable, using
one interpreter thread and five native threads. The committed small MOS
fixture exercises the array/dynamic-loop fallback. All ten local production
configurations ran native kernels.

### Release measurements

The following are medians of seven fresh CLI runs on the Ryzen AI Max+ 395.
Both modes used the release JIT build. The interpreter runs set
`SPX_VA_JIT=0`. Wall time includes parsing, binding, native compilation and
CLI output. Analysis time came from a separate build with monotonic timing
probes around the complete transient, including the initial operating point.
The machine exposed 32 logical workers. The default native pools used seven
on ring5 and 23 on ring21. With `-j8` the native pools used seven and eight.

| Deck | Requested workers | Interpreter wall (ms) | Native wall (ms) | Interpreter analysis (ms) | Native analysis (ms) |
|---|---|---:|---:|---:|---:|
| ring5 | 1 | 366 | 313 | 267.4 | 66.6 |
| ring5 | 8 | 223 | 292 | 119.0 | 37.1 |
| ring5 | default | 241 | 286 | 138.5 | 41.3 |
| ring21 | 1 | 1,399 | 529 | 1,251.1 | 241.9 |
| ring21 | 8 | 414 | 397 | 262.5 | 94.1 |
| ring21 | default | 458 | 405 | 281.8 | 117.0 |

Against the maintainer's ngspice+OSDI analysis references of 21–28 ms and
61–65 ms, the single-worker native times are **2.4–3.2× on ring5 and
3.7–4.0× on ring21**. The default native pools meet the 2× analysis target
on both rings. Native compilation accounts for the longer default wall time
on ring5. The two PSP103 corner-card specializations took median compile times
of 76.0 and 74.3 ms across the ring runs. Their finalized code and constant
pools occupied 288,592 and 289,040 bytes. All matching instances reused them.

| Native transient | Device groups (ms) | Ordered assembly (ms) | LU factor + solve (ms) | Integration, step control and other work (ms) |
|---|---:|---:|---:|---:|
| ring5, one worker | 54.7 | 2.5 | 8.7 | 0.6 |
| ring5, default | 27.0 | 4.6 | 8.3 | 0.8 |
| ring21, one worker | 209.3 | 9.3 | 21.5 | 1.8 |
| ring21, default | 66.1 | 22.2 | 26.5 | 3.1 |

Device-group time includes slot writes and parallel scheduling. Assembly
covers gathering slots and applying shunts and clamps. The last column is
the per-run remainder. Each column is independently medianed. Both rings
took 306 accepted steps, zero rejected steps, 917 Newton iterations and 1,224
assemblies. Timing the VA adapter itself on one worker gave **4.44 versus
20.87 µs** per ring5 instance call and **4.05 versus 23.48 µs** on ring21
(native versus interpreter).

The standalone production test measures native preparation separately from
the first evaluation. Compile time covers register specialization, Cranelift
lowering, code generation and finalization. Warm time is the median of 11
calls. Table entries are medians of three release test processes per backend.

| Production configuration | Native compile (ms) | Warm interpreter (µs) | Warm native (µs) |
|---|---:|---:|---:|
| PSP103VA | 31.2 | 7.58 | 1.60 |
| PSP103_VA | 31.6 | 7.93 | 1.73 |
| PSP103TVA | 43.4 | 9.63 | 2.22 |
| PSPNQS103VA | 34.1 | 18.40 | 2.77 |
| JUNCAP200 | 8.5 | 2.85 | 0.41 |
| vbic13_4t | 28.2 | 7.90 | 2.26 |
| vbic13_5t | 37.2 | 5.37 | 3.09 |
| vbic13_va (three terminals) | 49.2 | 9.82 | 3.70 |
| r3_cmc | 4.6 | 1.84 | 0.31 |
| mosvar | 14.6 | 2.45 | 0.61 |

Device evaluation accounts for about 86% of ring21 single-worker analysis.
The next code-generation experiment should pass selected partials directly as
block arguments at joins and schedule derivative uses closer to definitions
across math calls. Compare spill traffic, kernel size and adapter time against
the current build while retaining the bitwise tests. On the default ring21
pool, gather and LU together take about 49 ms. Precomputing contiguous runs
of source slots in gather order and reusing the numeric LU pivot order are
separate experiments for that path.

## 14. Optimization round 1 — startup

A Unix JIT build caches three compiler stages under `$SPX_CACHE_DIR`, defaulting
to `$XDG_CACHE_HOME/spx` or `~/.cache/spx`. Set `SPX_CACHE=0` or
`.option va_cache=0` to bypass all disk reads and writes. These settings still
allow identical bindings to share a kernel within a run. Other platforms use
in-process compilation.

* The front end records the include requests it actually made, including
  missing local overrides of built-in headers. On a hit it repeats those
  lookups and checks resolved names and content digests. The manifest retains
  the declarations of every validated module. Only bodies referenced by VA
  model cards in use need to be deserialized. Cold compilation checks unused
  modules and duplicate declarations too.
* The second stage stores the optimized scalar program for a binding. Its
  identity includes input text and includes, parameter bits and strings,
  explicit-supply flags, port connectivity and layout, temperatures, gmin and
  multiplier. Floating-point constants retain their IEEE 754 bits.
* The machine-code key adds the serialized program (including all hoisted
  constants), the native target, ISA flags, Cranelift settings and frame width.
  A build fingerprint covers the spx version, VA compiler source, built-in
  headers, Cargo.lock (including Cranelift), Rust compiler, target and build
  flags. Math, query and limiter calls load their current addresses from the
  call frame. A relocation-free code buffer and its required alignment can
  therefore be installed through Cranelift in a fresh process with ASLR.
  Functions with outstanding relocations use ordinary JIT finalization.
* Each immutable entry has a SHA-256 checksum bound to its key. The store
  pins an owner-controlled directory and checks the opened file's type,
  owner, write permissions and length. Writers publish completed temporary
  files with atomic rename. An unreadable or corrupt entry is a miss. Cache
  files occupy disk until the user removes them.
* Elaboration deduplicates complete binding keys before handing distinct
  jobs to Rayon. Indexed collection gives diagnostics a stable order. A
  per-key initialization cell also protects concurrent lazy callers. Instance
  history, the order of contributions and the first general evaluation retain
  their VA-5b semantics. This round uses synchronous preparation. A warm hit
  can run native code on the first eligible evaluation without a tier switch.

The netlist preprocessor reuses lexed filesystem libraries and parsed
expressions across subcircuit instances. It evaluates each expression in its
own parameter scope. Elaboration also shares identical flattened model cards
by parameter name, order and value bits. The VA macro processor copies runs
of literal bytes together while preserving diagnostic source positions.

### Reproducing the measurements

Run these commands from the checkout root with the local IHP test inputs installed.

```sh
cargo build --offline --release -p spx-cli --features jit
benches/run.py --profile
benches/compare.py /path/to/HEAD-release-spx target/release/spx
```

Each table entry is a median of five fresh CLI processes. The benchmark gives
each cold launch an empty private spx cache and follows it with a warm launch
of the same deck. The OS file cache remains warm. A separate set of launches
with `SPX_PROFILE=1` measures internal phases. Wall time uses the ordinary
CLI summary, with stdout discarded, and runs from process launch to exit.
The HEAD baseline is e8262638. Its analysis and phase measurements use the same
monotonic probes applied to the original implementation. The ngspice 44.2
reference medians are supplied by the maintainer and exclude OpenVAF
compilation.

### Release timings

AMD Ryzen AI Max+ 395, 32 logical CPUs. All values are milliseconds. HEAD
recompiles on every launch. The HEAD columns use the second launch of each
pair. Reference values are the supplied ngspice 44.2 medians.

| Deck | Workers | HEAD analysis | Warm analysis | Ref. analysis | HEAD wall | Cold wall | Warm wall | Ref. wall |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| psp_nmos_idvg | -j1 | 5.15 | 4.88 | 2.5 | 397.62 | 435.14 | 45.09 | 56 |
| psp_nmos_idvg | default | 5.00 | 4.87 | 2.5 | 406.41 | 429.67 | 53.36 | 56 |
| psp_inverter_dc | -j1 | 10.69 | 10.64 | 3.3 | 598.77 | 643.60 | 65.30 | 46 |
| psp_inverter_dc | default | 10.63 | 10.74 | 3.3 | 589.63 | 460.14 | 66.28 | 46 |
| psp_inverter_tran | -j1 | 17.69 | 17.61 | 4.3 | 569.53 | 648.69 | 75.57 | 42 |
| psp_inverter_tran | default | 16.88 | 15.42 | 4.3 | 572.16 | 461.70 | 67.78 | 42 |
| psp_ring5 | -j1 | 141.79 | 141.63 | 17.4 | 779.80 | 761.06 | 212.03 | 72 |
| psp_ring5 | default | 79.71 | 73.40 | 17.4 | 679.09 | 521.30 | 138.70 | 72 |
| psp_ring21 | -j1 | 575.52 | 573.34 | 65.4 | 1309.35 | 1273.43 | 687.15 | 127 |
| psp_ring21 | default | 222.77 | 228.64 | 65.4 | 959.17 | 748.25 | 363.89 | 127 |

The host ran substantially slower during final verification than during the
initial campaign measurements. The unchanged HEAD binary provides a control.
The table below gives the earlier medians. That candidate already had the
three-stage cache and parallel preparation. The final candidate adds splitting
for long straight-line programs. PSP takes the whole-function path in both.
Use the final table above for parity counts. The supplied reference executable
was unavailable for a contemporaneous rerun.

| Earlier run | HEAD wall, -j1 / default | Warm candidate wall, -j1 / default | HEAD / warm candidate analysis, -j1 |
|---|---:|---:|---:|
| psp_nmos_idvg | 202.2 / 200.9 | 26.2 / 30.5 | 2.07 / 2.57 |
| psp_inverter_dc | 298.6 / 297.6 | 38.8 / 29.7 | 4.31 / 5.59 |
| psp_inverter_tran | 290.4 / 286.0 | 43.1 / 37.3 | 10.28 / 9.21 |
| psp_ring5 | 352.6 / 332.8 | 98.9 / 68.9 | 62.60 / 57.52 |
| psp_ring21 | 565.5 / 465.9 | 315.3 / 171.8 | 238.87 / 226.17 |

### Startup profile by deck

Single-worker phase medians from the final table follow. The four netlist
figures cover the deck read, library expansion, subcircuit flattening and
parsing flattened cards. The three VA figures cover preprocessing, lexing
and parsing, and semantic analysis with typed lowering. The VA cache column
covers include validation, typed IR loading and cold publication. The
specialization and JIT columns include their own disk activity.

| Deck | Build/cache | Netlist read / expand / flatten / parse | VA preprocess / parse / sema | VA cache | Bind | Specialize | JIT | Output | Other |
|---|---|---|---|---:|---:|---:|---:|---:|---:|
| psp_nmos_idvg | HEAD | 0.02 / 0.75 / 2.38 / 0.24 | 44.0 / 63.5 / 62.8 | 0.0 | 3.5 | 6.4 | 181.6 | 0.03 | 35.7 |
| psp_nmos_idvg | Cold | 0.03 / 0.74 / 2.42 / 0.21 | 37.8 / 64.7 / 62.1 | 40.8 | 3.1 | 8.5 | 184.3 | 0.03 | 28.6 |
| psp_nmos_idvg | Warm | 0.01 / 0.70 / 2.41 / 0.21 | 0.0 / 0.0 / 0.0 | 30.9 | 2.4 | 2.2 | 1.7 | 0.03 | 7.9 |
| psp_inverter_dc | HEAD | 0.02 / 0.79 / 4.37 / 0.37 | 42.8 / 67.9 / 61.1 | 0.0 | 6.1 | 13.5 | 359.7 | 0.03 | 35.8 |
| psp_inverter_dc | Cold | 0.02 / 0.74 / 3.32 / 0.38 | 38.4 / 67.5 / 68.2 | 40.8 | 5.3 | 17.8 | 361.4 | 0.03 | 27.9 |
| psp_inverter_dc | Warm | 0.02 / 0.73 / 3.33 / 0.38 | 0.0 / 0.0 / 0.0 | 30.2 | 3.9 | 4.2 | 3.2 | 0.02 | 9.2 |
| psp_inverter_tran | HEAD | 0.03 / 0.76 / 4.28 / 0.37 | 44.0 / 57.8 / 54.7 | 0.0 | 3.9 | 13.2 | 351.1 | 0.02 | 33.4 |
| psp_inverter_tran | Cold | 0.02 / 0.71 / 3.38 / 0.40 | 37.4 / 66.3 / 59.1 | 41.5 | 2.6 | 17.8 | 349.4 | 0.03 | 31.0 |
| psp_inverter_tran | Warm | 0.02 / 0.71 / 3.30 / 0.38 | 0.0 / 0.0 / 0.0 | 29.0 | 2.4 | 4.3 | 3.3 | 0.03 | 10.7 |
| psp_ring5 | HEAD | 0.02 / 0.93 / 22.95 / 2.02 | 45.1 / 57.0 / 55.7 | 0.0 | 9.9 | 13.8 | 351.7 | 0.02 | 41.0 |
| psp_ring5 | Cold | 0.02 / 0.75 / 9.72 / 1.41 | 36.9 / 65.2 / 66.3 | 41.9 | 5.9 | 18.0 | 354.5 | 0.04 | 32.0 |
| psp_ring5 | Warm | 0.02 / 0.74 / 9.60 / 1.56 | 0.0 / 0.0 / 0.0 | 29.7 | 6.1 | 4.3 | 3.4 | 0.03 | 14.8 |
| psp_ring21 | HEAD | 0.02 / 0.83 / 93.39 / 9.67 | 45.4 / 54.2 / 51.2 | 0.0 | 36.9 | 14.1 | 350.9 | 0.02 | 54.7 |
| psp_ring21 | Cold | 0.02 / 0.77 / 34.29 / 7.46 | 36.6 / 65.0 / 59.0 | 39.4 | 18.3 | 17.7 | 352.8 | 0.02 | 45.7 |
| psp_ring21 | Warm | 0.03 / 0.78 / 40.26 / 9.46 | 0.0 / 0.0 / 0.0 | 26.2 | 17.3 | 4.2 | 3.3 | 0.02 | 34.2 |

HEAD binding time is the inclusive binding loop less specialization and
codegen. Other time is the per-process difference between launch-to-exit
time and disjoint phases, including analysis. It includes startup, pool
creation, the rest of elaboration, projection and measurements, dropping
the circuit and exit. Independently medianed columns need not sum.

Ring5 cold preparation took 373.2 ms with one worker and 208.7 ms with default
workers. Warm preparation took 8.4 and 4.4 ms respectively. On ring21,
single-worker flattening fell from 93.4 to 40.3 ms.

### Code generation experiments

These ring5 results are medians of five cold `-j1` runs from the earlier
settings experiment. Set `SPX_CACHE=0 SPX_PROFILE=1` and vary `SPX_CLIF_OPT`
and `SPX_CLIF_REGALLOC` to reproduce it. Compile time sums both JIT phases.
The byte counts include each function's constant pool.

| Opt level | Register allocator | Compile (ms) | NMOS bytes | PMOS bytes | Analysis (ms) |
|---|---|---:|---:|---:|---:|
| none | backtracking | 177.6 | 287,264 | 287,776 | 58.6 |
| none | single_pass | 60.0 | 488,240 | 489,296 | 100.2 |
| speed | backtracking | 589.4 | 859,440 | 860,128 | 129.4 |
| speed | single_pass | 119.1 | 1,282,048 | 1,282,896 | 247.2 |
| speed_and_size | backtracking | 475.4 | 859,440 | 860,128 | 126.3 |
| speed_and_size | single_pass | 109.7 | 1,282,048 | 1,282,896 | 248.9 |

Backtracking with `opt_level=none` remains the default. Single-pass allocation
saves cold compile time but increases both code size and evaluation time.
The 332,192-byte VBIC kernel compiled in 55.7 ms while the 287,264-byte PSP
NMOS took 87.3 ms and the 20,912-byte resistor took 5.0 ms. Size alone is
a poor predictor across models.

A controlled size probe repeated `y = sin(y) + 0.5 * V(p)` in a single
analog block. At 128, 256, 512 and 1,024 assignments, whole functions occupied
16, 32, 63 and 130 KiB and compiled in 3.8, 6.7, 11.9 and 27.4 ms. At 2,048
and 4,096 they reached 256 and 511 KiB and 76.8 and 171.2 ms. The compiler
now splits eligible stateless, straight-line programs with at least 8,192
register instructions into two functions. The first passes live primals,
partial derivatives and accumulated guard state through per-instance scratch.
The second rematerializes literals and preserves structural zeroes. The cache
stores both fragments and their required scratch size in a single entry.
In paired whole-versus-split measurements, the 2,048-assignment compile
fell from 108.0 to 99.8 ms and the 4,096-assignment compile from 235.1 to
217.8 ms. Analysis of the latter sweep took 18.24 versus 18.08 ms. Both
binaries returned identical results.

A separate two-function PSP prototype split near the middle at a control-flow
join. Paired pinned ring21 trials reduced cold compilation by 16% but slightly
increased analysis time. Its two bindings grew from 575,040 to 583,664 total
bytes. The production PSP kernels keep the single-function path. Cold and
warm CLI tests of the long straight-line path match the interpreter.

### Remaining parity gaps

In the final pass, NMOS Id–Vg meets the wall target with both worker settings.
All five warm single-worker analysis medians exceed the supplied references.
Ring21 spends 573.3 ms in single-worker analysis against a 127 ms total wall
budget. With default workers it spends 228.6 ms in analysis and 363.9 ms
overall. Even the short inverter transient spends about 29 ms restoring the
typed VA front end on one worker.

The next analysis experiment is to shorten live ranges of scalar partials
through math calls and control-flow joins. The VA-5b device-loop profile
identifies those evaluations as the dominant single-worker ring cost.
For startup, measure a compact binary representation of the typed IR and
reuse substituted PDK model templates across equal wrappers.

Both cached oracle suites passed (14/14 standard and 10/10 IHP). The release
comparison with HEAD clears each deck's save restriction and checks every
scale and every real and complex sample, including signed zero. All ten IHP
decks and ring21 matched on cold and warm runs. Single-worker and default
cold runs also produced identical artifact filenames and bytes. Cache tests
cover concurrent publication, separate corruption of each layer, changed
includes, both disable switches and restored multi-function code.
