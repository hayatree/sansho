# Contributing

Thank you for your interest. Sansho is dual-licensed under Apache-2.0 OR MIT, and keeping
that promise requires some rules that are stricter than usual.

## 1. Clean-room policy

Most open-source circuit simulators are copyleft. Code derived from them cannot be released under
Apache-2.0/MIT, so **do not read, copy, paraphrase or translate** their source code while working
on this project.

**Forbidden sources** (do not consult, do not paste into AI tools):

- Xyce (GPL-3.0)
- VACASK (AGPL-3.0)
- Gnucap (GPL)
- OpenVAF / OpenVAF-reloaded (GPL-3.0)
- Qucs, Qucs-S, QucsatorRF, ADMS (GPL)
- LGPL- or MPL-licensed parts of ngspice (e.g. `numparam`, `tclspice`, `src/osdi`)
- SuiteSparse / KLU source (LGPL)
- Manual text of commercial simulators (HSPICE, Spectre, PSpice, LTspice, ...)
- Any other source under GPL, LGPL, AGPL, MPL or an unclear/non-commercial license

**Allowed sources:**

- Academic papers, textbooks, theses (e.g. Nagel 1975) and public specifications
- BSD/MIT/Apache-licensed code — **only** if you keep its copyright notice and record it in
  `NOTICE` and `docs/PROVENANCE.md` *in the same pull request*
- Running external simulators (ngspice, Xyce) as black-box oracles in tests, without copying
  their code, test decks or outputs that carry a copyleft license

When in doubt, ask in an issue before writing code.

## 2. Provenance

- Every source file starts with a module doc comment containing a `Provenance:` line that names
  the papers, specifications or permissively licensed code it is based on.
- Significant algorithms are additionally listed in `docs/PROVENANCE.md`.

## 3. Dependencies

- Only permissive licenses are allowed (see `deny.toml`). CI runs `cargo deny check`.
- Libraries with other licenses (e.g. CUDA libraries, KLU) may only be used as optional,
  user-installed, dynamically loaded components, never as default dependencies.

## 4. Test data

- Write your own test circuits or derive them from published schematics.
- Do not copy test decks from copyleft projects (e.g. the Xyce regression suite).
- Do not commit PDK files; tests that need a PDK fetch it separately.
- Do not commit outputs of commercial simulators or publish benchmarks against them.

## 5. Language and style

- All code, comments, documentation and commit messages are in English.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
- Run the local CI before merging to main: `tools/ci.sh` (full) or `tools/ci.sh --quick`
  (fmt, clippy, tests). GitHub Actions runs only on pushes to main and on demand.
- The library must not panic: return `Result` instead of using `unwrap`/`expect`/`panic!`.

## 6. Sign-off

By submitting a contribution you certify that you have the right to submit it under both
licenses and that it complies with this policy. Add a `Signed-off-by:` line to your commits
([Developer Certificate of Origin](https://developercertificate.org/)).

## 7. Releases

Sansho currently distributes binaries through GitHub Releases. crates.io publication is
deferred. All workspace packages inherit `publish = false`.

Update the changelog and workspace version together. After fetching the locked dependencies,
run `python3 tools/third_party_licenses.py` to regenerate the binary notices offline.
Run the release workflow manually with its dry-run option to test the packages. A matching
version tag builds and tests the native targets, attests the archives and prepares a draft.
