//! Every diagnostic code used in the workspace is registered, and `docs/diagnostics.md` is
//! up to date with the registry (regenerate with `SANSHO_BLESS=1 cargo test -p sansho-diag`).
//!
//! Provenance: original work.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use sansho_diag::{CODES, Severity, lookup};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Code literals such as `"E-PARSE-ARITY"` in a source text.
fn code_literals(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len().saturating_sub(3) {
        if b[i] == b'"' && matches!(b[i + 1], b'E' | b'W' | b'I') && b[i + 2] == b'-' {
            let rest = &text[i + 1..];
            let end = rest.find('"').unwrap_or(0);
            let lit = &rest[..end];
            if lit.len() > 4
                && lit
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
            {
                out.push(lit.to_string());
            }
        }
    }
    out
}

#[test]
fn every_used_code_is_registered() {
    let mut files = Vec::new();
    rust_files(&root().join("crates"), &mut files);
    let mut missing = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for c in code_literals(&text) {
            if lookup(&c).is_none() {
                missing.push(format!("{c} ({})", f.display()));
            }
        }
    }
    assert!(missing.is_empty(), "unregistered codes: {missing:#?}");
}

fn render() -> String {
    let mut s = String::from(
        "# Diagnostic codes\n\n\
         <!-- Generated from crates/sansho-diag/src/codes.rs; do not edit by hand.\n     \
         Regenerate with `SANSHO_BLESS=1 cargo test -p sansho-diag`. -->\n\n\
         Every diagnostic carries a stable `code`. Codes starting with `E-` are errors (the input \
         cannot be simulated); `W-` codes are warnings and `I-` codes are informational. Diagnostics may also carry `culprits` \
         (implicated nodes, branches and devices), `attempts` (strategies that were tried) and \
         `suggestions` (specific remedies).\n\n\
         | Code | Severity | Summary |\n|---|---|---|\n",
    );
    for c in CODES {
        let sev = match c.severity {
            Severity::Info => "info",
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        s += &format!(
            "| [`{0}`](#{1}) | {sev} | {2} |\n",
            c.code,
            c.code.to_lowercase(),
            c.title
        );
    }
    for c in CODES {
        s += &format!("\n## {}\n\n**{}.** {}\n\n", c.code, c.title, c.explanation);
        for r in c.remedies {
            s += &format!("- {r}\n");
        }
    }
    s
}

#[test]
fn docs_are_up_to_date() {
    let path = root().join("docs/diagnostics.md");
    let want = render();
    if std::env::var_os("SANSHO_BLESS").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "docs/diagnostics.md is stale; run `SANSHO_BLESS=1 cargo test -p sansho-diag`"
    );
}
