//! Clean-room Verilog-A compact-model compiler and AD evaluator.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
#![cfg_attr(not(feature = "jit"), forbid(unsafe_code))]
#![deny(unsafe_code)]
#![warn(missing_docs)]
pub mod ast;
#[cfg(feature = "jit")]
mod cache;
#[cfg(feature = "jit")]
pub use cache::compile_source_cached;
mod dual;
mod eval;
mod ir;
pub mod lexer;
mod limit;
mod math;
mod parser;
mod sema;
#[cfg(feature = "jit")]
pub use eval::CircuitPair;
pub use eval::{
    Analysis, BindOptions, Bound, EvalInput, EvalOutput, EvalStamp, Layout, NoiseDrive, Scratch,
    StateIo,
};
pub use ir::{CompiledModule, ParamInfo};
pub use sema::compile_module;
pub mod preproc;
pub use ast::*;
use spx_diag::Diagnostic;

/// Supplies include text and its canonical source name. No filesystem access is implicit.
pub trait VaLoader {
    /// Load include text and its canonical name, relative to an optional source.
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String>;
    /// Look for a local standard header before using its built-in definition.
    /// Filesystem loaders can distinguish absence from a failure reading an existing file.
    fn standard_header(
        &self,
        path: &str,
        from: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        Ok(self.load(path, from).ok())
    }
}
/// Include loader that rejects external file access.
#[derive(Debug)]
pub struct NoVaFiles;
impl VaLoader for NoVaFiles {
    fn load(&self, path: &str, _: Option<&str>) -> Result<(String, String), String> {
        Err(format!("file access disabled: {path}"))
    }
}
/// Parse analog syntax; call `compile_module` for semantic validation and lowering.
pub fn parse_va(text: &str, loader: &dyn VaLoader) -> Result<SourceFile, Vec<Diagnostic>> {
    let pre = spx_diag::Phase::start("va_pre");
    let (source, mut diagnostics) = preproc::preprocess(text, loader);
    drop(pre);
    let _phase = spx_diag::Phase::start("va_parse");
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

/// Whether the process environment permits native Verilog-A evaluation.
#[cfg(feature = "jit")]
pub fn jit_enabled() -> bool {
    std::env::var_os("SPX_VA_JIT").is_none_or(|value| value != "0")
}

/// A semantically checked translation unit in source order.
#[derive(Debug)]
pub struct CompiledSource {
    /// File names for translating diagnostic spans.
    pub sources: Vec<String>,
    /// All module declarations, including validated types unused by the circuit.
    pub declarations: Vec<(spx_diag::Span, String)>,
    /// Compiled modules selected for binding.
    pub modules: Vec<CompiledModule>,
}

/// Compile every module, preserving source names for diagnostics.
pub fn compile_source(
    text: &str,
    loader: &dyn VaLoader,
) -> Result<CompiledSource, (Vec<Diagnostic>, Vec<String>)> {
    let ast = parse_va(text, loader).map_err(|ds| (ds, Vec::new()))?;
    let mut errors = Vec::new();
    let mut modules = Vec::new();
    for module in &ast.modules {
        match compile_module(&ast, Some(&module.name.name)) {
            Ok(compiled) => modules.push(compiled),
            Err(ds) => errors.extend(ds),
        }
    }
    if errors.is_empty() {
        Ok(CompiledSource {
            declarations: ast
                .modules
                .into_iter()
                .map(|m| (m.span, m.name.name))
                .collect(),
            sources: ast.sources,
            modules,
        })
    } else {
        Err((errors, ast.sources))
    }
}
