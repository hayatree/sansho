# Sansho v0.1.0 release audit

Reviewed the release working tree against base commit `bb73272` on 2026-11-20.
The review used tracked files, proposed release files, local Git history and the
locked Cargo cache. External simulator source and local PDK/model trees were
excluded from source inspection.

## Source and fixture provenance

- All 126 Rust files have a provenance line in the opening module documentation.
  The provenance record now indexes every workspace crate, its nested modules,
  tests, model headers and project tooling. [Module coverage](PROVENANCE.md#module-coverage)
- Added missing provenance comments to two example circuits, fourteen verification
  circuits, three toy library/include files, the SKY130 reference settings and the
  reference container recipe. The source inventory also covered C, Python, shell,
  Verilog-A and VAMS files.
- Reviewed the original decks and their history, include paths, the VBIC wrapper,
  large source tables and long text blocks. The BSIM4 catalog records parameter
  facts with manual page references. Model constants have documented sources.
  The large numerical tables in the verification and performance notes record
  experiments. The review found no evidence of imported simulator code, manual
  passages, production PDKs, third-party Verilog-A models or copied test decks.
  This was an offline inspection, without a comparison against external corpora.
- Removed the tracked ngspice parameter-check log and four generated benchmark
  decks. Each deck matched the generator's output byte for byte before removal.
  Ignore rules now cover the log and all five generated decks, as well as
  `va-models/` and `verify/out/`. [Benchmark reproduction](performance.md#large-circuits)

## Dependencies and privacy

- The locked graph changed only in the versions of the eleven workspace packages.
  All report version 0.1.0 and disallow publication. The dependency inventory
  covers 154 external crate versions across both binaries and the four release
  targets. Twelve packages needed the standard MIT text. Four Cranelift/Wasmtime
  packages used the complete Apache license with LLVM exceptions from
  `cranelift-codegen` at the identical upstream Git revision. Each omission is
  listed in the inventory. [Binary notices](../THIRD-PARTY-LICENSES.md)
- **License review item.** The existing `faer 0.24.4` dependency declares MIT in
  Cargo and distributes separate Eigen MPL-2.0, LAPACK BSD and SuiteSparse BSD
  notices. The inventory retains all four. Cargo-deny accepted the declared
  licenses. Which component-specific terms apply to the linked binaries remains
  unverified.
- Scans for credential formats, key blocks, credential assignments, email
  addresses, home paths, private network addresses and workstation names found
  no apparent secrets or personal contact details in project-authored files.
  The full upstream license texts contain three distinct email addresses.
  Those attribution texts are preserved in the binary notices.

## Validation

Linux x86-64 passed formatting, all-target Clippy with warnings denied, and
workspace tests with default and all features. The offline notice check and
cargo-deny's advisory, ban, license and source checks passed. The cached CI run
used a writable copy of the local advisory database. Cargo-deny reported
duplicate-version warnings for existing dependencies.

| Cached ngspice suite | Passed | Known differences | Skipped |
| --- | ---: | ---: | ---: |
| Standard decks | 53 | 3 | 0 |
| BSIM3 | 13 | 0 | 0 |
| BSIM4 | 22 | 0 | 0 |
| SKY130 | 7 | 1 | 0 |
| Verilog-A | 15 | 0 | 0 |

The known cases are E-table corner smoothing, the RC resistor AC sensitivity
vector, SFFM frequency order and SKY130 free-running ring phase drift.

The Linux JIT binaries both reported version 0.1.0 after archive extraction.
The README divider produced 0.5 V at its output, wrote binary raw data and
returned JSON. The MCP server completed stdio initialization. Local workflow
checks exercised tag/version rejection, dry-run selection, changelog extraction,
archive layouts and checksums. Packaging tests used stand-ins for the three
foreign targets.

Rust 1.98 was unavailable locally. Its check remains in hosted CI. Native Arm
Linux, macOS and Windows tests, GitHub attestation, the draft release and the
repository's private reporting setting require GitHub-side verification.
