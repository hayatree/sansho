//! Fresh-process tests for cache identity, publication and recovery.
//! Provenance: original fixtures from Ohm's law and the Verilog-AMS LRM.
#![cfg(all(feature = "jit", unix))]
#![allow(clippy::unwrap_used)]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn run(root: &Path, cache: &str, disabled: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_spx"));
    cmd.args(["run", "-j1", "--json"])
        .arg(root.join("test.cir"))
        .env("SPX_CACHE_DIR", root.join(cache))
        .env("SPX_PROFILE", "1")
        .env("SPX_CACHE", if disabled { "0" } else { "1" })
        .env("SPX_VA_JIT", "1");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn persistent_specializations() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("spx-cache-test-{}-{id}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let deck =
        "Cache fixture\n.hdl model.va\n.model m resistor\nV1 p 0 1\nN1 p 0 m\n.dc V1 0 1 0.5\n";
    fs::write(root.join("test.cir"), deck).unwrap();
    fs::write(root.join("model.va"), "`include \"coefficient.vams\"\nmodule resistor(p,n); electrical p,n; analog I(p,n) <+ `K * V(p,n); endmodule\n").unwrap();
    fs::write(root.join("coefficient.vams"), "`define K 2\n").unwrap();
    let reference = run(&root, "disabled", true);
    assert!(!root.join("disabled").exists());
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| run(&root, "shared", false)))
            .collect();
        for job in jobs {
            assert_eq!(job.join().unwrap().stdout, reference.stdout);
        }
    });
    let warm = run(&root, "shared", false);
    let log = |out: &Output| String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(warm.stdout, reference.stdout);
    assert!(!log(&warm).contains("spx-phase va_pre"));
    assert!(!log(&warm).contains("spx-code "));
    // Damage each published layer in turn, without relying on its filename.
    for file in fs::read_dir(root.join("shared"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    {
        fs::write(file.path(), b"truncated").unwrap();
        assert_eq!(run(&root, "shared", false).stdout, reference.stdout);
        assert!(fs::metadata(file.path()).unwrap().len() > 32);
    }
    fs::write(root.join("coefficient.vams"), "`define K 3\n").unwrap();
    let changed = run(&root, "shared", false);
    assert_ne!(changed.stdout, reference.stdout);
    assert_eq!(changed.stdout, run(&root, "disabled", true).stdout);
    assert!(log(&changed).contains("spx-code "));
    fs::write(root.join("test.cir"), format!("{deck}.option va_cache=0\n")).unwrap();
    let disabled = run(&root, "by-option", false);
    assert_eq!(disabled.stdout, changed.stdout);
    assert!(log(&disabled).contains("spx-code "));
    assert!(!root.join("by-option").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn split_kernel_round_trip() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("spx-split-test-{}-{id}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let deck =
        "Long chain\n.hdl chain.va\n.model m chain\nN1 a 0 m\nV1 a 0 1\n.dc V1 -0.5 0.5 0.25\n";
    let mut model =
        String::from("module chain(p,n); electrical p,n; real y; analog begin y=V(p,n);\n");
    model.push_str(&"y = sin(y) + 0.5 * V(p,n);\n".repeat(4096));
    model.push_str("I(p,n) <+ y; end endmodule\n");
    fs::write(root.join("chain.va"), model).unwrap();
    fs::write(root.join("test.cir"), deck).unwrap();
    let cold = run(&root, "cache", false);
    assert_eq!(
        String::from_utf8_lossy(&cold.stderr)
            .matches("spx-code ")
            .count(),
        2
    );
    let warm = run(&root, "cache", false);
    assert!(!String::from_utf8_lossy(&warm.stderr).contains("spx-code "));
    fs::write(root.join("test.cir"), format!("{deck}.option va_jit=0\n")).unwrap();
    let interpreter = run(&root, "unused", true);
    assert_eq!(cold.stdout, interpreter.stdout);
    assert_eq!(warm.stdout, interpreter.stdout);
    fs::remove_dir_all(root).unwrap();
}
