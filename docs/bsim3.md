# Built-in BSIM3v3.3 MOSFET

Provenance: Original Rust implementation from the UC Berkeley BSIM3v3.3 manual, its parameter and equation appendices, Appendix D and the July 2005 enhancement notes.

BSIM3 cards are extracted against the Berkeley reference implementation. Sansho follows behavior established solely by black-box observation of ngspice 44.2 where it differs from the printed manual. That release embeds the Berkeley model and may include ngspice-specific changes. [Compact-model policy](compatibility.md#compact-model-compatibility)

Use an M instance in drain, gate, source, bulk order with an NMOS or PMOS model. Levels 8 and 49 select the same equations. The defaults are mobMod=1, capMod=3, noiMod=1 and quasi-static operation. Omitting VERSION selects 3.3. The parser accepts a numeric version evaluating to 3.3 (including `3.3` and `3.30`) or the literal three-component label `3.3.0`. It compares multi-component labels as strings. For example, `3.3.1` and `3.3.00` produce an unsupported-version diagnostic.

```spice
.model nch nmos level=49 version=3.3.0 tox=8n vth0=0.54 k1=0.52 k2=-0.023
M1 out gate 0 body nch w=2u l=0.5u ad=0.6p as=0.6p pd=3u ps=3u
```

## Coverage by manual chapter

| Chapter | Coverage in Sansho |
|---|---|
| 1, model overview | Four-terminal silicon NMOS and PMOS. The same quasi-static operating point supplies DC, small-signal and transient analyses. |
| 2, physical effects | Vertical and lateral doping, body effect, short-channel and narrow-width threshold shifts, DIBL, poly depletion, mobility modes 1–3, velocity saturation and geometry-dependent parasitic resistance. |
| 3, unified current | Smoothed gate overdrive and drain bias, bulk-charge factor, intrinsic and resistive saturation, CLM, output DIBL, SCBE and impact-ionization substrate current. Appendix B.1.11 supplies the channel temperature laws. |
| 4, capacitance | capMod 0–3, all three XPART partitions, separate IV and CV dimensions, gate overlap and fringing. Mode 3 includes accumulation and inversion centroids and the surface-potential correction. |
| 5, NQS | Gap. Transient NQS requires a dynamic channel-charge state and AC NQS requires a frequency-dependent response. A nonzero effective NQSMOD or ACNQSMOD produces E-UNSUPPORTED at binding. |
| 6, parameter extraction | The simulator consumes model cards. Parameter extraction is outside its scope. |
| 7, benchmarks | Original transfer and output sweeps exercise two channel lengths, two polarities and reverse body bias. The manual's measured devices are outside this suite. |
| 8, noise | noiMod 1–4, equations 8.1–8.9. Independent channel thermal and flicker current generators feed the adjoint solver. Each physical diffusion resistance contributes thermal noise at its own nodes. Enhancement modes 5 and 6 are outside R5. |
| 9, MOS junctions | Area leakage, IJTH tangent continuation, GMIN, integrated bottom, field-sidewall and gate-sidewall charge. Temperature changes saturation current, zero-bias capacitances and potentials. |
| Appendices A, B and D | Parameter defaults, effective dimensions, area/perimeter scaling and L/W/P binning of the implemented coefficients. |

## Binding and analyses

Numbered model families use scaled drawn W and L for selection. The existing parser gives an exact model name precedence and uses inclusive lower and exclusive upper bounds with its relative boundary tolerance. Within a selected card, Appendix D coefficients use the effective IV dimensions after LINT/WINT and the length, width and cross corrections. BINUNIT=1 uses micrometres, the other setting uses metres. DLC/DWC and their geometry terms independently set the active area for CV.

W, L, AD, AS, PD and PS use the circuit geometry defaults and scale rules. NRD and NRS default to one square. Positive products with RSH allocate internal diffusion nodes. M scales terminal currents, charges, noise power and diffusion conductances. RDSW supplies the bias-dependent resistance inside the channel equation. TEMP, DTEMP and model TNOM specialize the binding in kelvin from netlist temperatures in degrees Celsius. Identical validated bindings share coefficients within a compile. Limiter history and terminal nodes belong to each instance.

The evaluator orients the intrinsic channel toward the higher normalized drain potential. Substrate current leaves the local drain. Overlap and diode terms follow the physical terminals through source/drain reversal. Four-lane forward AD supplies every terminal entry of G and C. The fixed-order assembly includes the terminal charges in transient error control. TF and PZ consume the same operating-point matrices. SENS follows the engine's existing parameter-rebinding and adjoint algorithm, including its parameter perturbations. [Analysis methods and supported sensitivity parameters](analyses.md)

## Equation choices and limits

- **Threshold defaults.** Appendix A.10 derives an omitted VTH0 from VFB, surface potential and K1. With both voltages omitted, the calculation starts at VFB = −1 V. A supplied PMOS VTH0 is signed. With both body coefficients omitted, A.10 derives them from doping, XT, VBX and VBM. With only one supplied, its partner takes the tabulated default. Equation 2.1.14 uses the chapter's typical diffusion doping of 10²⁰ cm⁻³. NCH and NSUB use cm⁻³. U0 above 1 uses cm²/(V·s), and smaller values use m²/(V·s).
- **Forward bias and numerical limits.** The reverse body-bias limit uses equation 2.1.26 with a finite −30 V fallback when the stationary point is absent. Above 0.8 times the surface potential, a positive rational continuation preserves the value and slope of the depletion potential. This extends the square root through strongly forward-biased Newton iterates. Small positive denominator bounds protect mobility, bulk charge and effective width for extreme fitted coefficients. At zero drain bias the conjugate form of equation 3.6.4 retains the linear-region conductance.
- **Charge partition.** Chapter 4's ordering of XPART labels conflicts with the explicit inequalities in Chapter 5. The latter determine the saturation drain fractions of 0.4, 0.5 and 0 for XPART below, equal to and above 0.5, respectively. Equations 4.3.19–26 determine inversion charge and source share. The drain receives the remainder. capMod 0 and 1 retain their piecewise region boundaries.
- **Flat band and charge thickness.** Modes 1–3 evaluate the IV threshold at zero body and drain bias for the CV flat band. The smoothing discriminant in 4.3.14 uses the magnitude of the flat-band voltage so a negative flat band has a real root. Mode 3 uses the MV/cm oxide field in 4.4.2–4 and the logarithm with its unity term in 4.4.5. A positive floor on K1ox in this correction and a zero lower bound on inversion drive handle the degenerate CTM limit.
- **Overlap.** The drain expression uses gate-to-drain voltage throughout. For mode 1, the positive-bias LDD slope equals the limiting negative-bias slope. Modes 2 and 3 use the continuous overlap equation in §4.5.2. CGSL/CGDL are accepted alongside the printed CGS1/CGD1.
- **Junctions.** Saturation current uses JS times the supplied junction area. JSSW is accepted. Sidewall capacitance still uses the perimeter, split at the effective width into gate and field edges. Zero area gives GMIN alone for junction current. The integral of each capacitance uses zero charge at zero junction bias. The forward branch integrates the manual's linear capacitance. The IJTH knee applies per device before multiplicity.
- **Noise normalization.** Mode 1 uses SPICE2 flicker and thermal noise. Mode 2 uses both BSIM3 laws. Mode 3 combines BSIM3 flicker with SPICE2 thermal, and mode 4 uses the reverse combination. For equation 8.2, Sansho interprets thermal energy in eV with the table's trap coefficients and uses the DC oxide capacitance density. The unspecified N* in the second term uses the 2 × 10¹⁴ m⁻² offset printed in the first term. The Berkeley behavior measured in ngspice 44.2 uses the Chapter 3 carrier densities at the source and effective drain. The weak and strong powers combine harmonically at the actual bias. Positive EM enables channel shortening. The capMod 3 inversion charge uses a 1 mV smooth limit on the surface-potential correction.
- **Diagnostics.** PARAMCHK enables Appendix A.10 range warnings for ACDE, MOIN, NOFF and VOFFCV. Invalid dimensions, temperatures and unsupported selectors produce binding diagnostics. Additional unconsumed parameters use the engine's unused-parameter warning.

## Verification status

The unit tests check all terminal current and charge derivatives numerically, current and charge balance, common-mode invariance, source/drain exchange, zero drain bias and continuity at region boundaries. Separate checks cover saturation partition, junction capacitance and limiting, binning units and the noise selector combinations. Circuit tests exercise the two MOS levels, version diagnostics, gain, model sensitivity, a finite pole and bit-identical assembly across nonlinear chunks. All thirteen cached ngspice 44.2 decks pass. All thirteen produce byte-identical binary raw output with one and two workers. [Deck inventory, scores and reference command](../verify/README.md#r5-bsim3v33)

### Gate admittance and ring timing

In the capacitance decks, Va, Vb and Vi drive three separate gates at −1.3, −0.1 and 1.35 V. Each gate sees a 1 V AC excitation with all other terminals at AC ground. The raw files give total gate capacitance through `−Im(I(Vsource))/(2πf)`. They contain no independent bulk measurement.

| capMod | Gate bias (V) | Sansho, CGBO=0 and NSUB centroid (fF) | 44.2 (fF) | Sansho with explicit gate–bulk overlap (fF) |
|---|---:|---:|---:|---:|
| 2 | −1.30 | 14.84339 | 14.91875 | 14.91875 |
| 2 | −0.10 | 6.81812 | 6.89346 | 6.89349 |
| 2 | 1.35 | 12.83126 | 12.90662 | 12.90662 |
| 3 | −1.30 | 14.39446 | 14.27907 | 14.46983 |
| 3 | −0.10 | 6.34070 | 6.41582 | 6.41606 |
| 3 | 1.35 | 11.98241 | 12.05784 | 12.05778 |

The last column adds only explicit overlap to the pre-R6 model. The two gate edges excluded from the active width in §4.2 have a combined width of `2 × DWC`. Assigning their oxide capacitance to the explicit §4.5.2 gate–bulk overlap gives `CGBO = 2 × DWC × Cox`. With DWC = WINT = 15 nm and Lactive = 0.582 µm, this adds 0.075365 fF at every bias. Sansho uses this measured default, with DWC falling back to WINT. Explicit CGBO, including zero, overrides it. The capMod 2 comparison scores 0.002. The three gate totals cannot identify the return terminal of a constant capacitance in the reference.

Four-terminal CGBO=0 controls with separate zero-volt source and bulk sensors check the signs in §4.3.1. At 1.35 V with capMod 2, the gate, drain, source and bulk entries of the gate-driven capacitance column are +12.8313, −4.8387, −6.4961 and −1.4964 fF. Changing XPART from 0 to 1 moves 3.0550 fF between drain and source. Gate and bulk stay fixed in both modes. Adding junction areas and perimeters leaves this column unchanged because the gate excitation produces zero AC junction voltage. Removing CF subtracts 0.64610 fF at every bias, equal to `2 × Wactive × CF`. Removing CGSL and CGDL instead subtracts 0.12360, 0.26377 and 0.39720 fF with increasing bias.

A second control isolates the capMod 3 accumulation centroid. Equation 4.4.2 weights the ACDE exponent by substrate doping NSUB. In the pre-R6 evaluator, setting NSUB to the card's NCH (2.1 × 10¹⁷ cm⁻³) lowered the accumulation value by 0.190757 fF. Explicit K1 and K2 isolated the centroid response. Overlap and accumulation-centroid corrections together give a capMod 3 score of 0.010. Including the 1 mV inversion-drive smoothing gives 0.041. Sansho uses the binned channel concentration NCH in the capMod 3 accumulation-centroid exponent. The resulting accumulation capacitance is 14.279069 fF against 14.279071 fF in the cache. NSUB still sets the substrate contribution to the body coefficients.

In the five-stage ring, linear interpolation between successive rising 0.9 V crossings at node A gives 3.405520 ns with the old defaults. Applying the overlap default alone gives 3.416890 ns. Overlap and centroid together give 3.416875 ns. Adding the capMod 3 inversion-drive smoothing gives 3.416931 ns versus 3.416982 ns in 44.2. The full waveform score is 0.020.

### Junction and channel temperature

The two equal junctions in `b3_temp` each see 0.57 V forward bias. The card specifies 0.90 fA of area saturation current and 0.24 fA of perimeter saturation current per junction at nominal temperature. At 125 °C, including both gives 8.248654 mA total. Using area alone gives 6.512095 mA against 6.517321 mA in the cache. The maximum error over the 31-point sweep is 5.226 µA (score 0.401).

The independent channel sensor agrees within 0.545 nA from −25 to 125 °C. At 125 °C its current is −39.5344 µA in Sansho and −39.5341 µA in the reference. Disabling UTE, KT1, UA1 or AT individually gives −60.9540, −32.2663, −45.4489 or −39.7992 µA in Sansho. Appendix B.1.11 also applies the temperature ratio to KT1L/Leff, KT2 × Vbseff, UB1, UC1 and PRT. This card exercises UB1 at its default. KT1L and PRT default to zero, and zero body bias suppresses the KT2 and UC1 terms. In a separate −0.4 V body-bias control, zeroing KT2 or UC1 moves the 125 °C channel current from −31.0546 µA to −30.7168 or −31.0860 µA, respectively.

The gate-driven capacitance decks have AC-grounded junctions. The loaded drain in `b3_gm_gds` tests sidewall charge directly through the gain response. At 1 GHz the cached output gain is −1.180208 + j0.189286. Sansho gives −1.180213 + j0.189299 with sidewall capacitance and −1.193776 + j0.118244 when both CJSW and CJSWG are zeroed. This establishes a measurable perimeter capacitance. The card gives the two edge densities the same value, so the measurement cannot distinguish them individually.

### Unified flicker noise

Before the probe-based revision, the 1 Hz input noise amplitudes were 7.30725 µV/√Hz in Sansho and 9.19425 µV/√Hz in 44.2. Output amplitudes were 16.83935 and 21.18780 µV/√Hz. The corresponding gains agree at 2.305. The prior Sansho input flicker power was about 63.2% of the reference. EF = 1 acts on power, giving a 1/√f amplitude in the flicker-dominated range. With NOIA, NOIB and NOIC zero, the total input thermal floor is 6.6842 nV/√Hz at 100 MHz. Subtracting the reference 1 Hz input power divided by 10⁸ from its 100 MHz power gives an estimated floor of 6.6852 nV/√Hz. Mode 4 with KF = 0 reproduces the mode 2 thermal control, including load and diffusion-resistor noise.

To check the prior model against equation 8.2 independently, replace the two diffusion resistances with equivalent external resistors (10.5 Ω drain, 14 Ω source for M = 2). The operating point gives 45.685 µA per copy, Vgs − Vth = 0.3513 V and Vdsat = 0.2888 V, placing it in the strong-inversion branch. Equations 8.3 and 8.4 give N₀ = 9.464 × 10¹⁵ and Nₗ = 1.684 × 10¹⁵ m⁻². With the R5 energy and N* conventions above, the trap integral and channel-shortening terms give 2.200 and 2.727 × 10⁻¹⁸ A²/Hz per copy at 1 Hz. Applying multiplicity in power and the measured intrinsic drain–source to output transimpedance of 5.364 kΩ gives 16.8393 µV/√Hz. Setting EM = 0 alone gives 15.0777 µV/√Hz. With only NOIA, NOIB or NOIC active, input amplitudes are 5.1382, 5.1295 and 0.8261 µV/√Hz including the common thermal floor.

The revised loaded deck gives 9.19289 µV/√Hz input and 21.18476 µV/√Hz output at 1 Hz. Its worst score is 0.074.

The R6 probe follow-up isolates the device by sensing drain current through a noiseless 1 kΩ transimpedance. NOIA = 0 eliminates the entire flicker component at high overdrive. EM = 0 suppresses the shortening term in saturation. One expression using the Chapter 3 sheet densities, the smoothed drain voltage and the harmonic combination of weak and strong powers fits all 43 settings. The largest error in nonzero 1 Hz flicker power is 0.031%. In the two lowest gate-bias spectra the thermal floor also resolves the smooth limit on inversion charge described above. The largest pointwise amplitude error across both spectra is 0.018%. The integrated input RMS at those two biases records a separate frequency-integration convention. [Measured powers, equations and integration comparison](../verify/probes/README.md#bsim3-noimod-2)

### Derived body coefficients on the R7 default cards

The threshold-model doping relations use the numerical coefficients `γ = 5.753e−12 √N / Cox` and `VBX = Φ − 7.7348e−4 N XT²`, with N in cm⁻³, Cox in F/m² and XT in metres. Re-evaluating the coefficients from modern SI constants previously gave 5.76153e−12 and 7.73296e−4. Through the default GAMMA1, GAMMA2 and VBX these small changes affect K1 and K2. When VTH0 is omitted, K1 enters both its derivation from VFB and the lateral doping correction.

At W/L = 10 µm/1 µm, VDS = 1.2 V and VBS = 0, replacing only the gamma coefficient moves the default threshold from 1.818110 to 1.815297 V. Using the model's VBX coefficient as well gives 1.815614 V, versus 1.815632 V in the 44.2 raw at all seven gate voltages. At VGS = 1.4 V the corrected ID, gm and gds are 58.666 pA, 1.4431 nS and 9.3900 pS. The reference gives 58.622 pA, 1.4421 nS and 9.3832 pS. The threshold shift changes the subthreshold current exponentially. [Default-card verification deck](../verify/decks/r7_bsim_observables.cir)
