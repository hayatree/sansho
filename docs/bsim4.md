# Built-in BSIM4 for silicon MOSFETs

Provenance: Original implementation of the UC Berkeley BSIM Group's *BSIM4 4.8.2 Technical Manual*, chapters 1–15 and Appendix A, with the published 4.8.2 update notes. The reference waveforms come from the maintainer's ngspice 44.2 runs.

Use a four-terminal MOS instance in drain, gate, source, bulk order and a model of type `nmos` or `pmos` with `level=54`. The channel sign convention changes with the type. DC, AC and transient analyses share a quasi-static terminal-current and charge model. The implementation is in `crates/spx-devices/src/bsim4/`. It uses the existing fixed-size forward-mode dual numbers and the engine's ordinary device-group interface.

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
8. Explicit resistive branches have a 1 µΩ series floor and a 1 mΩ body floor. The reduced body networks retain their node layout with GBMIN and low-resistance ties. Omitted diffusion geometry uses shared interior rectangles and isolated, shared or merged ends. Point contacts use a rectangular spreading-resistance estimate. An explicit zero perimeter or area overrides a derived value. Omitted NRD/NRS follow §12.2.2, which derives or disables diffusion resistance.
9. Stress sums evaluate the first 128 fingers individually, then integrate the reciprocal tail with endpoint and first derivative corrections. WPE needs layout moments. SC alone with active WPE produces E-UNSUPPORTED because the manual refers calculation of those moments to an external mask-distribution specification. Nonzero MTRLMOD also produces E-UNSUPPORTED. Parameter checks cover invalid selectors, positive dimensions and selected extreme dimensions or oxide thicknesses. A warning about an unused model coefficient identifies a coverage or inactive-branch limitation.
10. Newton iterations limit gate and drain steps and both junction biases. The junction limiter uses a minimum reference current of 1e-30 A solely to calculate its critical voltage. For UIC, the integrator takes a short follow-up step (one sixteenth of the initial interval) after a first-step voltage jump over 0.1 V plus VNTOL. This resolves the decay of the initial displacement-current impulse. The simulator and reference can still accept different time grids.

## Comparison with the cached reference

The generic bin/layout sweep combines two short multi-finger transistors with different SA/SB spacings and a long device. The combined drain current falls 1.65% below the reference at 1.2 V. A diagnostic simulation with KU0 set to zero in a temporary input reduced the discrepancy to 0.05% at that point. This isolates the difference to the mobility/velocity stress correction in §14.1.1. The delivered model applies the manual's correction. The available reference records the sum of the three currents.

The SKY130 inverter and ring have their worst scores at internal body-network nodes. On the inverter the worst sample is just before the end of the first input ramp. Its output node differs by 3.1 mV and the next stage by 26.5 mV at their respective worst samples. On the ring the worst sample is 76 fs into UIC startup. Both the junction displacement current and its interpolation depend on the early timesteps. In a diagnostic build forced to step on the reference time grid, the inverter score fell from 7.888 to 4.039 and the ring score from 11.400 to 1.331. Time discretization explains part of each discrepancy. The remaining equation-level difference has not been isolated.

### Cached deck results

The score and maximum absolute error belong to the signal with the highest normalized error in each deck. A score at or below 1 passes. Times are the median of three separate release-mode CLI processes on a warm filesystem.

| Deck | Worst signal | Score | Maximum absolute error | Check / run (s) |
|---|---|---:|---:|---:|
| `b4_cs_ac` | `v(d)` | 0.038 | 1.672e-4 V | — |
| `b4_inverter_tran` | `i(vin)` | 0.021 | 8.272e-8 A | — |
| `b4_n_bin_layout` | `i(vd)` | **8.235** | 3.080e-5 A | — |
| `b4_n_capmod` | `i(vg)` | 0.998 | 7.700e-6 A | — |
| `b4_n_diode` | `i(vb)` | 0.487 | 1.022e-5 A | — |
| `b4_n_idvd` | `i(vd)` | 0.375 | 4.186e-7 A | — |
| `b4_n_idvg_lin` | `i(vd)` | 0.250 | 2.885e-8 A | — |
| `b4_n_idvg_sat` | `i(vd)` | 0.216 | 2.407e-7 A | — |
| `b4_n_igate` | `i(vd)` | 0.031 | 4.984e-8 A | — |
| `b4_n_mobmod` | `i(vd)` | 0.135 | 3.309e-8 A | — |
| `b4_n_rnet` | `i(vd)` | 0.144 | 3.927e-7 A | — |
| `b4_n_temp` | `i(vd)` | 0.064 | 4.194e-8 A | — |
| `b4_n_tempmod` | `i(vd)` | 0.041 | 7.214e-8 A | — |
| `b4_p_iv` | `i(vd)` | 0.215 | 1.660e-8 A | — |
| `b4_ring5` | `v(a)` | 0.234 | 3.007e-3 V | — |
| `sky_cs_ac` | `v(m.xn.msky130_fd_pr__nfet_01v8#body)` | 0.181 | 4.167e-7 V | 0.154 / 0.167 |
| `sky_inverter_tran` | `v(m.xn2.msky130_fd_pr__nfet_01v8#dbody)` | **7.888** | 1.144e-4 V | 0.188 / 0.217 |
| `sky_nfet_idvd` | `i(vd)` | 0.137 | 3.659e-7 A | 0.163 / 0.154 |
| `sky_nfet_iv` | `i(vd)` | 0.119 | 1.717e-8 A | 0.164 / 0.158 |
| `sky_pfet_iv` | `i(vs)` | 0.173 | 2.918e-7 A | 0.158 / 0.209 |
| `sky_ring5` | `v(m.x1.xn.msky130_fd_pr__nfet_01v8#sbody)` | **11.400** | 6.759e-3 V | 0.169 / 0.220 |

The original cached deck suite passed 14/14 and the JIT Verilog-A suite passed 10/10. Their raw waveform values matched a separate build of the unchanged checkout.

The release microbenchmark measured 431.3 ns per intrinsic current, charge and both four-terminal Jacobians with default selectors. External network stamps and the matrix solve are excluded. The maintainer reports approximately 16 s for ngspice to load the SKY130 library.
