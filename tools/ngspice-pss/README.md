# PSS-enabled ngspice oracle (Docker)

The Debian `ngspice` package used by `tools/ngspice` is built without the experimental
periodic-steady-state analysis, so it silently ignores `.pss`. This image compiles the
ngspice 44.2 release with `--enable-pss` (ngspice 44 manual §28.1.8) so that Sansho's `.pss`
results can be compared against a black-box reference.

## Isolation procedure

The clean-room rule is that nobody who writes Sansho code ever sees ngspice source. Building
from source is allowed only under these conditions:

1. A person builds the image by running `tools/ngspice-pss/build.sh` in their own terminal.
   Do not run it from an agent session (Claude, Codex), and do not run it while a Codex round
   is active.
2. The tarball is downloaded, compiled and deleted inside one `RUN` step. No layer or
   build-cache entry contains source, and the source is never written to the host.
3. All configure, compiler and make output goes to `/dev/null`. If the build fails, report
   only that it failed. Never open, paste or debug the log or the source.
4. The runtime image holds only the installed executable, code models and init scripts.
   `/opt/ngspice-pss/BUILDINFO` records the version, the tarball SHA-256, the configure
   flags and the build time.
5. The image only produces reference results. Sansho's PSS was implemented beforehand, from
   the manual and the published literature (see `docs/PROVENANCE.md`).

## Use

From the repository root, the verifier runs the installed oracle on each original
probe. Each successful capture writes a validated `.ref.raw` and its convergence
transcript `.ref.log` under `verify/out/`. Cached CI reads the pair. A probe
marked pending still runs with Sansho and reports SKIP until its first reference
is present. A missing required capture, incomplete raw or mismatched transcript
fails verification.

```sh
cargo run --offline --release -p sansho-verify -- verify/pss-ng-decks
cargo run --offline --release -p sansho-verify -- --cached -v verify/pss-ng-decks
```

For an individual batch capture use the wrapper from the repository root.

```sh
tools/ngspice-pss/build.sh                       # once, by a person, never by an agent
SANSHO_NGSPICE_IMAGE=sansho-ngspice-pss:latest tools/ngspice/ngspice.sh -b deck.cir -r out.raw
```

`ngspice.sh` never auto-builds this image. It exits with an error if the image is missing.

For this image the wrapper uses a named container with pulling disabled. An
independent watchdog force-removes the container after 230 seconds. Exit and
signal cleanup also force-remove it. The host needs Docker, Python 3 and GNU
`timeout`. The wrapper requires an accessible local image and Docker daemon.

The original complete captures in `verify/out/pss-ng/` supply the μ = 1
Van der Pol (`1e-3`), LC branch (`1e-3` and `1e-5`), and three-stage ring
(`1e-3`) comparisons. Both LC logs and the ring log report non-convergence.
The tighter VdP run announced 0 Hz and the tighter ring stopped during shooting.
Neither capture contains two complete plots.

## Recorded build

Built 2026-11-21 23:12 UTC by the project owner in their own terminal. The ngspice 44.2
tarball had SHA-256 `e7dadfb7bd5474fd22409c1e5a67acdec19f77e597df68e17c5549bc1390d7fd`.
Configure flags: `--enable-pss --enable-cider --disable-debug --without-x`. Only the status
lines and `BUILDINFO` were seen. No source or build log was viewed.
