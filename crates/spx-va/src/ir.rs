//! Resolved typed tree IR for the analog interpreter.
//! Provenance: original implementation from general compiler literature and Accellera LRM 2.4.
use crate::ast;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    Real,
    Integer,
    String,
}
impl Ty {
    pub fn parse(s: &str) -> Self {
        match s {
            "integer" | "genvar" => Self::Integer,
            "string" => Self::String,
            _ => Self::Real,
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Expr {
    pub kind: E,
    pub ty: Ty,
    pub parameter: bool,
    pub reactive: bool,
}
#[derive(Debug, Clone)]
pub(crate) enum E {
    Num(f64),
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
    Integral(usize, Box<Expr>, Box<Expr>, spx_diag::Span),
    Unsupported(String, spx_diag::Span),
    Function(usize, Vec<Expr>),
    Given(usize),
    Connected(usize),
    Ddx(Box<Expr>, usize),
    Limit(usize, String, Vec<Expr>, Option<usize>),
}
#[derive(Debug, Clone)]
pub(crate) struct Variable {
    pub ty: Ty,
    pub dimensions: Vec<(Expr, Expr)>,
    pub init: Option<Expr>,
}
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
#[derive(Debug, Clone)]
pub(crate) struct Branch {
    pub a: Option<usize>,
    pub b: Option<usize>,
    pub current: Option<usize>,
    pub observed: bool,
    pub contributed: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub result: usize,
    pub args: Vec<(usize, String)>,
    pub locals: Vec<(usize, Variable)>,
    pub body: Vec<Stmt>,
}
/// Parameter metadata in declaration order.
#[derive(Debug, Clone)]
pub struct ParamInfo {
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
}
/// Immutable compiled device type. Unknown order is ports, internal nodes, branch currents.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Module identifier.
    pub name: String,
    /// Terminal names in declaration order (ground ports retain zero residual slots).
    pub ports: Vec<String>,
    /// Used internal electrical nodes, excluding ground.
    pub internal_nodes: Vec<String>,
    /// Names of extra current unknowns.
    pub branch_unknowns: Vec<String>,
    /// Parameter metadata.
    pub params: Vec<ParamInfo>,
    pub(crate) branches: Vec<Branch>,
    pub(crate) variables: Vec<(usize, Variable)>,
    pub(crate) functions: Vec<Function>,
    pub(crate) body: Vec<Stmt>,
    pub(crate) history: Vec<usize>,
    pub(crate) slots: usize,
    pub(crate) function_writes: std::collections::BTreeSet<usize>,
    pub(crate) states: usize,
    pub(crate) integrals: std::ops::Range<usize>,
    pub(crate) identity: std::sync::Arc<()>,
    pub(crate) implicit_m: bool,
    pub(crate) param_index: std::collections::BTreeMap<String, Option<usize>>,
    pub(crate) optional_ports: Vec<bool>,
}
impl CompiledModule {
    /// Unknown count before binding-time collapse. Use the bound layout for evaluation.
    pub fn unknown_len(&self) -> usize {
        self.ports.len() + self.internal_nodes.len() + self.branch_unknowns.len()
    }
    /// Number of per-instance limiting history slots.
    pub fn state_len(&self) -> usize {
        self.states
    }
}
