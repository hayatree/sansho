# Sansho v0.1.0 release audit

Reviewed the `release-prep` working tree against base commit `43b4424` on 2026-11-22.
This review replaces the 2026-11-20 audit of `bb73272`, which predated the R15–R22
model rounds, the solver replacement, the GPU batch and the Monte Carlo throughput
work. The review used tracked files, local Git history and the locked Cargo cache.
External simulator source and local PDK/model trees were excluded from inspection.

## Source and fixture provenance

- All 206 tracked Rust files have a provenance line in their opening module
  documentation. So do the 29 tracked C, Python, shell, Verilog-A and Docker
  files. Only Cargo, Clippy, cargo-deny and toolchain configuration files have
  none. [Module coverage](PROVENANCE.md#module-coverage)
- The provenance record has entries for every model and analysis round,
  including native VBIC, HICUM/L2, B4SOI 4.4, TXL/CPL, PSS and the one- and
  two-dimensional CIDER devices. It also covers the GPU diode kernel, the
  mixed-precision solver, Monte Carlo scheduling, sharing of compact-model
  bindings and the BSIM4 charge-sample bypass.
- The original sparse ordering, LU and dense QZ code replaced `faer`. Sansho
  therefore no longer links code that carries Eigen MPL-2.0, LAPACK or
  SuiteSparse component notices. That was the open license review item in the
  previous audit.
- The ngspice reference containers in `tools/` run ngspice only as a black-box
  executable. The PSS image is built by the maintainer from the 44.2 release
  tarball, and the recipe deletes the source inside the build. Sansho's PSS was
  implemented before that image existed. [PSS oracle](PROVENANCE.md#algorithms-and-references)
- The B4SOI parameter-check log and a Python bytecode file had been committed by
  mistake. Both were removed and are now covered by ignore rules. The bytecode
  embedded a local absolute path, so it was also removed from the history on
  2026-11-23. The two parameter-check logs, one for BSIM3 and one for B4SOI,
  remain in history. They are ngspice output for original decks and contain no
  source code or local paths.

## Dependencies and privacy

- The license inventory lists 82 external crate versions. It covers both
  release binaries with all features across the four release targets, so it
  also lists the optional `cudarc` CUDA bindings (MIT OR Apache-2.0). Four
  Cranelift/Wasmtime packages carry no license file. Each uses the Apache
  license with LLVM exceptions from `cranelift-codegen` at the same upstream
  revision. [Binary notices](../THIRD-PARTY-LICENSES.md)
- Release archives enable only the `jit` feature. The experimental `gpu`
  feature is available in source builds.
- No production PDK, foundry model or third-party Verilog-A model is tracked.
  SKY130, IHP SG13G2 and BSIM-CMG inputs are supplied locally in the
  git-ignored `va-models/` directory.
- Tracked files were scanned for private keys, access tokens, email addresses,
  home directory paths, private network addresses and workstation names. The
  scan found none in project-authored files. Every commit's author and
  committer address is a GitHub no-reply address.

## Validation

`tools/ci.sh --cached` passed on Linux x86-64. It covers formatting, all-target
Clippy with warnings denied, workspace tests with default and all features,
cargo-deny, the notice check and every cached verification suite.
`cargo +1.98.1 check --workspace --locked` passed on Arm64 Linux, both with and
without the `gpu` feature.

| Cached ngspice 44.2 suite | Passed | Known differences |
| --- | ---: | ---: |
| Standard decks | 148 | 17 |
| BSIM3 | 13 | 0 |
| BSIM4 | 22 | 0 |
| SKY130 | 7 | 1 |
| Verilog-A | 15 | 0 |
| R11 analog code models | 7 | 0 |
| R12 digital code models | 4 | 1 |
| R13 code models | 10 | 0 |
| PSS vs ngspice | 7 | 4 |
| PSS analytic | 6 | — |

The verification guide gives the cause of each known difference and the
tolerance it was compared with. [Verification method and decks](../verify/README.md)

The release build commands were run locally for Linux x86-64 and Arm64. Each
archive was built with `--features sansho-cli/jit,sansho-mcp/jit`, tested and
packaged in the release layout. macOS and Windows archives, GitHub attestation,
the draft release and the repository's private vulnerability reporting setting
require GitHub-side verification.
