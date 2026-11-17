//! Span-annotated analog syntax tree.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
use spx_diag::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFile {
    pub sources: Vec<String>,
    pub modules: Vec<Module>,
    pub natures: Vec<Definition>,
    pub disciplines: Vec<Definition>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    pub name: Ident,
    pub parent: Option<Ident>,
    pub properties: Vec<(Ident, Expr)>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: Ident,
    pub ports: Vec<Ident>,
    pub items: Vec<ModuleItem>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleItem {
    pub kind: ItemKind,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    PortDecl {
        direction: String,
        discipline: Option<Ident>,
        names: Vec<Ident>,
    },
    NetDecl {
        discipline: Ident,
        names: Vec<Ident>,
    },
    Ground(Vec<Ident>),
    Param(ParamDecl),
    Var(VarDecl),
    Branch {
        nodes: Vec<Expr>,
        names: Vec<Ident>,
    },
    Analog(Stmt),
    Function(AnalogFunction),
    AliasParam {
        name: Ident,
        target: Ident,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDecl {
    pub name: Ident,
    pub ty: String,
    pub local: bool,
    pub value: Expr,
    pub ranges: Vec<Range>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub exclude: bool,
    pub lower: Expr,
    pub upper: Option<Expr>,
    pub lower_closed: bool,
    pub upper_closed: bool,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub ty: String,
    pub name: Ident,
    pub dimensions: Vec<(Expr, Expr)>,
    pub value: Option<Expr>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AnalogFunction {
    pub name: Ident,
    pub ty: String,
    pub declarations: Vec<ModuleItem>,
    pub body: Vec<Stmt>,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    pub labels: Vec<Expr>,
    pub body: Stmt,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Block {
        name: Option<Ident>,
        declarations: Vec<ModuleItem>,
        statements: Vec<Stmt>,
    },
    Assign {
        lhs: Expr,
        rhs: Expr,
    },
    Contrib {
        access: Expr,
        rhs: Expr,
    },
    If {
        condition: Expr,
        then_stmt: Box<Stmt>,
        else_stmt: Option<Box<Stmt>>,
    },
    Case {
        selector: Expr,
        arms: Vec<CaseArm>,
    },
    For {
        init: Box<Stmt>,
        condition: Expr,
        step: Box<Stmt>,
        body: Box<Stmt>,
    },
    While {
        condition: Expr,
        body: Box<Stmt>,
    },
    Repeat {
        count: Expr,
        body: Box<Stmt>,
    },
    Event {
        events: Vec<Expr>,
        body: Box<Stmt>,
    },
    SysTask {
        name: Ident,
        args: Vec<Expr>,
    },
    Empty,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Real(f64),
    Str(String),
    Ident(Ident),
    Index(Box<Expr>, Box<Expr>),
    Unary {
        op: String,
        expr: Box<Expr>,
    },
    Binary {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Ternary {
        condition: Box<Expr>,
        yes: Box<Expr>,
        no: Box<Expr>,
    },
    Call {
        name: Ident,
        args: Vec<Expr>,
    },
    SysCall {
        name: Ident,
        args: Vec<Expr>,
    },
    Access {
        nature: Ident,
        args: Vec<Expr>,
        port_flow: bool,
    },
}
