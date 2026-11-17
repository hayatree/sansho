# Verilog-A compact-model compiler (`spx-va`)

Status: design (M4). Implementation proceeds in the phases listed at the end.

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
| `sema.rs` | (phase 2) symbol resolution, types, branch/port mapping, parameter ranges |
| `ir.rs`, `eval.rs` | (phase 2) lowered code and the AD interpreter |
| `device.rs` | (phase 3) `DeviceGroup` implementation for compiled modules |

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
- `@(initial_model)` / parameter-only expressions are evaluated once per model / instance
  (hoisting), not every iteration.
- `$limit(V(a,b), "pnjlim", ...)` uses spx's junction limiting and sets `noncon`.
- Collapsing of internal nodes (`V(a,b) <+ 0` with zero-valued resistance) is detected per
  instance.
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
| VA-1 | Preprocessor, lexer, parser, AST, diagnostics | Parses hand-written test models covering §3; robust errors |
| VA-2 | Semantic analysis, IR, AD interpreter | Evaluates test models; Jacobians match finite differences |
| VA-3 | Device integration, `.hdl`, `N` instances | VA diode/resistor match built-ins; engine tests pass |
| VA-4 | Coverage for real models, `$limit`, hoisting, node collapsing | User-supplied BSIM4/PSP103 parse and simulate; match ngspice+OpenVAF |
| VA-5 | Performance (hoisting, JIT) | Within 2× of OSDI-compiled models in ngspice |
