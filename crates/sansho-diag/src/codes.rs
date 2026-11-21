//! Registry of every diagnostic code: stable IDs with documentation and generic remedies.
//!
//! `docs/diagnostics.md` is generated from this table (see the `sansho-diag` tests), and a test
//! checks that every code used anywhere in the workspace is registered here.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use serde::Serialize;

use crate::Severity;

/// Documentation of one diagnostic code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CodeInfo {
    /// Stable identifier.
    pub code: &'static str,
    /// Severity with which the code is reported.
    pub severity: Severity,
    /// One-line summary.
    pub title: &'static str,
    /// What the condition means.
    pub explanation: &'static str,
    /// Generic remedies (specific ones are attached to each diagnostic).
    pub remedies: &'static [&'static str],
}

/// Look up a code.
pub fn lookup(code: &str) -> Option<&'static CodeInfo> {
    CODES.iter().find(|c| c.code.eq_ignore_ascii_case(code))
}

const fn e(
    code: &'static str,
    title: &'static str,
    explanation: &'static str,
    remedies: &'static [&'static str],
) -> CodeInfo {
    CodeInfo {
        code,
        severity: Severity::Error,
        title,
        explanation,
        remedies,
    }
}

const fn w(
    code: &'static str,
    title: &'static str,
    explanation: &'static str,
    remedies: &'static [&'static str],
) -> CodeInfo {
    CodeInfo {
        code,
        severity: Severity::Warning,
        title,
        explanation,
        remedies,
    }
}

/// All registered codes, grouped by prefix.
pub const CODES: &[CodeInfo] = &[
    CodeInfo {
        code: "I-CIDER-MATERIAL",
        severity: Severity::Info,
        title: "Silicon material report",
        explanation: "The numerical device requested its operating-temperature silicon properties with output material.",
        remedies: &[],
    },
    e(
        "E-CIDER-MODEL",
        "Invalid numerical semiconductor structure",
        "A numerical device model has an invalid mesh, spatial region, dopant profile, material constant or contact.",
        &[
            "Use increasing mesh coordinates and positive spacing, area, width and absolute temperature. Level one requires 3–4096 silicon vertices. The two-dimensional limit is 1024 tensor-product vertices with at least two on each axis.",
            "For a planar device, give every terminal an electrode on the active mesh. Use disjoint vertices for different terminals. Mesh indices start at one. Check the IDs of regions, materials, interfaces and contacts.",
            "A failed planar equilibrium solve produces this diagnostic during elaboration. Check gate work function in eV, interface charge in C/cm², doping in cm⁻³ and mesh spacings in µm.",
            "For level-one NBJT, place the base contact in the opposite-doped region between like-doped end contacts.",
        ],
    ),
    e(
        "E-PARSE-COND",
        "Invalid conditional netlist block",
        "A conditional directive is misplaced, unbalanced or has an invalid expression.",
        &["Check the parameter scope and the nesting of .if, .elseif, .else and .endif."],
    ),
    e(
        "E-LINE-PARAM",
        "Invalid transmission line parameters",
        "The line parameters do not define a supported passive two-port or RC ladder.",
        &[
            "For Y and P, supply a positive model LENGTH and a positive instance LEN when overriding it. Ground both TXL references.",
            "For T, supply Z0 and either TD in seconds or F in hertz with optional NL in wavelengths.",
            "For O, specify positive LEN and RLC, RC, LC or RG per-unit-length parameters. For U, specify positive L and K greater than one. An explicit N must be an integer from 1 through 4096.",
        ],
    ),
    e(
        "E-LINE-MATRIX",
        "Invalid transmission line matrix",
        "A per-unit-length line matrix has an incompatible size, a non-finite entry or an inadmissible eigenvalue.",
        &[
            "Supply N(N+1)/2 upper-triangle entries in row order. R and G also accept N diagonal entries. L and C must be positive definite, R and G positive semidefinite.",
        ],
    ),
    e(
        "E-POLY-ARITY",
        "Invalid polynomial dimension or controls",
        "The control list and coefficients must match the dimension of the polynomial.",
        &[
            "Give a positive integer dimension, one node pair (E/G) or branch name (F/H) per input, and at least one coefficient.",
        ],
    ),
    e(
        "E-COUPLING-K",
        "Invalid coupling coefficient",
        "A mutual inductance requires distinct windings and a finite coefficient in [-1, 1].",
        &["Check the winding names and the signed coupling coefficient."],
    ),
    e(
        "E-COUPLING-L",
        "Undefined or invalid winding",
        "A coupled winding must reference an inductor with positive effective inductance.",
        &["Use two existing L instance names in the same scope."],
    ),
    e(
        "E-BSOURCE-EXPR",
        "Invalid behavioural source expression",
        "A source expression contains an invalid function, probe or non-finite value or derivative.",
        &[
            "Check the function arguments, node and branch names and the expression domain at the operating point.",
        ],
    ),
    e(
        "E-SWITCH-MODEL",
        "Invalid switch model or control",
        "A switch requires a matching SW or CSW model, positive resistances and a resolvable control.",
        &["Check the model type and parameters and the controlling branch of a W element."],
    ),
    // --- Verilog-A compiler ---
    CodeInfo {
        code: "I-VA-JIT",
        severity: Severity::Info,
        title: "Verilog-A interpreter selected",
        explanation: "The native compiler could not specialize this binding. Evaluation continues with the interpreter.",
        remedies: &[],
    },
    e(
        "E-VA-SEMA",
        "Invalid Verilog-A semantics",
        "A name, type, assignment, branch access, or function call is invalid.",
        &[
            "Check declarations, argument counts, types and contribution targets at the reported span.",
        ],
    ),
    e(
        "E-VA-EVAL",
        "Verilog-A evaluation failed",
        "The model requested an error or its interpreter could not complete evaluation.",
        &["Check the model message, array bounds, loops and runtime conditions."],
    ),
    e(
        "E-VA-PARAM",
        "Invalid Verilog-A parameter binding",
        "A parameter name is unknown, cannot be overridden, or violates its declared type or range.",
        &["Use a declared non-local parameter name and a finite value within its allowed ranges."],
    ),
    e(
        "E-VA-SYNTAX",
        "Invalid Verilog-A syntax",
        "The analog source does not match the supported grammar.",
        &["Check the source at the reported position and docs/verilog-a.md."],
    ),
    e(
        "E-VA-UNSUPPORTED",
        "Unsupported Verilog-A construct",
        "The source uses a language construct outside the analog compiler subset.",
        &[
            "Check the source at the reported position and docs/verilog-a.md.",
            "Use `ddt` in analog statements. For repeated stateful calls, use an analog-for loop with a genvar so each iteration has its own state.",
        ],
    ),
    e(
        "E-VA-PREPROC",
        "Verilog-A preprocessing failed",
        "A macro or conditional directive is malformed or exceeds the expansion limit.",
        &["Check the source at the reported position and docs/verilog-a.md."],
    ),
    e(
        "E-VA-INCLUDE",
        "Verilog-A include failed",
        "An include could not be loaded or exceeds the nesting limit.",
        &["Check the source at the reported position and docs/verilog-a.md."],
    ),
    // --- Parsing ---
    e(
        "E-PARSE-ARITY",
        "Wrong number of fields",
        "An element or control card has too few or too many positional fields.",
        &[
            "Compare the line with the usage shown in the diagnostic. DC requires one or two complete source/start/stop/step groups. OP takes no arguments. A thermal VDMOS instance requires five nodes in drain, gate, source, junction and case order.",
        ],
    ),
    e(
        "E-PARSE-DUPLICATE",
        "Duplicate name",
        "Two elements or two `.model` cards share a name (names are case-insensitive).",
        &["Rename one of them."],
    ),
    e(
        "E-PARSE-ELEMENT",
        "Unsupported element type",
        "The first letter of the line does not name a supported element type.",
        &["Check the element letter against the supported devices in docs/compatibility.md."],
    ),
    e(
        "E-PARSE-EXPR",
        "Invalid parameter expression",
        "A `{...}` / `'...'` expression, `.param` or `.func` could not be parsed or evaluated \
         (syntax error, unknown parameter or function, or a parameter that depends on itself).",
        &[
            "Check the expression and that every parameter it uses is defined with `.param` \
           or as a subcircuit parameter.",
        ],
    ),
    e(
        "E-PARSE-INCLUDE",
        "Cannot include file",
        "An `.include` or `.lib` file could not be read, or the library has no such section. \
         Netlists passed as text (e.g. over MCP) cannot include files unless loaded from a \
         path.",
        &["Check the path; relative paths are resolved from the including file."],
    ),
    e(
        "E-PARSE-MEAS",
        "Invalid `.meas` card",
        "A `.meas` card could not be parsed.",
        &[
            "Check event qualifiers, derivative location and PARAM expression syntax. See docs/analyses.md.",
        ],
    ),
    e(
        "E-PARSE-MODEL",
        "Invalid or missing model",
        "A device references a model that is not defined, the model type does not match the \
         device, or a `.model` card is malformed.",
        &["Define the model with `.model <name> <type> (...)` or fix the name."],
    ),
    e(
        "E-PARSE-NUMBER",
        "Invalid number",
        "A token that must be a number could not be parsed (SPICE scale suffixes such as \
         `k`, `meg`, `u`, `n`, `p` are accepted).",
        &["Write the value as a plain number with an optional SPICE suffix."],
    ),
    e(
        "E-PARSE-SUBCKT",
        "Invalid subcircuit",
        "A `.subckt` definition is malformed (e.g. missing `.ends`), or an `X` instance \
         references an undefined subcircuit, connects the wrong number of nodes, or nests \
         recursively.",
        &["Compare the `X` line with the `.subckt` port list shown in the note."],
    ),
    e(
        "E-PARSE-UNEXPECTED",
        "Unexpected token",
        "A token appears where it is not allowed.",
        &["Remove the token or check the card syntax."],
    ),
    e(
        "E-PARSE-VALUE",
        "Invalid value",
        "A value is out of range, e.g. a zero resistance or a non-finite number.",
        &["Use a finite value in the valid range for the parameter."],
    ),
    e(
        "E-CONTROL",
        "Invalid control program",
        "A batch command, expression, or block could not be evaluated.",
        &["Check the command and source location against docs/control.md."],
    ),
    e(
        "E-CONTROL-LIMIT",
        "Control iteration limit exceeded",
        "The script exhausted its shared iteration budget.",
        &["Check loop conditions and increments."],
    ),
    e(
        "E-CONTROL-OUTPUT",
        "Control output denied or failed",
        "The output policy or filesystem prevented a control command from writing its file.",
        &["Use a permitted output directory and a relative filename."],
    ),
    w(
        "W-CONTROL-GRAPHICS",
        "Graphics command skipped",
        "Batch execution has no plotting window.",
        &["Use print, write or wrdata to inspect values."],
    ),
    w(
        "W-PARSE-IGNORED",
        "Parameter ignored",
        "A parameter was recognized syntactically but has no effect in this version.",
        &["Remove the parameter, or check `docs/architecture.md` for the supported set."],
    ),
    w(
        "W-PARSE-UNSUPPORTED",
        "Control card ignored",
        "A dot card is not supported and was ignored.",
        &["Remove the card; check the roadmap for its planned support."],
    ),
    w(
        "W-OPT-IGNORED",
        "Option ignored",
        "An `.options` entry is unknown or not supported.",
        &["Check the option name; unsupported options have no effect."],
    ),
    e(
        "E-OUTPUT-VECTOR",
        "Unknown device output",
        "The requested internal vector does not identify a supported instance and parameter.",
        &["Use the flattened instance name and a parameter listed in docs/analyses.md."],
    ),
    e(
        "E-PRINT",
        "Invalid print card",
        "The batch table needs an analysis name and at least one output selector.",
        &["Specify .print followed by op, dc, ac, tran or noise and the requested signals."],
    ),
    w(
        "W-PRINT",
        "Unavailable print column",
        "A print request names a missing analysis or signal.",
        &["Check the analysis card and the selected nodes, branches or vectors."],
    ),
    w(
        "W-PLOT-IGNORED",
        "ASCII plot skipped",
        "Sansho recognizes .plot and produces tabular output through .print.",
        &["Use .print for a table or save the raw plot."],
    ),
    w(
        "W-OPT-INTERP",
        "Transient output interpolation unavailable",
        "INTERP requests a uniform output grid. The output contains the solver's accepted timepoints.",
        &["Use the time column when processing transient results."],
    ),
    // --- Elaboration ---
    e(
        "E-ELAB-CONTROL",
        "Invalid controlling source",
        "A current-controlled source (F, H) references a name that is not a voltage source.",
        &["Point the controlling name at an independent voltage source (`V...`)."],
    ),
    e(
        "E-XSPICE-FILE",
        "Invalid code-model data file",
        "The model data file could not be opened inside the deck directory or contains invalid records.",
        &[
            "Use a relative path to a regular file and check its format against the model description.",
        ],
    ),
    e(
        "E-XSPICE-PORT",
        "Invalid code-model port",
        "A port has an invalid type, terminal count or vector shape, or a node connects both analog and digital terminals.",
        &["Check the port order and types in docs/xspice.md."],
    ),
    e(
        "E-XSPICE-OSCILLATION",
        "Event iteration limit",
        "A digital or mixed feedback loop did not settle at the indicated time.",
        &["Check feedback polarity and propagation delays in the loop."],
    ),
    e(
        "E-XSPICE-VALUE",
        "Invalid real event",
        "A real-valued driver, sum of drivers or analog bridge produced a nonfinite value.",
        &["Check model gains, offsets and the sum of the real drivers."],
    ),
    e(
        "E-XSPICE-TIME",
        "Invalid event time",
        "A scheduled edge or ramp endpoint cannot be represented at the current simulation time.",
        &["Check the units of the timing and frequency parameters."],
    ),
    e(
        "E-XSPICE-MODEL",
        "Unsupported code model",
        "The A element references an unsupported code-model family.",
        &["See docs/xspice.md for the supported families."],
    ),
    e(
        "E-ELAB-EMPTY",
        "Empty netlist",
        "The netlist has no elements.",
        &["Add circuit elements."],
    ),
    e(
        "E-ELAB-INTERNAL",
        "Internal elaboration error",
        "An internal consistency check failed. This is a bug in Sansho.",
        &["Please report the netlist that triggers it."],
    ),
    e(
        "E-ELAB-MODEL",
        "Unsupported model",
        "The model level or type is not supported.",
        &[
            "Use NJF/PJF level 1 or 2 for J devices. Built-in MOS channels accept levels 1–6, 8, 9, 49 and 54. See docs/compatibility.md for parameter coverage.",
        ],
    ),
    e(
        "E-ELAB-NOGROUND",
        "No ground",
        "No element connects to the reference node `0`.",
        &["Connect the circuit to node `0` (or `gnd`)."],
    ),
    e(
        "E-ELAB-PARAM",
        "Invalid instance parameter",
        "An instance parameter is out of range (e.g. a non-positive channel length).",
        &[
            "Fix the instance parameter.",
            "For HICUM/L2, check the version, AREA, M, QP0, junction parameters and FLSH/FLNQS/TUNODE selectors. [Parameter domains](hicum.md)",
            "For VBIC, check AREA, M, nominal temperature, positive emission coefficients and built-in potentials, nonnegative resistances, the WBE and WSP partitions, and the SELFT and QBM selectors.",
            "BSIM1/2 require positive effective dimensions, TOX in micrometres, PHI and low-field mobility. For JFET2, check the junction potential, exponents, geometry, lead impedances and relaxation times.",
        ],
    ),
    w(
        "W-ELAB-DANGLING",
        "Dangling node",
        "A node is connected to only one terminal; this usually indicates a typo.",
        &["Check the node name for typos."],
    ),
    w(
        "W-ELAB-NODC",
        "No DC path to ground",
        "Some nodes have no DC path to ground (e.g. only capacitors or gates attach to them); \
         the DC matrix may be singular.",
        &["Add a large resistor to ground, or set `.options rshunt=1e12`."],
    ),
    w(
        "W-ELAB-NODE",
        "Unknown node in `.ic` / `.nodeset`",
        "An initial condition or node set names a node that does not exist; it is ignored.",
        &["Check the node name (nodes inside subcircuits are named `x1.node`)."],
    ),
    w(
        "W-ELAB-VLOOP",
        "Loop of voltage sources or inductors",
        "Voltage sources and/or inductors form a loop, which makes the matrix singular.",
        &["Add a small series resistance to one element of the loop."],
    ),
    e(
        "E-UNSUPPORTED",
        "Unsupported device or external code model",
        "The selected device equation or analysis mode is unavailable, or the deck requests an external XSPICE process or shared library.",
        &[
            "See docs/bsim3.md and docs/bsim4.md for the available MOS selectors.",
            "NUMD and NBJT accept levels one and two. NUMOS selects the planar solver. Unknown CIDER cards and unsupported physical models give this diagnostic. See docs/cider.md.",
            "Use a built-in model in place of d_process, d_cosim or a library loaded with codemodel. See docs/xspice.md.",
        ],
    ),
    w(
        "W-BSIM3-PARAM",
        "BSIM3 parameter check",
        "A binned BSIM3 coefficient falls outside a recommended range in Appendix A.10.",
        &["Check the reported value and its geometry coefficients."],
    ),
    w(
        "W-BSIM4-PARAM",
        "BSIM4 parameter check",
        "A selected model or instance falls outside the recommended BSIM4 parameter range.",
        &["Inspect the named parameter, its units and the effective geometry."],
    ),
    w(
        "W-HICUM-SOA",
        "HICUM terminal voltage exceeds its rated limit",
        "With .option warn=1, an accepted OP, DC or transient sample exceeded a level-eight bipolar terminal voltage limit.",
        &[
            "Inspect the device, electrode pair, measured voltage and rating. The maxwarns option limits messages per device and rating.",
        ],
    ),
    w(
        "W-MODEL-IGNORED",
        "Model parameter ignored",
        "A `.model` parameter is not implemented in this version and has no effect.",
        &[
            "Check the model documentation for the supported parameters.",
            "JFET2 ignores AFAC, CDSS, LD, LG, LS, NFING and RG. MOS9 rejects XD, ALPHA and INPUT_DELTA. BSIM1/2 ignore WDF and DELL. VDMOS accepts TEXP0 and TEXP1.",
            "BSIMSOI reports unused model and instance keys, including unsupported bin coefficients. [Level 10 coverage](bsimsoi.md)",
            "VBIC reports unconsumed card keys. It accepts EBBE as an explicit dimensionless emitter-breakdown offset. [Native VBIC](vbic.md)",
            "For a diode, TBV1 and TBV2 are unrecognized. Use TCV (V/K) to subtract TCV × (T − TNOM) from BV.",
        ],
    ),
    w(
        "W-MODEL-TEMP",
        "Temperature not applied",
        "Older Sansho releases reported this warning when they left nominal semiconductor parameters unscaled.",
        &["Use a release with temperature scaling for the selected model."],
    ),
    // --- Lint ---
    w(
        "W-LINT-UNIT-MILLI",
        "`M` means milli",
        "In SPICE, `M`/`m` is the milli prefix (1e-3); mega is `MEG`. A value such as \
         `R1 a b 10M` is 10 mΩ, which is rarely intended.",
        &["Write `10meg` for 10 MΩ, or `10m` explicitly if milli is intended."],
    ),
    w(
        "W-LINT-UNIT-RANGE",
        "Implausible component value",
        "A value is far outside the usual range for its component type (e.g. a capacitor \
         of 1 F or more), which often means a missing scale suffix.",
        &["Add the intended scale suffix (`u`, `n`, `p`, ...)."],
    ),
    w(
        "W-LINT-UNUSED-MODEL",
        "Unused model",
        "A `.model` card is not referenced by any device.",
        &["Remove the model or check the model name on the device line."],
    ),
    w(
        "W-LINT-DEFAULT-GEOM",
        "Default MOSFET geometry",
        "A MOSFET has no `L` and/or `W`, so the default of 100 µm is used, which is rarely \
         intended.",
        &["Give `l=` and `w=` on the instance or set `.options defl= defw=`."],
    ),
    w(
        "W-LINT-MEAS-SIGNAL",
        "Measurement references an unknown signal",
        "A `.meas` card refers to a node or branch that does not exist in the circuit.",
        &[
            "Check the node name; branch currents are available for voltage sources and \
           inductors as `i(<name>)`.",
        ],
    ),
    w(
        "W-LINT-TRAN-STEP",
        "Suspicious transient step",
        "The `.tran` print step is larger than the stop time, or would create an extremely \
         large number of output points.",
        &["Choose `tstep` around `tstop / 1000`."],
    ),
    // --- Analysis ---
    e(
        "E-CONV-OP",
        "Operating point did not converge",
        "Newton iteration failed, and so did gmin stepping and source stepping.",
        &[
            "Look at the culprit nodes: floating gates, missing bias, or wrong polarity are \
             common causes.",
            "Increase the iteration limit with `.options itl1=500`.",
        ],
    ),
    e(
        "E-CONV-DC",
        "DC sweep did not converge",
        "At some sweep point the operating point could not be found, even from scratch.",
        &["Reduce the sweep step, or check the circuit at the reported sweep value."],
    ),
    e(
        "E-CONV-TRAN",
        "Timestep too small",
        "Newton iteration kept failing at a transient timepoint while the step was reduced \
         below the minimum.",
        &[
            "Look at the culprit nodes for discontinuities (ideal switches, very fast edges).",
            "Give sources finite rise and fall times; increase `.options itl4`.",
        ],
    ),
    e(
        "E-CONV-PSS",
        "Periodic shooting did not converge",
        "A transient stage or the periodic sensitivity system failed, or the mean normalized endpoint error exceeded steadycoeff at the shooting iteration limit.",
        &[
            "Check the frequency guess and initial conditions. Allow more stabilization time or shooting iterations. Tighten reltol to refine the shooting grid for sharp transitions.",
        ],
    ),
    e(
        "E-PSS-NOOSC",
        "Oscillation could not be resolved",
        "The selected oscillator node or branch has insufficient swing or too few rising crossings to identify a cycle.",
        &[
            "Choose a node or branch that oscillates. Check the startup excitation and extend tstab.",
        ],
    ),
    e(
        "E-ANALYSIS",
        "Invalid small-signal or Fourier analysis",
        "An analysis requires resolvable ports, a defined transfer, finite model noise and a valid analysis range.",
        &[
            "Check the named source, output probe, transient duration and model parameters.",
            "For .sp, check port numbering, finite port conversions, and two-port forward gain when requesting noise. Invalid Z0 or a nonintegral port number is a parse error.",
            "For .disto, the diagnostic names the device or bipolar extension whose higher derivatives are unavailable. Analytic support covers the diode, MOS1 and the Gummel–Poon subset in the analysis notes.",
            "HICUM/L2 requires a finite positive junction temperature. Its DISTO derivatives are unavailable.",
            "VBIC reports the instance when junction temperature becomes nonpositive or nonfinite. DISTO reports unavailable higher derivatives for the native level-four device.",
            "A self-heated VDMOS requires finite positive absolute junction temperature. Invalid RTHJC, RTHCA, CTHJ and channel parameters yield E-ELAB-PARAM.",
        ],
    ),
    e(
        "E-DC-SOURCE",
        "Invalid DC sweep source",
        "The sweep target must be a resistor, independent voltage or current source, or TEMP.",
        &[
            "Check both target names on a nested DC card. Each must identify a distinct resistor, independent source or TEMP.",
        ],
    ),
    e(
        "E-SOLVE-SINGULAR",
        "Singular matrix",
        "The solver encountered a singular Jacobian. A nonlinear operating-point solve tries the \
         enabled continuation methods because the matrix can lose rank at a trial bias.",
        &[
            "Look for floating nodes, loops of voltage sources or inductors, and cut sets of \
             current sources.",
            "Add `.options rshunt=1e12` to tie every node weakly to ground.",
        ],
    ),
    e(
        "E-SOLVE-BACKEND",
        "Linear solver failure",
        "The sparse linear solver reported an unexpected failure.",
        &["Please report the netlist that triggers it."],
    ),
    w(
        "W-MEAS-FAILED",
        "Measurement failed",
        "A `.meas` could not be evaluated (e.g. the trigger condition never occurred).",
        &[
            "Check the condition, event count and analysis range. DERIV requires two distinct samples. PARAM names must have a successful value at the point of evaluation.",
        ],
    ),
    w(
        "W-RUN-NOANALYSIS",
        "No analysis",
        "The netlist contains no analysis card.",
        &["Add `.op`, `.dc`, `.ac` or `.tran`."],
    ),
    e(
        "E-MC-COUNT",
        "Invalid Monte Carlo run count",
        "A statistical batch requires a positive run count.",
        &["Choose a count within the interface limit."],
    ),
    e(
        "E-MC-PARAM",
        "Invalid statistical parameter",
        "A statistical expression is unsupported in this field or produced an invalid numeric value.",
        &["Check the parameter expression and Gaussian sigma divisor."],
    ),
    e(
        "E-MC-TOPOLOGY",
        "Statistical topology change",
        "A statistical binding requires different device connectivity, internal nodes or stamps.",
        &["Use separate decks for different topologies."],
    ),
    // --- API / MCP ---
    e(
        "E-API-INPUT",
        "Invalid request",
        "A request to the session API or the MCP server is malformed.",
        &["Check the argument names and types against the tool schema."],
    ),
    e(
        "E-API-UNKNOWN-ID",
        "Unknown circuit or job",
        "The given circuit or job identifier does not exist in this session.",
        &["Use an identifier returned by `load` / `simulate` in the same session."],
    ),
    e(
        "E-API-UNKNOWN-SIGNAL",
        "Unknown signal",
        "The requested signal does not exist in the result.",
        &["Use one of the variable names listed in the job summary."],
    ),
    e(
        "E-API-ALTER",
        "Cannot alter parameter",
        "The instance or parameter does not exist, or the parameter cannot be changed.",
        &["Use an instance name from the netlist and a parameter it supports."],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_well_formed() {
        for (i, c) in CODES.iter().enumerate() {
            let prefix = match c.severity {
                Severity::Info => "I-",
                Severity::Error => "E-",
                Severity::Warning => "W-",
            };
            assert!(c.code.starts_with(prefix), "{}", c.code);
            assert!(
                CODES[i + 1..].iter().all(|o| o.code != c.code),
                "{}",
                c.code
            );
        }
    }
}
