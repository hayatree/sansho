//! Clean-room Verilog-A compact-model syntax frontend.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
#![forbid(unsafe_code)]
pub mod ast;
pub mod lexer;
mod parser;
pub mod preproc;
pub use ast::*;
use spx_diag::Diagnostic;

/// Supplies include text and its canonical source name. No filesystem access is implicit.
pub trait VaLoader {
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String>;
}
#[derive(Debug)]
pub struct NoVaFiles;
impl VaLoader for NoVaFiles {
    fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
        Err(format!("file access disabled: {path}"))
    }
}
/// Parse analog syntax; semantic validation and evaluation belong to later phases.
pub fn parse_va(text: &str, loader: &dyn VaLoader) -> Result<SourceFile, Vec<Diagnostic>> {
    let (source, mut diagnostics) = preproc::preprocess(text, loader);
    let (tokens, errors) = lexer::lex(&source);
    diagnostics.extend(errors);
    let (tree, errors) = parser::parse(tokens, source.sources);
    diagnostics.extend(errors);
    diagnostics.sort_by_key(|d| !d.is_error());
    if diagnostics.iter().any(Diagnostic::is_error) {
        Err(diagnostics)
    } else {
        Ok(tree)
    }
}
