# Verification decks

Every `.cir` file here is simulated by spx and by ngspice (run in Docker as a black-box
reference, see `tools/ngspice`), and the results are compared:

```sh
cargo run -p spx-verify            # all decks
cargo run -p spx-verify -- --cached -v verify/decks/bjt_dc.cir
```

The score of a variable is `max|ours − ref| / (rtol·max|ref| + atol)`; it passes when ≤ 1.
Defaults: `rtol = 2e-3` (OP/DC/AC), `rtol_tran = 1e-2`, `rtol_tran_i = 3e-2` (transient
currents), `rtol_meas = 1e-2`, `atol_v = 1 µV`, `atol_i = 1 nA`. A deck can override them with a
comment line such as `* verify: rtol_tran=3e-2`.

To declare an accepted reference difference, add a full-line comment to the deck.

```spice
* spx-verify: known-difference <reason>
```

The verifier still runs the simulation, compares the waveforms and measurements,
and prints scores. A completed annotated comparison reports `KNOWN` and the
reason, even if its scores fall within tolerance. The summary counts these decks
separately. A run containing only passes and known differences exits with zero.
The JSON array groups known comparisons at the end. Each entry retains the numeric comparison, with `status: "KNOWN"`
and a `known_difference` reason. The `pass` field records the tolerance result.
Missing inputs retain their `SKIP` status. Simulation errors, unreadable cached
references and problems that prevent comparison produce `FAIL`.

The three marked BSIM4 layout decks cover KU0 mobility stress. ngspice 44.2
returned identical Id with and without KU0 in the black-box control. spx uses
BSIM4 manual equation 14.5 by maintainer decision.

All decks are original circuits written for this project. Raw files go to `verify/out/`
(ignored by git).

## Built-in BSIM4 and SKY130

`verify/bsim4-decks` contains 22 PDK-independent level-54 decks. The eight decks in
`verify/sky130-decks` select the TT section of the externally installed volare
`sky130A/libs.tech/ngspice/sky130.lib.spice` under `va-models/volare-sky130`.
The deck-local `.spiceinit` applies to the reference simulator.

```sh
cargo run --offline --release -p spx-verify -- --cached verify/bsim4-decks
cargo run --offline --release -p spx-verify -- --cached verify/sky130-decks
```

Omit `--cached` when generating fresh black-box references. Scores use the same tolerances
and waveform interpolation as the existing suites. Current discrepancies and the equation
coverage are recorded in [the BSIM4 model notes](../docs/bsim4.md).

The B4-2 diagnostic inputs are `verify/diag-decks/b4_lod_mobility_control.cir`
and `verify/diag-decks/sky130/sky_ring5_uic_tight.cir`. Pass both paths to the harness
to generate their first references. The SKY130 input shares the TT suite's
deck-local reference settings.

## Production Verilog-A decks

Supply the SG13G2 inputs under the git-ignored `va-models/ihp/` at the checkout
root. Keep the `psp103/`, `vbic_1p3/` and `r3_cmc/` VA files together with their
adjacent includes. Place the IHP corner libraries and the files they include in
`ngspice-models/`. Each deck header lists its inputs.

The five `cmg_*.cir` decks use the external BSIM-CMG 111.2.1 entry point
`va-models/bsimcmg/bsimcmg.va` and its adjacent includes. From the checkout
root, generate references and compare the ten IHP and five CMG decks:

```sh
cargo run --release -p spx-verify -- verify/va-decks
```

A directory, `--dir verify/va-decks`, an individual deck, and a shell-expanded
`verify/va-decks/*.cir` are accepted. Passing `verify/decks` runs the original
suite. Use `-v` to display all waveform and measurement scores. The VA decks use
`.save` to select the ngspice reference signals. spx records all circuit unknowns.

spx reads the original deck. For the reference run, the harness invokes
`openvaf-r` from `spx-ngspice:latest` and writes an OSDI file in `verify/out/`
(or `--out DIR`). Its cache key includes the HDL entry point, the contents and
paths of its resolved local includes, and the configured image tag. The reference
deck loads these objects with `pre_osdi` in a control block. The container sees
the whole checkout at its host path. After replacing a compiler under the same
image tag, remove the cached `model-*.osdi` files before regenerating references.

`SKIP` identifies a missing circuit input and prints its path. Skips have no
effect on the exit status. Parse errors, tool errors and unacknowledged comparison failures
produce `FAIL`. A run with only skips exits successfully with zero passes.
JSON entries include `status` and a skipped entry includes `missing`.

```sh
cargo run --release -p spx-verify -- --cached verify/va-decks
```

Cached mode uses the local circuit inputs, `*.ref.raw` waveforms and, for measured
decks, `*.ref.meas` logs. It runs spx locally and requires no Docker process or
OSDI artifact. A missing reference for a locally runnable deck is a failure.

## Monte Carlo distributions

Run these commands from the repository root. The black-box wrapper uses the local ngspice Docker image. The SKY130 mirror requires the Volare `sky130A` tree at the path used by the other SKY130 tests. Its two includes select the TT 1.8 V NFET and the corresponding mismatch coefficients, with `mc_mm_switch=1`, `mc_pr_switch=0` and a 1 µm geometry scale. This limits the library text ngspice reads inside the loop.

```sh
cargo build --offline --release -p spx-cli
mkdir -p verify/out
(
  set -e
  docker image inspect "${SPX_NGSPICE_IMAGE:-spx-ngspice:latest}" >/dev/null
  cd verify/mc-decks
  for deck in rc_delay mos_delay sky_mirror; do
    ../../target/release/spx mc -j 2 -n 200 --seed 1 --json "$deck.cir" > "../out/$deck.mc.json"
    ../../tools/ngspice/ngspice.sh -b "${deck}_ngspice.cir" > "../out/$deck.mc.log" 2>&1
    python3 compare.py "../out/$deck.mc.json" "../out/$deck.mc.txt"
  done
)
```

With ngspice installed locally, `ngspice -b "${deck}_ngspice.cir"` runs the same driver from `verify/mc-decks`. Each driver seeds ngspice once, reads the circuit afresh for each draw, measures it, appends one numeric row and releases the circuit and plot. The output text starts with the measurement names. Retain all 200 rows from both simulators.

| Deck | Measurement | Independent check |
|---|---|---|
| `rc_delay` | First half-charge crossing after a unit step | For independent normals, mean delay is ln(2) μR μC and variance is ln²(2) (μR² σC² + μC² σR² + σR² σC²). This deck gives 693.147 ns mean and 34.667 ns standard deviation. |
| `mos_delay` | Falling propagation delay from the input half-supply crossing | The two MOS1 model cards reference common threshold and mobility parameters. |
| `sky_mirror` | Current entering the positive terminal of Vsense at 1 V | The diode-connected reference device sets the gate voltage at 20 µA. The output device has an independent local model draw. Supply current has negative sign. |

The two simulators use independent streams. The mean tolerance is four estimated standard errors of the difference, plus an optional preselected model allowance, `--model-rtol R`, times the absolute reference mean (default zero). The sample standard deviation ratio uses the central 99.9% F interval for two independent normal samples. At N=200 the allowed ratio is 0.79088 to 1.26442. This is an approximate sampling check for nonlinear circuit responses. The script rejects missing rows, failed measurements and a zero-spread reference.
