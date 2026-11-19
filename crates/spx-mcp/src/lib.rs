//! Model Context Protocol (MCP) server for spx.
//!
//! The server speaks JSON-RPC 2.0 with one message per line over stdio (the MCP stdio
//! transport). It is a thin shell over [`spx_api::Session`]: every tool returns the session's
//! structured result both as `structuredContent` and as compact JSON text.
//!
//! Provenance: original implementation of the publicly published Model Context Protocol
//! specification (modelcontextprotocol.io, revisions 2024-11-05 to 2025-11-25) and the
//! JSON-RPC 2.0 specification. No SDK code is used. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod tools;

use serde_json::{Value, json};
use spx_api::{Session, SessionOptions};

/// Protocol revisions this server implements, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "spx is a SPICE circuit simulator. Typical loop: validate_netlist \
(fast parse + lint) → simulate (returns a compact summary: operating point, signal statistics, \
.meas results, diagnostics with culprits and suggestions) → get_waveform / measure for details \
→ alter_and_rerun or sweep to explore the design. Use monte_carlo with a path or netlist, count n and seed to sample a statistical deck. Netlists use ngspice syntax (R, C, L, V, I, \
E, F, G, H, D, Q, M level 1; .op .dc .ac .tran .meas .model .options). Values are SI; note \
that `M` means milli, `MEG` means mega. Results are deterministic: the same input gives the \
same result_hash.";

/// JSON-RPC error codes.
mod rpc {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// An MCP server instance (one per client connection).
#[derive(Debug)]
pub struct Server {
    session: Session,
    protocol: &'static str,
}

impl Server {
    /// Create a server with a fresh session.
    pub fn new(opts: SessionOptions) -> Self {
        Self {
            session: Session::new(opts),
            protocol: PROTOCOL_VERSIONS[0],
        }
    }

    /// Handle one incoming line. Returns the response line, if any (notifications and
    /// responses from the client produce none).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let out = match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error(
                Value::Null,
                rpc::PARSE_ERROR,
                &format!("parse error: {e}"),
            )),
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> =
                    batch.into_iter().filter_map(|m| self.handle(m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => self.handle(msg),
        };
        out.map(|v| v.to_string())
    }

    /// Handle one JSON-RPC message.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to a server request (we send none) or garbage.
            return match id {
                Some(id) if msg.get("result").is_none() && msg.get("error").is_none() => {
                    Some(error(id, rpc::INVALID_REQUEST, "missing `method`"))
                }
                _ => None,
            };
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        // Notifications (no id) never get a response.
        let id = id?;
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::list() })),
            "tools/call" => self.call(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            _ => Err((
                rpc::METHOD_NOT_FOUND,
                format!("method `{method}` not found"),
            )),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, m)) => error(id, code, &m),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.protocol = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .copied()
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": self.protocol,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "spx",
                "title": "spx SPICE simulator",
                "version": spx_api::ENGINE_VERSION,
            },
            "instructions": INSTRUCTIONS,
        })
    }

    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((rpc::INVALID_PARAMS, "missing tool `name`".to_string()))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        if !tools::exists(name) {
            return Err((rpc::INVALID_PARAMS, format!("unknown tool `{name}`")));
        }
        let (structured, is_error) = match tools::call(&mut self.session, name, &args) {
            Ok(v) => (v, false),
            Err(e) => (serde_json::to_value(&e).unwrap_or_else(|_| json!({})), true),
        };
        let text = structured.to_string();
        Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": structured,
            "isError": is_error,
        }))
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
