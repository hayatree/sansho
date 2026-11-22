# R8 semiconductor models

The comparisons below use the maintainer’s ngspice 44.2 ASCII raw files and original circuits. All scores use the ordinary cached verifier tolerances.

## Vertical power MOSFET

Use three electrical terminals in drain, gate, source order, and a VDMOS model. PCHAN selects p-channel polarity. In forward operation let g be normalized intrinsic Vgs, d the normalized drain–source drop, and u the smooth overdrive `0.1 V · log(1 + exp((g−VTO)/(0.1 V)))` for the default transition. With v the smaller of u and MTRIODE·d, the channel current magnitude is `KP·v·(u−v/2)·(1+LAMBDA·d)/(1+THETA·max(g,0))`. Reverse conduction exchanges the channel ends. A distinct body diode remains tied to the physical terminals. RD, RG and RS introduce internal channel nodes. RB is in series with the body diode.

The Miller capacitance approaches CGDMIN for large negative normalized gate–drain voltage and CGDMAX for large positive voltage. The negative half uses atan and the positive half uses tanh, joined at equal value and slope with argument A·Vgd. The terminal stamp integrates these capacitances to charge before differentiating. CGS is constant. The body diode supports IS, N, RB, CJO, VJ, M, FC, TT and BV/IBV. Its avalanche term is an exponential referenced to BV and IBV. TT stores charge proportional to junction current.

In the output sweep, subtracting I·RS from gate drive and I·(RS+RD) from drain voltage gives the intrinsic biases. At Vds=6 V the three drain current magnitudes are 0.100444, 0.816615 and 2.074739 A. Using gate voltage in the mobility denominator accounts for the high-drive curves. At the lowest drive the saturation current implies 0.495684 V of effective overdrive at 0.494978 V of raw overdrive. The same 0.1 V softplus predicts the 46 nV excess at the middle drive. Across 183 points the maximum current error is 0.90 µA.

The defaults resolved by the gate and recovery decks are CJO=500 pF, CGDMIN=20 pF, CGDMAX=2 nF and A=1/V. With the gate card's explicit Miller values, subtracting gate–source current from gate current gives the Miller current. At 12 V reverse bias the gate sweep then gives 18.8214 pF of Miller capacitance and 125.000 pF from drain to source. The latter equals 500 pF / sqrt(1+12/0.8). A constant 125 pF also matches that AC bias, but gives an 85 mV maximum drain error during turn-on, versus 12 mV with depletion charge. In the recovery deck, dividing the integrated reverse current by the drain voltage increment gives about 1.04 nF at 0.31 V and 234 pF near 4 V. Subtracting the explicit depletion capacitance reveals the negative-Vgd atan branch of the default Miller curve. This accounts for the multi-volt error with the original TT and junction current law.

Instance M scales current and charge and divides the series resistances. TEMP, DTEMP and TNOM use the common temperature binding. KP scales by the absolute temperature ratio raised to MU. TCVTH contributes −TCVTH·ΔT to the normalized threshold. EG and XTI scale the junction saturation current. These sign and normalization choices need reference checks, particularly for p-channel cards. The R9 extensions and remaining temperature limits are described below.

## Level-one MESFET

Z takes drain, gate and source, with an NMF or PMF model. For positive normalized drain voltage d and gate overdrive u above zero, the channel law is `BETA·u²·(1+LAMBDA·d)/(1+B·u)` times the Statz knee factor. The factor is `1−(1−ALPHA·d/3)³` below 3/ALPHA and one above it. Swapping the channel ends reverses the current. RD and RS connect the intrinsic terminals. Gate junction conduction uses the operating thermal voltage.

For gate charge, the smooth high and low end potentials use `sqrt(Vds² + 1/ALPHA²)`. The high potential is smoothed at VTO with a 0.2 V width. CGS, PB and FC define its depletion charge, and CGD multiplies the low potential. At each step Sansho partitions the change in total gate charge along the two rectangular paths from the accepted (Vgs,Vgd) to the trial bias, averaging the contributions along each coordinate. Only a committed step advances this history. For a differential perturbation this gives Cgs=∂Qg/∂Vgs and Cgd=∂Qg/∂Vgd. Solving the loaded small-signal circuit using gm, go, RD and RS from the DC solution gives 2.041845 pF and 0.420852 pF from the complex gain sweep. The Statz potential derivatives at Vgs=−1.00635 V and Vgd=−3.19655 V give the same pair. The previous partition from a fixed zero bias produced an erroneous drain charge response even at infinitesimal signal. The model includes series thermal noise, channel thermal noise and KF/AF channel flicker noise. Temperature translation of fitted GaAs parameters is outside current coverage.

## Bipolar junction transistor and diode

With positive RCO the collector access resistance RC ends at contact x. The epitaxial branch connects x to the intrinsic collector i. Use NPN-normalized voltages, let `d = Vx − Vi`, and evaluate `k = sqrt(1 + GAMMA·exp((Vb−V)/Vt))` at each end. The conductivity-modulation voltage is `u = Vt·[ki−kx−log((1+ki)/(1+kx))]`. The normalized static current from x to i is

```text
Iepi = AREA · (d + u) / (RCO · (1 + |d|/VO))
```

The high-field term uses the contact-to-intrinsic voltage drop. At Vc = 0.25 V, VO = 2 V and GAMMA = 1e−9 the fixed-base measurements give d = 0.18634 V, u = 1.12555 V and 24.0015 mA. Putting d + u in the high-field term predicts 15.8446 mA. The GAMMA = 0 control isolates the unmodulated drift law. The 44.2 probes use NPNs with positive d and explicit VO. Sansho uses the drop magnitude for either current direction and defaults VO to 10 V when omitted. Neither the reverse-field extension nor the omitted-VO default was measured in these sweeps. Dedicated epitaxial resistance noise remains outside coverage.

AREAB and AREAC default to emitter AREA. For a vertical device the base–collector reverse diffusion current, leakage and depletion capacitance use AREAB. The substrate leakage and depletion capacitance use AREAC. SUBS=−1 exchanges those junction area factors. M scales all three effective areas. The three loaded RCO sweeps independently exercise the vertical case with AREA = 1, AREAB = 2 and AREAC = 3. At 3 V drive and RCO = 3 Ω, reconstructing the intrinsic collector with emitter area predicts 89.75 mA. Using AREAB gives 48.83 mA, matching the recorded 48.827 mA. The RCO = 50 Ω acceptance deck scores 0.020 on V(c), 0.024 on V(b) and 0.041 on supply current over all 31 points. Lateral geometry still lacks an independent comparison. [Current reconstruction and sweep results](../verify/probes/README.md#r8b-epitaxial-collector-and-urc)

QCO scales the stored charge at each end by AREA·(k−1). The QCO transient remains a known difference. With RCO=3 Ω, GAMMA=1e−11, QCO=1 pC and VO=10 V, the supplied 44.2 raw reaches +405.812 V and −59.835 V about 0.10 ns apart through a 100 Ω load on a 5 V rail. Sansho spans 0.1145 to 5.158 V. The static acceptance deck uses QCO=0.

For a standalone diode the multiplier applied to RS, TT or M is `1 + a·ΔT + b·ΔT²`, where ΔT is operating temperature minus TNOM in kelvins. The coefficient pairs are TRS1/2, TTT1/2 and TM1/2. TRS aliases TRS1.

TCV defaults to zero and adjusts the positive breakdown voltage before avalanche-knee matching, `BV(T) = BV − TCV·ΔT`. Its units are V/K. A positive TCV moves the reverse terminal voltage toward zero as temperature rises above TNOM. At fixed reverse current, rematching the knee contributes a small additional shift. With BV = 6.2 V, IBV = 1 mA and TCV = 0.0007 V/K, an 80 K rise reduces BV by 56 mV. At 1 mA the matched knee predicts a +56.002876 mV terminal shift, agreeing with the maintainer’s 100 °C measurement. Sansho and ngspice 44.2 both ignore TBV1 and TBV2 with a warning. Alternate temperature selectors remain unsupported. [Paired-diode measurements and predictions](../verify/probes/README.md#diode-breakdown-under-temp)

## Junction FET level two

R14 adds the dispersive JFET2 implementation and separate probes for the uncertain parameter mappings. [Equations and verification status](r14-models.md#junction-fet-level-2)

## R9 power-law MOSFET

For MOS level 6 the saturation voltage is KV·u^NV and the saturation current before output modulation is KC·W/Leff·u^NC. The normalized effective overdrive u includes the classical body threshold, a GAMMA1·Vbs term and a SIGMA·Vds term. Below threshold the channel is off. In the linear region, the ratio x = Vds/Vdsat gives the current multiplier x(2−x). Above the knee this multiplier is one. The output modulation factor is 1 + (LAMBDA0 − LAMBDA1·Vbs)·Vds. Reverse bias exchanges the channel terminals and PMOS uses polarity-normalized voltages. The transistor shares the classical MOS body junctions, Meyer charge, overlaps, access resistances, temperature binding and operating-point probes. Defaults used here are KC=50 µA/V^NC, NC=1, KV=2 and NV=0.5. The current law follows Sakurai and Newton. The parameter mapping, defaults and inherited temperature translation require comparison with 44.2.

## R9 VDMOS extensions

KSUBTHRES sets the softplus voltage scale (default 0.1 V). Its gate intercept is the polarity-normalized VTO − TCVTH·(T − TNOM) + SUBSHIFT. The measured SUBSHIFT translation is one volt per volt. Varying PHI alone gives identical 55 °C transfers. PHI gives an ignored-parameter warning. R14 makes both drain resistance exponents configurable. [TEXP0 and TEXP1](r14-models.md#vdmos-drain-resistance)

With positive RD and omitted exponents the isothermal drain access branch uses `RD + RQ·(T/TNOM)^0.3·u/(VQ + u)`, where u is the magnitude of the **external** drain–source voltage. The finite-RS drain sweeps distinguish that voltage from either intrinsic terminal drop. The paired temperature sweeps confirm the exponent from 0 to 90 °C. RD = 0 disables the branch even for nonzero RQ. Its thermal voltage noise density squared is 4kTR when present.

### Thermal terminals

`M1 d g s tj tcase model thermal` exposes junction and case voltages in thermal units of 1 V/K and currents in units of 1 A/W. Let j and c be those voltages. In the electrical equations the absolute junction temperature is `Tj = Tamb_K + j`. At j = 0 the clamped transfer equals the isothermal control. At a fixed nonzero j it depends on ambient. The isothermal mobility factor is `(Tamb_K/TNOM_K)^MU`. Sansho applies a further `(Tj/Tamb_K)^(2·MU)` to KP under self-heating. The heated transfer at a 30 K rise confirms the exponent for MU = 0, −0.75 and −1.5 and the threshold shift of −TCVTH·j. The same transfer gives RS = 0.020 Ω and a drain access resistance of 0.092289 Ω for RD = 0.080 Ω. Sansho uses the inferred multiplier `(Tj/Tamb_K)^1.5` on RD under self-heating.

For one device the port equations implemented in Sansho are

```text
flow = (j − c) / RTHJC
junction current into device = flow + 2·CTHJ·dj/dt − Pheat − Tamb_C/RTHCA
case current into device     = c/RTHCA − flow
```

RTHJC connects junction to case. The case has a shunt to thermal ground and the ambient drive enters the junction. The 100, 400 and 1000 K/W RTHCA sweeps confirm both inverse-resistance terms. An open case reaches `Tamb_C + RTHCA·Pheat` in DC when the device supplies all the junction heat. A case tied to node 0 holds c at zero. A case step reaches the junction through RTHJC. The measured pole with a clamped case is 2·RTHJC·CTHJ. Thermal resistances are in K/W and CTHJ is in J/K. Defaults used by Sansho are RTHJC = 1, RTHCA = 1000 and CTHJ = 10 µJ/K. CTHJC produces the standard ignored-model-parameter warning.

In forward DC operation the junction receives terminal power less the source access loss I²RS, including the drain access loss. Sansho uses the static electrical stamp for heat and omits stored electrical energy from that term. In forward body-diode operation the terminal power heats the junction, including the I²RB loss. Under a fast gate ramp the integrator samples the crossing of the heated gate threshold to resolve the onset of conduction.

With M parallel copies Sansho divides both resistances by M and multiplies storage by M. The thermal measurements used one device per junction. THERMAL requires exactly five nodes in drain, gate, source, junction and case order. Supplying three or four gives E-PARSE-ARITY. The three-node electrical form omits THERMAL. [Measured voltages, currents and verifier scores](../verify/probes/README.md#r9d-thermal-probe-results)
