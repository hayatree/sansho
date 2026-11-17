//! Integration coverage of the original analog frontend.
//! Provenance: original tests from Accellera Verilog-AMS LRM 2.4 syntax; see docs/PROVENANCE.md.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use spx_va::{
    lexer::{TokenKind, lex},
    preproc::preprocess,
    *,
};
struct Memory;
impl VaLoader for Memory {
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        assert_eq!(from, Some("<input>"));
        if path == "part.va" {
            Ok((
                "part.va".into(),
                "module included();\n real x;\nendmodule".into(),
            ))
        } else {
            Err("missing".into())
        }
    }
}
#[test]
fn models() {
    for text in [
        include_str!("models/resistor.va"),
        include_str!("models/diode.va"),
        include_str!("models/mos.va"),
    ] {
        let tree = parse_va(text, &NoVaFiles).unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(tree.modules.len(), 1);
        assert_eq!(
            tree.natures
                .iter()
                .filter(|n| n.name.name == "Voltage")
                .count(),
            1
        );
        assert!(
            tree.modules[0]
                .items
                .iter()
                .any(|i| matches!(i.kind, ItemKind::Analog(_)))
        );
    }
    let d = parse_va(include_str!("models/diode.va"), &NoVaFiles).unwrap();
    assert!(
        d.modules[0]
            .items
            .iter()
            .any(|i| matches!(&i.kind,ItemKind::Function(f) if f.name.name=="bounded"))
    );
    assert!(
        d.modules[0].items.iter().any(
            |i| matches!(&i.kind,ItemKind::Param(p) if p.name.name=="mode" && p.ranges.len()==2)
        )
    );
    let m = parse_va(include_str!("models/mos.va"), &NoVaFiles).unwrap();
    let block = m.modules[0]
        .items
        .iter()
        .find_map(|i| {
            if let ItemKind::Analog(s) = &i.kind {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    let StmtKind::Block {
        name,
        declarations,
        statements,
    } = &block.kind
    else {
        panic!()
    };
    assert_eq!(name.as_ref().unwrap().name, "evaluate");
    assert_eq!(declarations.len(), 1);
    assert!(
        statements
            .iter()
            .any(|s| matches!(&s.kind,StmtKind::Case{arms,..} if arms[0].labels.len()==2))
    );
    assert!(
        statements
            .iter()
            .any(|s| matches!(s.kind, StmtKind::For { .. }))
    );
}
#[test]
fn macros_conditionals_and_includes() {
    let text = "`define ADD(a,b) ((a)+\\\n(b))\n`define YES\n`ifdef YES\n`ifndef NO\nmodule m(); analog x=`ADD(1,2); endmodule\n`else\nbad\n`endif\n`elsif NO\nbad\n`endif\n`include \"part.va\"\n";
    let tree = parse_va(text, &Memory).unwrap();
    assert_eq!(tree.modules.len(), 2);
    assert_eq!(tree.modules[1].span.file, 1);
    assert_eq!(tree.modules[1].items[0].span.line, 2);
    let (p, e) = preprocess(
        "`define A 1\n`undef A\n`ifdef A\nbad\n`else\nok\n`endif",
        &NoVaFiles,
    );
    assert!(e.is_empty());
    assert!(p.text.contains("ok"));
    assert!(!p.text.contains("bad"));
}
#[test]
fn lexer_tokens() {
    let (p, e) = preprocess(
        "1T 2G 3M 4K 5k 6m 7u 8n 9p 1f 2a 'd10 'h1F 8'b1010 \\odd.name  $vt <+ ** <<< >>> === !== (* hint=1 *)",
        &NoVaFiles,
    );
    assert!(e.is_empty());
    let (t, e) = lex(&p);
    assert!(e.is_empty());
    assert_eq!(t[2].kind, TokenKind::Real(3e6));
    assert_eq!(t[11].kind, TokenKind::Int(10));
    assert_eq!(t[12].kind, TokenKind::Int(31));
    assert_eq!(t[14].kind, TokenKind::Ident("odd.name".into()));
    assert_eq!(t[16].kind, TokenKind::Symbol("<+".into()));
}
#[test]
fn diagnostics_and_limits() {
    let errors = parse_va(
        "module m(); real ; integer ; analog begin x=; y=; end endmodule",
        &NoVaFiles,
    )
    .unwrap_err();
    assert!(errors.len() >= 4);
    assert!(
        errors
            .iter()
            .all(|e| e.code == "E-VA-SYNTAX" && e.span.is_some())
    );
    for text in ["`define A `A\n`A", "`ifdef M\nx", "`else"] {
        assert!(
            parse_va(text, &NoVaFiles)
                .unwrap_err()
                .iter()
                .any(|e| e.code == "E-VA-PREPROC")
        );
    }
    assert_eq!(
        parse_va("`include \"missing\"", &NoVaFiles).unwrap_err()[0].code,
        "E-VA-INCLUDE"
    );
    assert!(
        parse_va("module m(); always x=1; endmodule", &NoVaFiles)
            .unwrap_err()
            .iter()
            .any(|e| e.code == "E-VA-UNSUPPORTED")
    );
}

#[test]
fn exact_positions_and_events() {
    let text = "`define SCALE(x) (x*2)\nmodule m();\n analog begin\n  x=`SCALE(3); y=4;\n  @(cross(V(p),1) or timer(1n)) ;\n end\nendmodule";
    let tree = parse_va(text, &NoVaFiles).unwrap();
    let ItemKind::Analog(block) = &tree.modules[0].items[0].kind else {
        panic!()
    };
    let StmtKind::Block { statements, .. } = &block.kind else {
        panic!()
    };
    assert_eq!(statements[0].span, spx_diag::Span::new(4, 3));
    assert_eq!(statements[1].span, spx_diag::Span::new(4, 16));
    let StmtKind::Assign { rhs, .. } = &statements[0].kind else {
        panic!()
    };
    assert_eq!(rhs.span, spx_diag::Span::new(4, 5));
    assert!(matches!(&statements[2].kind,StmtKind::Event{events,..} if events.len()==2));
}

#[test]
fn precedence_and_full_declarations() {
    let tree=parse_va("nature Custom : Voltage; units=\"V\"; endnature\ndiscipline custom; potential Custom; domain continuous; enddiscipline\nmodule m(input electrical p, output electrical n);\n real a[0:2][1:3]=0; integer b;\n analog function integer choose; input x; output y; inout z; integer x,y,z; begin choose=x; end endfunction\n analog begin a[0]=1+2*3**2; b=1?2:3?4:5; V(<p>) <+ 0; end endmodule",&NoVaFiles).unwrap();
    assert_eq!(tree.modules[0].ports.len(), 2);
    let ItemKind::Analog(block) = &tree.modules[0].items.last().unwrap().kind else {
        panic!()
    };
    let StmtKind::Block { statements, .. } = &block.kind else {
        panic!()
    };
    let StmtKind::Assign { rhs, .. } = &statements[0].kind else {
        panic!()
    };
    let ExprKind::Binary { op, right, .. } = &rhs.kind else {
        panic!()
    };
    assert_eq!(op, "+");
    assert!(matches!(&right.kind,ExprKind::Binary{op,..} if op=="*"));
}

#[test]
fn include_depth_and_comments() {
    struct Recursive;
    impl VaLoader for Recursive {
        fn load(&self, _: &str, _: Option<&str>) -> Result<(String, String), String> {
            Ok(("again".into(), "`include \"again\"".into()))
        }
    }
    assert!(
        parse_va("`include \"again\"", &Recursive)
            .unwrap_err()
            .iter()
            .any(|d| d.code == "E-VA-INCLUDE")
    );
    let (p, e) = preprocess(
        "/* `undefined\n`include \"absent\"\n*/\nmodule m(); // `undefined\nparameter string s=\"`undefined\"; endmodule",
        &NoVaFiles,
    );
    assert!(e.is_empty());
    assert!(lex(&p).1.is_empty());
    let (p, e) = preprocess("`define A 1\nx=`A;\ny=\\\n2;", &NoVaFiles);
    assert!(e.is_empty());
    let (t, e) = lex(&p);
    assert!(e.is_empty());
    assert_eq!(
        t.iter().find(|t| t.kind == TokenKind::Int(2)).unwrap().span,
        spx_diag::Span::new(4, 1)
    );
}

#[test]
fn repeated_builtin_include_and_string_escapes() {
    let tree=parse_va("`include \"disciplines.vams\"\n`include \"disciplines.vams\"\nmodule m(); parameter string s=\"a\\101\\n\"; endmodule",&NoVaFiles).unwrap();
    assert_eq!(
        tree.natures
            .iter()
            .filter(|n| n.name.name == "Voltage")
            .count(),
        1
    );
    let ItemKind::Param(p) = &tree.modules[0].items[0].kind else {
        panic!()
    };
    assert_eq!(p.value.kind, ExprKind::Str("aA\n".into()));
}

#[test]
fn header_fallback_multiline_invocations_and_predefines() {
    struct Header;
    impl VaLoader for Header {
        fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
            if path == "constants.h" {
                Ok((path.into(), "`define USER_SCALE 4".into()))
            } else {
                Err("absent".into())
            }
        }
    }
    let text = r#"`include "constants.h"
`include "discipline.h"
`include "constants.vams"
`define SUM(a,b) ((a)+(b)) // this comment ends at the definition
`ifdef __VAMS_ENABLE__
`ifdef __VAMS_COMPACT_MODELING__
module headers();
parameter real v=`SUM(
    `USER_SCALE,
    `M_PI_4);
endmodule
`endif
`endif"#;
    let ast = parse_va(text, &Header).unwrap();
    assert!(ast.disciplines.iter().any(|d| d.name.name == "electrical"));
    let m = compile_module(&ast, Some("headers")).unwrap();
    let b = m.bind(&[]).unwrap();
    assert_eq!(m.param(&b, "V"), Some(4. + std::f64::consts::FRAC_PI_4));
}
