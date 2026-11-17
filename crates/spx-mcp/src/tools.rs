//! MCP tool definitions and their mapping onto the session API.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use serde::Serialize;
use serde_json::{Value, json};
use spx_api::{ApiError, Change, Reduce, Session};
use spx_diag::Diagnostic;

/// Default and maximum point budget of `get_waveform`.
const DEFAULT_POINTS: usize = 200;
const MAX_POINTS: usize = 10_000;
/// Maximum number of values of one `sweep`.
const MAX_SWEEP: usize = 1000;

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    read_only: bool,
    schema: fn() -> Value,
}

const NETLIST: &str = "SPICE netlist text (ngspice syntax; the first line is the title)";
const PATH: &str = "path of a netlist file on the server's machine (use instead of `netlist` \
                    when the deck uses .include / .lib)";

const TOOLS: &[Tool] = &[
    Tool {
        name: "validate_netlist",
        title: "Validate netlist",
        description: "Parse, elaborate and lint a netlist without simulating it. Returns ok, \
                      circuit statistics (nodes, unknowns, devices, signal names) and all \
                      diagnostics (errors and warnings such as unit mistakes, floating nodes, \
                      unknown .meas signals). Fast; use before simulate.",
        read_only: true,
        schema: || {
            json!({"type":"object","properties":{"netlist":{"type":"string","description":NETLIST},
                   "path":{"type":"string","description":PATH}}})
        },
    },
    Tool {
        name: "load_netlist",
        title: "Load netlist",
        description: "Load a netlist into the session and return a circuit_id for simulate, \
                      alter_and_rerun, sweep and export_deck.",
        read_only: false,
        schema: || {
            json!({"type":"object","properties":{"netlist":{"type":"string","description":NETLIST},
                   "path":{"type":"string","description":PATH}}})
        },
    },
    Tool {
        name: "estimate_cost",
        title: "Estimate cost",
        description: "Order-of-magnitude estimate of points, result memory and run time of a \
                      loaded circuit's analyses.",
        read_only: true,
        schema: || {
            json!({"type":"object","properties":{"circuit_id":{"type":"string"}},
                   "required":["circuit_id"]})
        },
    },
    Tool {
        name: "simulate",
        title: "Simulate",
        description: "Run the analyses of a circuit (given by circuit_id, or by netlist text, \
                      which is loaded first). `analyses` optionally replaces the netlist's \
                      analysis cards, e.g. [\"tran 1n 1u\", \"ac dec 20 1 1g\"]. Returns a \
                      compact summary: job_id, status, operating-point values, per-signal \
                      min/max/first/last, .meas results with units, diagnostics, input and \
                      result hashes. Fetch waveforms with get_waveform.",
        read_only: false,
        schema: || {
            json!({"type":"object","properties":{
                "circuit_id":{"type":"string"},
                "netlist":{"type":"string","description":NETLIST},
                "path":{"type":"string","description":PATH},
                "analyses":{"type":"array","items":{"type":"string"},
                            "description":"analysis cards replacing those in the netlist"}}})
        },
    },
    Tool {
        name: "get_waveform",
        title: "Get waveform",
        description: "Fetch one signal of a job as x/y arrays, downsampled (LTTB, keeps peaks \
                      and edges) to at most max_points points. Signals: v(node), v(a,b), \
                      i(vsource). AC results default to dB; reduce can also be mag, phase \
                      (degrees), re or im.",
        read_only: true,
        schema: || {
            json!({"type":"object","properties":{
                "job_id":{"type":"string"},
                "signal":{"type":"string","description":"e.g. v(out), v(a,b), i(v1)"},
                "analysis":{"type":"integer","minimum":0,
                            "description":"analysis index in the job (default: last with the signal)"},
                "reduce":{"type":"string","enum":["auto","real","mag","db","phase","re","im"],
                          "description":"auto (default): real value, or dB for AC results"},
                "from":{"type":"number","description":"start of the sweep range"},
                "to":{"type":"number","description":"end of the sweep range"},
                "max_points":{"type":"integer","minimum":3,"maximum":MAX_POINTS,
                              "default":DEFAULT_POINTS}},
                   "required":["job_id","signal"]})
        },
    },
    Tool {
        name: "measure",
        title: "Measure",
        description: "Evaluate .meas-style measurements on an existing job without rerunning, \
                      e.g. \"tran tr trig v(out) val=0.1 rise=1 targ v(out) val=0.9 rise=1\", \
                      \"ac bw when vdb(out)=-3 fall=1\", \"tran p avg i(vdd) from=1u to=2u\".",
        read_only: true,
        schema: || {
            json!({"type":"object","properties":{
                "job_id":{"type":"string"},
                "measurements":{"type":"array","items":{"type":"string"}}},
                   "required":["job_id","measurements"]})
        },
    },
    Tool {
        name: "alter_and_rerun",
        title: "Alter and rerun",
        description: "Change parameters of a loaded circuit and (by default) simulate again. \
                      Targets: an instance (param `value` for R/C/L/sources/gains; `w`, `l`, \
                      `area`, ... for devices), a model name (any model parameter) or \
                      `.options`. The change persists in the session.",
        read_only: false,
        schema: || {
            json!({"type":"object","properties":{
                "circuit_id":{"type":"string"},
                "changes":{"type":"array","items":{"type":"object","properties":{
                    "target":{"type":"string"},"param":{"type":"string"},"value":{"type":"number"}},
                    "required":["target","param","value"]}},
                "analyses":{"type":"array","items":{"type":"string"}},
                "rerun":{"type":"boolean","default":true}},
                   "required":["circuit_id","changes"]})
        },
    },
    Tool {
        name: "sweep",
        title: "Parameter sweep",
        description: "Simulate a circuit for each value of one parameter, in parallel, and \
                      return a table of measurements (the stored circuit is not changed). \
                      Give `values`, or `start`/`stop`/`points` (linear) with optional \
                      `log: true`.",
        read_only: false,
        schema: || {
            json!({"type":"object","properties":{
                "circuit_id":{"type":"string"},
                "target":{"type":"string"},"param":{"type":"string"},
                "values":{"type":"array","items":{"type":"number"}},
                "start":{"type":"number"},"stop":{"type":"number"},
                "points":{"type":"integer","minimum":1,"maximum":MAX_SWEEP},
                "log":{"type":"boolean"},
                "measurements":{"type":"array","items":{"type":"string"}}},
                   "required":["circuit_id","target","param"]})
        },
    },
    Tool {
        name: "explain_failure",
        title: "Explain failure",
        description: "Explain the diagnostics of a job (job_id) or a diagnostic code (code): \
                      what it means, culprits, what was tried, and remedies.",
        read_only: true,
        schema: || json!({"type":"object","properties":{"job_id":{"type":"string"},"code":{"type":"string"}}}),
    },
    Tool {
        name: "export_deck",
        title: "Export deck",
        description: "Return the canonical SPICE deck of a loaded circuit with all alterations \
                      applied; it runs unchanged in ngspice for cross-checking.",
        read_only: true,
        schema: || {
            json!({"type":"object","properties":{"circuit_id":{"type":"string"}},
                   "required":["circuit_id"]})
        },
    },
];

pub(crate) fn exists(name: &str) -> bool {
    TOOLS.iter().any(|t| t.name == name)
}

pub(crate) fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": (t.schema)(),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": false,
                    "idempotentHint": t.read_only,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError {
        diagnostics: vec![Diagnostic::error("E-API-INPUT", msg)],
    }
}

fn str_arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, ApiError> {
    a.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("missing string argument `{k}`")))
}

fn opt_f64(a: &Value, k: &str) -> Result<Option<f64>, ApiError> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| bad(format!("argument `{k}` must be a number"))),
    }
}

fn strings(a: &Value, k: &str) -> Result<Option<Vec<String>>, ApiError> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(v)) => v
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad(format!("`{k}` must be an array of strings")))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::String(s)) => Ok(Some(vec![s.clone()])),
        Some(_) => Err(bad(format!("`{k}` must be an array of strings"))),
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| bad(format!("serialization failed: {e}")))
}

/// Circuit id from `circuit_id`, or by loading `netlist`. Returns the id and load warnings.
fn circuit(s: &mut Session, a: &Value) -> Result<(String, Option<Value>), ApiError> {
    if let Some(id) = a.get("circuit_id").and_then(Value::as_str) {
        return Ok((id.to_string(), None));
    }
    let rep = load(s, a).map_err(|e| {
        if a.get("netlist").is_none() && a.get("path").is_none() {
            bad("give `circuit_id`, `netlist` or `path`")
        } else {
            e
        }
    })?;
    Ok((rep.circuit_id.clone(), Some(to_json(&rep)?)))
}

/// Load from `path` or `netlist`.
fn load(s: &mut Session, a: &Value) -> Result<spx_api::LoadReport, ApiError> {
    match a.get("path").and_then(Value::as_str) {
        Some(p) => s.load_file(std::path::Path::new(p)),
        None => s.load(str_arg(a, "netlist")?),
    }
}

pub(crate) fn call(s: &mut Session, name: &str, a: &Value) -> Result<Value, ApiError> {
    match name {
        "validate_netlist" => match a.get("path").and_then(Value::as_str) {
            Some(p) => to_json(&s.validate_file(std::path::Path::new(p))),
            None => to_json(&s.validate(str_arg(a, "netlist")?)),
        },
        "load_netlist" => to_json(&load(s, a)?),
        "estimate_cost" => to_json(&s.estimate(str_arg(a, "circuit_id")?)?),
        "simulate" => {
            let (id, loaded) = circuit(s, a)?;
            let analyses = strings(a, "analyses")?;
            let job = to_json(&s.simulate(&id, analyses.as_deref())?)?;
            Ok(match loaded {
                Some(l) => json!({ "load": l, "job": job }),
                None => job,
            })
        }
        "get_waveform" => {
            let reduce = match a.get("reduce").and_then(Value::as_str) {
                Some(r) => Reduce::parse(r).ok_or_else(|| bad(format!("unknown reduce `{r}`")))?,
                None => Reduce::Auto,
            };
            let analysis = a
                .get("analysis")
                .and_then(Value::as_u64)
                .map(|i| i as usize);
            let max_points = a
                .get("max_points")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_POINTS, |n| n as usize)
                .clamp(3, MAX_POINTS);
            let w = s.waveform(
                str_arg(a, "job_id")?,
                str_arg(a, "signal")?,
                analysis,
                reduce,
                opt_f64(a, "from")?,
                opt_f64(a, "to")?,
                max_points,
            )?;
            to_json(&w)
        }
        "measure" => {
            let m = strings(a, "measurements")?.ok_or_else(|| bad("missing `measurements`"))?;
            to_json(&json!({ "measures": s.measure(str_arg(a, "job_id")?, &m)? }))
        }
        "alter_and_rerun" => {
            let id = str_arg(a, "circuit_id")?.to_string();
            let changes: Vec<Change> = a
                .get("changes")
                .cloned()
                .ok_or_else(|| bad("missing `changes`"))
                .and_then(|v| {
                    serde_json::from_value(v).map_err(|e| bad(format!("invalid `changes`: {e}")))
                })?;
            let alter = to_json(&s.alter(&id, &changes)?)?;
            if a.get("rerun").and_then(Value::as_bool) == Some(false) {
                return Ok(json!({ "alter": alter }));
            }
            let analyses = strings(a, "analyses")?;
            let job = to_json(&s.simulate(&id, analyses.as_deref())?)?;
            Ok(json!({ "alter": alter, "job": job }))
        }
        "sweep" => {
            let values = sweep_values(a)?;
            let m = strings(a, "measurements")?.unwrap_or_default();
            to_json(&s.sweep(
                str_arg(a, "circuit_id")?,
                str_arg(a, "target")?,
                str_arg(a, "param")?,
                &values,
                &m,
            )?)
        }
        "explain_failure" => {
            let key = a
                .get("job_id")
                .or_else(|| a.get("code"))
                .and_then(Value::as_str)
                .ok_or_else(|| bad("give `job_id` or `code`"))?;
            to_json(&s.explain(key)?)
        }
        "export_deck" => {
            let id = str_arg(a, "circuit_id")?;
            Ok(json!({ "circuit_id": id, "dialect": "ngspice", "deck": s.export(id)? }))
        }
        _ => Err(bad(format!("unknown tool `{name}`"))),
    }
}

fn sweep_values(a: &Value) -> Result<Vec<f64>, ApiError> {
    if let Some(v) = a.get("values") {
        let v: Vec<f64> = v
            .as_array()
            .ok_or_else(|| bad("`values` must be an array of numbers"))?
            .iter()
            .map(|x| x.as_f64().ok_or_else(|| bad("`values` must be numbers")))
            .collect::<Result<_, _>>()?;
        if v.is_empty() || v.len() > MAX_SWEEP {
            return Err(bad(format!("`values` must have 1 to {MAX_SWEEP} entries")));
        }
        return Ok(v);
    }
    let (Some(start), Some(stop)) = (opt_f64(a, "start")?, opt_f64(a, "stop")?) else {
        return Err(bad("give `values`, or `start`, `stop` and `points`"));
    };
    let n = a.get("points").and_then(Value::as_u64).unwrap_or(11) as usize;
    if n == 0 || n > MAX_SWEEP {
        return Err(bad(format!("`points` must be 1 to {MAX_SWEEP}")));
    }
    let log = a.get("log").and_then(Value::as_bool) == Some(true);
    if log && (start <= 0.0 || stop <= 0.0) {
        return Err(bad("a log sweep needs positive `start` and `stop`"));
    }
    Ok((0..n)
        .map(|k| {
            let t = if n == 1 {
                0.0
            } else {
                k as f64 / (n - 1) as f64
            };
            if log {
                start * (stop / start).powf(t)
            } else {
                start + (stop - start) * t
            }
        })
        .collect())
}
