//! Exercise the reference command boundary without Docker or third-party models.
//! Provenance: original CLI fixtures and analytic reference for an ideal voltage source.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn stages_hdl_caches_includes_and_skips_missing_inputs() {
    let root = std::env::temp_dir().join(format!("sansho-verify-va-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let lib = root.join("library with spaces");
    let decks = root.join("decks");
    let out = root.join("out");
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(&decks).unwrap();
    let deck = decks.join("bias.cir");
    fs::write(&deck,"bias\n.lib '../library with spaces/load.lib' typ\n.model dev load\nV1 p 0 0\nN1 p 0 dev\n.dc V1 0 1 1\n.measure dc peak max v(p)\n.end\n").unwrap();
    fs::write(lib.join("load.lib"), ".lib typ\n.HDL \"load.va\"\n.endl\n").unwrap();
    fs::write(lib.join("load.va"), "`include \"body.inc\"\n").unwrap();
    let model = "module load(p,n); electrical p,n; analog I(p,n)<+V(p,n); endmodule\n";
    fs::write(lib.join("body.inc"), model).unwrap();
    let docker = root.join("docker");
    fs::write(
        &docker,
        r#"#!/usr/bin/env bash
set -eu
if [[ "$1" == image ]]; then exit 0; fi
while [[ "$1" != sansho-ngspice:latest ]]; do
    if [[ "$1" == --entrypoint ]]; then
        [[ "$2" == /usr/local/bin/openvaf-r && "$4" == "$PWD/load.va" && "$5" == -o ]]
        printf 'compile\n' >> "$SANSHO_VERIFY_OUT/calls"
        printf 'fixture\n' > "$6"
        exit 0
    fi
    shift
done
shift
[[ "$1" == -b ]]
if [[ "$#" == 4 ]]; then
    [[ "$3" == -r ]]
    cat > "$4" <<'RAW'
Title: bias
Plotname: DC transfer characteristic
Flags: real
No. Variables: 2
No. Points: 2
Variables:
0 v-sweep voltage
1 v(p) voltage
Values:
0 0
  0
1 1
  1
RAW
else
    printf 'peak = 1\n'
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(root.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let run = |cached: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sansho-verify"));
        cmd.current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .env("PATH", &path)
            .env_remove("SANSHO_NGSPICE_IMAGE")
            .arg("--out")
            .arg(&out)
            .arg("--dir")
            .arg(&decks);
        if cached {
            cmd.arg("--cached");
        }
        let result = cmd.output().unwrap();
        let log = String::from_utf8(result.stdout).unwrap();
        assert!(
            result.status.success(),
            "{log}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        log
    };
    assert!(run(false).contains("PASS"));
    assert!(run(false).contains("PASS"));
    let reference = fs::read_to_string(out.join("bias.ref.cir")).unwrap();
    assert_eq!(reference.matches("pre_osdi").count(), 1);
    let library = fs::read_to_string(out.join("bias.include-0.lib")).unwrap();
    assert_eq!(library, ".lib typ\n.endl\n");
    let meas = fs::read_to_string(out.join("bias.meas.cir")).unwrap();
    assert_eq!(meas.matches(".control").count(), 1);
    assert!(meas.find("pre_osdi").unwrap() < meas.find("\nrun\n").unwrap());
    assert!(meas.contains("meas dc peak max v(p)"));
    fs::write(lib.join("body.inc"), format!("// edited\n{model}")).unwrap();
    assert!(run(false).contains("PASS"));
    assert_eq!(
        fs::read_to_string(out.join("calls")).unwrap(),
        "compile\ncompile\n"
    );
    fs::write(&docker, "#!/bin/sh\nexit 17\n").unwrap();
    assert!(run(true).contains("PASS"));
    let annotated = fs::read_to_string(&deck).unwrap().replace(
        "bias\n",
        "bias\n* sansho-verify: known-difference independent reference gain\n",
    );
    fs::write(&deck, annotated.replace(".dc V1 0 1 1", ".dc V1 0 2 2")).unwrap();
    let log = run(true);
    assert!(
        log.contains("KNOWN")
            && log.contains("1 known")
            && log.contains("independent reference gain")
    );
    fs::remove_file(lib.join("body.inc")).unwrap();
    let log = run(false);
    assert!(log.contains("SKIP") && log.contains("body.inc"));
    fs::remove_file(lib.join("load.lib")).unwrap();
    let log = run(true);
    assert!(log.contains("SKIP") && log.contains("load.lib"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn expected_reference_failure_requires_the_matching_diagnostic() {
    let root = std::env::temp_dir().join(format!("sansho-verify-rejection-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let deck = root.join("bias.cir");
    fs::write(&deck, "bias\n* sansho-verify: known-difference fixture rejection\n* sansho-verify: reference-error unsupported analysis\nV1 p 0 1\n.op\n.end\n").unwrap();
    let runner = root.join("reference");
    fs::write(
        &runner,
        "#!/bin/sh\necho 'unsupported analysis' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
    let run = |cached| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sansho-verify"));
        cmd.arg("--reference")
            .arg(&runner)
            .arg("--out")
            .arg(&root)
            .arg(&deck);
        if cached {
            cmd.arg("--cached");
        }
        cmd.output().unwrap()
    };
    for cached in [false, true] {
        let out = run(cached);
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 known"));
    }
    fs::write(
        root.join("bias.ref.error"),
        "reference simulator exited with exit status: 1: wrong diagnostic",
    )
    .unwrap();
    assert!(!run(true).status.success());
    fs::remove_file(root.join("bias.ref.error")).unwrap();
    assert!(!run(true).status.success());
    fs::remove_dir_all(root).unwrap();
}
