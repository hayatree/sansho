# Built-in BSIM4 for silicon MOSFETs

Provenance: Original implementation of the UC Berkeley BSIM Group's *BSIM4 4.8.2 Technical Manual*, chapters 1–15 and Appendix A, with the published 4.8.2 update notes. The reference waveforms come from the maintainer's ngspice 44.2 runs.

Use a four-terminal MOS instance in drain, gate, source, bulk order and a model of type `nmos` or `pmos` with `level=54`. The channel sign convention changes with the type. DC, AC and transient analyses share a quasi-static terminal-current and charge model. The implementation is in `crates/spx-devices/src/bsim4/`. It uses the existing fixed-size forward-mode dual numbers and the engine's ordinary device-group interface.

## Terminal evaluation and bypass

Chunks without internal drain, source, body or gate nodes store terminal indices
and model bindings separately. The IV and CV equations depend on voltage
differences. Forward AD uses drain, gate and bulk directions with source held
fixed. Translation invariance supplies the source column as the negative sum.
A wired source and bulk need only the drain and gate directions for an unlimited
intrinsic stamp. Junctions use one derivative per physical branch. The matrix
receives the combined four-terminal currents and charges. The LTE controller
also receives the four intrinsic, four overlap and four signed junction charges.

A prepared model retains the zero-body-bias electrostatics and its body derivative
after geometry, temperature and layout adjustments. The mobility-0 transport
specialization requires constant effective width, unit saturation lambda and
disabled Coulomb scattering, overshoot and ballistic corrections. Other settings
use the general transport equations. Exact charge sampling uses the electrostatic
and CV equations with zero AD lanes.

During Newton iteration the default `.options bypass=1` permits reuse when
all four terminal voltages are within `vntol` of the original linearization.
Currents and charges receive the first-order correction at the current iterate.
The combined stamp additionally requires unchanged `gmin`, completed limiting
and ordinary bias initialization. Cross-instance sharing requires identical
model bindings and voltage bits. `.options bypass=0` selects exact-bias reuse.
The converged charge sample always uses the actual voltages. The M5-3 benchmarks
measure the accuracy and work counts for both bypass settings. [Performance measurements](performance.md#m5-3-single-worker-bsim4)

## Equation coverage

| Manual section | Evaluation in spx |
|---|---|
| 1, dielectric and geometry | Electrical, physical and extraction oxide thicknesses, polysilicon depletion, per-finger effective IV, CV and junction dimensions. Silicon substrate and gate. |
| 2, threshold | Surface potential and depletion depth, derived or supplied body coefficients, lateral pocket doping, short- and narrow-channel effects, body bias, DIBL and drain-induced threshold shifts. |
| 3, channel charge | Subthreshold swing, effective overdrive, poly-corrected gate voltage and finite inversion-layer centroid. |
| 4.1–4.3, oxide tunneling | Accumulation and inversion gate-body currents with igbMod 1, gate-channel and edge currents with igcMod 1 and 2. Includes drain/source partition and the effective barrier thickness. The manual gives no separate gate-body equation for igbMod 2. |
| 5.1–5.8, channel transport | Bulk-charge factor, mobility modes 0–6, internal and external bias-dependent extension resistance, saturation velocity, saturation-root solution, effective drain bias, CLM, output DIBL, pocket DITS and SCBE. |
| 5.9, transport limits | Field-dependent overshoot with LAMBDA and a source injection velocity limit when VTL is supplied and positive. |
| 6, body current | Impact ionization and both GIDL/GISL selectors, with separate drain and source coefficients. |
| 7, capacitance | Intrinsic accumulation, depletion and inversion for capMod 0–2, cvchargeMod 0/1, charge-thickness correction, three XPART choices, overlap and fringing. Terminal charge sums to zero by construction. |
| 8, alternative materials | Selecting a nonzero MTRLMOD produces E-UNSUPPORTED during binding. |
| 9.1, non-quasi-static response | Selecting transient or AC NQS produces E-UNSUPPORTED during binding. |
| 9.2–9.3, RF | Gate resistance modes 0–3, the middle-gate overlap connection and a controlled channel-charging conductance. Body modes 0–2 include drain-side and source-side junction nodes. |
| 10, noise | The evaluated card retains the noise selectors and Appendix A.10 coefficients. Noise spectra require a future noise analysis. |
| 11, junctions | Bottom, isolation-sidewall and gate-edge leakage and charge, dioMod 0–2, avalanche and limiting currents, trap-assisted reverse current. |
| 12, layout | Supplied or derived drain/source area, perimeter and squares, finger count, gate contacts and electrode resistance. Shared/contact diffusion resistances use a rectangular approximation. |
| 13, temperature | Threshold, mobility, velocity, resistance, diode current and junction capacitance specialization for tempMod 0–3. Circuit TEMP and model TNOM are converted from °C to kelvin. |
| 14–15, stress and well proximity | Per-finger inverse isolation distances, reference-normalized mobility and velocity, threshold, K2 and ETA0 shifts. WPE uses supplied SCA, SCB and SCC with WPEMOD enabled. |
| Appendix A | The public `bsim4::catalog::PARAMETERS` table gives the card name, nominal value and unit, binning flag, model/instance scope and printed manual page. A derived default refers to its input parameters or equation. The table also lists dialect aliases. Material-model selectors, SC-only WPE and NQS are outside the evaluated scope. |

The numerical comparison suite exercises fewer combinations than this table. In particular, the high mobility selectors, velocity overshoot, trap-assisted leakage, all RF reductions and cvchargeMod 1 lack dedicated ngspice reference sweeps. The unit tests compare analytic AD derivatives with numerical perturbations independently of the reference decks.

## Binding, scale and the SKY130 dialect

- A used model expression sees the primitive instance's supplied numeric parameters, the enclosing subcircuit parameters and then globals. Model selection uses explicit drawn length and width. For level 54 the selection width is total width divided by NF. An exact model name takes precedence over a numbered family. Numbered bins use inclusive lower and exclusive upper bounds with the parser's relative boundary tolerance of 1e-9.
- Model expressions see the netlist values of L and W before geometric scaling. The final `.option scale` multiplies supplied MOS lengths, widths, perimeters and isolation distances. Area scales quadratically. Model dimensions such as TOXE and the bin bounds are already in physical units. The effective dimensions used with L, W and P bin coefficients include XL/XW and the IV geometry reductions. BINUNIT 1 uses micrometres, BINUNIT 2 uses metres.
- Parameter definitions can refer forward or form an unused cycle. A referenced unresolved name or cycle produces a diagnostic. Unselected model expressions can contain unresolved parameters. Structural errors such as malformed card headers or duplicate model names can still be reported while reading a library.
- Identical arguments to the same model definition can share evaluated values. A single named model with different live argument values can produce distinct bound cards. The flat netlist writer exports evaluated live cards. Unused symbolic templates can retain only their literal fields. The parser retains the version field, including three-part strings such as `4.8.2`. The version identifies the card. It does not select a historical simulator implementation.
- `gauss(nom,spread,sigma)`, `agauss(nom,spread,sigma)`, `unif(nom,spread)` and `aunif(nom,spread)` return their nominal value when the visible SKY130 statistical switches are zero. Zero spread is also deterministic. Active random sampling is unsupported. This rule covers nominal TT simulation.
- An undeclared M or MULT on an X line replicates its enclosed MOS, BJT, diode, R, L and C elements. Factors accumulate through nested calls. A multiplier declared by the subcircuit is an ordinary parameter available to its own formulas. This distinction matters on the SKY130 wrappers. A MOS primitive scales its current and charge by its own multiplier. Parallel copies multiply C and divide R and L.

## Numerical conventions and limitations

1. We take the smaller saturation root in rationalized form and use the rationalized smoothing quadratic for effective drain bias. This gives zero current at zero drain bias without subtracting nearly equal voltages. The channel equations use a local positive-drain frame. Physical drain/source junctions and edge currents retain their own parameters.
2. The electrostatic short-channel factor uses negative exponentials with its dimensionless argument bounded below by 1e-8. The softplus takes its asymptotes beyond ±35. The common junction exponential continues linearly above 80. These choices limit overflow and loss of precision during Newton iteration.
3. The body-bias transition uses 1 mV and the upper surface-potential limit is 0.95 PHI. The lower limit from K1/K2 lies between −30 and −3 V before applying VBM. Strong poly inversion limits the poly potential drop to 1.12 V with a 50 mV transition. The gate doping interval for poly depletion is 1e18 through 1e25 cm⁻³, excluding the upper end.
4. Denominators in the bulk-charge and mobility corrections, length corrections and gate factors have a dimensionless floor of 0.05. Swing has a floor of 0.1, saturation lambda a floor of 0.01, and bias-dependent width a floor of 0.1 nm. The discriminant floor is 1e-30. Inversion-centroid powers use positive field arguments. Floors have zero derivative on the clamped side.
5. Effective CV flat-band and drain bias use 20 mV smoothing. The flat-band radical uses the magnitude of the zero-bias flat-band voltage, with a 1e-30 V floor to define its derivative at zero. The accumulation centroid transition width is 1e-3 TOXE. The capMod 2 surface-potential transition uses 1 mV. Overdrive below 1e-25 V has zero partitioned inversion charge. Negative XPART suppresses intrinsic charge. XPART below, equal to and above 0.5 select 40/60, 50/50 and 0/100 saturation partition respectively. CKAPPA has a lower bound of 0.01 V.
6. Oxide tunneling uses 10 mV edge-voltage smoothing. The channel partition expressions use a dimensionless 1e-4 symmetric regularization, adding 2e-4 to their shared denominator. An omitted PIGCD uses the oxide-barrier approximation with a 1e-12 V overdrive floor. GIDL mode 1 limits the body-voltage denominator using GIDLCLAMP, with a minimum magnitude of 1e-12 V.
7. Junction charge integrates the reverse-bias depletion capacitance and continues the capacitance linearly in forward bias. Limiting currents use tangent continuations. We use ln(T/TNOM) for the junction saturation-density power law so the multiplier is one at nominal temperature. The printed logarithm in equations 13.39–45 is singular there. Temperature coefficients of junction capacitance are fractional per kelvin, consistent with Appendix A.13. Built-in junction potentials are bounded below by 10 mV. Trap-assisted current uses its forward asymptote within 1e-4 of the characteristic-voltage pole, measured as a fraction of that voltage.
8. Explicit resistive branches have a 1 µΩ series floor and a 1 mΩ body floor. The reduced body networks retain their node layout with GBMIN and low-resistance ties. Omitted diffusion geometry uses shared interior rectangles and isolated, shared or merged ends. Point contacts use a rectangular spreading-resistance estimate. An explicit zero perimeter or area overrides a derived value. With PERMOD=1, §12.2.1 subtracts the gate edge from a supplied total perimeter. The result can be negative when a wrapper supplies zero. The manual leaves this case unspecified. We use nonnegative physical lengths for leakage and reverse depletion. In forward bias the three terms of §11.2 give a single affine capacitance. We retain the signed perimeter in its intercept and slope, each bounded below by zero to keep the continuation passive. Charge is continuous and zero at zero junction bias. An undersized perimeter can give different one-sided capacitances there. Gate-edge leakage uses its own width even when bottom area and isolation perimeter are zero. Omitted NRD/NRS follow §12.2.2, which derives or disables diffusion resistance.
9. Stress sums evaluate the first 128 fingers individually, then integrate the reciprocal tail with endpoint and first derivative corrections. WPE needs layout moments. SC alone with active WPE produces E-UNSUPPORTED because the manual refers calculation of those moments to an external mask-distribution specification. Nonzero MTRLMOD also produces E-UNSUPPORTED. Parameter checks cover invalid selectors, positive dimensions and selected extreme dimensions or oxide thicknesses. A warning about an unused model coefficient identifies a coverage or inactive-branch limitation.
10. Newton iterations limit gate and drain steps and both junction biases. The junction limiter uses a minimum reference current of 1e-30 A solely to calculate its critical voltage. After a first-step UIC voltage jump over 0.1 V plus VNTOL, the integrator limits the follow-up step by the shortest positive local Cii/Gii among registered junction-body nodes that moved by more than VNTOL. These diagonal values are from the last Newton linearization and estimate the local RC relaxation time. The step stays between the timestep floor and the first interval. Without a responding RC node the existing one-sixteenth rule applies.
11. Every intrinsic terminal, overlap terminal and physical junction contributes its unassembled charge to transient error control, including ground-connected terminals. The integrator uses the same divided differences and tolerances as for the summed MNA rows, including internal nodes. Local histories advance only on acceptance. On startup it holds the first interval for a second sample before growth unless a UIC jump requires reduction. It clips the proposed interval at source corners. After a corner, one backward-Euler step supplies a right-sided current for trapezoidal integration. Until three accepted charge samples are available, error control can use the first-order estimate.

## Comparison with the cached reference

### Mobility stress

Equations 14.3–6 and §14.1.3 specify the ratio `(1 + rho) / (1 + rho_ref)`. Both rhos use KU0 divided by the same size and temperature factor. Only the actual inverse isolation distances are averaged over fingers. For `b4_n_lod_ku0` the inverse-distance sum is 2.948505 per µm, compared with 1.886792 per µm for the reference geometry. Its U0 multiplier is 1.039488. For `b4_n_lod_nf` the four-finger average gives 0.958365 as the U0 multiplier. The unit tests exercise the size factor, reference normalization, nonzero TKU0 and distinct mobility and velocity ratios.

All saved values in the 121-point KU0-only reference are identical to the zero-KU0/KVSAT control reference. With KU0 zero, equation 14.6 also gives unit velocity scaling in the control. As a separate experiment, suppressing only the U0 multiplier in spx produced scores of 0.220 for KU0, 0.237 for NF and 0.017 for the combined bin/layout deck. Velocity scaling, threshold shifts and the spacing average were active in that experiment. The maintainer selected equation 14.5 for spx. In a separate ngspice 44.2 black-box probe, Id was identical with KU0 omitted and with KU0 enabled at SA = SB = 0.3 µm and SAREF = SBREF = 1 µm. The verifier reports the KU0, NF and combined bin/layout decks as `KNOWN` with their measured scores. The cached traces establish the missing measurable mobility response, but do not identify its cause inside the reference run.

### Body-network transients

Figure 9.3 and §11.2 place each junction between its physical diffusion and the corresponding db or sb body node. Intrinsic bulk charge belongs to bNodePrime. The generic ramp test exposed an integration restart error on those nodes. At 300.1 ps both simulators reached −35.03 µV on db. At 300.3 ps B4-1's second backward-Euler step gave 203.55 µV against 330.17 µV in the reference. Returning to trapezoidal integration after the first right-sided derivative resolves that discrepancy. Holding the initial interval until a second startup sample is available also resolves the initial ramp errors.

The SKY130 inverter has a second effect. Its second-stage wrappers pass zero perimeter, and the output briefly swings outside the supply rails. Evaluating spx's discrete KCL on the saved voltages at 175 ps gave a 1.74 µA drain-body discrepancy on the second NMOS, with the opposite discrepancy at the output. The reverse-biased junction at 177 ps agreed to within 0.1 nA. Retaining the signed perimeter in the forward capacitance reduced the inverter score from 4.112 after the restart fixes to 0.190.

The earlier UIC version of the five-stage SKY130 ring begins with a 1.8 V supply imposed on a zero-initialized supply node under UIC. The first accepted samples at 20 fs agree within tolerance across all signals. At the next reference sample, 28.0002 fs, the fourth NMOS internal body is at 152.936 mV. The spx waveform interpolates to 125.740 mV there. On that body row the saved 20–28 fs interval has a 7.5 µA KCL residual using backward Euler and a 2.06 mA residual using trapezoidal integration. At the following sample, 44 fs, the residuals are 1.48 mA and 3.49 µA respectively. These measurements support a backward-Euler step followed by trapezoidal integration. In a separate refinement experiment we kept the same first accepted state and limited subsequent intervals through 200 fs to 0.25 fs and then 0.125 fs. The values were 124.205 and 124.185 mV. Refining this part of the spx trajectory does not reproduce the coarse reference sample. From 10 ps onward the worst score over all signals, using the full-deck normalization, is 0.367. The source of the reference's initial post-impulse step size remains unresolved.

#### Five-stage ring started at the .ic operating point

The current `sky_ring5` starts at a constrained operating point. Its 1,508 reference samples and the spx samples reach a 2 ps step after startup. The worst main node is D (score 0.277). Its 25 crossings of 0.9 V drift from a 1.4 fs spx lag at the first crossing to 80.6 fs at the last. The fourth NMOS body error grows from at most 0.97 µV over the first 150 samples to 10.57 µV over the last 150. At 2.827–2.837 ns the error alternates sign at every sample. This is characteristic of trapezoidal ringing on a fast body RC network.

To isolate the body equations, we replayed each saved trajectory on its own time grid. We held A–E at their saved voltages at each Newton solve, initialized the full state from sample zero, took two backward-Euler steps, then used trapezoidal integration with the converged spx charges. Replaying spx reproduced all 15 NMOS body traces within 0.01 nV. Replaying the reference gave these peak absolute errors for the fourth NMOS.

| Fourth NMOS node | Free ring (µV) | Reference-driven replay (µV) |
|---|---:|---:|
| body | 10.57 | 1.28 |
| sbody | 3.99 | 0.63 |
| dbody | 8.74 | 1.01 |

Across all five stages the largest reference-driven error was 1.32 µV, within the ordinary tolerance for every body trace. On the reference trajectory we also evaluated the fourth NMOS discrete KCL directly, using `f[n] + f[n−1] + 2(q[n]−q[n−1])/h` on trapezoidal steps. Peak residuals at body, sbody and dbody were 57.6, 0.0015 and 33.3 nA against peak charge terms of 18.64, 0.0484 and 35.98 µA. This checks the individual junction connections as well as the intrinsic body charge.

Halving the spx step from 2 to 1, 0.5 and 0.25 ps moved the last D crossing from +0.081 ps relative to the fixed reference to −1.079, −1.369 and −1.441 ps. Successive shifts decreased roughly fourfold. Refining spx alone therefore moves it away from this coarse reference. The terminal-driven replay and the passing NFET/PFET charge and generic RBODY controls localize the out-of-tolerance error to the free-running switching trajectory and its sampled body response. A small fixed-trajectory discrepancy remains. These measurements do not establish an error in the body conductance or junction charge partition. The free-running deck keeps its ordinary score and failure status.

### Cached deck results

Worst signal and maximum absolute difference use the ordinary verification harness. A score at or below 1 passes. All 30 cached decks ran. Three KU0 comparisons report a known difference, and one free-running transient exceeds tolerance.

| Deck | Worst signal | Score | Maximum absolute difference |
|---|---|---:|---:|
| `b4_cs_ac` | `v(d)` | 0.038 | 1.672e-4 V |
| `b4_inverter_tran` | `v(out)` | 0.015 | 1.930e-4 V |
| `b4_n_bin_layout` | `i(vd)` | **8.235** | 3.080e-5 A |
| `b4_n_capmod` | `i(vg)` | 0.036 | 2.744e-7 A |
| `b4_n_diode` | `i(vb)` | 0.487 | 1.022e-5 A |
| `b4_n_idvd` | `i(vd)` | 0.375 | 4.186e-7 A |
| `b4_n_idvg_lin` | `i(vd)` | 0.250 | 2.885e-8 A |
| `b4_n_idvg_sat` | `i(vd)` | 0.216 | 2.407e-7 A |
| `b4_n_igate` | `i(vd)` | 0.031 | 4.984e-8 A |
| `b4_n_lod_ku0` | `i(vd)` | **12.886** | 1.895e-5 A |
| `b4_n_lod_kvsat` | `i(vd)` | 0.220 | 3.233e-7 A |
| `b4_n_lod_kvth0` | `i(vd)` | 0.777 | 2.336e-6 A |
| `b4_n_lod_nf` | `i(vd)` | **14.234** | 4.383e-5 A |
| `b4_n_mobmod` | `i(vd)` | 0.135 | 3.309e-8 A |
| `b4_n_nf_geo` | `i(vd)` | 0.220 | 6.427e-7 A |
| `b4_n_rbody_tran` | `v(m1#dbody)` | 0.012 | 6.013e-8 V |
| `b4_n_rnet` | `i(vd)` | 0.144 | 3.927e-7 A |
| `b4_n_temp` | `i(vd)` | 0.064 | 4.194e-8 A |
| `b4_n_tempmod` | `i(vd)` | 0.041 | 7.214e-8 A |
| `b4_n_wpe` | `i(vd)` | 0.220 | 3.233e-7 A |
| `b4_p_iv` | `i(vd)` | 0.215 | 1.660e-8 A |
| `b4_ring5` | `v(a)` | 0.231 | 2.972e-3 V |
| `sky_cs_ac` | `v(m.xn.msky130_fd_pr__nfet_01v8#body)` | 0.180 | 4.161e-7 V |
| `sky_inverter_tran` | `v(m.xn2.msky130_fd_pr__nfet_01v8#body)` | 0.190 | 2.726e-6 V |
| `sky_nfet_charge` | `v(m.xm1.msky130_fd_pr__nfet_01v8#body)` | 0.020 | 7.073e-8 V |
| `sky_nfet_idvd` | `i(vd)` | 0.137 | 3.659e-7 A |
| `sky_nfet_iv` | `i(vd)` | 0.119 | 1.717e-8 A |
| `sky_pfet_charge` | `i(vb)` | 0.010 | 3.796e-9 A |
| `sky_pfet_iv` | `i(vs)` | 0.173 | 2.918e-7 A |
| `sky_ring5` | `v(m.x4.xn.msky130_fd_pr__nfet_01v8#body)` | **2.079** | 1.057e-5 V |

The original 14 decks and ten JIT Verilog-A decks passed and all 24 spx raw files matched their pre-change SHA-256 digests. The ten Verilog-A decks also passed in the interpreter. Two additional inputs in `verify/diag-decks` have cached maintainer references. The mobility control measures separate drain currents for two spacings on one model card alongside a device with the hand-calculated mobility. The close-spacing and explicit-mobility currents agree at every spx sweep point. The tight-tolerance ring keeps the 2 ps maximum timestep and the initial UIC jump.

In B4-1, median check / run times in seconds for three warm release CLI invocations were 0.154 / 0.167 for `sky_cs_ac`, 0.188 / 0.217 for `sky_inverter_tran`, 0.163 / 0.154 for `sky_nfet_idvd`, 0.164 / 0.158 for `sky_nfet_iv`, 0.158 / 0.209 for `sky_pfet_iv` and 0.169 / 0.220 for `sky_ring5`.

The B4-1 release microbenchmark measured 431.3 ns per intrinsic current, charge and both four-terminal Jacobians with default selectors. External network stamps and the matrix solve are excluded. The maintainer reports approximately 16 s for ngspice to load the SKY130 library.
