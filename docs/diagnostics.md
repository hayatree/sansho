# Diagnostic codes

<!-- Generated from crates/spx-diag/src/codes.rs; do not edit by hand.
     Regenerate with `SPX_BLESS=1 cargo test -p spx-diag`. -->

Every diagnostic carries a stable `code`. Codes starting with `E-` are errors (the input cannot be simulated); `W-` codes are warnings. Diagnostics may also carry `culprits` (implicated nodes, branches and devices), `attempts` (strategies that were tried) and `suggestions` (specific remedies).

| Code | Severity | Summary |
|---|---|---|
| [`E-VA-SEMA`](#e-va-sema) | error | Invalid Verilog-A semantics |
| [`E-VA-PARAM`](#e-va-param) | error | Invalid Verilog-A parameter binding |
| [`E-VA-SYNTAX`](#e-va-syntax) | error | Invalid Verilog-A syntax |
| [`E-VA-UNSUPPORTED`](#e-va-unsupported) | error | Unsupported Verilog-A construct |
| [`E-VA-PREPROC`](#e-va-preproc) | error | Verilog-A preprocessing failed |
| [`E-VA-INCLUDE`](#e-va-include) | error | Verilog-A include failed |
| [`E-PARSE-ARITY`](#e-parse-arity) | error | Wrong number of fields |
| [`E-PARSE-DUPLICATE`](#e-parse-duplicate) | error | Duplicate name |
| [`E-PARSE-ELEMENT`](#e-parse-element) | error | Unsupported element type |
| [`E-PARSE-EXPR`](#e-parse-expr) | error | Invalid parameter expression |
| [`E-PARSE-INCLUDE`](#e-parse-include) | error | Cannot include file |
| [`E-PARSE-MEAS`](#e-parse-meas) | error | Invalid `.meas` card |
| [`E-PARSE-MODEL`](#e-parse-model) | error | Invalid or missing model |
| [`E-PARSE-NUMBER`](#e-parse-number) | error | Invalid number |
| [`E-PARSE-SUBCKT`](#e-parse-subckt) | error | Invalid subcircuit |
| [`E-PARSE-UNEXPECTED`](#e-parse-unexpected) | error | Unexpected token |
| [`E-PARSE-VALUE`](#e-parse-value) | error | Invalid value |
| [`W-PARSE-CONTROL`](#w-parse-control) | warning | `.control` block skipped |
| [`W-PARSE-IGNORED`](#w-parse-ignored) | warning | Parameter ignored |
| [`W-PARSE-UNSUPPORTED`](#w-parse-unsupported) | warning | Control card ignored |
| [`W-OPT-IGNORED`](#w-opt-ignored) | warning | Option ignored |
| [`E-ELAB-CONTROL`](#e-elab-control) | error | Invalid controlling source |
| [`E-ELAB-EMPTY`](#e-elab-empty) | error | Empty netlist |
| [`E-ELAB-INTERNAL`](#e-elab-internal) | error | Internal elaboration error |
| [`E-ELAB-MODEL`](#e-elab-model) | error | Unsupported model |
| [`E-ELAB-NOGROUND`](#e-elab-noground) | error | No ground |
| [`E-ELAB-PARAM`](#e-elab-param) | error | Invalid instance parameter |
| [`W-ELAB-DANGLING`](#w-elab-dangling) | warning | Dangling node |
| [`W-ELAB-NODC`](#w-elab-nodc) | warning | No DC path to ground |
| [`W-ELAB-NODE`](#w-elab-node) | warning | Unknown node in `.ic` / `.nodeset` |
| [`W-ELAB-VLOOP`](#w-elab-vloop) | warning | Loop of voltage sources or inductors |
| [`W-MODEL-IGNORED`](#w-model-ignored) | warning | Model parameter ignored |
| [`W-MODEL-TEMP`](#w-model-temp) | warning | Temperature not applied |
| [`W-LINT-UNIT-MILLI`](#w-lint-unit-milli) | warning | `M` means milli |
| [`W-LINT-UNIT-RANGE`](#w-lint-unit-range) | warning | Implausible component value |
| [`W-LINT-UNUSED-MODEL`](#w-lint-unused-model) | warning | Unused model |
| [`W-LINT-DEFAULT-GEOM`](#w-lint-default-geom) | warning | Default MOSFET geometry |
| [`W-LINT-MEAS-SIGNAL`](#w-lint-meas-signal) | warning | Measurement references an unknown signal |
| [`W-LINT-TRAN-STEP`](#w-lint-tran-step) | warning | Suspicious transient step |
| [`E-CONV-OP`](#e-conv-op) | error | Operating point did not converge |
| [`E-CONV-DC`](#e-conv-dc) | error | DC sweep did not converge |
| [`E-CONV-TRAN`](#e-conv-tran) | error | Timestep too small |
| [`E-DC-SOURCE`](#e-dc-source) | error | Invalid DC sweep source |
| [`E-SOLVE-SINGULAR`](#e-solve-singular) | error | Singular matrix |
| [`E-SOLVE-BACKEND`](#e-solve-backend) | error | Linear solver failure |
| [`W-MEAS-FAILED`](#w-meas-failed) | warning | Measurement failed |
| [`W-RUN-NOANALYSIS`](#w-run-noanalysis) | warning | No analysis |
| [`E-API-INPUT`](#e-api-input) | error | Invalid request |
| [`E-API-UNKNOWN-ID`](#e-api-unknown-id) | error | Unknown circuit or job |
| [`E-API-UNKNOWN-SIGNAL`](#e-api-unknown-signal) | error | Unknown signal |
| [`E-API-ALTER`](#e-api-alter) | error | Cannot alter parameter |

## E-VA-SEMA

**Invalid Verilog-A semantics.** A name, type, assignment, branch access, or function call is invalid.

- Check declarations, argument counts, types and contribution targets at the reported span.

## E-VA-PARAM

**Invalid Verilog-A parameter binding.** A parameter name is unknown, cannot be overridden, or violates its declared type or range.

- Use a declared non-local parameter name and a finite value within its allowed ranges.

## E-VA-SYNTAX

**Invalid Verilog-A syntax.** The analog source does not match the supported grammar.

- Check the source at the reported position and docs/verilog-a.md.

## E-VA-UNSUPPORTED

**Unsupported Verilog-A construct.** The source uses a language construct outside the analog compiler subset.

- Check the source at the reported position and docs/verilog-a.md.

## E-VA-PREPROC

**Verilog-A preprocessing failed.** A macro or conditional directive is malformed or exceeds the expansion limit.

- Check the source at the reported position and docs/verilog-a.md.

## E-VA-INCLUDE

**Verilog-A include failed.** An include could not be loaded or exceeds the nesting limit.

- Check the source at the reported position and docs/verilog-a.md.

## E-PARSE-ARITY

**Wrong number of fields.** An element or control card has too few or too many positional fields.

- Compare the line with the usage shown in the note.

## E-PARSE-DUPLICATE

**Duplicate name.** Two elements or two `.model` cards share a name (names are case-insensitive).

- Rename one of them.

## E-PARSE-ELEMENT

**Unsupported element type.** The first letter of the line does not name a supported element type.

- Check the element letter; subcircuits (`X`) and some sources are not supported yet.

## E-PARSE-EXPR

**Invalid parameter expression.** A `{...}` / `'...'` expression, `.param` or `.func` could not be parsed or evaluated (syntax error, unknown parameter or function, or a parameter that depends on itself).

- Check the expression and that every parameter it uses is defined with `.param` or as a subcircuit parameter.

## E-PARSE-INCLUDE

**Cannot include file.** An `.include` or `.lib` file could not be read, or the library has no such section. Netlists passed as text (e.g. over MCP) cannot include files unless loaded from a path.

- Check the path; relative paths are resolved from the including file.

## E-PARSE-MEAS

**Invalid `.meas` card.** A `.meas` card could not be parsed.

- See `docs/architecture.md` for the supported `.meas` forms.

## E-PARSE-MODEL

**Invalid or missing model.** A device references a model that is not defined, the model type does not match the device, or a `.model` card is malformed.

- Define the model with `.model <name> <type> (...)` or fix the name.

## E-PARSE-NUMBER

**Invalid number.** A token that must be a number could not be parsed (SPICE scale suffixes such as `k`, `meg`, `u`, `n`, `p` are accepted).

- Write the value as a plain number with an optional SPICE suffix.

## E-PARSE-SUBCKT

**Invalid subcircuit.** A `.subckt` definition is malformed (e.g. missing `.ends`), or an `X` instance references an undefined subcircuit, connects the wrong number of nodes, or nests recursively.

- Compare the `X` line with the `.subckt` port list shown in the note.

## E-PARSE-UNEXPECTED

**Unexpected token.** A token appears where it is not allowed.

- Remove the token or check the card syntax.

## E-PARSE-VALUE

**Invalid value.** A value is out of range, e.g. a zero resistance or a non-finite number.

- Use a finite value in the valid range for the parameter.

## W-PARSE-CONTROL

**`.control` block skipped.** ngspice `.control` scripts are not executed by spx.

- Use analysis cards and `.meas` instead of control scripts.

## W-PARSE-IGNORED

**Parameter ignored.** A parameter was recognized syntactically but has no effect in this version.

- Remove the parameter, or check `docs/architecture.md` for the supported set.

## W-PARSE-UNSUPPORTED

**Control card ignored.** A dot card is not supported and was ignored.

- Remove the card; check the roadmap for its planned support.

## W-OPT-IGNORED

**Option ignored.** An `.options` entry is unknown or not supported.

- Check the option name; unsupported options have no effect.

## E-ELAB-CONTROL

**Invalid controlling source.** A current-controlled source (F, H) references a name that is not a voltage source.

- Point the controlling name at an independent voltage source (`V...`).

## E-ELAB-EMPTY

**Empty netlist.** The netlist has no elements.

- Add circuit elements.

## E-ELAB-INTERNAL

**Internal elaboration error.** An internal consistency check failed. This is a bug in spx.

- Please report the netlist that triggers it.

## E-ELAB-MODEL

**Unsupported model.** The model level or type is not supported.

- Use a supported level (level 1 for diodes, BJTs and MOSFETs in this version).

## E-ELAB-NOGROUND

**No ground.** No element connects to the reference node `0`.

- Connect the circuit to node `0` (or `gnd`).

## E-ELAB-PARAM

**Invalid instance parameter.** An instance parameter is out of range (e.g. a non-positive channel length).

- Fix the instance parameter.

## W-ELAB-DANGLING

**Dangling node.** A node is connected to only one terminal; this usually indicates a typo.

- Check the node name for typos.

## W-ELAB-NODC

**No DC path to ground.** Some nodes have no DC path to ground (e.g. only capacitors or gates attach to them); the DC matrix may be singular.

- Add a large resistor to ground, or set `.options rshunt=1e12`.

## W-ELAB-NODE

**Unknown node in `.ic` / `.nodeset`.** An initial condition or node set names a node that does not exist; it is ignored.

- Check the node name (nodes inside subcircuits are named `x1.node`).

## W-ELAB-VLOOP

**Loop of voltage sources or inductors.** Voltage sources and/or inductors form a loop, which makes the matrix singular.

- Add a small series resistance to one element of the loop.

## W-MODEL-IGNORED

**Model parameter ignored.** A `.model` parameter is not implemented in this version and has no effect.

- Check the model documentation for the supported parameters.

## W-MODEL-TEMP

**Temperature not applied.** Temperature scaling of model parameters is not implemented yet.

- Simulate at TNOM (27 °C) until temperature scaling lands.

## W-LINT-UNIT-MILLI

**`M` means milli.** In SPICE, `M`/`m` is the milli prefix (1e-3); mega is `MEG`. A value such as `R1 a b 10M` is 10 mΩ, which is rarely intended.

- Write `10meg` for 10 MΩ, or `10m` explicitly if milli is intended.

## W-LINT-UNIT-RANGE

**Implausible component value.** A value is far outside the usual range for its component type (e.g. a capacitor of 1 F or more), which often means a missing scale suffix.

- Add the intended scale suffix (`u`, `n`, `p`, ...).

## W-LINT-UNUSED-MODEL

**Unused model.** A `.model` card is not referenced by any device.

- Remove the model or check the model name on the device line.

## W-LINT-DEFAULT-GEOM

**Default MOSFET geometry.** A MOSFET has no `L` and/or `W`, so the default of 100 µm is used, which is rarely intended.

- Give `l=` and `w=` on the instance or set `.options defl= defw=`.

## W-LINT-MEAS-SIGNAL

**Measurement references an unknown signal.** A `.meas` card refers to a node or branch that does not exist in the circuit.

- Check the node name; branch currents are available for voltage sources and inductors as `i(<name>)`.

## W-LINT-TRAN-STEP

**Suspicious transient step.** The `.tran` print step is larger than the stop time, or would create an extremely large number of output points.

- Choose `tstep` around `tstop / 1000`.

## E-CONV-OP

**Operating point did not converge.** Newton iteration failed, and so did gmin stepping and source stepping.

- Look at the culprit nodes: floating gates, missing bias, or wrong polarity are common causes.
- Increase the iteration limit with `.options itl1=500`.

## E-CONV-DC

**DC sweep did not converge.** At some sweep point the operating point could not be found, even from scratch.

- Reduce the sweep step, or check the circuit at the reported sweep value.

## E-CONV-TRAN

**Timestep too small.** Newton iteration kept failing at a transient timepoint while the step was reduced below the minimum.

- Look at the culprit nodes for discontinuities (ideal switches, very fast edges).
- Give sources finite rise and fall times; increase `.options itl4`.

## E-DC-SOURCE

**Invalid DC sweep source.** The `.dc` source is not an independent voltage or current source.

- Sweep a `V` or `I` element.

## E-SOLVE-SINGULAR

**Singular matrix.** The circuit matrix cannot be factored: the circuit equations do not determine every unknown.

- Look for floating nodes, loops of voltage sources or inductors, and cut sets of current sources.
- Add `.options rshunt=1e12` to tie every node weakly to ground.

## E-SOLVE-BACKEND

**Linear solver failure.** The sparse linear solver reported an unexpected failure.

- Please report the netlist that triggers it.

## W-MEAS-FAILED

**Measurement failed.** A `.meas` could not be evaluated (e.g. the trigger condition never occurred).

- Check the condition level and the analysis range.

## W-RUN-NOANALYSIS

**No analysis.** The netlist contains no analysis card.

- Add `.op`, `.dc`, `.ac` or `.tran`.

## E-API-INPUT

**Invalid request.** A request to the session API or the MCP server is malformed.

- Check the argument names and types against the tool schema.

## E-API-UNKNOWN-ID

**Unknown circuit or job.** The given circuit or job identifier does not exist in this session.

- Use an identifier returned by `load` / `simulate` in the same session.

## E-API-UNKNOWN-SIGNAL

**Unknown signal.** The requested signal does not exist in the result.

- Use one of the variable names listed in the job summary.

## E-API-ALTER

**Cannot alter parameter.** The instance or parameter does not exist, or the parameter cannot be changed.

- Use an instance name from the netlist and a parameter it supports.
