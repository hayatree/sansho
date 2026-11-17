//! Structured diagnostics shared by all spx crates.
//!
//! Every error or warning carries a stable machine-readable `code`, an optional source span and
//! free-form notes, so that both humans and agents can act on it.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

use serde::Serialize;

/// A position in a netlist source (1-based line and column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
}

impl Span {
    /// Create a span.
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The input cannot be processed.
    Error,
    /// The input was processed but may not mean what the user intended.
    Warning,
}

/// A structured error or warning.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Stable identifier, e.g. `E-PARSE-001`.
    pub code: &'static str,
    /// Severity.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Source location, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// Additional notes and suggestions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Create an error.
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span: None,
            notes: Vec::new(),
        }
    }

    /// Create a warning.
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    /// Attach a source span.
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Attach a note.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.span {
            Some(s) => write!(f, "{sev}[{}] at {s}: {}", self.code, self.message)?,
            None => write!(f, "{sev}[{}]: {}", self.code, self.message)?,
        }
        for n in &self.notes {
            write!(f, "\n  note: {n}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}
