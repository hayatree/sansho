//! Span-annotated analog syntax tree.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
use spx_diag::Span;

/// An identifier with its source position.
#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    /// Declared identifier.
    pub name: String,
    /// Original source position.
    pub span: Span,
}
/// Parsed translation unit with preprocessor source names.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFile {
    /// Canonical source names indexed by span file IDs.
    pub sources: Vec<String>,
    /// Module declarations in source order.
    pub modules: Vec<Module>,
    /// Nature definitions.
    pub natures: Vec<Definition>,
    /// Discipline definitions.
    pub disciplines: Vec<Definition>,
    /// Original source position.
    pub span: Span,
}
/// A nature or discipline definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    /// Declared identifier.
    pub name: Ident,
    /// Optional inherited definition.
    pub parent: Option<Ident>,
    /// Ordered property names and values.
    pub properties: Vec<(Ident, Expr)>,
    /// Original source position.
    pub span: Span,
}
/// A module declaration and its ordered items.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Declared identifier.
    pub name: Ident,
    /// Terminal order from the module header.
    pub ports: Vec<Ident>,
    /// Declarations and analog blocks in source order.
    pub items: Vec<ModuleItem>,
    /// Original source position.
    pub span: Span,
}
/// A source-positioned module or block declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItem {
    /// Syntactic form.
    pub kind: ItemKind,
    /// Original source position.
    pub span: Span,
}
/// Kinds of module and local declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// Port direction and optional discipline declaration.
    PortDecl {
        /// Port direction: input, output, or inout.
        direction: String,
        /// Declared physical discipline.
        discipline: Option<Ident>,
        /// Identifiers declared by this item.
        names: Vec<Ident>,
    },
    /// Discipline declaration for nets.
    NetDecl {
        /// Declared physical discipline.
        discipline: Ident,
        /// Identifiers declared by this item.
        names: Vec<Ident>,
    },
    /// Ground net declaration.
    Ground(Vec<Ident>),
    /// Parameter declaration.
    Param(ParamDecl),
    /// Variable declaration.
    Var(VarDecl),
    /// Named branch declaration.
    Branch {
        /// One or two branch endpoint expressions.
        nodes: Vec<Expr>,
        /// Identifiers declared by this item.
        names: Vec<Ident>,
    },
    /// Analog behavior block.
    Analog(Stmt),
    /// Analog function declaration.
    Function(AnalogFunction),
    /// Alternative parameter name.
    AliasParam {
        /// Declared identifier.
        name: Ident,
        /// Aliased parameter identifier.
        target: Ident,
    },
}
/// Placement of external overrides specified by the parameter's type attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Explicit model-card parameter.
    Model,
    /// Instance-line parameter.
    Instance,
}
/// A parameter declaration with default and validation constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    /// Declared identifier.
    pub name: Ident,
    /// Declared language type.
    pub ty: String,
    /// Whether this is a localparam.
    pub local: bool,
    /// Explicit placement attribute. Unannotated parameters permit instance overrides.
    pub placement: Option<ParamKind>,
    /// Optional initializer or parameter default.
    pub value: Expr,
    /// Declared from/exclude constraints.
    pub ranges: Vec<Range>,
    /// Original source position.
    pub span: Span,
}
/// A from/exclude interval or singleton constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    /// Whether matching values are excluded.
    pub exclude: bool,
    /// Lower endpoint or singleton value.
    pub lower: Expr,
    /// Upper endpoint, absent for a singleton.
    pub upper: Option<Expr>,
    /// Whether the lower endpoint is included.
    pub lower_closed: bool,
    /// Whether the upper endpoint is included.
    pub upper_closed: bool,
    /// Original source position.
    pub span: Span,
}
/// A scalar or array variable declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    /// Declared language type.
    pub ty: String,
    /// Declared identifier.
    pub name: Ident,
    /// Array bounds in declaration order.
    pub dimensions: Vec<(Expr, Expr)>,
    /// Optional initializer or parameter default.
    pub value: Option<Expr>,
    /// Original source position.
    pub span: Span,
}
/// An analog function with declarations and executable body.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalogFunction {
    /// Declared identifier.
    pub name: Ident,
    /// Declared language type.
    pub ty: String,
    /// Declarations local to this scope.
    pub declarations: Vec<ModuleItem>,
    /// Executable function or statement body.
    pub body: Vec<Stmt>,
    /// Original source position.
    pub span: Span,
}
/// A statement with its source position.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    /// Syntactic form.
    pub kind: StmtKind,
    /// Original source position.
    pub span: Span,
}
/// One case arm; empty labels represent default.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    /// Matching expressions; empty for default.
    pub labels: Vec<Expr>,
    /// Executable function or statement body.
    pub body: Stmt,
    /// Original source position.
    pub span: Span,
}
/// Analog procedural statement variants.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// Scoped sequential block.
    Block {
        /// Declared identifier.
        name: Option<Ident>,
        /// Declarations local to this scope.
        declarations: Vec<ModuleItem>,
        /// Statements in execution order.
        statements: Vec<Stmt>,
    },
    /// Blocking assignment.
    Assign {
        /// Assignment destination.
        lhs: Expr,
        /// Assigned or contributed expression.
        rhs: Expr,
    },
    /// Analog branch contribution.
    Contrib {
        /// Contribution access expression.
        access: Expr,
        /// Assigned or contributed expression.
        rhs: Expr,
    },
    /// Conditional statement.
    If {
        /// Conditional expression.
        condition: Expr,
        /// Statement executed for a true condition.
        then_stmt: Box<Stmt>,
        /// Optional statement executed for a false condition.
        else_stmt: Option<Box<Stmt>>,
    },
    /// First-match case selection.
    Case {
        /// Case selector expression.
        selector: Expr,
        /// Case arms in source order.
        arms: Vec<CaseArm>,
    },
    /// Initialized conditional loop.
    For {
        /// Loop initialization statement.
        init: Box<Stmt>,
        /// Conditional expression.
        condition: Expr,
        /// Loop update statement.
        step: Box<Stmt>,
        /// Executable function or statement body.
        body: Box<Stmt>,
    },
    /// Conditional loop.
    While {
        /// Conditional expression.
        condition: Expr,
        /// Executable function or statement body.
        body: Box<Stmt>,
    },
    /// Counted loop.
    Repeat {
        /// Repeat count expression.
        count: Expr,
        /// Executable function or statement body.
        body: Box<Stmt>,
    },
    /// Event-controlled statement.
    Event {
        /// Event expressions joined by or.
        events: Vec<Expr>,
        /// Executable function or statement body.
        body: Box<Stmt>,
    },
    /// System task invocation.
    SysTask {
        /// Declared identifier.
        name: Ident,
        /// Arguments in source order.
        args: Vec<Expr>,
    },
    /// Empty statement.
    Empty,
}
/// An expression with its source position.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Syntactic form.
    pub kind: ExprKind,
    /// Original source position.
    pub span: Span,
}
/// Analog expression variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer literal.
    Int(i64),
    /// Real literal.
    Real(f64),
    /// String literal.
    Str(String),
    /// Identifier reference.
    Ident(Ident),
    /// Braced list of values, e.g. a noise spectrum table.
    List(Vec<Expr>),
    /// Port branch terminal in an angle-bracket branch declaration.
    Port(Ident),
    /// Array element access.
    Index(Box<Expr>, Box<Expr>),
    /// Unary operator expression.
    Unary {
        /// Operator spelling.
        op: String,
        /// Operand expression.
        expr: Box<Expr>,
    },
    /// Binary operator expression.
    Binary {
        /// Operator spelling.
        op: String,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
    /// Conditional expression.
    Ternary {
        /// Conditional expression.
        condition: Box<Expr>,
        /// Expression selected when true.
        yes: Box<Expr>,
        /// Expression selected when false.
        no: Box<Expr>,
    },
    /// Function call.
    Call {
        /// Declared identifier.
        name: Ident,
        /// Arguments in source order.
        args: Vec<Expr>,
    },
    /// System function call.
    SysCall {
        /// Declared identifier.
        name: Ident,
        /// Arguments in source order.
        args: Vec<Expr>,
    },
    /// Potential or flow access.
    Access {
        /// Access function identifier.
        nature: Ident,
        /// Arguments in source order.
        args: Vec<Expr>,
        /// Whether angle-bracket port-flow syntax was used.
        port_flow: bool,
    },
}
