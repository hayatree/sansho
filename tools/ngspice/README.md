# ngspice oracle (Docker)

ngspice is used only as a **black-box reference**: we run its binary and compare its output
files with ours. Its sources are never read, copied or linked (see `CONTRIBUTING.md`).

```sh
tools/ngspice/ngspice.sh -b deck.cir -r out.raw   # builds the image on first use
```

- The image is Debian `trixie-slim` with the distribution `ngspice` package.
- The wrapper mounts the repository root at its host path and preserves the working directory.
  sansho-verify also mounts its output directory when that directory is outside the checkout.
  The `--compile-va` invocation selects `/usr/local/bin/openvaf-r` as the image entry point.
- The container has no network access, runs as your UID, and writes ASCII raw files
  (`SPICE_ASCIIRAWFILE=1`).
- Override the image tag with `SANSHO_NGSPICE_IMAGE`.
