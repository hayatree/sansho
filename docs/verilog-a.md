# Verilog-A compact-model compiler (`spx-va`)

Status: VA-1, VA-2 and VA-3 implemented (M4). Compiled modules run in OP, DC, AC and transient analyses.

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
| `lexer.rs` | Tokens: identifiers (incl. escaped `\foo `), system identifiers (`$vt`), numbers (integer, real, SI suffixes `T G M K k m u n p f a`), strings, operators, attributes `(* ... *)` skipped |
| `ast.rs` | Syntax tree (see §3) |
| `parser.rs` | Recursive descent with precedence climbing for expressions |
| `sema.rs` | Symbol resolution, type checks, branch/port mapping, linear `ddt` validation |
| `ir.rs`, `eval.rs`, `dual.rs`, `limit.rs` | Resolved typed tree, dense dynamic AD interpreter, parameter binding, limiting |
| `spx-devices/src/va.rs` | VA-3 `DeviceGroup` adapter for compiled modules |

## 3. Supported language subset (target: BSIM4, PSP103, BSIM-CMG, HICUM/L2 sources)

- `module name(ports); ... endmodule`, port directions (`inout`, `input`, `output`), discipline
  declarations of ports and internal nets (`electrical`), `ground`.
- Natures/disciplines: `` `include "disciplines.vams" `` and `"constants.vams"` are provided
  built-in (written from the LRM's definitions, not copied from any distribution).
- `parameter real|integer name = expr [from|exclude ranges];`, `localparam`, string parameters,
  `aliasparam`.
- Variables: `real`, `integer` (scalars and arrays), `genvar`.
- Named branches: `branch (a, b) name;`.
- `analog begin ... end`, `analog function` definitions.
- Statements: blocking assignment, `if/else`, `case/default`, `for`, `while`, `repeat`, named
  blocks `begin : name` with local declarations, contributions `I(a,b) <+ e`, `V(a,b) <+ e`,
  indirect contributions are out of scope, `@(initial_step)` / `@(initial_model)` /
  `@(initial_instance)` event blocks, system tasks `$strobe/$display/$write/$warning/$error/
  $finish/$debug` (no-ops or diagnostics).
- Expressions: full operator set with precedence of the LRM, ternary, built-in math
  (`exp ln log sqrt pow abs min max floor ceil sin cos tan asin acos atan atan2 sinh cosh tanh
  asinh acosh atanh hypot limexp`), access functions `V(n)`, `V(a,b)`, `I(...)`, `V(<a>)` port
  flows, `ddt`, `ddx`, `idt` (error for now), `$temperature`, `$vt`, `$vt(T)`, `$abstime`,
  `$mfactor`, `$param_given`, `$port_connected`, `$simparam("name", default)`, `$limit`,
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

## 4. Evaluation model (phase 2–3)

- One compiled module = one device type. Unknowns are terminal voltages plus internal-node
  voltages (and branch currents for `V() <+` on branches that need them).
- The analog block is lowered once per module to an IR whose values carry a value and a gradient
  with respect to the instance's unknowns (dynamic-length forward-mode AD). Each evaluation
  produces, per node, the resistive current `f` and the reactive charge `q` (arguments of `ddt`)
  with exact Jacobians, which map directly onto spx's `G`, `C`, `f`, `q` slots (dense core stamp
  over the instance's nodes).
- Parameter defaults are evaluated at binding. Initialization events execute when the caller sets
  `initial_step`; general parameter-expression hoisting remains VA-5 work.
- `$limit(V(a,b), "pnjlim", vt, vcrit)` uses an independently written logarithmic limiter based
  on Nagel (1975) and sets `noncon`. `fetlim` uses an original bounded voltage step.
- Voltage contributions allocate branch-current unknowns; internal-node collapsing is deferred
  to VA-4.
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
| VA-4 | Coverage for real models, `$limit`, hoisting, node collapsing | User-supplied BSIM4/PSP103 parse and simulate; match ngspice+OpenVAF |
| VA-5 | Performance (hoisting, JIT) | Within 2× of OSDI-compiled models in ngspice |


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
its retained variables. Output buffers and variable storage are reused; expression temporaries
still allocate dynamic gradient vectors. Hoisting and allocation optimization remain VA-5 work.
Treat the public compiled metadata as read-only.

Unknown order is `ports`, `internal_nodes`, `branch_unknowns`. Ground ports retain an unused,
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

Initialization events all use `input.initial_step`; the caller controls when to assert it.
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
- Reading a branch current requires a voltage contribution on that branch. Port-flow accesses,
  `idt`, cross/timer events, recursion, and block-local parameters are unsupported.
- Arrays are one-dimensional with parameter-only bounds and at most 100,001 elements;
  array function arguments and their initializers are unsupported. Function nesting and
  semantic expression nesting are capped at 128. Integer arithmetic uses f64 storage, so
  integers beyond the exact f64 range are not represented exactly.
- Noise generators return zero. Node collapsing, general expression hoisting and JIT remain deferred.

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
synthetic source loaders have only the built-in `disciplines.vams` and `constants.vams`.

Each file is compiled once during elaboration, all its modules are cached, and compiled modules
are shared across analysis circuits. Duplicate module names are errors. Bindings and retained
interpreter variables belong to individual instances and analyses. Fixed chunks of 64 instances,
dense private stamps and ordered gather preserve thread-count determinism.

`.model name module parameter=value ...` selects a loaded module. Module and numeric parameter
names at the SPICE boundary are case-insensitive; original VA source identifiers retain their
language semantics. Instance overrides follow model overrides. `Nname nodes... model
parameter=value ...` connects terminals in module port order; ground is supported. Subcircuits
rename N terminals and local model references. Internal nodes appear as `v(n1#net)`, branch
currents as `i(n1#branch)`. Wrong port counts, unknown modules and invalid bindings report
`E-ELAB-PARAM`; VA compiler errors retain their VA code, point to the HDL card, and carry a
VA source location note. Runtime failures report `E-VA-EVAL`.

Instance numeric parameters support session alterations and group rebinding; a VA parameter
named `dc` can be swept with the engine's existing `.dc instance start stop step` interface.
VA devices inject no independent AC excitation. They contribute their conductance and charge
Jacobians to small-signal assembly. No implicit gmin conductance is added: a model can use
`$simparam("gmin", default)` explicitly.

Limitations: the interpreter remains allocation-heavy and every VA instance is classified as
nonlinear. All terminals are conservatively treated as DC-connected by topology lint. AC
linearization uses the existing DC evaluation context, so `analysis("ac")` is not yet exposed
through this adapter. Initialization events follow `init_junctions` and transient time zero;
they may execute on multiple Newton evaluations at the initial point. Includes are read from
the filesystem at elaboration; exported decks retain HDL paths and require those files.
Rollback restores limiter state; retained procedural variables are not separately snapshotted.
Models should recompute voltage-dependent quantities on every evaluation.

Integration tests cover built-in diode agreement (IS, N, RS, depletion charge and transit time),
resistor/capacitor OP/DC/AC/transient behavior, analytic RC response, hierarchical instances,
parameter overrides, initialization, runtime errors and bit-identical 1/4-thread results.
No network, external code, interface headers or new third-party dependencies were used.
