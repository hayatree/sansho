# Native VBIC bipolar transistor

A level-four NPN or PNP card selects the native model for OP, DC (including a temperature axis), AC, noise and transient analysis. R15 targets the VBIC 1.2 interface in ngspice 44. All seven acceptance decks and 219 completed probe settings pass against cached ngspice 44.2 measurements in R15c. The original suite contributes 187 passes and two partial references. The ten R15b follow-up templates contribute 32 passes and two partial references. All four partial references are aborted TD transients. The acceptance waveforms also replay bit-for-bit with one and two workers. A 257-transistor test exercises parallel assembly. The separate Verilog-A route remains available for cards targeting VBIC 1.3. [Acceptance decks](#acceptance-decks) · [Probe index](../verify/probes/README.md#r15-native-vbic)

## Terminals and scaling

```spice
Q1 c b e model
Q2 c b e s model area=2 m=3
Q3 c b e s tj model
.model model npn(level=4 selft=1 rth=200 cth=60n)
```

The omitted substrate connects to ground. The optional fifth pin represents junction rise in kelvin above ambient. With SELFT=1 and positive RTH, a connected fifth pin receives the dissipated power. Omitting it leaves the device at ambient temperature. A fifth pin tied to 0 clamps the device to ambient. With heating disabled, the fifth pin draws zero current. A model can therefore share a temperature-rise net with external thermal elements. Ngspice 44 §§7.3.1 and 7.3.4 give the terminal and heating conventions.

Define `a = AREA·M`, defaulting both factors to one. Equations below describe one unit transistor. Sansho multiplies electrical current, stored charge, thermal conductance and thermal capacitance by a. This puts unit resistances in parallel and retains knee current densities. The noise section gives the separate AREA and M laws. AREA also accepts the positional spelling after the model. The junction ambient in °C is (instance TEMP or circuit temperature) + instance DTEMP + model DTEMP. TNOM and TREF name the extraction temperature. IC accepts a B–E/C–E tuple, and ICVBE and ICVCE can specify the voltages separately.

The internal paths are C–RCX–CX–RCI–CI, B–RBX–BX–RBI–BI, E–RE–EI, S–RS–SI and CX–RBP–BP. Omitted RBI, RCI and RBP default to 0.1 Ω. Omitted RBX is an ideal connection. An explicit zero for any of these four resistances gives 0.01 Ω. A negative RCI also gives 0.01 Ω. The resulting finite branches participate in DC, AC and noise. Internal voltages below use NPN polarity, so each voltage difference and electrical branch current reverses sign for a PNP.

## Transport, base and substrate currents

Let `D(v,I,n) = I·expm1(v/(n·VT))` with `VT = kT/q`. In the native VBIC voltage law, k/q uses the historical rounded constants 1.380662e−23 J/K and 1.602189e−19 C. Dividing the intrinsic base voltage by T·log1p(F/IS) on the default-card sweep gives 8.61734789e−5 V/K from 0.60 through 0.80 V after the base-lead and GMIN corrections. This identifies the ratio. The noise prefactors use the workspace SI constants. Define `F = D(Vbi,ei, IS, NF)` and `R = D(Vbi,ci, IS·ISRR, NR)`, where the comma between node names denotes their voltage difference. Let `Je` and `Jc` be the emitter and collector depletion integrals divided by their zero-bias capacitances. The depletion functions have units of volts and remain defined for zero junction capacitance.

```text
w = 1 + Je/VER + Jc/VEF
h = F/IKF + R/IKR
qb = (w + (w^(1/NKF) + 4h)^NKF)/2        for QBM < 0.5
qb = w·(1 + (1 + 4h)^NKF)/2               for QBM ≥ 0.5
If = F/qb                 Ir = R/qb
```

A zero Early voltage or knee current omits its reciprocal term. Forward transport flows CI→EI, with reverse transport subtracted. Independent ideal and recombination currents use IBEI/NEI and IBEN/NEN at B–E, IBCI/NCI and IBCN/NCN at B–C. WBE weights the B–E branch at BI and its complement at BX. Positive VBBE adds reverse B–E current with scale IBBE and slope NBBE·VT, using EBBE as the dimensionless exponential offset. Its default cancels the breakdown current at zero bias. Weak avalanche subtracts `(If−Ir−Ibc)·AVC1·U·exp(−AVC2·U^(MC−1))` from the B–C current, where d = PC−Vbi,ci and U = (d + sqrt(d² + (0.1 V)²))/2.

The substrate transistor conducts BX→SI with BP as its base. Its forward drive Fp is the WSP-weighted sum of D at BX–BP and BI–CI. Its reverse drive Rp uses SI–BP. Both use ISP and NFP. The parasitic base charge is `qp = (1 + sqrt(1 + 4Fp/IKP))/2`, and transport is `(Fp−Rp)/qp`. IBEIP/IBENP drive BX–BP with NCI/NCN, and IBCIP/IBCNP drive SI–BP with NCIP/NCNP. The conductances of RBI and RBP are qb/RBI and qp/RBP.

## Quasi-saturation and charge

At CI and CX evaluate `k = sqrt(1 + GAMM·exp((Vbi−Vcollector)/VT))`. With normalized drop d from CX to CI and endpoint values ki and kx, the epitaxial current is

```text
u = d + VT·(ki−kx − log((1+ki)/(1+kx)))
Iepi = (u/RCI) / sqrt(1 + (u/(VO + sqrt(d² + (0.1 V)²)/(2·HRCF)))²)
```

Zero VO removes the high-field division. Zero HRCF removes the term containing its reciprocal. Each collector endpoint stores `QCO·(k−1)` against BI. Kull et al. supply the conductivity integral. The reverse-epi and VO sweeps resolve the high-field denominator in both current directions.

For junction potential P and grading m, integrate `(1−v/P)^(−m)` and subtract the zero-bias integral. FC sets the forward join at `c = FC·P`. A nonpositive AJ continues the capacitance linearly above c. For positive AJ, use the smoothed lower voltage `z = c + (v−c−sqrt((v−c)²+4AJ²))/2`, integrate at z, and add `(v−z)·(1−FC)^(−m)`. A positive VRT introduces a lower join at −VRT with constant capacitance beyond reach-through for nonpositive AJ. For positive AJC, ART and VRT the two joins share the interval from l = −VRT to c. Let D = c−l, b = (c+l)/2, C(u) = (1−u/P)^(−m) and let J be its integral. The distances to both endpoints define a bounded integration voltage z. Interpolating the endpoint capacitances accounts for the voltage outside the integrated interval. Both voltage increments are referenced to zero bias.

```text
z(v) = b + (v−b)·D / (sqrt((v−l)² + (D·ART)²) + sqrt((v−c)² + (D·AJC)²))
Δz = z(v)−z(0)
Ce = C(l) + (C(c)−C(l))·(z(v)−l)/D
Q/CJC = J(z(v))−J(z(0)) + (v−Δz)·Ce
```

At nominal temperature this expression gives the normalized B–C depletion charge. The reach-through option applies to the intrinsic B–C depletion charge (CJC), as specified in §27.5.2.

CJE and its WBE fraction follow the two B–E junctions. CJC attaches to BI–CI, CJEP to BX–BP and CJCP to SI–BP. CBEO, CBCO and CCSO are linear external overlaps. BI–EI stores `TF_eff·If`, BI–CI stores `TR·R` and BX–BP stores `TR·Fp`. The forward transit time is

```text
TF_eff = TF·(1 + QTF·w)·(1 + XTF·u²·exp(Vbi,ci/(1.44·VTF)))
u = F/(F + ITF) for F > 0, or 1 when ITF is zero
```

At nonpositive F, u is zero. Zero VTF removes the exponent. Section 27.5.2 lists TD in seconds. Positive TD filters forward transport with `H(s) = 1/(1 + s·TD + (s·TD)²/3)`. Two internal DAE states implement the same transfer for transient and AC, with unit DC gain. The charge equation uses undelayed If.

## Temperature, self-heating and noise

For absolute temperature ratio r = T/Tnom and temperature change ΔT, define `L(I,x,E,n) = I·exp((x·log(r) + E·(r−1)/VT)/n)`. Transport uses XIS and EA, parasitic transport uses XIS and EAP. The ISRR multiplier uses XISR and DEAR. Ideal junctions use XII with EAIE, EAIC or EAIS. Recombination uses XIN with EANE, EANC or EANS. NF and NR receive the factor `1 + TNF·ΔT`, and AVC2 receives `1 + TAVC·ΔT`. VBBE uses the quadratic factor from TVBBE1/2, and NBBE the linear factor from TNBBE.

Each lead resistance uses `Rnom·r^X`. XRB supplies the extrinsic exponent when XRBX is absent. XRBI and XRBP default independently to zero. XRC supplies the extrinsic exponent XRCX. XRCI defaults independently to zero. The emitter and substrate use XRE and XRS. VO and IKF use XVO and XIKF. GAMM scales by `r^XIS·exp(EA·(r−1)/VT)`. For each depletion junction Sansho uses the corresponding ideal activation energy E, `P(T) = E + r·(Pnom−E) − 3VT·log(r)`, and `C(T) = Cnom·(Pnom/P(T))^m`. The electrical, charge and temperature derivatives are evaluated together with forward-mode differentiation.

Section 7.3.4 couples dissipated power to a parallel RTH/CTH. For junction rise θ, the current into the thermal pin is `a·(θ/RTH + CTH·dθ/dt − P)`. P sums voltage times static current over all physical electrical branches, including contact losses. Displacement currents, numerical GMIN shunts and phase-control rows contribute zero heat. At DC an unloaded junction reaches θ = P·RTH. All electrical branches see the resulting temperature at each Newton iterate.

Shot spectra are one-sided `2q·|I|`. The collector source uses forward transport If. The bias dependence of reverse transport, the B–C diode and avalanche enters the noise transfer through the small-signal equations. Each finite lead contributes `4kT·g`, with the modulated base conductance and the low-field conductance for the epitaxial lead.

For the intrinsic B–E and pocket BX–BP noise sources, the instance current is `In = AREA·M·Idiode + GMIN·Vjunction`, with voltage and current in NPN polarity. Each contributes shot power `2q·|In|` and flicker power `M·KFN·|In/M|^AFN/f^BFN` across its own junction. The forward transport shot power uses the area-scaled current alone. Sansho also includes shot and flicker from the WBE-split extrinsic B–E current, and shot noise from parasitic transport and the SI–BP diode. Those additional source strengths have yet to be isolated.

## Outputs and diagnostics

The Gummel–Poon output set is available as `@q1[ic]`, `ib`, `ie`, `is`, `gm`, `go`, `gpi`, `gmu`, `cpi` and `cmu`. Terminal currents include their charge derivatives in transient. The current sum cancels across the four electrical terminals even with internal lead drops. The conductances and capacitances describe intrinsic partial derivatives at the operating point. VBIC also exposes intrinsic `vbe`, `vbc` and dissipated power `p`.

Both values of UNKNOWNFIT trigger an ignored-key warning in 44.2. All three explicit EBBE values are accepted silently. With IBBE=1 µA, increasing EBBE from zero to 1e−6 shifts i(Vb) at zero bias by −1 pA. Sansho matches both recognition and current. Numeric VERS and VREF are metadata. Independent AREAB and AREAC are outside the §27.5.2 instance interface. Invalid SELFT, nonpositive AREA/M or invalid emission coefficients give `E-ELAB-PARAM`. A nonpositive operating junction temperature or a request for higher-order DISTO derivatives gives `E-ANALYSIS`. [Diagnostic catalog](diagnostics.md)

## Acceptance decks

| Deck | Measurement | Largest absolute residuals against 44.2 |
|---|---|---|
| [Gummel](../verify/decks/r15_vbic_gummel.cir) | Forward collector and base current over low injection, recombination and rolloff. | Ic 13.3 pA, Ib 2.85 fA |
| [Output curves](../verify/decks/r15_vbic_output.cir) | Collector-current knee and base voltage at five base drives. | Vb 0.706 mV, Ic 1.95 µA |
| [Short-circuit AC gain](../verify/decks/r15_vbic_ft.cir) | Complex collector and base currents. In Sansho the magnitude ratio crosses unity between 100 and 133.4 MHz. | Ic 18.2 pA, Ib 203 pA (complex differences) |
| [Thermal pulse](../verify/decks/r15_vbic_thermal.cir) | Junction rise peaks at 0.200 K in Sansho. Cooling follows the 12 µs nominal time constant. | Ic 5.09 µA, Ib 50.9 nA, junction rise 91.4 µK |
| [Substrate collection](../verify/decks/r15_vbic_substrate.cir) | Lateral current as the collector leaves saturation. | Ib 1.82 nA, Is 1.55 nA, Ic 0.245 nA |
| [PNP](../verify/decks/r15_vbic_pnp.cir) | Signed complementary Gummel currents with AREA=1.5 and M=2. | Ic 35.9 pA, Ib 39.2 fA |
| [Temperature sweep](../verify/decks/r15_vbic_temperature.cir) | Collector and base currents from −20 to 100 °C. | Ic 0.962 µA, Ib 4.94 nA |

## Evidence from ngspice 44.2

| Measurement | What the sweep establishes |
|---|---|
| Default card, strong breakdown and complex junction admittances | Explicit IS=1e−16 A reproduces the default collector current. At 27 °C the coupled B–E and B–C charge currents identify 0.1 Ω common base resistance and 0.1 Ω additional collector resistance. The substrate capacitance sees 0.1 Ω to the pocket. The same defaults reproduce high-injection DC and the −4.2 V breakdown point (1.486 A). Low-current base residuals resolve an additional B–E GMIN path, placed at BX–EI in Sansho. At −0.35 V the PNP base shunt contributes 2.6 pA for AREA·M=3. Scaling the shunts like physical currents would add 5.2 pA to its magnitude. |
| AJE, AJC, AJS | At the B–E join the measured current at 1 MHz goes from 114.715 µA in quadrature for negative AJ to 105.082 µA for AJ=0.1 V. The lower integration voltage at the join is c−AJ. The same factor of four under the square root reproduces the collector and substrate measurements. |
| AVC1/2 and RCI/GAMM/VO/HRCF | Subtracting the zero-AVC1 run gives the avalanche multiplier. Inverting it gives U=1.05237 V at d=1.04999 V and U=2.05121 V at d=2.04999 V. Independently, reconstructing the intrinsic collector drop from base and collector currents gives an effective limiting voltage of 0.2677 V at VO=0.2 V, HRCF=0.8 and d=−0.0418 V. Both experiments resolve the 0.1 V rounding in their respective equations. |
| QBM, NKF, VEF/VER, ISRR and substrate DC | Both base-charge branches, the ISRR multiplier and WSP interpolation reproduce the measured sweeps at their sampled biases. The pocket current varies with IKP and the RBP drop. |
| TF, TR, QCO, WBE and TD | The complex supply currents support the modeled intrinsic storage and partition. The 0, 30 and 300 ps AC traces agree with the second-order phase response through 10 GHz. The QTF probe has unit depletion width throughout. |
| Temperature and heating | Both DTEMP offsets add to an explicit TEMP. Isolating RCX and RCI in the XRC=2 experiment gives a worst collector error of 0.47 µA with the exponent on RCX and 37.8 µA with it on RCI. The measured ideal/recombination, reverse transport and hot-breakdown sweeps support the corresponding temperature maps. Junction-capacitance sweeps agree from −20 to 85 °C. With the fifth pin omitted, changing RTH from 0 to 500 K/W leaves the entire reference Gummel curve identical. Connected, externally loaded and disabled heating, and electrical/thermal AREA and M scaling pass the pulse comparisons. |
| White and flicker noise | The RCI sweep distinguishes the forward transport shot source from the previous combination of net transport and net B–C shot noise. At RCI=2 Ω the output density is 3.51317 nV/√Hz. At AFN=1.4, four times AREA and four copies give 1.407 and 1.203 nV/√Hz respectively at 1 Hz. The white floor scales in power with AREA·M. |

### R15c follow-up measurements

| Probe | Observation and inference |
|---|---|
| QBM domain | Each log reports seven DC rows with no parameter warning. The current vectors for −1, 0 and 0.49 are identical, as are those for 0.51, 1 and 2. At the final bias the collector magnitudes are 600.744 and 364.395 µA respectively. Sansho uses 0.5 as the transition and assigns equality to the upper branch. The probes bracket the transition and leave equality unmeasured. |
| RCI domain | All four logs report nine AC rows without warnings. At 10 GHz the real part of the reciprocal collector transfer admittance is 0.110 Ω for RCI=−1 and 0, 0.200 Ω for 0.1 and the omitted control, and 1.100 Ω for 1. Subtracting the common 0.100 Ω intrinsic base resistance gives the effective RCI directly. |
| Explicit zero base resistances | The reverse-noise input transfer resolves 0.020 Ω total across the intrinsic collector and base leads. The earlier RBX=0 base-noise transfer and the small XRB dependence of the RBX=0 temperature trace support a finite 0.010 Ω access lead. In the earlier RBP=0 run at Vc=0.02 V, reconstructing BX, SI and CI from measured terminal currents and lead drops gives a substrate-current residual of 178 pA with BP at CX and 0.016 pA with a 0.010 Ω pocket lead. |
| Reverse noise and flicker | At VBE=0 and IBCI=0, reverse transport is about 2.57 µA. A full reverse transport shot term alone would give about 0.91 nV/√Hz through the 1 kΩ readout. The measurement is 1.865 pV/√Hz. With A the measured gain, subtracting `A²·4kT·0.020 Ω` from the output power leaves the shot equivalent of 0.62 pA through 1 kΩ at all three IBCI settings. This equals GMIN times the 0.62 V pocket bias. The earlier base readouts at Vb=0.70 V and Vc=2 V also resolve the intrinsic 0.70 pA and pocket 1.30 pA terms. At AFN=0.7 including both predicts the measured 0.01136% increase in flicker power. Varying AREA and M resolves the per-instance GMIN term. |
| ART | Dividing the quadrature current by 2πf·CJC gives normalized capacitances 0.901276, 0.865278 and 0.837328 at junction biases −0.4, −0.5 and −0.6 V with ART=0.4. Differentiating the two-sided charge law reproduces all nine capacitances across ART=0.01, 0.1 and 0.4. The largest complex current error is 3.6 fA. |
| XRB | At 87 °C, increasing XRB from 0 to 2 changes the access-resistance transistor's collector current from 202.753 to 152.970 µA. The 500 Ω intrinsic-resistance device changes by only 1.37 nA, consistent with its explicitly zero RBX. The substrate-current trace is identical across all three settings. |
| QTF, TR, HRCF and heat | The nonunit-width probe confirms `TF·(1+QTF·w)` in high injection. The isolated pocket charge follows `TR·Fp` through 10 GHz. Zero HRCF leaves VO as the high-field limiting voltage. With a 200 Ω thermal shunt, RTH=0 gives exactly zero rise at every bias. Positive RTH of 100 and 500 K/W gives peak rises of 0.04964 and 0.10670 K. |

### Remaining differences

- The original 200 ps and 1 ns TD references abort at 2.00054 ns and 2.96875 ps. The Gear follow-ups also abort at the internal Q1 branch, at 2.300 ns (241 points) and 2.225 ns (233 points), with a 1.25e−23 s timestep. On the available 200 ps Gear prefix the largest current difference is 244 nA against a reference peak of 35.0 nA. Sansho completes the 14 ns Gear simulations. The nonzero-TD turn-on and settling waveform remains unverified.
- The output deck retains a 0.706 mV worst base-voltage deviation at Vc=0.17 V. Interpolation between adaptive thermal timesteps leaves a 5.09 µA collector difference on the pulse edge. Small DC and AC residuals persist around nonlinear operating points.
- The exact QBM tie, positive resistances below 0.01 Ω, other negative lead resistances and other untested parameter domains remain open. The XRB sweep tests the default mapping. Precedence when both XRB and XRBX appear on a card remains unmeasured.
- Separate noise measurements would be needed for WBE-split base drive, conducting pocket junctions and parasitic transport. AC leaves the additive equilibrium charge in QCO undetermined. Other open items include parasitic activation energies, hot GAMM, untested aliases and small-signal output conventions.

The parameter table and terminal conventions come from ngspice 44 §§7.3.1, 7.3.4 and 27.5.2. The physical basis is C. C. McAndrew et al., “VBIC95, the vertical bipolar inter-company model,” IEEE JSSC 31(10), 1996, and “BJT modeling with VBIC, basics and V1.3 updates,” MSM 2003. Epitaxial transport follows G. M. Kull et al., IEEE TED 32 (1985). R15b and R15c use the original project probes and their cached black-box outputs. [Probe comparisons](../verify/probes/README.md#r15b-comparison)
