//! Resolved typed tree IR for the analog interpreter.
//! Provenance: original implementation from general compiler literature and Accellera LRM 2.4.
use crate::ast;
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    #[default]
    Real,
    Integer,
    String,
}
impl Ty {
    #[cfg(feature = "jit")]
    fn is_real(&self) -> bool {
        *self == Ty::Real
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "integer" | "genvar" => Self::Integer,
            "string" => Self::String,
            _ => Self::Real,
        }
    }
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) struct Expr {
    #[cfg_attr(feature = "jit", serde(rename = "k"))]
    pub kind: E,
    #[cfg_attr(
        feature = "jit",
        serde(rename = "t", default, skip_serializing_if = "Ty::is_real")
    )]
    pub ty: Ty,
    #[cfg_attr(
        feature = "jit",
        serde(rename = "p", default, skip_serializing_if = "std::ops::Not::not")
    )]
    pub parameter: bool,
    #[cfg_attr(
        feature = "jit",
        serde(rename = "q", default, skip_serializing_if = "std::ops::Not::not")
    )]
    pub reactive: bool,
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) enum E {
    Num(#[cfg_attr(feature = "jit", serde(with = "crate::cache::bits"))] f64),
    Str(std::sync::Arc<str>),
    Param(usize),
    Var(usize),
    Index(usize, Box<Expr>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
    Voltage(Option<usize>, Option<usize>),
    Current(usize),
    Call(String, Vec<Expr>),
    Derivative(Option<usize>, Box<Expr>, sansho_diag::Span),
    Integral(usize, Box<Expr>, Box<Expr>, sansho_diag::Span),
    Unsupported(String, sansho_diag::Span),
    Function(usize, Vec<Expr>),
    Given(usize),
    Connected(usize),
    Ddx(Box<Expr>, usize),
    Limit(usize, String, Vec<Expr>, Option<usize>),
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) struct Variable {
    pub ty: Ty,
    pub dimensions: Vec<(Expr, Expr)>,
    pub init: Option<Expr>,
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) enum Stmt {
    Block(Vec<(usize, Variable)>, Vec<Stmt>),
    Assign(Expr, Expr),
    Contrib(usize, bool, f64, Expr),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Case(Expr, Vec<(Vec<Expr>, Stmt)>),
    For(Box<Stmt>, Expr, Box<Stmt>, Box<Stmt>),
    While(Expr, Box<Stmt>),
    Repeat(Expr, Box<Stmt>),
    Event {
        initial: bool,
        startup: bool,
        last: bool,
        body: Box<Stmt>,
    },
    Task(String, Vec<Expr>),
    Empty,
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) struct GenLoop {
    pub var: usize,
    pub initial: Expr,
    pub condition: Expr,
    pub next: Expr,
}

#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) struct Branch {
    pub a: Option<usize>,
    pub b: Option<usize>,
    pub current: Option<usize>,
    pub observed: bool,
    pub contributed: bool,
}
#[cfg_attr(feature = "jit", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub result: usize,
    pub args: Vec<(usize, String)>,
    pub locals: Vec<(usize, Variable)>,
    pub body: Vec<Stmt>,
}
// These public types contain checked indices. Only the private artifact reader
// may reconstruct them from bytes after verifying ownership, key and checksum.
macro_rules! cached_struct {
    ($(#[$doc:meta])* $name:ident, $image:ident, $remote:literal { $($fields:tt)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        #[cfg_attr(feature = "jit", derive(serde::Serialize))]
        pub struct $name { $($fields)* }

        #[cfg(feature = "jit")]
        #[derive(serde::Deserialize)]
        #[serde(remote = $remote)]
        pub(crate) struct $image { $($fields)* }
    };
}

cached_struct! {
/// Parameter metadata in declaration order.
ParamInfo, ParamInfoImage, "ParamInfo" {
    /// Declared parameter name.
    pub name: String,
    /// LRM type name: real, integer, or string.
    pub ty: String,
    /// Default expression in the source.
    pub default: ast::Expr,
    /// Inclusive/exclusive validation ranges.
    pub ranges: Vec<ast::Range>,
    /// Alternative external names.
    pub aliases: Vec<String>,
    /// Local parameters cannot be supplied externally.
    pub local: bool,
    /// Explicit model or instance placement attribute.
    pub placement: Option<ast::ParamKind>,
    pub(crate) value: Expr,
    pub(crate) limits: Vec<(bool, Expr, Option<Expr>, bool, bool)>,
} }
cached_struct! {
/// Immutable compiled device type. Unknown order is ports, internal nodes, branch currents.
CompiledModule, CompiledModuleImage, "CompiledModule" {
    /// Module identifier.
    pub name: String,
    /// Terminal names in declaration order (ground ports retain zero residual slots).
    pub ports: Vec<String>,
    /// Used internal electrical nodes, excluding ground.
    pub internal_nodes: Vec<String>,
    /// Names of extra current unknowns.
    pub branch_unknowns: Vec<String>,
    /// Parameter metadata.
    #[cfg_attr(feature = "jit", serde(with = "parameter_images"))]
    pub params: Vec<ParamInfo>,
    #[cfg(feature = "jit")]
    #[serde(skip)]
    pub(crate) kernels: std::sync::Arc<crate::eval::program::Cache>,
    #[cfg(feature = "jit")]
    #[serde(skip)]
    pub(crate) artifact: Option<crate::cache::Artifact>,
    pub(crate) branches: Vec<Branch>,
    pub(crate) variables: Vec<(usize, Variable)>,
    pub(crate) functions: Vec<Function>,
    pub(crate) body: Vec<Stmt>,
    pub(crate) history: Vec<usize>,
    #[cfg(feature = "jit")]
    pub(crate) direct_start: bool,
    pub(crate) slots: usize,
    pub(crate) function_writes: std::collections::BTreeSet<usize>,
    pub(crate) states: usize,
    /// Whether any derivative is evaluated through its own DAE row.
    pub has_derivative_states: bool,
    pub(crate) derivative_loops: std::collections::BTreeMap<usize, Vec<GenLoop>>,
    pub(crate) operator_rows: std::ops::Range<usize>,
    #[cfg_attr(feature = "jit", serde(skip))]
    pub(crate) identity: std::sync::Arc<()>,
    pub(crate) implicit_m: bool,
    pub(crate) param_index: std::collections::BTreeMap<String, Option<usize>>,
    pub(crate) optional_ports: Vec<bool>,
} }

#[cfg(feature = "jit")]
mod parameter_images {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    #[derive(Deserialize)]
    struct Checked(#[serde(with = "ParamInfoImage")] ParamInfo);
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ParamInfo>, D::Error> {
        Vec::<Checked>::deserialize(d).map(|v| v.into_iter().map(|p| p.0).collect())
    }
    pub fn serialize<S: Serializer>(v: &[ParamInfo], s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }
}
impl CompiledModule {
    /// Unknown count before binding-time collapse. Use the bound layout for evaluation.
    pub fn unknown_len(&self) -> usize {
        self.ports.len() + self.internal_nodes.len() + self.branch_unknowns.len()
    }
    /// Whether this module row holds an analog operator state.
    pub fn is_operator_state(&self, original: usize) -> bool {
        self.operator_rows.contains(&original)
    }

    /// Number of per-instance limiting history slots.
    pub fn state_len(&self) -> usize {
        self.states
    }
}
