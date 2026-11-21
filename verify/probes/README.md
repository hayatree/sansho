# Compact-model probes for ngspice 44.2

Each original deck runs at its printed setting with a single analysis card. From the repository root, run the sweeps with a local ngspice 44.2 executable. The driver varies the one value listed on each deck's `* sweep:` line. It writes the exact circuit, ASCII raw file and log for every setting under `verify/out/probes/`.

The R13c sweep lines name the parameter before its settings. The driver
substitutes each numeric setting into `.param probe=`. Older decks list
the settings alone. For decks ending in a control-block `quit`, the log
contains the requested printed records. Each setting has a 300-second timeout. Set `SANSHO_PROBE_TIMEOUT` to a positive integer in seconds to override it. On timeout the driver logs `PROBE-TIMEOUT`, kills the process group, forcibly removes the named container when using the repository wrapper, and continues. The final summary lists failed settings and the sweep exits with failure.

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

## R12 digital histories

The maintainer ran eight original decks at 16 parameter settings with ngspice 44.2. Replaying the expanded `verify/out/probes/r12_*.cir` decks in Sansho produced 98 histories, including both transients in the counter probe. All 404 entries match in node order, level and strength. Of these, 400 match in time to 1e−18 s. The four exceptions belong to a single clock edge. Times below are in ns.

For example, replay `sansho run -j 1 verify/out/probes/r12_counter_grid_0.2n.cir` and compare each node with its “Time or Step” table in the adjacent log. All 16 expanded decks replay directly. For the 13 runs ending in `showmod`, Sansho prints the model cards with their explicit parameters. The 44.2 listing reports “No matching models”. The comparison uses the preceding event and voltage records.

| Probe | 44.2 observations and Sansho replay |
|---|---|
| `r12_bridges` | On the rising ramp, 0.07 V still gives low, 0.15 V gives unknown, 0.85 V is still unknown and 0.95 V gives high. The default window in Sansho is 0.1–0.9 V. The instantaneous edges at 3.15, 3.95, 44.15 and 44.95 precede the default ADC by exactly 1 ns. With both thresholds at 1.5 V the comparator stays low throughout the ascending equality plateau, rises at 14.01 and falls upon reaching equality at 33. The default DAC uses 0, 0.5 and 1 V and slews at ±1 V/ns. The −1/+3 V DAC continues from −0.84 V at 4.95 and +2.60 V at 45.95, with slopes +0.2 and −0.5 V/ns. The piecewise ramps fit all 549 reference and 560 Sansho printed voltage rows within 1e−13 V. The custom DAC never settles at its unknown target in this probe. |
| `r12_buffer` | The 0.5 ns source width gives a 0.70 ns digital pulse (3.125–3.825). Both the fixed-delay buffer and inverter pass both edges at 5.125 and 5.825. With 2 ns rise and 0.25 ns fall, the scheduled return at 4.075 overtakes the rise at 5.125 and the output stays low. For the 3 ns width, the same asymmetric output rises at 5.125 and falls at 6.575. Both sweeps match. |
| `r12_bus` | The pulls give `0r` and `1r`. The biased bus progresses through `1r → 0s → Us → 1s → 1r` with changes at 4.65, 10.65, 16.85 and 22.85. The isolated low driver releases to `0z`. The default tristate changes at 5.15 and 17.35, exactly 1 ns after enable changes. All seven histories match, including the strength-only release. |
| `r12_storage` | The default DFF, TFF and closed D latch start at `0s`. A DFF and closed latch with IC 0, 1 and 2 start low, high and unknown respectively. Complementary outputs invert definite levels and preserve unknown. A transparent D latch with high data starts high for all ICs. A self-fed buffer settles low. The clock rises at 4.15 and the responding storage outputs change at 6.15. The enabled SR latch goes high → unknown → high at 0, 6.15 and 9.35. All 42 histories match. |
| `r12_load` | At both 1 and 100 pF, the ADC emits unknown/high/unknown/low at 5.035, 5.225, 12.825 and 13.000. Light and heavy branches reproduce every transition 1 ns later. The inverter complements definite values at those same times, and the NAND and cascaded buffers add another 1 ns. All 14 histories match. This measures propagation under load. The listing supplies no numeric default input capacitance. |
| `r12_divider` | With modulo four and high widths of one and three clocks, the initial (narrow, wide) pairs for counts 0–3 are (0,0), (1,1), (0,1), (0,1). Count advances on rising input. Output is high at counts 1 through `high_cycles`, with 1 ns propagation. From count zero both first rise at 4.15, then fall at 10.15 and 22.15 respectively. All four sweeps match through 55 ns. |
| `r12_osc` | At 100 MHz both oscillators start at `0u`. The first strong high occurs at 7.5 for duty 0.25 and 2.5 for duty 0.75. Both fall at 10 and repeat every 10. Omitted propagation delays and explicit zeros give identical histories in both simulators. |
| `r12_counter_grid` | On the coarse grid ngspice samples 0.25 and 0.75 V at 34.05 and 34.15. Sansho samples 0.30 and 0.70 V at 34.06 and 34.14. The resulting (clock, Q0, carry, Q1) transitions are (34.25, 34.65, 34.65, 35.05) versus (34.24, 34.64, 34.64, 35.04). At 20 ps maximum step all transitions agree, including this quartet at (34.21, 34.61, 34.61, 35.01). |

### Counter timestep and TRTOL

The 44.2 logs announce `Reducing trtol to 1 for xspice 'A' devices`. Both counter runs report zero transient event timestep backups. The counter contains resistors, ideal voltage drives and event storage. Its analog equations have no dynamic charge for the truncation-error test. Adding `.options trtol=1` and `.options trtol=7` in separate Sansho replays yields byte-identical event histories and printed analog samples (276 output lines each). The measured 10 ps comes from the first accepted sample above threshold after the 34 ns source corner. A 5 V/ns DAC slew converts it to the observed 50 mV peak error.

The available offline documentation did not establish an automatic A-device TRTOL override. Sansho keeps the specified tolerance. The coarse counter is a known numerical difference at the original verification threshold. Its regression bounds that edge to the measured 10 ps and checks the other output edges against the coarse reference.

### R12d boundary measurements

Run `sansho run -j 1 verify/out/probes/r12c_boundaries_0.1.cir` and compare with the adjacent ngspice 44.2 log. All 22 entries across the four histories agree in level and strength, with event times within 1e−18 s.

| Node | Successive states | Times in both simulators (ns) |
|---|---|---|
| `d`, zero-delay ADC | 0s → Us → 1s → Us → 0s | 0, 5.01, 16, 22.15, 22.90 |
| `w`, asymmetric ADC | 0s → Us → 1s → Us → 0s | 0, 7.01, 18, 22.40, 23.15 |
| `q`, asymmetric buffer on `d` | 0s → Us → 1s → Us → 0s | 0, 7.01, 18, 22.40, 23.15 |
| `osc`, 100 MHz, duty 0.25 | 0u → 1s → 0s → 1s → 0s → 1s → 0s | 0, 7.5, 10, 17.5, 20, 27.5, 30 |

The default ADC stays low on the 0.1 V plateau, reports unknown at 0.101 and 0.899 V, and reaches high at 0.9 V. Relative to `d`, both `w` and `q` take the 2 ns rise delay for low → unknown and unknown → high. Both take the 0.25 ns fall delay for high → unknown and unknown → low. Before the correction, Sansho’s mean-delay rule put the first unknown at 6.135 ns and suppressed the second. The oscillator drives at the phase crossings even with the same explicit delay pair. Sansho previously added those delays and placed the first two edges at 9.5 and 10.25 ns.

With DAC rails of −1 and +3 V and 1 ns full-swing times, the output reaches the implicit +1 V midpoint at 5.51 ns and holds it through 16 ns. Linearly interpolating Sansho’s 419 analog samples onto all 355 reference times gives maximum absolute differences below 6e−15 V for the input and 5e−14 V for the DAC output.

## R13c XSPICE residuals

Run the five self-contained probes from the repository root in the oracle environment.
Keep each expanded circuit and its adjacent log or raw.

```sh
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/r13_*.cir
```

| Probe | What to measure in 44.2 |
|---|---|
| `r13_genlut_unknown` | The same input drives the first pin, the second pin or both pins on three LUTs. Subtract each input event time from the corresponding output event time. Only the first pin delay varies (0.1, 0.5, 0.9 ns), crossing the second pin's fixed 0.4 ns. This separates earliest, latest and fixed-pin selection for simultaneous unknowns. Low→U and high→U have distinct output delays of 1 and 0.2 ns. The additional tristate tables expose direct releases and changes between U and Z. |
| `r13_ram_write` | Compare the bare output symbol with a copy driving a weak pulldown. At high data, a released output leaves the pulled bus at `0r` and a strong output drives it to `1s`. Data changes twice during one selected write. Compare the delays from data, write enable and select to the output across the three read-delay settings. |
| `r13_real_gain` | Compare `value`, `scaled`, `held` and the two analog plateaus while varying only the final bridge gain (0, 0.5, 1, 2). The expected real levels are (0.25, 1.25), then (0.5, 2.5), then the sampled pair after the rising clocks. The gain-zero case distinguishes a bridge that ignores gain from excess gain upstream. |
| `r13_real_transition` | A single 0→1→0 real signal drives a unity-gain bridge. At fixed maximum step 0.05 ns compare time to settle and the sample-to-sample error ratios for transition times 0.2, 0.4 and 0.8 ns. A linear ramp reaches its rail after one transition time. A repeated fractional update leaves a tail with error factor `1 − Δt / transition_time`. Use the actual recorded Δt. |
| `r13_zener_resistance` | Directly impose reverse voltage and read `−i(Vrev)`, including at 4.7 V. Remove the parallel leakage before taking log-current slopes. At 10 mA the predicted e-fold voltages are 15, 30, 60 and 1000 mV. The wide knee makes the low-bias continuation measurable. |

The batch-deck measurements and the gain difference are recorded in the
[XSPICE verification notes](../../docs/xspice.md#r13c-residuals).

## R13d probe results

These measurements compare the maintainer’s 17 ngspice 44.2 runs of the five
R13c decks with Sansho as of R13d. The digital times below are in ns. Replay an expanded circuit with
`sansho run -j 1 verify/out/probes/<stem>_<setting>.cir`. For a zener DC sweep,
add `-r <output.raw>` and compare the source current at each bias.

| Probe | 44.2 measurements and Sansho replay |
|---|---|
| `r13_genlut_unknown` | Input `a` changes at 3.075, 7.075, 11.075 and 15.075. Subtract the pin delay from the output lag on `p` or `q` to get 1, 1, 0.2 and 0.2. For the tied pins `r`, the low → Uu lags are 1.1, 1.5 and 1.9 across the sweep. The Uu → high lags are 1.4, 1.5 and 1.9. This identifies the first unknown pin and the maximum delay for recovery to a known address. The direct high → Z → high path gives `1s → Uz → 1s` with output delays 0.2 and 1. The path through an unknown gives `1s → Uu → 0z → Uu → 1s` with output delays 0.2, 0.4, 0.6 and 1. Sansho matches all 93 symbols and 90 times to 1e−18 s. The three Uu → 0z times are each 0.2 ns late. |
| `r13_ram_write` | Relative to 6.075, 11.075, 18.075, 22.075, 26.075 and 30.075, the bare output emits `1z, 0z, 1z, 1s, Uz, 1s` exactly 0.25, 1 or 2 ns later according to `read_delay`. The pulled bus resolves to `0r, 0r, 0r, 1s, 0r, 1s`. Stored data tracks both changes during a selected write. All resolved state changes agree. Sansho coalesces the repeated `0r` records. Interpolating both DAC outputs onto the reference rows gives a maximum difference below 3.4 × 10⁻¹⁴ V. |
| `r13_real_gain` | In all four settings `value` changes from 0.25 to 1.25 at 9.825. The reference `scaled` history is 0 at startup, 0.5 at 0.8 and 2.5 at 10.225. Both held values arrive at 4.575 and 14.575. At 4.625 the outputs for gains 0, 0.5, 1 and 2 are 0, 0.03125, 0.0625 and 0.125 V. At 14.625 they are 0.5, 0.625, 0.75 and 1 V. At each time the gain-zero trace gives the previous internal state and gain one gives the current state. Interpolating between or extrapolating from those two gives the other two gains even on the shortened clock-corner steps. All four approach 0.5 and 2.5 V. Replay agrees with all 1,992 printed analog rows within 5.1 × 10⁻⁷ V. Sansho starts `scaled` at 0.5. The subsequent data and clocked events match. |
| `r13_real_transition` | The input rises at 2.375 and falls at 6.375. At 2.425 the unity-gain outputs are 0.25, 0.125 and 0.0625 V for transition times of 0.2, 0.4 and 0.8 ns. In a 0.05 ns step the remaining error is multiplied by 0.75, 0.875 or 0.9375. Using the actual step in `1 − Δt/T` also predicts the corner samples. For example, with T = 0.8 ns the 6.000 → 6.005 ns step takes 0.9907073 to 0.9907653 V. Both polarities agree over all 660 printed analog rows within 5.1 × 10⁻⁸ V. |
| `r13_zener_resistance` | Each raw contains 1,941 forced biases from −0.1 to 4.75 V. The shunt conductance is 42.55319 nS. After removing the shunt, use the ratio of the successive 25 mV current differences ending at 4.725 and 4.750 V to recover e-fold voltages of 15, 30, 60 and 1000 mV. The corresponding currents at 4.7 V after shunt subtraction are 10.000000, 10.000000, 9.991557 and 9.999969 mA. For 100 Ω the total reverse-oriented currents at −0.1, 0, +0.5 and +4.7 V are −9.183058 µA, approximately 0, 59.565376 µA and 10.000169 mA. They identify the forward-side tangent and a reverse exponential normalized at both zero and the rating. Maximum differences across the four full replays are 0.201 nA, 0.259 nA, 19.43 µA and 33.10 nA. At 6 Ω the measured high-current factor of 0.999156 agrees with evaluating the exponential by its tangent from the preceding 2.5 mV bias, `(1+h/S) exp(−h/S)`. The device uses the analytic curve. |

Running `sansho-verify --cached verify/r13-decks` gives ten passes, zero known
differences and zero skips. The worst scores by deck are 0.487 (hyst), 0.422
(transformer), 0.064 (zener) and below 0.001 for genlut, ilimit, multi_pwl,
oneshot, ram, real and state. The real deck's largest absolute error across all
288 reference samples is 3.27 × 10⁻¹¹ V or less.
[Model equations and scope](../../docs/xspice.md#r13d-measured-laws)

### R13d follow-up sweep settings

```sh
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/r13d_*.cir
```

| Probe | Measurement that distinguishes the remaining choices |
|---|---|
| `r13d_genlut_release` | Sweep the fall delay through 0.1, 0.2 and 0.5 ns. Parallel LUTs independently double the rise delay or the static pin delay. Subtract the changing input's fixed 0.1 ns from Uu → Z and Z → Uu lags. The low-start and direct-release controls also test which bare level accompanies Z. |
| `r13d_real_startup` | Sweep the amplifier delay through 0.1, 0.4 and 0.7 ns. Compare zero-delay and 0.3 ns converters, and amplifiers with zero and −1 IC. Measure the initial value and first non-IC publication time on each node to separate converter startup, amplifier delay and IC behavior. |
| `r13d_real_short` | With a nonzero initial real input, compare unity and double gain for T = 0, 0.025, 0.05 and 0.1 ns. The 0.05 ns grid crosses Δt/T = 1. Measure the DC value, first increment, possible overshoot and subsequent state on both edges to distinguish clamping from extrapolation and identify the zero-time limit. |

## R13e probe results

The maintainer supplied ten ngspice 44.2 logs for these three sweeps. Replay
each existing expanded circuit with
`sansho run -j 1 verify/out/probes/r13d_<probe>_<setting>.cir`. All 150
event records match in value (including digital strength) and in time to
1e−18 s. All ten Sansho probe replays produce byte-identical CLI output with
one and four threads. Replaying the earlier `r13_genlut_unknown` and `r13_real_gain`
sweeps also reproduces all 93 and 60 event records respectively, including
the three previously late releases and the amplifier startup publications.
The cached verifier passes all ten R13 batch decks at their original
tolerances, with zero known differences and zero skips. The largest
normalized score is 0.487 on `r13_hyst`. The maximum `r13_real` voltage
error is 3.262 × 10⁻¹¹ V over 288 reference points.

The following times are in ns.

| Probe | Measured result |
|---|---|
| `r13d_genlut_release` | Subtract the changing pin's 0.1 ns delay from each lag. With fall F = 0.1, 0.2, 0.5, the high-start Uu → 0z lags then measure 0.2, 0.4, 1.0 and the 0z → Uu lags measure 0.3, 0.6, 1.5. Doubling the rise delay changes only the final return to high. Doubling the static pin delay changes none of the times. Starting low gives 1 + F for release and 1 + 2F for the return to Uu. The direct high → Z → high control emits `Uz` with output delays F and R. All 93 records agree. |
| `r13d_real_startup` | Both converters publish 0.25 at zero. The amplifier outputs start at 0, 0 and −1, then all reach 0.5 at 0.2, 0.8 or 1.4 as amplifier delay D takes 0.1, 0.4 or 0.7. Later the fast converter publishes 1.25 at 4.0625, the slow one at 4.3625, and their amplifiers publish 2.5 one D later. All 45 records agree. Interpolating Sansho's analog output onto all 882 printed times gives maximum error below 5 × 10⁻⁷ V. |
| `r13d_real_short` | All four real histories give 0.5, 1.5 and 0.5 at 0, 2.375 and 3.375. The DC voltages at gains one and two are 0.5 and 1 V. At 2.425, the pair is (1.5, 3) for T = 0, 0.025 and 0.05, and (1, 1.5) for T = 0.1. At 2.475 the T = 0.1 pair is (1.25, 1.5). At 3.425 the T = 0.05 pair is (0.5, −0.5), followed by (0.5, 1) at 3.475. The negative sample is consistent with applying double gain to the increment from 1.5 down to 0.5. |

The short-bridge analog comparison below uses linear interpolation at the
140 printed reference times per run. The count in parentheses is the number
of rows differing by more than 2 µV, above the observed print rounding.

| T (ns) | Maximum unity error in V (rows) | Maximum double-gain error in V (rows) |
|---|---:|---:|
| 0 | 1 (1) | 2 (1) |
| 0.025 | 0 (0) | 1.5 (4) |
| 0.05 | 0 (0) | 1.5 (54) |
| 0.1 | ≤ 5.1 × 10⁻⁷ (0) | 0.5 (2) |

**Open items.** For positive T, the reference keeps the DC double-gain
output of 1 V at 0.5 and 1 ps, then gives 0.5 V at 2 ps. All runs have
the same initial step sequence. At nominal Δt = T, the printed times cannot
establish which side of equality the actual interval occupies. Sansho uses
direct gain at exact equality. Finally, the zero-time bridge applies the
rising event in its 2.375 ns reference sample but still reports the old
value in its 3.375 ns sample. Sansho publishes both edges at the real
event. The positive-T 0.025 ns trace also has one discrepant event sample
at 2.375 ns. The logs do not determine the ordering at finer precision.
[Model laws](../../docs/xspice.md#r13e-measured-laws)

## R14 classical models

The maintainer supplied 390 ASCII raw files plus logs, including truncated runs. The tables identify the one-variable experiments. Use the R14b commands below to regenerate the failed settings and collect the remaining measurements. [Equations and interpretation](../../docs/r14-models.md)

### Parker–Skellern junction FET

| JFET2 | What the sweep distinguishes |
|---|---|
| [p](r14_jfet2_p.cir), [q](r14_jfet2_q.cir) | Linear-region and saturation power dependence. Lower XI makes the Q limit measurable. |
| [xi](r14_jfet2_xi.cir), [z](r14_jfet2_z.cir) | Saturation scale and rounding of the knee in the drain-bias families. |
| [vst](r14_jfet2_vst.cir), [mvst](r14_jfet2_mvst.cir) | Semilog transfer slope below cutoff and its drain-bias dependence. |
| [lfgam](r14_jfet2_lfgam.cir), [lfg1](r14_jfet2_lfg1.cir), [lfg2](r14_jfet2_lfg2.cir) | Low-frequency threshold feedback and its separate dependence on the two end voltages. |
| [hfgam](r14_jfet2_hfgam.cir), [hfg1](r14_jfet2_hfg1.cir), [hfg2](r14_jfet2_hfg2.cir) | Drain-excited admittance across the gate relaxation pole. The three runs isolate high-frequency opposite-end feedback and each bias sensitivity. |
| [hfeta](r14_jfet2_hfeta.cir), [hfe1](r14_jfet2_hfe1.cir), [hfe2](r14_jfet2_hfe2.cir) | Gate-excited admittance across the same pole. Inspect amplitude and sign of the instantaneous gate-feedback terms. |
| [taug](r14_jfet2_taug.cir), [taud](r14_jfet2_taud.cir) | Pole frequency for gate feedback at fixed heating, and for heating at fixed gate feedback. |
| [delta](r14_jfet2_delta.cir) | Channel-current reduction with intrinsic dissipation at several gate and drain biases. |
| [cgs](r14_jfet2_cgs.cir), [cgd](r14_jfet2_cgd.cir) | AC source and drain currents with two unequal junction biases. These distinguish the physical assignment of the gate capacitances. |
| [acgam](r14_jfet2_acgam.cir), [xc](r14_jfet2_xc.cir) | Cross-terminal gate-charge coupling and loss of capacitance at a pinched-off end. |
| [vbi](r14_jfet2_vbi.cir), [fc](r14_jfet2_fc.cir) | Junction charge curvature at unequal biases and the onset of its forward continuation. |
| [cdss](r14_jfet2_cdss.cir) | Ignored-parameter control under drain excitation. |
| [lg](r14_jfet2_lg.cir), [ld](r14_jfet2_ld.cir), [ls](r14_jfet2_ls.cir) | Flat lead controls. The logs flag these inductances and RG as ignored. |
| [ibd](r14_jfet2_ibd.cir), [vbd](r14_jfet2_vbd.cir), [n](r14_jfet2_n.cir) | Reverse gate knee magnitude and position, then forward gate ideality from the semilog slope. |
| [afac](r14_jfet2_afac.cir), [nfing](r14_jfet2_nfing.cir) | Ignored-parameter controls at fixed electrical bias. |
| [polarity](r14_jfet2_polarity.cir) | Mirrored NJF and PJF transfer with equal signed VTO settings. |
| [model_temp](r14_jfet2_model_temp.cir), [instance_temp](r14_jfet2_instance_temp.cir), [tnom](r14_jfet2_tnom.cir) | Model and instance TEMP are Celsius. TNOM sets the junction reference temperature. |
| [kf](r14_jfet2_kf.cir), [af](r14_jfet2_af.cir) | Flicker amplitude and current exponent with a noiseless transimpedance readout. |

### MOS9 compared with MOS3

| MOS9 | What the sweep distinguishes |
|---|---|
| [xl](r14_mos9_xl.cir), [xw](r14_mos9_xw.cir), [wd](r14_mos9_wd.cir) | Length and width mask adjustments and the diffusion-width correction. Compare the two sense currents at each drain and gate bias. |
| [delvto](r14_mos9_delvto.cir), [delvto_p](r14_mos9_delvto_p.cir) | Threshold offset sign and magnitude on NMOS and PMOS. |
| [kappa](r14_mos9_kappa.cir), [vmax](r14_mos9_vmax.cir) | Post-knee length modulation and velocity-limited saturation. The comparison spans the knee. |
| [eta](r14_mos9_eta.cir), [delta](r14_mos9_delta.cir) | Drain-dependent threshold shift and the narrow-width correction. |
| [theta](r14_mos9_theta.cir), [xj](r14_mos9_xj.cir) | Transverse mobility reduction and depletion sharing with finite substrate doping. |
| [nfs](r14_mos9_nfs.cir) | Weak-inversion gate family around and below the source threshold. |
| [xd](r14_mos9_xd.cir), [alpha](r14_mos9_alpha.cir), [input_delta](r14_mos9_input_delta.cir) | 44.2 rejected each original card. The replacement positive controls sweep NSUB, TOX and DELTA respectively. |

### Extracted parameters common to early BSIM

| BSIM1 | BSIM2 | What the sweep distinguishes |
|---|---|---|
| [lvfb](r14_bsim1_lvfb.cir), [wvfb](r14_bsim1_wvfb.cir) | [lvfb](r14_bsim2_lvfb.cir), [wvfb](r14_bsim2_wvfb.cir) | Flat-band shift from inverse length or width. Each deck measures two devices with a twofold change of that single dimension. |
| [dl](r14_bsim1_dl.cir), [dw](r14_bsim1_dw.cir) | [dl](r14_bsim2_dl.cir), [dw](r14_bsim2_dw.cir) | Effective-dimension reduction in micrometres at fixed drawn geometry. |
| [vfb_p](r14_bsim1_vfb_p.cir) | [vfb_p](r14_bsim2_vfb_p.cir) | Mirrored PMOS/NMOS transfer as the signed flat-band coefficient changes. |
| [k2](r14_bsim1_k2.cir) | [k2](r14_bsim2_k2.cir) | Threshold shift from the second body-effect term under fixed reverse body bias. |
| [tox](r14_bsim1_tox.cir) | [tox](r14_bsim2_tox.cir) | Oxide-thickness units and transconductance scaling across weak and strong inversion. |
| [vdd](r14_bsim1_vdd.cir) | [vdd](r14_bsim2_vdd.cir) | Mobility interpolation reference voltage with different low- and high-field mobilities. |
| [n0](r14_bsim1_n0.cir), [nb](r14_bsim1_nb.cir), [nd](r14_bsim1_nd.cir) | [n0](r14_bsim2_n0.cir), [nb](r14_bsim2_nb.cir), [nd](r14_bsim2_nd.cir) | Weak-inversion intercept slope, body dependence and drain dependence. Compare semilog gate transfers. |
| [temp](r14_bsim1_temp.cir), [junction_temp](r14_bsim1_junction_temp.cir) | [temp](r14_bsim2_temp.cir), [junction_temp](r14_bsim2_junction_temp.cir) | Extraction temperature at fixed operating temperature, then circuit temperature with a forward body-junction sweep. |
| [mult](r14_bsim1_mult.cir) | [mult](r14_bsim2_mult.cir) | Drain-current magnitude for parallel copies with unchanged drawn dimensions. |
| [xpart](r14_bsim1_xpart.cir) | [xpart](r14_bsim2_xpart.cir) | Real and imaginary gate, drain, source and bulk currents at saturation. Drain/source displacement partition distinguishes 40/60 from 0/100. |
| [pbsw](r14_bsim1_pbsw.cir) | [pbsw](r14_bsim2_pbsw.cir) | Reverse-biased sidewall capacitance with explicit perimeter and drain-only AC drive. |
| [wdf](r14_bsim1_wdf.cir), [dell](r14_bsim1_dell.cir) | [wdf](r14_bsim2_wdf.cir), [dell](r14_bsim2_dell.cir) | Default diffusion geometry with areas and perimeters omitted. Both parameters warn in Sansho. A changing bulk admittance would reveal an implicit geometry rule. |
| [kf](r14_bsim1_kf.cir), [af](r14_bsim1_af.cir) | [kf](r14_bsim2_kf.cir), [af](r14_bsim2_af.cir) | Flicker level and current-power exponent through a noiseless 1 kΩ transimpedance. |

### BSIM1 bias fits

| BSIM1 | What the sweep distinguishes |
|---|---|
| [mus](r14_bsim1_mus.cir), [x3ms](r14_bsim1_x3ms.cir) | High-drain mobility and its endpoint slope. Inspect output conductance on either side of the measurement voltage. |
| [x2mz](r14_bsim1_x2mz.cir), [x2ms](r14_bsim1_x2ms.cir) | Body sensitivity of the zero-drain and high-drain mobilities. |
| [u1](r14_bsim1_u1.cir), [x2u1](r14_bsim1_x2u1.cir), [x3u1](r14_bsim1_x3u1.cir) | Strength of velocity reduction, its body correction and its drain correction. |
| [x2u0](r14_bsim1_x2u0.cir) | Body correction to transverse-field mobility degradation. |
| [x2e](r14_bsim1_x2e.cir), [x3e](r14_bsim1_x3e.cir) | Body and drain sensitivities of the barrier-lowering coefficient. |

### BSIM2 bias fits

| BSIM2 | What the sweep distinguishes |
|---|---|
| [mus0](r14_bsim2_mus0.cir), [mu0b](r14_bsim2_mu0b.cir), [musb](r14_bsim2_musb.cir) | High-field mobility and separate body slopes at the two extraction endpoints. |
| [mu20](r14_bsim2_mu20.cir), [mu2b](r14_bsim2_mu2b.cir), [mu2g](r14_bsim2_mu2g.cir) | Saturating mobility term at fixed unequal mobility endpoints. Vary the base, body and gate coefficients independently. |
| [mu30](r14_bsim2_mu30.cir), [mu3b](r14_bsim2_mu3b.cir), [mu3g](r14_bsim2_mu3g.cir) | Linear drain-field mobility coefficient and its body and gate dependences. The nonzero tanh shape separates the linear slope from the VDD endpoint constraint. |
| [mu40](r14_bsim2_mu40.cir), [mu4b](r14_bsim2_mu4b.cir), [mu4g](r14_bsim2_mu4g.cir) | Curvature of the drain-field interpolation and its body and gate dependence. |
| [uab](r14_bsim2_uab.cir), [ub0](r14_bsim2_ub0.cir), [ubb](r14_bsim2_ubb.cir) | Transverse-field degradation. Compare the linear body correction and the quadratic gate coefficient and its body correction. |
| [u1b](r14_bsim2_u1b.cir), [u1d](r14_bsim2_u1d.cir) | Velocity-limiting field sensitivity to body and drain voltages. |
| [etab](r14_bsim2_etab.cir) | Body correction to drain-induced threshold lowering. |
| [vof0](r14_bsim2_vof0.cir), [vofb](r14_bsim2_vofb.cir), [vofd](r14_bsim2_vofd.cir) | Weak-inversion offset, its body slope and its drain slope. Inspect transfer shifts below the lower spline endpoint. |
| [vglow](r14_bsim2_vglow.cir), [vghigh](r14_bsim2_vghigh.cir) | Position and continuity of the two joins in the transition from diffusion to drift current. |
| [ai0](r14_bsim2_ai0.cir), [aib](r14_bsim2_aib.cir), [bi0](r14_bsim2_bi0.cir), [bib](r14_bsim2_bib.cir) | Above-knee hot-electron enhancement, its exponential slope and both body sensitivities. |
| [vgg](r14_bsim2_vgg.cir), [vbb](r14_bsim2_vbb.cir) | Gate and bulk clipping at twice the signed extraction voltage. |

### Vertical power MOSFET

| VDMOS | What the sweep distinguishes |
|---|---|
| [texp0_ambient](r14_vdmos_texp0_ambient.cir), [texp0_junction](r14_vdmos_texp0_junction.cir) | RD-only device at elevated ambient temperature or clamped junction rise. The ambient control resolves the explicit RD exponent. The clamped control is flat in 44.2. |
| [texp1_ambient](r14_vdmos_texp1_ambient.cir), [texp1_junction](r14_vdmos_texp1_junction.cir) | RQ-only device with finite VQ. RD = 0 removes the branch. All settings coincide in 44.2. |

### Control output

| Control | What the sweep distinguishes |
|---|---|
| [numdgt](r14_control_numdgt.cir) | Scalar measurement, echo, complex print columns, print line and complex wrdata fields at positive and negative precision. Examine the log and the per-setting .control/fields.dat side by side. |

### R14b reference runs

From the repository root, run the eight repaired probes and the new isolating probes with an installed ngspice 44.2 or the reference wrapper. The driver saves a deck and log per setting, plus a raw or control text file. It reports failed settings together after the last run. Keep the logs, including the four NUMDGT logs, and the corresponding `*.control/fields.dat` files.

```bash
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh \
  verify/probes/r14_bsim{1,2}_junction_temp.cir \
  verify/probes/r14_bsim2_u1b.cir verify/probes/r14_control_numdgt.cir \
  verify/probes/r14_mos9_{alpha,input_delta,kappa,xd}.cir \
  verify/probes/r14b_*.cir
cargo run --release -q -p sansho-verify --features jit -- verify/decks/r14_bsim1_inverter.cir
```

| R14b probe stems (all begin with r14b_) | Quantity to extract |
|---|---|
| bsim1_rail | Near-zero drain-bias slope and diffusion plateau versus MUS at fixed strong gate drive. |
| bsim1_sparse, bsim2_sparse | Drain sweep at three gate biases with VDD omitted, to locate the mobility endpoint. |
| bsim1_gate_charge, bsim2_gate_charge | Gate-driven terminal capacitances across cutoff, weak inversion and saturation. |
| bsim2_triode_charge | Drain-excited terminal capacitances at low drain bias across inversion. |
| bsim1_noise_bias, bsim2_noise_bias, bsim1_white_bias, bsim2_white_bias | Bias sensitivity of the 1 Hz intercept and, with KF = 0, the white floor, using the same noiseless transimpedance as the AF sweeps. |
| bsim2_inverter_trap, bsim2_inverter_gear | Peak and trapezoidal mean of supply current across input ramp endpoints at three maximum timesteps. |
| jfet2_id | Inspect the model log for a descriptive text ID on an otherwise supported card. |
| jfet2_xc_bias | XC dependence as the gate approaches cutoff. |
| vdmos_rd_hot, vdmos_rthjc | Clamped RD-only exponents with explicit RTHJC, then mixed resistance with variable RTHJC. |
| vdmos_texp0_ambient, vdmos_texp0_junction, vdmos_texp1_ambient, vdmos_texp1_junction | Vary each explicit exponent independently with both RD and RQ present, under ambient or junction heating. |

Both original BSIM junction probes used an unsupported instance TEMP. The replacements sweep .temp. The original BSIM2 U1B sweep crossed a negative velocity coefficient with omitted VBB. The replacement sets VBB = −1 V and uses smaller signed coefficients. The three rejected MOS9 inputs have separate elaboration tests. Their replacement decks retain the original filenames. KAPPA = 0 parsed but stopped after three samples, so the revised deck starts at 0.05. The control deck sets the Nutmeg variable directly.

### R14c results

All eight repaired probes and the R14b settings have been examined. Across the current R14 and R14b templates, 457 of 484 completed waveform settings pass. The four NUMDGT settings reproduce the numeric listings and wrdata bytes. The three bare `ID=PF1` settings stop at parameter substitution with undefined PF1 on line 8. No waveform is expected for these three runs. The 27 waveform differences comprise 24 early-BSIM noise settings and the BSIM2 25 ps trap, 25 ps Gear and 5 ps Gear inverters. The 1 ps trap and Gear runs both pass. [Measured laws and acceptance deck status](../../docs/r14-models.md)

## R15 native VBIC

Each template changes one numeric input. Use supply currents for the DC and AC comparisons. For AC, retain both components of the complex current. The thermal probes record junction **rise** in kelvin. The noise readout uses the current through the base-bias source and an ideal 1 kΩ transimpedance. Keep the logs for the unknown-key and EBBE controls. All 189 settings across the 64 templates ran with Sansho. R15b compares all 189 settings against the cached 44.2 outputs below. [Equations, defaults and open choices](../../docs/vbic.md)

```sh
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/r15_vbic_*.cir
```

The driver continues through failed settings and reports them together after the final deck. For the seven acceptance waveforms, the verifier accepts `verify/decks/r15_vbic_*.cir` as positional inputs.

| Probe | Measurement |
|---|---|
| [aje](r15_vbic_aje.cir) | Imaginary drive current at the forward continuation of the B–E junction. |
| [ajc](r15_vbic_ajc.cir) | Imaginary drive current at the forward continuation of the B–C junction. |
| [ajs](r15_vbic_ajs.cir) | Imaginary drive current at the forward continuation of the substrate junction. |
| [vrt](r15_vbic_vrt.cir) | Collector capacitance past the reverse reach-through voltage. |
| [rci](r15_vbic_rci.cir) | Collector current versus voltage at five base currents, varying RCI. |
| [gamm](r15_vbic_gamm.cir) | Collector current versus voltage at five base currents, varying GAMM. |
| [vo](r15_vbic_vo.cir) | Collector current versus voltage at five base currents, varying VO. |
| [hrcf](r15_vbic_hrcf.cir) | Collector current versus voltage at five base currents, varying HRCF. |
| [vef](r15_vbic_vef.cir) | Forward and saturated current versus VEF at several injection levels. |
| [ver](r15_vbic_ver.cir) | Forward and saturated current versus VER at several injection levels. |
| [qbm](r15_vbic_qbm.cir) | Forward and saturated current versus QBM at several injection levels. |
| [nkf](r15_vbic_nkf.cir) | Forward and saturated current versus NKF at several injection levels. |
| [avc1](r15_vbic_avc1.cir) | Base-current reversal and collector multiplication as reverse bias increases. |
| [avc2](r15_vbic_avc2.cir) | Base-current reversal and collector multiplication as reverse bias increases. |
| [vbbe](r15_vbic_vbbe.cir) | Emitter reverse voltage at the specified breakdown current. |
| [wsp](r15_vbic_wsp.cir) | Substrate and base currents across distinct intrinsic and pocket junction biases. |
| [rbp](r15_vbic_rbp.cir) | Substrate and base currents across distinct intrinsic and pocket junction biases. |
| [ikp](r15_vbic_ikp.cir) | Substrate and base currents across distinct intrinsic and pocket junction biases. |
| [wbe](r15_vbic_wbe.cir) | Complex short-circuit base and collector currents versus frequency for WBE. |
| [td](r15_vbic_td.cir) | Complex short-circuit base and collector currents versus frequency for TD. |
| [tf](r15_vbic_tf.cir) | Complex short-circuit base and collector currents versus frequency for TF. |
| [qtf](r15_vbic_qtf.cir) | Complex short-circuit base and collector currents versus frequency for QTF. |
| [xtf](r15_vbic_xtf.cir) | Complex short-circuit base and collector currents versus frequency for XTF. |
| [qco](r15_vbic_qco.cir) | Complex short-circuit base and collector currents versus frequency for QCO. |
| [xis](r15_vbic_xis.cir) | Collector and base currents over temperature, isolating XIS. |
| [xii](r15_vbic_xii.cir) | Collector and base currents over temperature, isolating XII. |
| [xin](r15_vbic_xin.cir) | Collector and base currents over temperature, isolating XIN. |
| [ea](r15_vbic_ea.cir) | Collector and base currents over temperature, isolating EA. |
| [eaie](r15_vbic_eaie.cir) | Collector and base currents over temperature, isolating EAIE. |
| [eaic](r15_vbic_eaic.cir) | Collector and base currents over temperature, isolating EAIC. |
| [tnf](r15_vbic_tnf.cir) | Collector and base currents over temperature, isolating TNF. |
| [tavc](r15_vbic_tavc.cir) | Collector and base currents over temperature, isolating TAVC. |
| [xre](r15_vbic_xre.cir) | Collector and base currents over temperature, isolating XRE. |
| [xrbi](r15_vbic_xrbi.cir) | Collector and base currents over temperature, isolating XRBI. |
| [xrc](r15_vbic_xrc.cir) | Collector and base currents over temperature, isolating XRC. |
| [rth](r15_vbic_rth.cir) | Junction-rise amplitude and decay constant after a power pulse. |
| [cth](r15_vbic_cth.cir) | Junction-rise amplitude and decay constant after a power pulse. |
| [kfn](r15_vbic_kfn.cir) | Base-current noise through a noiseless 1 kΩ transimpedance, varying KFN. |
| [afn](r15_vbic_afn.cir) | Base-current noise through a noiseless 1 kΩ transimpedance, varying AFN. |
| [bfn](r15_vbic_bfn.cir) | Base-current noise through a noiseless 1 kΩ transimpedance, varying BFN. |
| [rbx](r15_vbic_rbx.cir) | Base-current noise through a noiseless 1 kΩ transimpedance, varying RBX. |
| [area](r15_vbic_area.cir) | Electrical currents and junction heating with AREA applied to the complete device. |
| [m](r15_vbic_m.cir) | Electrical currents and junction heating with M applied to the complete device. |
| [unknownfit](r15_vbic_unknownfit.cir) | Log warning versus rejection for an unknown numeric model key. |
| [junction_temp](r15_vbic_junction_temp.cir) | Reverse and near-zero bias capacitance at three junction temperatures. |
| [shot_bias](r15_vbic_shot_bias.cir) | White base noise level versus independently calculable diode current. |
| [isrr](r15_vbic_isrr.cir) | Reverse transport normalization in saturation. |
| [td_step](r15_vbic_td_step.cir) | Step shape and group delay with static transport and no diffusion capacitance. |
| [tr](r15_vbic_tr.cir) | Reverse transit charge, measured at forward B–C bias. |
| [selft](r15_vbic_selft.cir) | On/off behavior of the fifth pin with a finite external DC path. |
| [implicit_rth](r15_vbic_implicit_rth.cir) | Base and collector currents with SELFT enabled and the thermal pin omitted. |
| [cap_partition](r15_vbic_cap_partition.cir) | Base impedance across its spreading-resistance pole at three depletion partitions. |
| [default_is](r15_vbic_default_is.cir) | Compare the collector currents of a default card and an explicit IS card. |
| [ebbe](r15_vbic_ebbe.cir) | Log and reverse current for an explicit breakdown-offset input. |
| [noise_area](r15_vbic_noise_area.cir) | Flicker and white levels versus AREA with AFN unequal to one. |
| [noise_m](r15_vbic_noise_m.cir) | Flicker and white levels versus M with AFN unequal to one. |
| [reverse_epi](r15_vbic_reverse_epi.cir) | Epitaxial knee and current direction across the collector to emitter reversal. |
| [temp_precedence](r15_vbic_temp_precedence.cir) | Whether instance DTEMP changes the current with instance TEMP and model DTEMP set. |
| [model_dtemp](r15_vbic_model_dtemp.cir) | Whether model DTEMP adds to a fixed instance TEMP. |
| [xrcx](r15_vbic_xrcx.cir) | Extrinsic collector exponent with fixed shared and intrinsic exponents. |
| [xisr](r15_vbic_xisr.cir) | Reverse saturation-current ratio when the B–C junction dominates transport. |
| [tvbbe1](r15_vbic_tvbbe1.cir) | Hot and cold breakdown current around the nominal emitter knee. |
| [noise_rci](r15_vbic_noise_rci.cir) | White collector-noise floor in quasi-saturation with variable epitaxial resistance. |
| [avalanche_noise](r15_vbic_avalanche_noise.cir) | White base-current noise as impact generation opposes forward base current. |

### R15b comparison

Build the native executable, then compare the cached per-setting decks, raw files and logs from the repository root.

```sh
cargo build --offline --release -p sansho-cli --features jit
python3 verify/compare_vbic.py
cargo run --offline --release -q -p sansho-verify --features jit -- --cached -v verify/decks/r15_*.cir
```

| Outcome | R15 | R15b |
|---|---:|---:|
| Complete waveform and model warning match | 124 | 187 |
| Complete reference with a waveform or warning mismatch | 63 | 0 |
| Partial reference, excluded from pass count | 2 | 2 |

The driver prints each signal's score, maximum absolute error, reference peak and number of reference samples. The score is max absolute error divided by (absolute floor + relative tolerance × reference peak). The DC and complex AC tolerance is 0.2%, with floors of 1 nA and 1 µV. Transient tolerances are 3% for currents and 1% for voltages after linear interpolation onto reference times. Noise uses 0.2% with a 1e−30 floor, for both spectral densities and both integrated results. Nested DC compares in sweep order. For the two-point linear AC probes, 44.2 saved only the first frequency. Only that common frequency enters the score. The driver also compares ignored-parameter warnings. Both UNKNOWNFIT settings warn, all EBBE settings are accepted silently, and none of the R15 settings was rejected at binding.

The 200 ps and 1 ns TD transient references aborted with “Timestep too small” at the internal Q1 branch. Their 65,848 and 22 recorded samples are scored up to their last time, with the partial-reference status retained. All seven acceptance decks pass. Five have tighter tolerances in R15b. [Laws, numerical evidence and remaining differences](../../docs/vbic.md#evidence-from-ngspice-442)

### R15b follow-up probes

```sh
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/r15b_vbic_*.cir
python3 verify/compare_vbic.py --pattern 'r15b_vbic_*'
```

The ten templates contain 34 settings. R15c passes all 32 completed references, including the four nonbinary QBM values and negative RCI that 44.2 accepted silently. The two positive-TD Gear references aborted and remain partial. Together with the earlier suite this gives 219 passes and four partial references. [R15c measurements and open items](../../docs/vbic.md#r15c-follow-up-measurements)

| Probe | Measurement to distinguish |
|---|---|
| [TD with Gear](r15b_vbic_td_gear.cir) | Starts from zero forward transport, uses a finite rise and a capped Gear timestep. Tests delay and settling after turn-on. |
| [Reverse-only shot noise](r15b_vbic_reverse_noise.cir) | At VBE=0, IBCI separates reverse transport noise from B–C diode noise. |
| [Reach-through smoothing](r15b_vbic_art.cir) | Three junction biases straddle −VRT with positive AJC. The ART sweep measures the reverse transition width. |
| [Pocket TR](r15b_vbic_pocket_tr.cir) | Suppresses intrinsic transport to isolate the parasitic charge with finite RBP and IKP. |
| [QTF base width](r15b_vbic_qtf_width.cir) | Nonunit Early width and high injection distinguish depletion width from total base charge in transit storage. |
| [QBM domain](r15b_vbic_qbm_domain.cir) | Tests rejection, warning, or branch selection at negative, fractional and above-unit QBM. |
| [RCI domain](r15b_vbic_rci_domain.cir) | Compares negative, zero and finite RCI to an omitted-resistance card on the same AC sweep. |
| [XRB](r15b_vbic_xrb.cir) | Isolates access, intrinsic and pocket resistance exponents on a single temperature sweep. |
| [External heating](r15b_vbic_external_heat.cir) | Provides a DC path outside the fifth pin to measure power injection at zero RTH. |
| [Zero HRCF](r15b_vbic_hrcf_zero.cir) | Measures the zero-selector branch at finite VO with strong reverse current. |

## R16 native HICUM/L2

Each `r16_hicum_*.cir` template varies one setting through its `* sweep:` line and `.param probe=` assignment. DC and AC probes save source currents so the oracle comparison uses external measurements. The thermal probes also record junction rise. The noise probes use a current-controlled voltage source to read the driving-source current. R16 uses the supplied 44.2 parameter listing for the nominal card. All 251 settings from 85 templates completed on Sansho. Five sweeps gave identical waveforms across their settings. Three exercise ignored card fields (FLCOMP, FLCONO, ZETACX), one changes an SOA warning threshold, and one varies RTH with the thermal pin omitted. The R16b results below use the subsequently supplied 44.2 waveforms.

Run the native matrix from the repository root. Each setting must return a nonempty, finite raw plot. Temporary per-setting decks and raws are removed after the run.

```sh
cargo build --offline --release -p sansho-cli --features jit
python3 verify/compare_vbic.py --native-only --pattern 'r16_hicum_*'
```

The maintainer collected the follow-up oracle plots with the sweep script. The comparison driver replays the matching expanded circuit, scores each real or complex signal, interpolates transient samples onto reference times, and reports missing and partial references separately from full passes.

```sh
NGSPICE=tools/ngspice/ngspice.sh verify/probes/sweep.sh verify/probes/r16_hicum_*.cir
python3 verify/compare_vbic.py --pattern 'r16_hicum_*'
```

| Measurement | Probe stems after `r16_hicum_` |
|---|---|
| Low-injection scale, common hole charge, heterojunction weighting and high-drive compression | c10, qp0, ich, mcf, depleted_width, hjei, hjci, ahjei, hf0, hfe, hfc |
| Internal, peripheral and external diodes, reverse emitter field, avalanche | recombination, peripheral, ibcx, tunode, abet, qavl, kavl, alfav |
| Base modulation, crowding, parallel base charge, leads and electrical scaling | rbi0, fcrbi, fqi, fdqr0, lead, area, m, gmin |
| Lateral substrate injection, SC diode, substrate storage and terminal impedance | itss, msf, sub_reverse, tsf, csu, vptsp |
| Forward capacitance ceiling, collector punch-through and overlap partition | ajei, forward_bc_cap, vptci, fbcpar, fbepar |
| Bias-dependent transit, emitter charge, collector storage and lateral spreading | tbvl, dt0h, tef0, thcs, fthc, latb, tbhrec, tr |
| Critical current, low-field smoothing, reach-through and barrier suppression | vlim, vces, aick, delck, vpt, barrier, hot_knee |
| Time-domain NQS response and separate current/charge phase shifts | flnqs, alit, alqf |
| Heat sources, thermal port activation, time constant and area dependence | flsh, cth, thermal_omitted, external_heat, thermal_area, alrth, thermal_exponent |
| Temperature precedence, gap law, gain, access resistance, transit time and unresolved card controls | temp, gap, region_gap, junction_temp, f1vg, alb, zetare, alt0, zetacx, flcomp |
| White and flicker spectra, placement, equilibrium and reverse injection, geometry and copies | shot, substrate_shot, transport_shot, avalanche_noise, thermal_noise, kf, cfbe, kfre, noise_area, noise_m, flcono |
| Acceptance of the collector-substrate SOA rating and warning threshold | soa |

All seven acceptance decks now have 44.2 references. [R16 equations and evidence](../../docs/hicum.md)

## R16b black-box fit

The original cache comparison passed 1/7 acceptance decks and 68/251 probe settings. Before the follow-up oracle runs, R16b passed **6/7 acceptance decks** and **241 probe settings**. Of the other settings, three had completed matching reference decks (VPTSP=1 and the two nonzero avalanche-noise gains), six cached decks predated the template repair, and one new QAVL setting lacked a reference. All 251 revised R16 settings and the 36 follow-up settings run on Sansho. The cached PNP acceptance current follows a reverse-BC tangent across the sweep. The PNP probes below distinguish that observation from a general polarity convention. [Equations and measured limits](../../docs/hicum.md)

The original `temp` logs reject instance DTEMP. The revised circuit varies TEMP alone. All three original `kavl` settings and `qavl=0` have zero completed rows and an operating-point failure at 0.7 V. The repaired KAVL sweep specifies a finite 10 fC avalanche onset charge and 10 fF collector capacitance. QAVL now uses 3, 10 and 30 fC. All nine repaired KAVL, QAVL and TEMP settings now have passing 44.2 comparisons. The comparison driver reports stale expanded circuits independently of waveform failures.

| R16b probe | Measurement |
|---|---|
| `r16b_pnp_pins` | Independently read currents in three-, four- and five-terminal PNPs at the same biases. |
| `r16b_pnp_bc` | Isolate the BC diode, including a zero-injection control and mirrored NPN drive. |
| `r16b_pnp_transport` | Disable BE and BC diodes and vary emitter resistance to distinguish reverse transport from junction current. |
| `r16b_pnp_load` | A passive base resistor gives an independent current from two saved voltages. Compare it with the source current at OP and during a sweep. |
| `r16b_gm_tangent` | Compare 1 Hz collector admittance with the centered 2 µV DC difference from two independently biased copies. |
| `r16b_sub_temperature` | Measure the lateral saturation derivative away from nominal temperature at three MSF settings. |
| `r16b_charge_delay` | Isolate differential transit time by varying TEF0 at fixed low-injection time. |
| `r16b_rbi_partition` | Reconstruct the internal base drop as a function of drive at three FDQR0 settings. |
| `r16b_heterojunction` | Test the depletion-width mapping independently of AHJEI by changing RHJEI. |
| `r16b_punch` | Measure both sides and the center of the substrate punch-through join. |
| `r16b_knee_temperature` | Save Ick and Tf alongside collector current to separate velocity limiting and emitter storage. |
| `r16b_avalanche_gain` | Isolate the avalanche part of base admittance by subtracting a matched zero-FAVL device. |

## R16c cached results

The combined comparison passes 276 of 287 settings. Ten completed PNP settings show the polarity difference documented with the isolated BC mirror. Their templates list the affected settings and signals on a `* known-difference:` line. Independent signals and the zero-injection control retain their ordinary tolerances. The 1 kΩ load is an oracle non-termination with an incomplete raw header. The driver still checks the native operating point and sweep for this setting. The seven acceptance decks give six passes and one known difference. [Model equations and quantitative PNP evidence](../../docs/hicum.md)

```sh
cargo build --offline --release -p sansho-cli --features jit
python3 verify/compare_vbic.py --pattern 'r16*'
cargo run --offline --release -q -p sansho-verify --features jit -- --cached verify/decks/r16_*.cir
```
