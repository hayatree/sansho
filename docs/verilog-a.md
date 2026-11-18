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
| VA-5b | Optional Cranelift CPU backend with scalar forward AD | Native and interpreted IHP vectors agree bit for bit. See the [performance report](performance.md) for current timings. |

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
interpreter variables belong to individual instances and analyses. Fixed instance chunks, private stamps and ordered gather preserve thread-count
determinism. Section 16 describes the current native chunk sizes.

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

## 12. VA-5 register evaluation

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
register program with that pass disabled. `jit/abi.rs` owns finalized functions and releases their executable pages.

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

### Persistent compilation and circuit specialization

A private content-addressed cache stores the translated module, specialized
register program and relocatable native image. The manifest replays include
lookups and validates their contents, including local standard-header overrides.
The compiler fingerprint, binding, circuit wiring and backend settings identify
the native image. Each entry binds its SHA-256 checksum to its key. The Unix reader pins the
owner-controlled cache directory and checks the opened file's owner, type,
permissions and size. Concurrent writers publish complete files by atomic rename.
Set `SPX_CACHE_DIR` to choose a private directory or `SPX_CACHE=0` to disable it.

Eligible stateless straight-line programs of at least 8,192 instructions split
into two functions. Live values pass through per-instance scratch, and the cache
stores both code fragments and their handoff size. Relocation-free images load
through Cranelift in a fresh process. Other images use ordinary JIT finalization.

The circuit adapter identifies grounded and tied terminals after net remapping.
Grounded rows and columns can be omitted. Tied ungrounded terminals retain
separate tangent columns for ordered assembly. DC-only analyses can discard
the reactive stamp. Stateless transient kernels can reuse a stamp at identical
bias, temperature and gmin when their inputs and simulator queries permit it.

With eight or more VA instances, matching pure bindings in the same chunk can
evaluate two biases in SIMD lanes. Synthetic biases predict boolean outcomes.
The packed kernel checks predictions in each active lane and uses masks at
divergent branches. A failed guard discards both lanes and triggers scalar
evaluation. Each instance writes its original slots in deterministic order.

During transient Newton iteration, a constant ground-referenced source can
fix the last voltage increment in the sparse elimination order to exactly
zero. At the specified voltage the paired kernel can omit the corresponding
AD seed columns. The circuit checks the source, ordering and iterate before
selecting this kernel. Full and projected stamps have distinct cache keys.

Round 6 warm time ratios against ngspice 44.2 with OSDI models, estimated
from 41 interleaved release pairs against `63b70bd`, appear below.

| Deck | Analysis −j1 | Wall −j1 | Wall default |
|---|---:|---:|---:|
| nmos_idvg | 0.205× | 0.512× | 0.591× |
| inverter_dc | 0.333× | 0.552× | 0.460× |
| inverter_tran | 0.414× | 0.563× | 0.574× |
| ring5 | 0.572× | 0.682× | 0.548× |
| ring21 | 0.760× | 0.650× | 0.493× |

See [Performance](performance.md) for PSP measurements, the round history and
commands for reproducing the full-vector comparison and timings.
