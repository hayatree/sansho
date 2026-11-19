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

## R1 device and temperature decks

| Deck in `verify/decks` | Comparison |
|---|---|
| `transformer_load.cir` | Loaded winding voltages and currents in AC and transient analysis |
| `behavior_multiplier.cir` | Parameterized two-input multiplication under a DC sweep and two sinusoidal inputs |
| `behavior_limiter.cir` | DC soft clipping, its incremental AC transconductance, and an E table/G value load away from table breakpoints |
| `behavior_table_corners.cir` | KNOWN: E table with interior breakpoints; ngspice smooths the corners, spx interpolates linearly |
| `switch_hold.cir` | With a 10 ns maximum step, two hold capacitors sampled by voltage and current controls with corresponding hysteresis thresholds |
| `junction_temperature.cir` | Diode forward drop and BJT gain over a −20 to 100 °C sweep with model TNOM |
| `temperature_instances.cir` | Diode and BJT branch currents with absolute instance temperatures and offsets from a 55 °C ambient |
| `mos_temperature.cir` | Body-biased MOS1 drain current as threshold and mobility vary with temperature |
| `passive_ic.cir` | RC discharge and RL decay from capacitor-voltage and inductor-current initial conditions under UIC |
| `mos_geometry_scale.cir` | AC gain and junction loading with lengths, perimeters and areas specified in scaled layout units |

A new deck can carry `* spx-verify: pending-reference` while awaiting its
first maintainer-generated raw file. Cached verification runs spx first and
reports SKIP when that reference is absent. Once the reference exists it is
compared normally. Remove the marker when publishing the reference so its
subsequent absence is an error.

## R2 analysis decks

| File in `verify/decks` | Check |
|---|---|
| `noise_divider.cir` | Two 2 kΩ resistors at 300.15 K give output density √(4 kT · 1 kΩ), a factor of two for input referral, and a flat spectrum integrated over 10 Hz–100 kHz. |
| `noise_diode.cir` | Current-input shot and 1/f noise in a forward-biased junction with series resistance. |
| `noise_bjt_ce.cir` | Base and collector noise through a common-emitter stage with internal and external resistances. |
| `noise_mos1_cs.cir` | Channel thermal and KF flicker noise in a level-one common-source stage. |
| `noise_bsim4_cs.cir` | PDK-independent level-54 common-source stage with tnoiMod 1, fnoiMod 1 and gate/body resistance. |
| `tf_divider.cir` | DC gain 0.25, input resistance 4 kΩ and output resistance 750 Ω. |
| `sens_divider.cir` | Output derivatives with respect to the source and the two independent resistor values. |
| `sens_rc_ac.cir` | Complex derivatives of 1/(1 + jωRC) with respect to R and C through the RC corner. |
| `four_clipped.cir` | Odd harmonics and THD of a sine wave clipped by antiparallel diodes. |
| `pz_lead_lag.cir` | Voltage transfer with a pole at −1e6 and a zero at −5e5 rad/s. |

The Fourier comparison derives harmonic amplitudes and THD from the reference
transient in the batch raw file. The analytic tests independently check the DFT
on a known two-tone signal. The noise decks use small absolute floors appropriate
to spectral density.

The verifier strips one outer voltage or current wrapper on the noise,
integrated noise, TF, sensitivity and pole-zero plots, case-insensitively
on both sides. Sensitivity compares the intersection of parameter names.
ngspice exports additional instance and model-parameter derivatives. An empty
intersection fails. Poles and zeros are compared separately as complex
multisets, preserving multiplicity. The one-to-one pairing minimizes the
largest normalized complex distance using the deck's existing tolerances.

For `sens_rc_ac`, spx and the cached 44.2 raw agree on C1 and VIN. The raw
contains a nonzero R1 derivative only at the first frequency. For this circuit
the derivative is −jωC/(1+jωRC)². At 17.783 kHz it is
−4.93896e−4 + j5.49045e−5 V/Ω. spx checks both components against that
formula at every point in its analytic test. The separate reference vector
`v(r1_ac)` tracks this derivative across the sweep (about 1 ppm relative).
The comparison of the shared name R1 includes all nine samples.

### R2 noise audit

For `noise_bsim4_cs`, NF and M are both one, the effective geometry is
4 µm × 0.5 µm and the operating temperature is 300.15 K. At the spx bias,
Ids = 112.971 µA, Vds = 1.57406 V, Vdseff = 0.281767 V, Vgsteff = 0.292150 V
and μeff = 0.0261413 m²/(V·s). The intrinsic gm, gmb and gds are 638.647,
87.294 and 10.770 µS respectively. The low-frequency voltage gain magnitude
is 1.25036 in spx and 1.25058 in the reference (output/input noise ratio).

With the gate and body networks disabled, two parallel 4 µm devices and one
4 µm device with M=2 give 7.627815 µV/√Hz at 10 Hz. A single 8 µm device
with NF=2 has the same width per finger and agrees within 3 parts in 10⁹
there and 1 part in 10⁶ at 10 MHz.

| Independent spx source | Output at 10 MHz (nV/√Hz) |
|---|---:|
| 2 kΩ load | 5.63638 |
| Channel current, equation 10.12 | 2.51318 |
| Source-side voltage, equation 10.11 | 4.78916 |
| Unified flicker | 5.64094 |
| Gate electrode (4.8 Ω) | 0.35269 |
| Body network, five 50 Ω branches combined in power | 0.11002 |
| Physical source/drain diffusion and gate shot currents | 0 |

The cache contains only total spectra. Independent spx generators were
projected onto the output at fixed bias. As a cross-check, adding 1 mΩ of
explicit source and drain resistance gives the same source-side contribution
to six significant digits. At the low-frequency end, solving for a white
term and a 1/f term in the first two reference power samples gives a white
amplitude of 7.82000 nV/√Hz. The spx white sources give 7.82032 nV/√Hz.
The corresponding 10 Hz flicker amplitudes are 6.03242 µV/√Hz in the cache
and 5.64094 µV/√Hz in spx.

Using the 4.8.2 appendix A.10 defaults, spx obtains N₀ = 2.099e16,
Nₗ = 2.060e15 and N* = 2.070e15 m⁻². At 1 Hz the two inversion terms
in equation 10.2 are 6.933e−17 and 3.644e−17 A²/Hz. The subthreshold
term is 3.858e−16 A²/Hz, giving 8.301e−17 A²/Hz after blending.
A diagnostic run with explicit NOIC=8.75e9 gives 6.02971 µV/√Hz at 10 Hz.
The 4.8.2 manual prints 8.75 and the cached raw does not record the
oracle's parameter value. The model retains the published default and
the deck retains the residual failure.

`noise_bsim4_cs.cir` sets NOIC explicitly and passes. `noise_bsim4_noic_default.cir`
keeps the manual default and is a known difference (see `docs/bsim4.md`, Noise
defaults). `sens_rc_ac.cir` is a known difference for the reference's R1 vector
described above.

## R3 element and model decks

| File in `verify/decks` | Check |
|---|---|
| `line_reflections.cir` | Matched 50 Ω line and a 25 Ω driver into a 150 Ω termination. Compare near and far end AC phase, delayed ramps and successive reflections. TD and F/NL specify the same 5 ns delay. |
| `jfet_amplifier.cir` | Common-source NJF with source degeneration and series resistances. Gate sweep, gain and roll-off from CGS/CGD, and a sinusoidal transient. |
| `mos2_idvd.cir`, `mos3_idvd.cir` | Three gate biases under a common drain-voltage sweep and reverse body bias. Each sense-source current gives one output characteristic. |
| `mos2_idvg.cir`, `mos3_idvg.cir` | Three drain biases under a common gate sweep, from cutoff into strong inversion. |
| `mos2_inverter.cir`, `mos3_inverter.cir` | Complementary devices switching a capacitive load on both edges. Compare output transitions and supply current. |
| `poly_sources.cir` | POLY(2) differential voltage multiplier and POLY(1) current-controlled cubic. DC checks signs and AC checks the local slopes. |

The nine decks run locally through spx and have cached ngspice 44 waveforms. Analytic tests independently cover port common
mode, reflection amplitudes, phase and delay sensitivity, JFET polarity, noise and
capacitance, MOS channel integrals and derivatives, and all four POLY source types.

## R4 sweeps, devices and output

Across the standard, BSIM4, SKY130 and Verilog-A suites, the local R4 run
produced 86 passes and seven previously recorded known differences. The maintainer subsequently supplied ngspice 44.2
references for the original six R4 decks.

The four supplied `mos2_short_channel{,_hi}` and `mos3_short_channel{,_hi}` decks
each contain 61 drain biases at one gate voltage. At the standard DC tolerances all
four pass against the cached ngspice 44.2 raws. At gate biases of 1.5 and 2.5 V,
the largest absolute drain-current errors are 0.207 and 0.362 pA for MOS2 and
69.7 and 105 pA for MOS3. The MOS2 change includes narrow-width charge in the
channel integral and velocity-saturation boundary and uses the NFS-shifted turn-on
voltage in the mobility field. MOS3 uses its level-specific DELTA coefficient.
These four decks omit NSUB. Both MOS2 cards set LAMBDA explicitly to zero.

| New deck | Quantity exercised |
|---|---|
| `dc_nested.cir` | An inner ambient-temperature sweep repeated at two values of a voltage source with implicit DC value. |
| `mos_depletion.cir` | Nested drain and bulk sweeps with NSUB, XJ, VMAX, omitted MOS2 LAMBDA, NEFF and MOS3 KAPPA. |
| `bjt_transport.cir` | A common-emitter stage with spreading resistance, split collector charge, bias-dependent TF and excess phase, in DC, AC and transient. |
| `diode_extended.cir` | Forward and breakdown currents, perimeter leakage, and small-signal junction capacitance. |
| `am_source.cir`, `sffm_source.cir` | Delayed AM current and SFFM voltage, each with its own resistive load. |
| `meas_events.cir` | Repeated crossings of a reference voltage, delayed event selection, slopes, aggregates and a scalar derived from earlier results. |

The cached `bjt_transport`, `dc_nested` and `mos_depletion` comparisons pass.
The split AM and SFFM decks need their own raw references. The combined raw is
from the original source cards. Replaying those cards gives an AM output
score of 0.001 (24 µV maximum interpolation error over 1008 samples).
The SFFM voltage differs by up to 1.347 V in manual order. Evaluating the
frequency-swapped equation, with its corresponding 0.125 MDI limit,
reproduces that cached vector within 3 × 10⁻¹⁵ V. All seven current decks
produced byte-identical binary raw files with one and two workers. [Source conventions](../docs/compatibility.md#modulated-independent-sources)

The transient stimulus in `meas_events` is unchanged. The 44.2 raw contains
1 V at the new window edges (1 and 3 ms) and 0 V at 2 ms. Bracketing samples give
+1000 V/s at 0.5 ms and −1000 V/s at the last falling 0.25 V crossing. The
cached scalar log predates the window change and the derivative-vector reference
commands. It needs regeneration for a scalar comparison.

The existing `diode_iv.cir` also checks the default breakdown slope. At −6 V
the cached source current is 0.369711 A. With RS = 2 Ω the junction voltage
is −5.26058 V. Dividing the excess reverse voltage above BV by
Vt × ln(I/IBV) gives 1.05, the card's N. Inheriting N when NBV is omitted
keeps the normalized current score at 0.006 across all 691 points.

### Extended diode comparison with ngspice 44.2

The 50 Ω series resistor makes source bias and junction voltage different.
To isolate the effects, small DC decks drove the diode directly at junction
voltages taken from the cached curve. Each used IS=10 fA, N=1, AREA=2 and
PJ=10 µm at 27 °C. The entries are signed anode currents including GMIN.
Only the last row comes from ngspice.

| Enabled beyond the base junction | At −1.550000 V (pA) | At +0.627408 V (µA) |
|---|---:|---:|
| None | −1.570 | 685.10 |
| JSW=100 pA | −1.571 | 719.36 |
| ISR=5 pA, NR=1.8 | −17.542 | 689.49 |
| IKF=1 mA | −1.570 | 432.17 |
| Full R4 model | −17.543 | 451.90 |
| Cached full model | −1.571 | 451.84 |

Doubling AREA from 1 to 2 doubled the bottom current. Changing PJ from zero
to 10 µm added 34.26 µA at +0.627408 V for either area. Doubling PJ doubled
that increment. The ISR branch uses the R4 depletion-width factor
`((1 − Vd/VJ)² + 0.005)^(M/2)` on the NR exponential. Massobrio and
Antognetti discuss the width dependence and generation current in chapter 1,
§§1.1 and 1.9.1. At −1.55 V the reference is consistent with just the
bottom and sidewall leakage plus GMIN. The manual gives no reverse-bias
switch for ISR that explains this difference.

For positive total junction current I, equation 7.3 gives
`I / (1 + sqrt(I / (AREA × IKF)))`. Applying it before adding GMIN gives
451.874 µA at a source bias of +0.65 V, compared with 451.837 µA in the
cache and 537.6 µA before the correction. At the +0.6 V AC bias, both AC
output vectors pass across 1 MHz to 1 GHz. [ngspice user manual, §7.2.2](https://ngspice.sourceforge.io/docs/ngspice-manual.pdf#page=139)

With only BV, IBV and NBV enabled, a direct −3 V bias draws 2.000 mA in
reverse. The manual scales IBV by AREA. Algorithm 7.1 moves the avalanche
knee toward zero by `NBV × Vt × ln(IBVeff / Ileak)` when the target exceeds
the leakage at −BV. NBV defaults to N. When the target lies below the leakage,
the knee stays at −BV and the leakage sets the minimum current.

| Source bias | spx reverse current | Cached reverse current | Cached junction voltage |
|---|---:|---:|---:|
| −3.10 V | 2.0000 mA | 1.70066 mA | −3.014967 V |
| −2.85 V | 15.535 µA | 8.25196 µA | −2.849587 V |

Using the two cached junction voltages and log current ratio gives an
exponential slope of 1.2000 thermal voltages, in agreement with NBV.
Extrapolation to −3 V gives 1.0500 mA. Section 7.2.2 gives 2 mA for this
card. The combined cache cannot establish an alternative geometry rule.
The deck retains its DC failure and its original tolerances.

`diode_extended.cir` stops the sweep at −2.5 V and passes. The breakdown region moved to
`diode_breakdown.cir`, a known difference: spx follows the manual's area scaling of IBV,
while ngspice 44.2 produces about half that current at −BV for this card. The textbook
treatment of BV adjustment (Massobrio & Antognetti) is a follow-up item.
