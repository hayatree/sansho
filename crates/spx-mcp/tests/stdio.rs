//! End-to-end: drive the `spx-mcp` binary over stdio through a complete design loop
//! (milestone M3 exit criterion).
//!
//! Provenance: original test.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spx-mcp"))
            .args(["--threads", "2"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id, "{v}");
        v
    }

    /// Call a tool; returns (structuredContent, isError).
    fn tool(&mut self, name: &str, args: Value) -> (Value, bool) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, res["structuredContent"]);
        (
            res["structuredContent"].clone(),
            res["isError"].as_bool().unwrap(),
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Common-source NMOS amplifier with a resistive load.
const AMP: &str = "cs amp
VDD vdd 0 3.3
VIN in 0 DC 1 AC 1
RD vdd out 10k
M1 out in 0 0 nch l=1u w=10u
CL out 0 1p
.model nch nmos vto=0.7 kp=100u lambda=0.02 tox=10n cgso=0.3n cgdo=0.3n
.ac dec 20 1k 10g
.meas ac gain max vdb(out)
.meas ac bw when vdb(out)=7 fall=1
.end
";

fn gain(job: &Value) -> f64 {
    job["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "gain")
        .unwrap()["value"]
        .as_f64()
        .unwrap()
}

#[test]
fn design_loop_over_stdio() {
    let mut c = Client::start();
    let init = c.request(
        "initialize",
        json!({"protocolVersion":"2025-06-18","capabilities":{},
               "clientInfo":{"name":"test","version":"0"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    c.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let tools = c.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for t in [
        "validate_netlist",
        "simulate",
        "get_waveform",
        "measure",
        "alter_and_rerun",
        "sweep",
        "explain_failure",
        "export_deck",
        "estimate_cost",
    ] {
        assert!(names.contains(&t), "{t} missing");
    }

    // 1. Validate: a unit typo is caught before simulating.
    let (v, err) = c.tool(
        "validate_netlist",
        json!({"netlist": AMP.replace("RD vdd out 10k", "RD vdd out 10M")}),
    );
    assert!(!err && v["ok"] == true);
    assert!(
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "W-LINT-UNIT-MILLI")
    );

    // 2. Simulate from text: summary first.
    let (r, err) = c.tool("simulate", json!({"netlist": AMP}));
    assert!(!err, "{r}");
    let cid = r["load"]["circuit_id"].as_str().unwrap().to_string();
    let job = &r["job"];
    assert_eq!(job["status"], "ok");
    let g0 = gain(job);
    assert!(g0 > 5.0, "gain {g0} dB");
    assert_eq!(job["measures"][0]["unit"], "dB");

    // 3. Waveform with a point budget.
    let (w, err) = c.tool(
        "get_waveform",
        json!({"job_id": job["job_id"], "signal": "v(out)", "max_points": 40}),
    );
    assert!(!err);
    assert_eq!(w["unit"], "dB");
    assert_eq!(w["y"].as_array().unwrap().len(), 40);

    // 4. Design step: a wider device raises the gain.
    let (r, err) = c.tool(
        "alter_and_rerun",
        json!({"circuit_id": cid, "changes": [{"target":"m1","param":"w","value":20e-6}]}),
    );
    assert!(!err, "{r}");
    let g1 = gain(&r["job"]);
    assert!(g1 > g0 + 1.0, "{g0} → {g1}");

    // 5. Sweep the load resistor in parallel.
    let (sw, err) = c.tool(
        "sweep",
        json!({"circuit_id": cid, "target":"rd", "param":"value",
               "start": 5e3, "stop": 20e3, "points": 4}),
    );
    assert!(!err, "{sw}");
    let gains: Vec<f64> = sw["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["measures"][0]["value"].as_f64().unwrap())
        .collect();
    assert!(gains.windows(2).all(|p| p[1] > p[0]), "{gains:?}");

    // 6. Measure after the fact.
    let (m, err) = c.tool(
        "measure",
        json!({"job_id": r["job"]["job_id"], "measurements": ["ac g10k find vdb(out) at=10k"]}),
    );
    assert!(!err);
    assert!((m["measures"][0]["value"].as_f64().unwrap() - g1).abs() < 0.1);

    // 7. Export the modified deck for ngspice cross-checks.
    let (d, err) = c.tool("export_deck", json!({"circuit_id": cid}));
    assert!(!err);
    assert!(d["deck"].as_str().unwrap().contains("w=2e-5"));

    // 8. Failures are structured and explained.
    let (f, err) = c.tool(
        "simulate",
        json!({"netlist": "float\nV1 a 0 1\nC1 a b 1n\nC2 b 0 1n\n.op\n"}),
    );
    assert!(!err);
    assert_eq!(f["job"]["status"], "error");
    let (x, err) = c.tool("explain_failure", json!({"job_id": f["job"]["job_id"]}));
    assert!(!err);
    assert_eq!(x["codes"][0]["code"], "E-SOLVE-SINGULAR");
    assert_eq!(x["diagnostics"][0]["culprits"][0]["name"], "b");

    // Errors in requests are tool errors, not protocol errors.
    let (e, err) = c.tool("simulate", json!({"circuit_id": "c999"}));
    assert!(err);
    assert_eq!(e["diagnostics"][0]["code"], "E-API-UNKNOWN-ID");
}

#[test]
fn protocol_errors() {
    let mut c = Client::start();
    let r = c.request("no/such/method", json!({}));
    assert_eq!(r["error"]["code"], -32601);
    let r = c.request("tools/call", json!({"name": "nope"}));
    assert_eq!(r["error"]["code"], -32602);
    let r = c.request("initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(
        r["result"]["protocolVersion"],
        spx_mcp::PROTOCOL_VERSIONS[0]
    );
    let r = c.request("ping", json!({}));
    assert!(r["result"].is_object());
}

#[test]
fn statistical_batch_is_bounded_and_ordered() {
    let mut client = Client::start();
    let deck = "MC test\n.param r=unif(1000,0.1)\nR1 a 0 {r}\nV1 a 0 1\n.dc v1 0 1 1\n.meas dc current find i(v1) at=1\n.end";
    let (report, err) = client.tool("monte_carlo", json!({"netlist":deck,"seed":9,"n":3}));
    assert!(!err, "{report}");
    assert_eq!(report["statistics"][0]["count"], 3);
    assert_eq!(report["runs"][2]["index"], 2);
    assert_ne!(report["runs"][1]["measures"], report["runs"][2]["measures"]);
    assert!(report["runs"][2].get("waves").is_none());
    for n in [0, 257] {
        let (error, bad) = client.tool("monte_carlo", json!({"netlist":deck,"n":n}));
        assert!(bad && error["diagnostics"][0]["code"] == "E-API-INPUT");
    }
    let many = deck.replace(
        ".end",
        &(".meas dc current2 find i(v1) at=1\n".repeat(32) + ".end"),
    );
    let (error, bad) = client.tool("monte_carlo", json!({"netlist":many,"n":256}));
    assert!(
        bad && error["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("256 KiB")
    );
}
