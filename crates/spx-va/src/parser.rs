//! Recursive descent and precedence-climbing analog parser.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4 and general compiler techniques; see docs/PROVENANCE.md.
use crate::{
    ast::*,
    lexer::{Token, TokenKind},
};
use spx_diag::{Diagnostic, Span};
type PResult<T> = Result<T, Diagnostic>;
struct Parser {
    access: std::collections::BTreeSet<String>,
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
    depth: usize,
}
pub fn parse(tokens: Vec<Token>, sources: Vec<String>) -> (SourceFile, Vec<Diagnostic>) {
    let mut p = Parser {
        access: ["V", "I", "Temp", "Pwr"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        tokens,
        pos: 0,
        errors: Vec::new(),
        depth: 0,
    };
    let mut source = SourceFile {
        sources,
        modules: Vec::new(),
        natures: Vec::new(),
        disciplines: Vec::new(),
        span: Span::new(1, 1),
    };
    while !p.eof() {
        let before = p.pos;
        let result = if p.at("module") {
            p.module().map(|m| source.modules.push(m))
        } else if p.at("nature") || p.at("discipline") {
            let nature = p.at("nature");
            p.definition().map(|d| {
                if nature {
                    for (property, value) in &d.properties {
                        if property.name == "access"
                            && let ExprKind::Ident(access) = &value.kind
                        {
                            p.access.insert(access.name.clone());
                        }
                    }
                    source.natures.push(d);
                } else {
                    source.disciplines.push(d);
                }
            })
        } else {
            Err(p.unsupported("expected module, nature or discipline"))
        };
        if let Err(e) = result {
            p.errors.push(e);
            p.recover();
        }
        if p.pos == before {
            p.bump();
        }
    }
    (source, p.errors)
}
impl Parser {
    fn token(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }
    fn span(&self) -> Span {
        self.token().span
    }
    fn eof(&self) -> bool {
        matches!(self.token().kind, TokenKind::Eof)
    }
    fn at(&self, s: &str) -> bool {
        match &self.token().kind {
            TokenKind::Ident(v) | TokenKind::System(v) | TokenKind::Symbol(v) => v == s,
            _ => false,
        }
    }
    fn bump(&mut self) -> Token {
        let t = self.token().clone();
        if !self.eof() {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.at(s) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn error(&self, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error("E-VA-SYNTAX", msg).with_span(self.span())
    }
    fn unsupported(&self, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error("E-VA-UNSUPPORTED", msg).with_span(self.span())
    }
    fn need(&mut self, s: &str) -> PResult<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.error(format!("expected `{s}`")))
        }
    }
    fn ident(&mut self) -> PResult<Ident> {
        let t = self.token().clone();
        if let TokenKind::Ident(name) = t.kind {
            self.bump();
            Ok(Ident { name, span: t.span })
        } else {
            Err(Diagnostic::error("E-VA-SYNTAX", "expected identifier").with_span(t.span))
        }
    }
    fn names(&mut self) -> PResult<Vec<Ident>> {
        let mut v = vec![self.ident()?];
        while self.eat(",") {
            v.push(self.ident()?);
        }
        Ok(v)
    }
    fn recover(&mut self) {
        while !self.eof()
            && ![";", "end", "endmodule", "endfunction", "endcase", "module"]
                .iter()
                .any(|s| self.at(s))
        {
            self.bump();
        }
        self.eat(";");
    }
    fn definition(&mut self) -> PResult<Definition> {
        let span = self.span();
        let nature = self.eat("nature");
        if !nature {
            self.need("discipline")?;
        }
        let name = self.ident()?;
        let parent = if self.eat(":") {
            Some(self.ident()?)
        } else {
            None
        };
        self.eat(";");
        let mut properties = Vec::new();
        let end = if nature { "endnature" } else { "enddiscipline" };
        while !self.at(end) && !self.eof() {
            let key = self.ident()?;
            self.eat("=");
            let value = self.expr(0)?;
            self.need(";")?;
            properties.push((key, value));
        }
        self.need(end)?;
        Ok(Definition {
            name,
            parent,
            properties,
            span,
        })
    }
    fn module(&mut self) -> PResult<Module> {
        let span = self.span();
        self.need("module")?;
        let name = self.ident()?;
        let mut ports = Vec::new();
        let mut items = Vec::new();
        if self.eat("(") {
            let mut direction: Option<String> = None;
            let mut discipline = None;
            while !self.at(")") && !self.eof() {
                if ["input", "output", "inout"].iter().any(|s| self.at(s)) {
                    direction = Some(self.ident()?.name);
                    discipline = None;
                }
                let first = self.ident()?;
                let port = if matches!(self.token().kind, TokenKind::Ident(_)) {
                    discipline = Some(first);
                    self.ident()?
                } else {
                    first
                };
                ports.push(port.clone());
                if let Some(d) = &direction {
                    items.push(ModuleItem {
                        span: port.span,
                        kind: ItemKind::PortDecl {
                            direction: d.clone(),
                            discipline: discipline.clone(),
                            names: vec![port],
                        },
                    });
                }
                if !self.eat(",") {
                    break;
                }
            }
            self.need(")")?;
        }
        self.need(";")?;
        while !self.eof() && !self.at("endmodule") {
            let before = self.pos;
            match self.items() {
                Ok(v) => items.extend(v),
                Err(e) => {
                    self.errors.push(e);
                    self.recover();
                }
            }
            if before == self.pos {
                self.bump();
            }
        }
        self.need("endmodule")?;
        Ok(Module {
            name,
            ports,
            items,
            span,
        })
    }
    fn items(&mut self) -> PResult<Vec<ModuleItem>> {
        let span = self.span();
        let kind = if self.eat("analog") {
            if self.eat("function") {
                ItemKind::Function(self.function(span)?)
            } else {
                ItemKind::Analog(self.stmt()?)
            }
        } else if self.at("parameter") || self.at("localparam") {
            return self.parameters();
        } else if ["real", "integer", "genvar"].iter().any(|s| self.at(s)) {
            return self.variables();
        } else if self.eat("aliasparam") {
            let name = self.ident()?;
            self.need("=")?;
            let target = self.ident()?;
            self.need(";")?;
            ItemKind::AliasParam { name, target }
        } else if self.eat("branch") {
            self.need("(")?;
            let nodes = if self.eat("<") {
                let port = self.ident()?;
                self.need(">")?;
                self.need(")")?;
                vec![Expr {
                    span: port.span,
                    kind: ExprKind::Port(port),
                }]
            } else {
                self.arguments(")")?
            };
            let names = self.names()?;
            self.need(";")?;
            ItemKind::Branch { nodes, names }
        } else if self.eat("ground") {
            let names = self.names()?;
            self.need(";")?;
            ItemKind::Ground(names)
        } else if ["input", "output", "inout"].iter().any(|s| self.at(s)) {
            let direction = self.ident()?.name;
            let first = self.ident()?;
            let (discipline, mut names) = if matches!(self.token().kind, TokenKind::Ident(_)) {
                (Some(first), vec![self.ident()?])
            } else {
                (None, vec![first])
            };
            while self.eat(",") {
                names.push(self.ident()?);
            }
            self.need(";")?;
            ItemKind::PortDecl {
                direction,
                discipline,
                names,
            }
        } else {
            if [
                "always", "initial", "assign", "generate", "wire", "reg", "digital",
            ]
            .iter()
            .any(|s| self.at(s))
            {
                return Err(self.unsupported("unsupported module construct"));
            }
            let discipline = self.ident()?;
            let names = self.names()?;
            self.need(";")?;
            ItemKind::NetDecl { discipline, names }
        };
        Ok(vec![ModuleItem { kind, span }])
    }
    fn parameters(&mut self) -> PResult<Vec<ModuleItem>> {
        let span = self.span();
        let mut placement = self.token().placement;
        let local = self.eat("localparam");
        if !local {
            self.need("parameter")?;
        }
        placement = self.token().placement.or(placement);
        let ty = if ["real", "integer", "string"].iter().any(|s| self.at(s)) {
            self.ident()?.name
        } else {
            "real".into()
        };
        let mut items = Vec::new();
        loop {
            let name_placement = self.token().placement;
            let name = self.ident()?;
            self.need("=")?;
            let value = self.expr(0)?;
            let mut ranges = Vec::new();
            while self.at("from") || self.at("exclude") {
                let rs = self.span();
                let exclude = self.eat("exclude");
                if !exclude {
                    self.need("from")?;
                }
                let bracket = self.at("[") || self.at("(");
                let lower_closed = self.eat("[");
                if bracket && !lower_closed {
                    self.need("(")?;
                }
                let lower = self.expr(0)?;
                let upper = if bracket {
                    self.need(":")?;
                    Some(self.expr(0)?)
                } else {
                    None
                };
                let upper_closed = if bracket {
                    if self.eat("]") {
                        true
                    } else {
                        self.need(")")?;
                        false
                    }
                } else {
                    false
                };
                ranges.push(Range {
                    exclude,
                    lower,
                    upper,
                    lower_closed,
                    upper_closed,
                    span: rs,
                });
            }
            items.push(ModuleItem {
                span,
                kind: ItemKind::Param(ParamDecl {
                    name,
                    ty: ty.clone(),
                    placement: name_placement.or(placement),
                    local,
                    value,
                    ranges,
                    span,
                }),
            });
            if !self.eat(",") {
                break;
            }
        }
        self.need(";")?;
        Ok(items)
    }
    fn variables(&mut self) -> PResult<Vec<ModuleItem>> {
        let span = self.span();
        let ty = self.ident()?.name;
        let mut items = Vec::new();
        loop {
            let name = self.ident()?;
            let mut dimensions = Vec::new();
            while self.eat("[") {
                let a = self.expr(0)?;
                self.need(":")?;
                let b = self.expr(0)?;
                self.need("]")?;
                dimensions.push((a, b));
            }
            let value = if self.eat("=") {
                Some(self.expr(0)?)
            } else {
                None
            };
            items.push(ModuleItem {
                span,
                kind: ItemKind::Var(VarDecl {
                    ty: ty.clone(),
                    name,
                    dimensions,
                    value,
                    span,
                }),
            });
            if !self.eat(",") {
                break;
            }
        }
        self.need(";")?;
        Ok(items)
    }
    fn function(&mut self, span: Span) -> PResult<AnalogFunction> {
        let ty = if self.at("real") || self.at("integer") {
            self.ident()?.name
        } else {
            "real".into()
        };
        let name = self.ident()?;
        self.need(";")?;
        let mut declarations = Vec::new();
        let mut body = Vec::new();
        while !self.at("endfunction") && !self.eof() {
            if ["input", "output", "inout", "real", "integer"]
                .iter()
                .any(|s| self.at(s))
            {
                declarations.extend(self.items()?);
            } else {
                body.push(self.stmt()?);
            }
        }
        self.need("endfunction")?;
        Ok(AnalogFunction {
            name,
            ty,
            declarations,
            body,
            span,
        })
    }
    fn stmt(&mut self) -> PResult<Stmt> {
        if self.depth >= 128 {
            return Err(self.unsupported("syntax nesting exceeds 128"));
        }
        self.depth += 1;
        let result = self.statement();
        self.depth -= 1;
        result
    }
    fn statement(&mut self) -> PResult<Stmt> {
        let span = self.span();
        let kind = if self.eat(";") {
            StmtKind::Empty
        } else if self.eat("begin") {
            let name = if self.eat(":") {
                Some(self.ident()?)
            } else {
                None
            };
            let mut declarations = Vec::new();
            let mut statements = Vec::new();
            while !self.at("end") && !self.at("endmodule") && !self.eof() {
                let before = self.pos;
                let result = if ["real", "integer", "genvar", "parameter", "localparam"]
                    .iter()
                    .any(|s| self.at(s))
                {
                    self.items().map(|v| declarations.extend(v))
                } else {
                    self.stmt().map(|s| statements.push(s))
                };
                if let Err(e) = result {
                    self.errors.push(e);
                    self.recover();
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.need("end")?;
            if self.eat(":") {
                self.ident()?;
            }
            StmtKind::Block {
                name,
                declarations,
                statements,
            }
        } else if self.eat("if") {
            let condition = self.paren()?;
            let then_stmt = Box::new(self.stmt()?);
            let else_stmt = if self.eat("else") {
                Some(Box::new(self.stmt()?))
            } else {
                None
            };
            StmtKind::If {
                condition,
                then_stmt,
                else_stmt,
            }
        } else if self.eat("case") {
            let selector = self.paren()?;
            let mut arms = Vec::new();
            while !self.at("endcase") && !self.eof() {
                let span = self.span();
                let mut labels = Vec::new();
                if self.eat("default") {
                    self.eat(":");
                } else {
                    labels.push(self.expr(0)?);
                    while self.eat(",") {
                        labels.push(self.expr(0)?);
                    }
                    self.need(":")?;
                }
                let body = self.stmt()?;
                arms.push(CaseArm { labels, body, span });
            }
            self.need("endcase")?;
            StmtKind::Case { selector, arms }
        } else if self.eat("for") {
            self.need("(")?;
            let init = Box::new(self.simple(false)?);
            self.need(";")?;
            let condition = self.expr(0)?;
            self.need(";")?;
            let step = Box::new(self.simple(false)?);
            self.need(")")?;
            let body = Box::new(self.stmt()?);
            StmtKind::For {
                init,
                condition,
                step,
                body,
            }
        } else if self.eat("while") {
            let condition = self.paren()?;
            let body = Box::new(self.stmt()?);
            StmtKind::While { condition, body }
        } else if self.eat("repeat") {
            let count = self.paren()?;
            let body = Box::new(self.stmt()?);
            StmtKind::Repeat { count, body }
        } else if self.eat("@") {
            self.need("(")?;
            let mut events = vec![self.expr(0)?];
            while self.eat("or") || self.eat(",") {
                events.push(self.expr(0)?);
            }
            self.need(")")?;
            let body = Box::new(self.stmt()?);
            StmtKind::Event { events, body }
        } else {
            return self.simple(true);
        };
        Ok(Stmt { kind, span })
    }
    fn simple(&mut self, semi: bool) -> PResult<Stmt> {
        let span = self.span();
        let lhs = self.expr(0)?;
        let kind = if self.eat("=") {
            let rhs = self.expr(0)?;
            StmtKind::Assign { lhs, rhs }
        } else if self.eat("<+") {
            let rhs = self.expr(0)?;
            StmtKind::Contrib { access: lhs, rhs }
        } else {
            match lhs.kind {
                ExprKind::SysCall { name, args } => StmtKind::SysTask { name, args },
                _ => return Err(self.error("expected assignment, contribution or system task")),
            }
        };
        if semi {
            self.need(";")?;
        }
        Ok(Stmt { kind, span })
    }
    fn paren(&mut self) -> PResult<Expr> {
        self.need("(")?;
        let e = self.expr(0)?;
        self.need(")")?;
        Ok(e)
    }
    fn arguments(&mut self, end: &str) -> PResult<Vec<Expr>> {
        let mut v = Vec::new();
        if !self.at(end) {
            loop {
                v.push(self.expr(0)?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.need(end)?;
        Ok(v)
    }
    fn expr(&mut self, min: u8) -> PResult<Expr> {
        if self.depth >= 128 {
            return Err(self.unsupported("expression nesting exceeds 128"));
        }
        self.depth += 1;
        let result = self.expression(min);
        self.depth -= 1;
        result
    }
    fn expression(&mut self, min: u8) -> PResult<Expr> {
        let span = self.span();
        let t = self.token().clone();
        if matches!(&t.kind, TokenKind::Eof)
            || [
                ";",
                "end",
                "endmodule",
                "endfunction",
                "endcase",
                ")",
                "]",
                ",",
                ":",
            ]
            .iter()
            .any(|v| self.at(v))
        {
            return Err(self.error("expected expression"));
        }
        self.bump();
        let mut left = match t.kind {
            TokenKind::Int(v) => Expr {
                span,
                kind: ExprKind::Int(v),
            },
            TokenKind::Real(v) => Expr {
                span,
                kind: ExprKind::Real(v),
            },
            TokenKind::Str(v) => Expr {
                span,
                kind: ExprKind::Str(v),
            },
            TokenKind::Ident(name) | TokenKind::System(name) => {
                let system = name.starts_with('$');
                let id = Ident { name, span };
                let called = self.eat("(");
                let mut port_flow = false;
                let args = if called {
                    if self.access.contains(&id.name) && self.eat("<") {
                        port_flow = true;
                        let net = self.ident()?;
                        self.need(">")?;
                        self.need(")")?;
                        vec![Expr {
                            span: net.span,
                            kind: ExprKind::Ident(net),
                        }]
                    } else {
                        self.arguments(")")?
                    }
                } else {
                    Vec::new()
                };
                let kind = if system {
                    ExprKind::SysCall { name: id, args }
                } else if called && self.access.contains(&id.name) {
                    ExprKind::Access {
                        nature: id,
                        args,
                        port_flow,
                    }
                } else if called {
                    ExprKind::Call { name: id, args }
                } else {
                    ExprKind::Ident(id)
                };
                Expr { span, kind }
            }
            TokenKind::Symbol(op) if op == "{" => Expr {
                span,
                kind: ExprKind::List(self.arguments("}")?),
            },
            TokenKind::Symbol(op) if op == "(" => {
                let e = self.expr(0)?;
                self.need(")")?;
                e
            }
            TokenKind::Symbol(op)
                if ["+", "-", "!", "~", "&", "~&", "|", "~|", "^", "~^", "^~"]
                    .contains(&op.as_str()) =>
            {
                let expr = Box::new(self.expr(12)?);
                Expr {
                    span,
                    kind: ExprKind::Unary { op, expr },
                }
            }
            _ => {
                return Err(Diagnostic::error("E-VA-SYNTAX", "expected expression").with_span(span));
            }
        };
        // Limit the accumulated left subtree as well as recursive right operands.
        let mut links = 0;
        loop {
            if links >= 128 {
                return Err(self.unsupported("expression nesting exceeds 128"));
            }
            links += 1;
            if self.eat("[") {
                let index = self.expr(0)?;
                self.need("]")?;
                left = Expr {
                    span,
                    kind: ExprKind::Index(Box::new(left), Box::new(index)),
                };
                continue;
            }
            if min == 0 && self.eat("?") {
                let yes = Box::new(self.expr(0)?);
                self.need(":")?;
                let no = Box::new(self.expr(0)?);
                left = Expr {
                    span,
                    kind: ExprKind::Ternary {
                        condition: Box::new(left),
                        yes,
                        no,
                    },
                };
                continue;
            }
            let op = match &self.token().kind {
                TokenKind::Symbol(s) => s.clone(),
                _ => break,
            };
            let prec = match op.as_str() {
                "||" => 1,
                "&&" => 2,
                "|" => 3,
                "^" | "~^" | "^~" => 4,
                "&" => 5,
                "==" | "!=" | "===" | "!==" => 6,
                "<" | ">" | "<=" | ">=" => 7,
                "<<" | ">>" | "<<<" | ">>>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                "**" => 11,
                _ => break,
            };
            if prec < min {
                break;
            }
            self.bump();
            let right = self.expr(prec + 1)?;
            left = Expr {
                span,
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }
}
