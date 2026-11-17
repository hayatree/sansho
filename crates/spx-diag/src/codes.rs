//! Registry of every diagnostic code: stable IDs with documentation and generic remedies.
//!
//! `docs/diagnostics.md` is generated from this table (see the `spx-diag` tests), and a test
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
    // --- Parsing ---
    e(
        "E-PARSE-ARITY",
        "Wrong number of fields",
        "An element or control card has too few or too many positional fields.",
        &["Compare the line with the usage shown in the note."],
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
        &["Check the element letter; subcircuits (`X`) and some sources are not supported yet."],
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
        &["See `docs/architecture.md` for the supported `.meas` forms."],
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
    w(
        "W-PARSE-CONTROL",
        "`.control` block skipped",
        "ngspice `.control` scripts are not executed by spx.",
        &["Use analysis cards and `.meas` instead of control scripts."],
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
    // --- Elaboration ---
    e(
        "E-ELAB-CONTROL",
        "Invalid controlling source",
        "A current-controlled source (F, H) references a name that is not a voltage source.",
        &["Point the controlling name at an independent voltage source (`V...`)."],
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
        "An internal consistency check failed. This is a bug in spx.",
        &["Please report the netlist that triggers it."],
    ),
    e(
        "E-ELAB-MODEL",
        "Unsupported model",
        "The model level or type is not supported.",
        &["Use a supported level (level 1 for diodes, BJTs and MOSFETs in this version)."],
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
        &["Fix the instance parameter."],
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
    w(
        "W-MODEL-IGNORED",
        "Model parameter ignored",
        "A `.model` parameter is not implemented in this version and has no effect.",
        &["Check the model documentation for the supported parameters."],
    ),
    w(
        "W-MODEL-TEMP",
        "Temperature not applied",
        "Temperature scaling of model parameters is not implemented yet.",
        &["Simulate at TNOM (27 °C) until temperature scaling lands."],
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
        "E-DC-SOURCE",
        "Invalid DC sweep source",
        "The `.dc` source is not an independent voltage or current source.",
        &["Sweep a `V` or `I` element."],
    ),
    e(
        "E-SOLVE-SINGULAR",
        "Singular matrix",
        "The circuit matrix cannot be factored: the circuit equations do not determine every \
         unknown.",
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
        &["Check the condition level and the analysis range."],
    ),
    w(
        "W-RUN-NOANALYSIS",
        "No analysis",
        "The netlist contains no analysis card.",
        &["Add `.op`, `.dc`, `.ac` or `.tran`."],
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
