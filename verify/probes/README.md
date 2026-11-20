# Compact-model probes for ngspice 44.2

Each original deck runs at its printed setting with a single analysis card. From the repository root, run the sweeps with a local ngspice 44.2 executable. The driver varies the one value listed on each deck's `* sweep:` line. It writes the exact circuit, ASCII raw file and log for every setting under `verify/out/probes/`.

```sh
verify/probes/sweep.sh
# Select the noise families using the repository's installed reference image
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/b3_noise2_*.cir
```

The maintainer supplied all 64 ngspice 44.2 runs. For the comparison, run
`sansho run -j 1 verify/out/probes/<deck>_<value>.cir -r /tmp/<deck>_<value>.raw`
for each saved circuit. “Before” below means the uncommitted R6 implementation
before these probe results. The CI verifier takes explicit suite directories
and only their immediate `.cir` files. These experiments run separately.

## BSIM3 noiMod 2

The `b3_noise2` reference gives 9.19425 µV/√Hz input noise at 1 Hz. Before these probes Sansho gave 7.30725 µV/√Hz at essentially the same gain and thermal floor. The revised model gives 9.19289 µV/√Hz.

In these probes an ideal drain supply fixes Vds. A noiseless 1 kΩ current-controlled voltage source reads its branch current. Thus `onoise_spectrum² / (1000 Ω)²` is the drain-current power density in A²/Hz. The default device has Vgs = 1.05 V, Vds = 1.7 V, L = 0.65 µm, W = 2.8 µm and T = 27 °C. The gain `onoise_spectrum / inoise_spectrum` is a cross-check on bias and input referral. Each run has one gate/drain bias and one frequency sweep. Compare power at equal frequencies and fit a constant plus a flicker component in the flat-gain band. Squaring the amplitude before subtracting the thermal floor matters, especially below threshold.

| Deck suffix | Hypothesis and discriminating signature |
|---|---|
| `ef` | A frequency-law difference changes the slope. Power proportional to 1/f^EF has the same 1 Hz intercept for all three EF values and a log-log slope of −EF. A fixed noise normalization shifts the intercept at every EF. |
| `noia` | A different weighting of the logarithmic carrier-density term changes the incremental power per unit NOIA. Use zero as the intercept and compare the three positive settings. A common units factor would multiply all three trap-coefficient slopes equally. |
| `noib` | The linear-density term weights the difference between source and drain carrier densities. A changed slope relative to NOIA implicates their relative weighting or the endpoint densities. |
| `noic` | The quadratic-density term weights the difference of squared endpoint densities. The 10× and 100× settings make this small baseline contribution measurable. Curvature in power versus coefficient would distinguish blending or clipping from a single linear trap integral. |
| `em` | An EM-dependent shortening term changes the saturated 1 Hz power while leaving the thermal floor alone. Check for a discontinuity at zero, a low-field plateau and a logarithmic rise at higher field. The prior Sansho model could have finite shortening at EM=0 from the excess drain field. |
| `vgs` | A different weak/strong inversion transition gives a bias-dependent residual around turn-on. A change in endpoint density weighting can persist at high overdrive. Compare the reference/Sansho flicker-power ratio across the sweep. |
| `vds` | A shortening or drain-end-field discrepancy should grow near and above saturation. A discrepancy already present at 50 mV also implicates the channel integral or its normalization. |
| `l` | An effective noise-length correction changes the reference/Sansho ratio most at the shortest length. A different length exponent gives a trend across the whole sweep. Use pointwise ratios to account for the length dependence of DC current and gain. |
| `w` | A width normalization or effective-width difference produces a systematic width dependence in the power ratio. A fixed spectrum multiplier gives a flat ratio in both geometry sweeps. |
| `temp` | A missing thermal-energy or temperature-dependent carrier-density term changes the power ratio with kelvin temperature at fixed TNOM. A constant units conversion gives a flat ratio after accounting for each simulator's bias-dependent response. |

### Measured flicker and thermal power

For each spectrum, square the output amplitude and divide by the squared
1 kΩ transimpedance. Fit drain-current power as `T + F / f^EF` with frequency
in Hz. The difference between the 1 Hz and 100 MHz samples divided by
`1 − (10⁸)^−EF` gives F. The intermediate samples independently verify
the slope. All 33 reference points per setting agree with the fit to within
2.1 × 10⁻¹⁵ relative power. The table gives F at 1 Hz in **10⁻¹⁸ A²/Hz**.

| Varied quantity | Setting | Before | ngspice 44.2 | Revised Sansho |
|---|---:|---:|---:|---:|
| EF | 0.5 | 4.959 | 7.86949 | 7.86709 |
| EF | 1 | 4.959 | 7.86949 | 7.86709 |
| EF | 1.5 | 4.959 | 7.86949 | 7.86709 |
| EM (V/m) | 0 | 3.98474 | 2.52374 | 2.52297 |
| EM (V/m) | 1e6 | 4.02397 | 6.05668 | 6.05484 |
| EM (V/m) | 4.1e7 | 4.959 | 7.86949 | 7.86709 |
| EM (V/m) | 1e8 | 5.60222 | 9.11102 | 9.10825 |
| L (µm) | 0.15 | 1490.64 | 1533.1 | 1532.64 |
| L (µm) | 0.325 | 64.7603 | 75.864 | 75.8412 |
| L (µm) | 0.65 | 4.959 | 7.86949 | 7.86709 |
| L (µm) | 1.3 | 0.498537 | 1.11253 | 1.1122 |
| NOIA | 0 | 2.51809 | 0 | 0 |
| NOIA | 4e19 | 3.73855 | 5.43466 | 5.43302 |
| NOIA | 8e19 | 4.959 | 7.86949 | 7.86709 |
| NOIA | 1.6e20 | 7.39991 | 12.7172 | 12.7133 |
| NOIB | 0 | 2.50496 | 4.90263 | 4.90113 |
| NOIB | 1.5e4 | 3.73198 | 6.3896 | 6.38765 |
| NOIB | 3e4 | 4.959 | 7.86949 | 7.86709 |
| NOIB | 6e4 | 7.41304 | 10.8082 | 10.8049 |
| NOIC | 0 | 4.89495 | 7.80775 | 7.80537 |
| NOIC | 2e-13 | 4.959 | 7.86949 | 7.86709 |
| NOIC | 2e-12 | 5.53546 | 8.42459 | 8.42203 |
| NOIC | 2e-11 | 11.3001 | 13.9214 | 13.9172 |
| TEMP (°C) | -25 | 5.9476 | 9.30812 | 9.3053 |
| TEMP (°C) | 0 | 5.43725 | 8.57734 | 8.57473 |
| TEMP (°C) | 27 | 4.959 | 7.86949 | 7.86709 |
| TEMP (°C) | 75 | 4.23088 | 6.77463 | 6.77256 |
| TEMP (°C) | 125 | 3.57525 | 5.80753 | 5.80575 |
| Vds (V) | 0.05 | 0.0849275 | 0.109235 | 0.109201 |
| Vds (V) | 0.2 | 1.02811 | 1.3965 | 1.39607 |
| Vds (V) | 0.35 | 3.66657 | 3.41969 | 3.41864 |
| Vds (V) | 0.7 | 4.12008 | 5.65907 | 5.65735 |
| Vds (V) | 1.7 | 4.959 | 7.86949 | 7.86709 |
| Vgs (V) | 0.35 | 2.56885e-09 | 8.16729e-10 | 8.16982e-10 |
| Vgs (V) | 0.55 | 0.000201906 | 6.25684e-05 | 6.25663e-05 |
| Vgs (V) | 0.7 | 0.284169 | 0.0669895 | 0.0669737 |
| Vgs (V) | 0.8 | 3.05168 | 1.13328 | 1.13295 |
| Vgs (V) | 1.05 | 4.959 | 7.86949 | 7.86709 |
| Vgs (V) | 1.35 | 12.507 | 18.3037 | 18.2981 |
| W (µm) | 0.35 | 0.430455 | 0.61819 | 0.618005 |
| W (µm) | 1.4 | 2.16092 | 3.36577 | 3.36476 |
| W (µm) | 2.8 | 4.959 | 7.86949 | 7.86709 |
| W (µm) | 5.6 | 11.2758 | 17.9461 | 17.9406 |

The unchanged intercept across EF and the slopes −0.5, −1 and −1.5 establish
the frequency law. A fixed multiplier fails the bias and length sweeps.
Among settings with nonzero reference flicker, the reference/before power ratio ranges from 0.236 at Vgs = 0.7 V to 2.232
at L = 1.3 µm. The discrepancy already reaches 28.6% at Vds = 50 mV.
NOIA = 0 extinguishes flicker with finite NOIB and NOIC. Positive NOIA
restores it, and the NOIB and NOIC increments bend at large coefficients.
EM = 0 removes the shortening term even at Vds = 1.7 V.

One law fits all ten families without adjusting a trap coefficient. Use the
Chapter 3 channel sheet densities in the Chapter 8 strong-inversion integral,
with `N₀ = Cox × Vgsteff / q` and
`Nₗ = N₀ × max(0, 1 − Abulk × Vdseff / (Vgsteff + 2 Vt))`.
Use the same effective drain bias for excess drain field. Positive EM enables
the nonnegative logarithmic shortening term. Combine strong and weak powers
as `S × W / (S + W)` at the actual bias throughout the sweep. The weak term
is proportional to NOIA. The original thermal-energy units and the
2 × 10¹⁴ m⁻² density offset give a maximum relative error of 0.031% in
nonzero flicker power. Both 44.2 and revised Sansho give exactly zero flicker with NOIA = 0.

The independent thermal intercept tests the capMod 3 inversion charge.
At Vgs = 0.35 V the reference output thermal amplitude is 3.670 pV/√Hz.
Direct subtraction of the surface-potential correction gives 3.207 pV/√Hz.
Limiting that correction smoothly to the available CV overdrive with a 1 mV
transition gives 3.671 pV/√Hz. Across all 43 spectra the largest thermal
power error is 0.036%. The largest pointwise amplitude error in either
output or input noise is 0.018% over 1–10⁸ Hz.

**Bandwidth integration.** In the two weakest gate-bias runs the gain varies
appreciably over a single frequency interval. At Vgs = 0.35 V, integrating
the reference output power over each interval and dividing by the squared
gain at its upper frequency reproduces the saved input RMS of 0.901070 mV.
Applied to the reference spectra, this rule reproduces the input RMS in
all 43 raw files to within 1.2 × 10⁻¹⁵ relative error.
Using each endpoint's own gain in the log-power integral gives 1.025851 mV.
Sansho uses the latter analysis convention. Its input RMS differs from the
saved scalar by 13.85% at 0.35 V and 5.09% at 0.55 V. The loaded
`b3_noise2` CI deck passes for both spectra and both integrals at its
original tolerances. [Noise integration](../../docs/analyses.md#noise)

## SPICE3 diode breakdown

In `diode_iv`, correcting the source voltage for 2 Ω of series resistance gives −5.260579 V at 0.369711 A. The avalanche exponential extrapolates to 1.0000 mA at −BV with slope N × Vt. In `diode_breakdown`, using the saved junction voltages gives 1.0500 mA with slope NBV × Vt. At AREA=2, a plain area factor predicts 2 mA and an area-independent IBV predicts 1 mA. The measured 5% excess also equals PJ × JSW / (AREA × IS). These two cards change several quantities together.

The probes drive the diode directly and include a sample at −BV. The common model has BV=3 V, IBV=1 mA, NBV=1.2, AREA=2, PJ=10 µm, IS=10 fA and JSW=0.1 nA/m. Read positive reverse current from `i(Vbias)`. Estimate the exponential slope and intercept using several points either side of −BV, then inspect the low-current end of the sweep for the leakage floor and the transition into avalanche. Subtract the known shunt contribution, −GMIN × Vd, when resolving femtoamps.

| Deck suffix | Hypothesis and discriminating signature |
|---|---|
| `area` | At AREA=1, 2 and 4, area-scaled IBV predicts 1, 2 and 4 mA at −BV. An unscaled target predicts 1 mA throughout. An unscaled target times total/bottom saturation current predicts 1.100, 1.050 and 1.025 mA. |
| `pj` | A sidewall prefactor predicts 1.000, 1.025, 1.050 and 1.100 mA at fixed AREA=2. A PJ-independent intercept combined with a changing deep-reverse leakage floor would confine the sidewall effect to leakage. |
| `is` | With a total/bottom prefactor, multiplying IS by ten divides the excess above IBV by ten. With a fixed target, the high-current intercept stays fixed while the leakage floor and onset move. |
| `ibv` | A BV adjustment matched to the diffusion current predicts an onset displaced toward zero by roughly NBV × Vt × ln(target/leakage) for well-separated scales. The smallest IBV values test clamping when leakage and target become comparable. Compare the intercept, slope and knee independently. |
| `isr` | If recombination participates in matching the breakdown branch, the onset or intercept should move with ISR. A change confined to the pre-avalanche floor separates recombination leakage from avalanche normalization. |
| `nbv` | A pure slope change scales the semilog inverse slope with NBV and preserves the current at −BV. A fitted intercept that moves with NBV also implicates the voltage used to match the branches. |

### Measured reverse current, knee and slope

Subtract the shunt from the saved source current, `I = i(Vbias) + GMIN × Vd`.
The table gives I at −3 V and the measured leakage at −1.8 V. The inverse
slope is the voltage change per natural-log change in I from −3.04 to −3.02 V.
Extrapolating that line to the total saturation current gives the positive
knee magnitude B. The transition is resolved on a 2 mV grid.

| Varied quantity | Setting | Before at −BV (A) | 44.2 at −BV (A) | B (V) | Slope (mV) | Leakage (fA) | Revised max error¹ (%) |
|---|---:|---:|---:|---:|---:|---:|---:|
| AREA | `1` | 0.001 | 0.0011 | 2.21386 | 31.0379 | 11 | 0.000859 |
| AREA | `2` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| AREA | `4` | 0.004 | 0.001025 | 2.25689 | 31.0379 | 41 | 0.000811 |
| IBV | `10f` | 2.00226e-11 | 2.1e-14 | 3.00000 | 31.0379 | 21 | 0.000044 |
| IBV | `1p` | 2.00226e-11 | 2.1e-14 | 3.00000 | 31.0379 | 21 | 0.000044 |
| IBV | `1n` | 2e-09 | 1.047858e-09 | 2.66424 | 31.0379 | 21 | 0.000360 |
| IBV | `10u` | 2e-05 | 1.05e-05 | 2.37831 | 31.0379 | 21 | 0.000678 |
| IBV | `1m` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| IS | `1e-15` | 0.002 | 0.0015 | 2.16390 | 31.0379 | 3 | 0.000914 |
| IS | `1e-14` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| IS | `1e-13` | 0.002 | 0.001005 | 2.30684 | 31.0379 | 201 | 0.000757 |
| ISR | `0` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| ISR | `5p` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| ISR | `50p` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| NBV | `1` | 0.002 | 0.00105 | 2.36281 | 25.8649 | 21 | 0.000835 |
| NBV | `1.2` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| NBV | `1.5` | 0.002 | 0.00105 | 2.04422 | 38.7974 | 21 | 0.000835 |
| PJ | `0` | 0.002 | 0.001 | 2.23537 | 31.0379 | 20 | 0.000835 |
| PJ | `5u` | 0.002 | 0.001025 | 2.23537 | 31.0379 | 20.5 | 0.000835 |
| PJ | `10u` | 0.002 | 0.00105 | 2.23537 | 31.0379 | 21 | 0.000835 |
| PJ | `20u` | 0.002 | 0.0011 | 2.23537 | 31.0379 | 22 | 0.000835 |

¹ Largest pointwise relative error in source current across all 621 samples,
including shunt current. The revised model agrees within 0.00092% over all
13,041 points. The pre-avalanche floor tracks bottom plus sidewall saturation
current with the small reciprocal-cubic correction of diode equation 7.1.
Changing ISR by a factor of ten leaves the whole reference sweep unchanged.
The loaded CI deck also resolves ISR current at −50 mV and the diffusion-only
branch at −100 mV, bracketing the model's −3 N Vt boundary.

For the common card the bottom and sidewall saturation currents are 20 fA
and 1 fA. The high-current intercept follows unscaled IBV times their
total-to-bottom ratio. AREA, PJ and IS vary that ratio independently.
The transition and the 1 nA residual identify the correction at lower targets.
Write `Ibottom = AREA × IS`, `Itotal = Ibottom + PJ × JSW`,
`s = NBV × Vt` and B for the positive knee magnitude. In avalanche the
reverse current is `Itotal × exp((−Vd − B)/s)`. The observed matching rule
for one copy is

```text
IBV ≤ Ibottom × BV/(N Vt)    B = BV
otherwise                   IBV/Ibottom = exp((BV − B)/s) − 1 + B/(N Vt)
```

The linear term is consistent with the first-order diffusion-current
approximation described in the manual. With the common card the clamp
threshold is 2.32 pA, accounting for the equal 10 fA and 1 pA traces.
At 1 nA a simple total/bottom prefactor would give 1.050000 nA at −BV.
Including the knee correction gives the measured 1.047858 nA. Varying NBV
moves the knee and sets the slope while preserving the high-current intercept.
All six probe families use positive IS, N = 1 and one copy. The multiplier
scales parallel copies in Sansho, and IS = 0 retains the previous nominal
area-scaled target because the bottom-current match is undefined there. [ngspice manual, §7.2.2](https://ngspice.sourceforge.io/docs/ngspice-manual.pdf#page=139)

## R8b epitaxial collector and URC

All 23 exact circuits in the five R8b families have ngspice 44.2 raw files. Each comparison below uses the saved circuit for that setting. All 23 pass the ordinary verifier tolerances.

### Uniform line measurements

In `r8_urc_fmax`, compare complex `v(a)` with `v(e1)` through `v(e7)` in the **same** 1 GHz sample. Only FMAX changes between runs. The estimate uses the line totals R = 200 Ω and C = 60 pF, with K = 1.6.

| FMAX (GHz) | 0.10 | 0.70 | 0.85 | 1.00 | 1.30 | 1.50 | 1.70 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Logarithmic estimate | 0.125 | 4.265 | 4.678 | 5.024 | 5.582 | 5.886 | 6.153 |
| Matching explicit N | 3 | 4 | 4 | 5 | 5 | 5 | 6 |

The low-frequency sample fixes the lower bound. The two samples between 5.5 and 6 distinguish truncation from nearest-integer rounding. In `r8_urc_resistance`, the end access resistance is N·L·RSPERL. Moving one tap toward the centre divides resistance by K. For N = 1, 2, 4, 5 and 8, the end values are 0.6, 1.2, 2.4, 3.0 and 4.8 Ω. Across all 35 samples from 10 MHz to 1 GHz, this ladder reproduces both complex port observables within 1.7e−13 V and 2.2e−15 A. [Ladder normalization and automatic count](../../docs/distributed-lines.md#uniform-rc-ladder)

### Epitaxial current and reverse junction area

On the VO and GAMMA cards the collector and base sources directly measure terminal currents. Let Ic = −i(Vc) and Ib = −i(Vb). With unity base charge, negligible shunt current, BF = 150 and BR = 3, the forward and reverse diffusion currents satisfy `If = (Ic + 4 Ib)/(1 + 4/150)` and `Ir = 3 (Ib − If/150)`. In saturation, recover the intrinsic collector voltage as `Vi = Vb − Vt log1p(Ir/(IS·Abc))`, where Abc is the base–collector junction area. The ideal contacts in these two sweeps give Vx = Vc, Vb = 0.8 V and Abc = 1.

At Vc = 0.25 V and VO = 2 V, write d for Vx − Vi and u for the integrated conductivity-modulation voltage. The following currents are in mA. Both calculated columns use (d + u)/RCO as the low-field current.

| GAMMA | d (V) | u (V) | Measured Ic | Divide by 1 + d/VO | Divide by 1 + (d + u)/VO |
|---:|---:|---:|---:|---:|---:|
| 0 | 0.23814 | 0 | 4.2560 | 4.2560 | 4.2560 |
| 1e−13 | 0.23795 | 0.00942 | 4.4214 | 4.4214 | 4.4028 |
| 1e−11 | 0.23302 | 0.23345 | 8.3557 | 8.3557 | 7.5649 |
| 1e−9 | 0.18634 | 1.12555 | 24.0015 | 24.0015 | 15.8446 |

The independent VO sweep checks 0.5, 2, 5 and 100,000 V. Across the eight fixed-base runs of 61 points each, the implemented model’s largest absolute deviations are 0.285 µA on collector current and 0.073 µA on base current.

For the loaded RCO sweeps first subtract the measured RC and RB drops from the external collector and base voltages. At 3 V drive the measured collector currents are 48.827, 39.658 and 24.219 mA for RCO = 3, 20 and 50 Ω. Reconstructing Vi using Abc = AREA = 1 gives 89.751, 43.710 and 25.258 mA from the branch law. With Abc = AREAB = 2 the law gives 48.831, 39.658 and 24.219 mA. Across each full drive sweep the worst verifier scores are 0.0594, 0.0731 and 0.0414 respectively. The 50 Ω run supplies the reference for `r8_bjt_quasisaturation`. [Current and charge equations](../../docs/r8-semiconductors.md#bipolar-junction-transistor-and-diode)

### Diode breakdown under .temp

Each probe drives two otherwise identical diodes at 1 mA reverse current. Both have BV = 6.2 V, IBV = 1 mA and model TNOM = 20 °C. Subtracting `v(control)` from `v(test)` cancels the common series drop.

In `r8_diode_tbv_temp.cir`, only the test diode specifies TBV1 and TBV2. The maintainer’s ngspice 44.2 runs gave exactly equal node voltages at −20, 20, 60 and 100 °C. Each log in `verify/out/probes/r8_diode_tbv_temp_*.log` reports both parameters as unrecognized and ignored.

In `r8_diode_tcv_temp.cir`, only the test diode specifies TCV = 0.0007 V/K. At 100 °C the maintainer measured −6.148482324 V at the test node and −6.204485200 V at the control. The remaining TCV sweep points are predictions awaiting measurement.

| Temperature (°C) | 44.2 TCV shift (mV) | Sansho knee prediction (mV) |
|---:|---:|---:|
| −20 | Pending | −28.000000 |
| 20 | Pending | 0 |
| 60 | Pending | +28.000016 |
| 100 | +56.002876 | +56.002876 |

The prediction uses `BV(T) = BV − TCV·(T − TNOM)`. At 100 °C, solving the avalanche exponential at exactly 1 mA and including the series drop gives −6.148294617 V and −6.204297493 V. Both are about 0.188 mV above the respective 44.2 operating points. Their difference agrees to better than 1e−6 relative. The 2.876 µV beyond the 56 mV change in BV comes from matching the knee at the adjusted breakdown voltage.

To repeat the sweeps, run

```sh
verify/probes/sweep.sh verify/probes/r8_diode_{tbv,tcv}_temp.cir
```

## R9b RF and power MOSFET

Run the original one-parameter experiments with the existing 44.2 sweep driver. Each file supplies its own values. Keep the ASCII raws and logs, including model parameter warnings.

```sh
verify/probes/sweep.sh verify/probes/r9_*.cir
```

| Probe suffix after `r9_` | Measurement |
|---|---|
| `sp_noise_temp` | A matched pad with available gain 1/4 gives NF = 6.0206 dB at every setting if source normalization follows circuit temperature. A fixed reference temperature gives a temperature-dependent NF. |
| `vdmos_subshift` | Vary SUBSHIFT alone at nominal temperature. Invert the saturation current and the 80 mV softplus to recover the intercept on both sides of threshold. |
| `vdmos_phi` | Vary PHI alone at a fixed 55 °C. Compare hot gate transfers to resolve any independent threshold translation. |
| `vdmos_rq` | Sweep drain voltage at two gates, with finite RS and three RQ values. Subtract I·RS from Vgs and invert the triode law for intrinsic Vds. The measured drain access drop distinguishes external Vds, external drain to intrinsic source, intrinsic Vds and gate-controlled alternatives. |
| `vdmos_rq_temp` | Paired devices with RQ=0.6 Ω and RQ=0 at four temperatures. Extract KP and the gate intercept from the control. Infer the RQ increment in triode and test the 0.3 temperature exponent. |
| `vdmos_rthjc`, `vdmos_cthjc` | A 1 W heat-current pulse at the junction of an electrically idle MOSFET isolates the thermal network. Vary one thermal parameter per deck. A second device dissipates approximately 1 W internally with MU=TCVTH=0. Compare its measured −2·I(Vd) and junction response to the external heat pulse. The plateau gives K/W and the cooling tail gives the pole. |
| `vdmos_case` | With zero device power, step the case by 5 V from three starting voltages. Initial junction offset, instantaneous feedthrough, relaxation and case current distinguish a junction-to-case capacitor from one to thermal ground and reveal an independent boundary path. |
| `vdmos_tj`, `vdmos_ambient` | Compare a device with clamped thermal pins to an isothermal control over the gate transfer. Vary only the junction voltage or only ambient temperature. The slope and intercept of √Id in saturation separate mobility and threshold shifts and test whether the exposed junction voltage is a rise above ambient. |
| `vdmos_rs_heat` | Clamp both thermal pins at zero and sweep the gate at fixed drain voltage. Vary only RS. Compare incremental junction clamp current to terminal power and to terminal power less I²RS, subtracting the zero-gate baseline. |

### Measured on ngspice 44.2

All 37 cached R9b sweep points pass the ordinary verifier. The largest score is 0.825 on the gate-edge drain current of the electrically heated step fixture. The filename `vdmos_cthjc` is retained to match the regenerated cache. Its model card sweeps **CTHJ**. The maintainer observed a warning for the older spelling CTHJC.

| Sweep | Observation and inference |
|---|---|
| RTHJC | At CTHJ = 100 µJ/K, fitting the idle device's cooling tail from 3.1 to 3.5 ms gives 200.0, 600.0 and 1000.0 µs for 1, 3 and 5 K/W. Subtract the initial junction voltage before taking the log ratio. At 1 K/W the 1 W source raises the junction by 1.000 K. The separate electrical heater draws 0.500005 A at 2 V and gives the same rise within 0.001 K near the plateau. |
| CTHJ | At RTHJC = 3 K/W the same fit gives 120.0, 600.0 and 2400.0 µs for 20, 100 and 400 µJ/K. The effective junction storage in the transient port equation is 2·CTHJ. The earlier ignored-CTHJC run gave 99.98 µs at 5 K/W, consistent with an omitted CTHJ of 10 µJ/K. |
| Case | At ambient 27 °C, the three idle junctions start 0.135 K above their clamped cases. Case clamp currents at 0, 10 and 27 V are +27, +17 and 0 mW. A 5 V, 1 µs case ramp changes the junction by only 0.002524 V during the ramp, followed by exponential relaxation. These measurements put the storage from junction to thermal ground and imply a 1 mW/K case shunt. |
| Junction and ambient | At zero case voltage, case clamp current is Vtj/5. Subtracting electrical heat and adding Vtj/5 to the junction clamp current leaves 0, 27, 55 and 90 mW at the four ambient settings. The ambient drive enters the junction. At 27 °C ambient the heated/control current ratios at Vtj = 0, 10, 27 and 55 V are 1, 0.906358, 0.772279 and 0.603642. At Vtj = 27 V the ratios at 0, 27, 55 and 90 °C are 0.753683, 0.772279, 0.788827 and 0.806424. The whole gate transfer scales by `(1 + Vtj/Tamb_K)^−3` for MU = −1.5, TCVTH = 0. |
| Source access heat | At Vg = 4 V, Vd = 2 V and RD = 0.08 Ω, subtracting the idle clamp current gives 4.624636, 4.004519 and 2.826227 W for RS = 0, 0.05 and 0.2 Ω. Each agrees with `Id·Vds − Id²·RS` within 11 nW. The drain access loss contributes to forward channel heating. |
| RQ | With RS = 0.2 Ω, invert the triode law after subtracting I·RS from the gate drive. At external Vds = 3 V, Vg = 5 V and RQ = 0.9 Ω, Id is 2.525830 A. The inferred field voltage is 2.999983 V while the external-drain to intrinsic-source drop is 2.494834 V. Both gate sweeps identify the external drain–source voltage magnitude as the field variable. |
| RQ temperature | At Vg = 5 V the RQ = 0 control gives KP from its known RD and intrinsic Vds. Inverting the RQ = 0.6 Ω branch then gives drift multipliers 0.972118, 1.000000, 1.027118 and 1.058825 at 0, 27, 55 and 90 °C. These follow `(T/TNOM)^0.3`. |
| SUBSHIFT and PHI | Inverting the saturation current and 80 mV softplus gives intercepts 1.960, 2.000 and 2.040 V at nominal temperature for SUBSHIFT = −40, 0 and +40 mV. The 55 °C transfer deck gives 2.012 V with SUBSHIFT = +40 mV and TCVTH = 1 mV/K. All 77 hot drain currents are identical across PHI = 0.45, 0.65 and 0.85 V. |
| RF noise | At 0, 16.85, 27 and 80 °C, NF and NFmin are 6.020599913 dB, SOpt is zero to rounding, and Rn is 46.875 Ω. Cy₁₁ rises from 5.028322 to 6.501014 × 10⁻²² A²/Hz. The available source noise is k·Tamb per hertz and the physical Norton spectrum scales with absolute temperature. |

The thermal acceptance deck scores 0.001 on V(tj), 0.729 on V(d) and 0.327 on I(Vcc) over 443 points after the hot RD correction. Maximum junction error is 0.348 mK. The worst electrical error occurs at the falling gate edge.

### R9d thermal probe results

All 12 follow-up sweeps pass against the maintainer’s ngspice 44.2 raws. The scores below are the largest normalized errors over all saved signals, biases and three parameter values for each probe. The pass limit is 1.

| Probe | Prediction and measurement | Worst score |
|---|---|---:|
| `vdmos_rthca` | With 0.25 W injected at 40 °C, the predicted open-case voltage is 40 + 0.25·RTHCA. The measured voltages are 65, 140 and 290 V at 100, 400 and 1000 K/W. At a 10 V case clamp the corresponding junction voltages are 13.25, 11.75 and 11.45 V and the clamp currents are +0.550, +0.325 and +0.280 A. Both the ambient junction drive and the case shunt scale with 1/RTHCA. | 1.45×10⁻¹¹ |
| `vdmos_implicit_case` | The maintainer’s syntax check ruled out the predicted three-node implicit boundary. THERMAL needs explicit junction and case terminals. The revised deck compares an open case with node 0. At RTHCA = 1000 K/W and Vg = 2.5 V their junction voltages are 147.930 and 2.094 V, and drain currents are 120.194 and 391.765 mA. At the first bias the open junction is 27.135 V. Starting the coupled solve at the actual body bias removes the spurious 587 V starting solution. | 0.156 |
| `vdmos_hot_channel` | At a 30 K rise the saturation and triode branches confirm the predicted KP factor `(330.15/300.15)^(2·MU)` for MU = 0, −0.75 and −1.5 and the 30 mV threshold reduction for TCVTH = 1 mV/K. In the access branch, saturation fixes RS at 0.020 Ω. Inverting the triode current at Vg = 4 V gives RD = 0.092289 Ω for all three MU values. The previous 0.080 Ω prediction missed up to 20.02 mA. The corrected RD factor `(Tj/Tamb)^1.5` fits this rise, with a maximum current error of 11.1 nA across the sweeps. | 2.91×10⁻⁶ |
| `vdmos_diode_heat` | At Vd = −0.8 V, subtracting the idle 0.027 W from the junction clamp gives 21.666397, 0.410366 and 0.109424 W for RB = 0, 0.2 and 1 Ω. These agree with the predicted full terminal power within 3.2 µW at that bias. Excluding I²RB would give 0.357743 and 0.090716 W for the two finite resistances. | 0.0463 |
