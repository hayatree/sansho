//! `sansho-mcp`: MCP server over stdio.
//!
//! ```text
//! sansho-mcp [--threads N]
//! ```
//!
//! Register with an MCP client, e.g. `claude mcp add sansho -- /path/to/sansho-mcp`.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::process::ExitCode;

use sansho_api::SessionOptions;
use sansho_mcp::Server;

fn main() -> ExitCode {
    let mut opts = SessionOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" | "-j" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => opts.threads = n,
                None => {
                    eprintln!("sansho-mcp: --threads needs a positive integer");
                    return ExitCode::from(2);
                }
            },
            "--version" | "-V" => {
                println!("sansho-mcp {}", sansho_api::ENGINE_VERSION);
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!(
                    "usage: sansho-mcp [--threads N]\nMCP server over stdio (JSON-RPC, one message per line)."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("sansho-mcp: unknown argument `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    let mut server = Server::new(opts);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if let Some(reply) = server.handle_line(&line)
            && (writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err())
        {
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
