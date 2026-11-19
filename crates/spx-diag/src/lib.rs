//! Structured diagnostics shared by all spx crates.
//!
//! Every error or warning carries a stable machine-readable `code`, an optional source span and
//! free-form notes, so that both humans and agents can act on it.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod codes;

pub use codes::{CODES, CodeInfo, lookup};

use serde::Serialize;

/// A position in a netlist source (1-based line and column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct Span {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// Source file index: 0 is the main netlist, others index the netlist's `sources`
    /// (`.include` / `.lib` files).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub file: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl Span {
    /// Create a span in the main netlist.
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col, file: 0 }
    }

    /// The same position in source file `file`.
    pub fn in_file(self, file: u32) -> Self {
        Self { file, ..self }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file == 0 {
            write!(f, "{}:{}", self.line, self.col)
        } else {
            write!(f, "file#{}:{}:{}", self.file, self.line, self.col)
        }
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational status for a successful operation.
    Info,
    /// The input cannot be processed.
    Error,
    /// The input was processed but may not mean what the user intended.
    Warning,
}

/// What a [`Culprit`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CulpritKind {
    /// A circuit node (its voltage).
    Node,
    /// A branch current unknown (voltage source, inductor, ...).
    Branch,
    /// A device instance.
    Device,
    /// A `.model` card.
    Model,
}

/// A circuit object implicated by a diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Culprit {
    /// Object kind.
    pub kind: CulpritKind,
    /// Object name, e.g. `out`, `v(out)`, `m5`.
    pub name: String,
    /// Why this object is implicated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Last value of the unknown, if applicable (V or A).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Last Newton update relative to the convergence tolerance (> 1 means not converged).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_ratio: Option<f64>,
}

impl Culprit {
    /// A culprit with a reason.
    pub fn new(kind: CulpritKind, name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            reason: Some(reason.into()),
            value: None,
            update_ratio: None,
        }
    }
}

/// Confidence of a [`Suggestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Likely to fix the problem.
    High,
    /// Often helps.
    Medium,
    /// Worth trying.
    Low,
}

/// A concrete remedy.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    /// What to do, e.g. "add `.options rshunt=1e12`".
    pub action: String,
    /// How likely it is to help.
    pub confidence: Confidence,
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
    /// Additional notes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Circuit objects implicated, most relevant first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub culprits: Vec<Culprit>,
    /// Strategies that were tried before giving up, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<String>,
    /// Suggested remedies, most promising first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
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
            culprits: Vec::new(),
            attempts: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Create an informational diagnostic.
    pub fn information(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            ..Self::error(code, message)
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

    /// Attach a culprit.
    pub fn with_culprit(mut self, c: Culprit) -> Self {
        self.culprits.push(c);
        self
    }

    /// Record the strategies that were tried.
    pub fn with_attempts<S: Into<String>>(mut self, a: impl IntoIterator<Item = S>) -> Self {
        self.attempts.extend(a.into_iter().map(Into::into));
        self
    }

    /// Attach a suggested remedy.
    pub fn with_suggestion(mut self, action: impl Into<String>, confidence: Confidence) -> Self {
        self.suggestions.push(Suggestion {
            action: action.into(),
            confidence,
        });
        self
    }

    /// Documentation of this diagnostic's code, if registered.
    pub fn info(&self) -> Option<&'static CodeInfo> {
        lookup(self.code)
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Info => "info",
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
        for c in &self.culprits {
            write!(f, "\n  culprit: {} `{}`", kind_name(c.kind), c.name)?;
            if let Some(v) = c.value {
                write!(f, " = {v:.6e}")?;
            }
            if let Some(r) = &c.reason {
                write!(f, " ({r})")?;
            }
        }
        if !self.attempts.is_empty() {
            write!(f, "\n  tried: {}", self.attempts.join(" → "))?;
        }
        for s in &self.suggestions {
            write!(f, "\n  help: {}", s.action)?;
        }
        Ok(())
    }
}

fn kind_name(k: CulpritKind) -> &'static str {
    match k {
        CulpritKind::Node => "node",
        CulpritKind::Branch => "branch",
        CulpritKind::Device => "device",
        CulpritKind::Model => "model",
    }
}

impl std::error::Error for Diagnostic {}

/// Enable the detailed Newton/device counters separately from phase benchmarks.
pub fn profile_devices() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SPX_PROFILE_DEVICES").is_some())
}

/// Enable aggregate phase timers for a profiled process.
pub fn profile_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SPX_PROFILE").as_deref() == Ok("1"))
}

/// Optional wall-clock probe for startup and analysis benchmarks.
#[derive(Debug)]
pub struct Phase(&'static str, Option<std::time::Instant>);

impl Phase {
    /// Emit a machine-readable elapsed time on drop when SPX_PROFILE is set.
    pub fn start(name: &'static str) -> Self {
        Self(name, profile_enabled().then(std::time::Instant::now))
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "spx-phase {} {:.6}",
                self.0,
                start.elapsed().as_secs_f64() * 1000.
            );
        }
    }
}
