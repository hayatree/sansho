# Native VBIC bipolar transistor

A level-four NPN or PNP card selects the native model for OP, DC (including a temperature axis), AC, noise and transient analysis. R15 targets the VBIC 1.2 interface in ngspice 44. Seven native acceptance decks, 64 probe templates and 189 individual probe runs exercise these equations. All acceptance waveforms replayed bit-for-bit with one and two workers. A 257-transistor test also exercises parallel assembly. R15b compares all 189 settings to cached ngspice 44.2 black-box measurements. All 187 completed references and all seven acceptance decks pass. Two reference transients aborted. The separate Verilog-A route remains available for cards targeting VBIC 1.3. [Acceptance decks](#acceptance-decks) · [Probe index](../verify/probes/README.md#r15-native-vbic)

## Terminals and scaling

```spice
Q1 c b e model
Q2 c b e s model area=2 m=3
Q3 c b e s tj model
.model model npn(level=4 selft=1 rth=200 cth=60n)
```

The omitted substrate connects to ground. The optional fifth pin represents junction rise in kelvin above ambient. With SELFT=1 and positive RTH, a connected fifth pin receives the dissipated power. Omitting it leaves the device at ambient temperature. A fifth pin tied to 0 clamps the device to ambient. With heating disabled, the fifth pin draws zero current. A model can therefore share a temperature-rise net with external thermal elements. Ngspice 44 §§7.3.1 and 7.3.4 give the terminal and heating conventions.

Define `a = AREA·M`, defaulting both factors to one. Equations below describe one unit transistor. Sansho multiplies electrical current, stored charge, thermal conductance and thermal capacitance by a. This puts unit resistances in parallel and retains knee current densities. The noise section gives the separate AREA and M laws. AREA also accepts the positional spelling after the model. The junction ambient in °C is (instance TEMP or circuit temperature) + instance DTEMP + model DTEMP. TNOM and TREF name the extraction temperature. IC accepts a B–E/C–E tuple, and ICVBE and ICVCE can specify the voltages separately.

The internal paths are C–RCX–CX–RCI–CI, B–RBX–BX–RBI–BI, E–RE–EI, S–RS–SI and CX–RBP–BP. RBI, RCI and RBP default to 0.1 Ω. A specified zero merges the two nodes in Sansho. Internal voltages below use NPN polarity, so each voltage difference and electrical branch current reverses sign for a PNP.

## Transport, base and substrate currents

Let `D(v,I,n) = I·expm1(v/(n·VT))` with `VT = kT/q`. In the native VBIC voltage law, k/q uses the historical rounded constants 1.380662e−23 J/K and 1.602189e−19 C. Dividing the intrinsic base voltage by T·log1p(F/IS) on the default-card sweep gives 8.61734789e−5 V/K from 0.60 through 0.80 V after the base-lead and GMIN corrections. This identifies the ratio. The noise prefactors use the workspace SI constants. Define `F = D(Vbi,ei, IS, NF)` and `R = D(Vbi,ci, IS·ISRR, NR)`, where the comma between node names denotes their voltage difference. Let `Je` and `Jc` be the emitter and collector depletion integrals divided by their zero-bias capacitances. The depletion functions have units of volts and remain defined for zero junction capacitance.

```text
w = 1 + Je/VER + Jc/VEF
h = F/IKF + R/IKR
qb = (w + (w^(1/NKF) + 4h)^NKF)/2        for QBM=0
qb = w·(1 + (1 + 4h)^NKF)/2               for QBM=1
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

For junction potential P and grading m, integrate `(1−v/P)^(−m)` and subtract the zero-bias integral. FC sets the forward join at `c = FC·P`. A nonpositive AJ continues the capacitance linearly above c. For positive AJ, use the smoothed lower voltage `z = c + (v−c−sqrt((v−c)²+4AJ²))/2`, integrate at z, and add `(v−z)·(1−FC)^(−m)`. A positive VRT introduces a lower join at −VRT with constant capacitance beyond reach-through for nonpositive AJ. Sansho smooths this lower join with ART for positive AJ. The combined AJ/ART shape awaits measurement. The reach-through option applies to the intrinsic B–C depletion charge (CJC), as specified in §27.5.2.

CJE and its WBE fraction follow the two B–E junctions. CJC attaches to BI–CI, CJEP to BX–BP and CJCP to SI–BP. CBEO, CBCO and CCSO are linear external overlaps. BI–EI stores `TF_eff·If`, BI–CI stores `TR·R` and BX–BP stores `TR·Fp`. The forward transit time is

```text
TF_eff = TF·(1 + QTF·w)·(1 + XTF·u²·exp(Vbi,ci/(1.44·VTF)))
u = F/(F + ITF) for F > 0, or 1 when ITF is zero
```

At nonpositive F, u is zero. Zero VTF removes the exponent. Section 27.5.2 lists TD in seconds. Positive TD filters forward transport with `H(s) = 1/(1 + s·TD + (s·TD)²/3)`. Two internal DAE states implement the same transfer for transient and AC, with unit DC gain. The charge equation uses undelayed If.

## Temperature, self-heating and noise

For absolute temperature ratio r = T/Tnom and temperature change ΔT, define `L(I,x,E,n) = I·exp((x·log(r) + E·(r−1)/VT)/n)`. Transport uses XIS and EA, parasitic transport uses XIS and EAP. The ISRR multiplier uses XISR and DEAR. Ideal junctions use XII with EAIE, EAIC or EAIS. Recombination uses XIN with EANE, EANC or EANS. NF and NR receive the factor `1 + TNF·ΔT`, and AVC2 receives `1 + TAVC·ΔT`. VBBE uses the quadratic factor from TVBBE1/2, and NBBE the linear factor from TNBBE.

Each lead resistance uses `Rnom·r^X`. XRB supplies the default for XRBX, XRBI and XRBP. XRC supplies the extrinsic exponent XRCX. XRCI defaults independently to zero. The emitter and substrate use XRE and XRS. VO and IKF use XVO and XIKF. GAMM scales by `r^XIS·exp(EA·(r−1)/VT)`. For each depletion junction Sansho uses the corresponding ideal activation energy E, `P(T) = E + r·(Pnom−E) − 3VT·log(r)`, and `C(T) = Cnom·(Pnom/P(T))^m`. The electrical, charge and temperature derivatives are evaluated together with forward-mode differentiation.

Section 7.3.4 couples dissipated power to a parallel RTH/CTH. For junction rise θ, the current into the thermal pin is `a·(θ/RTH + CTH·dθ/dt − P)`. P sums voltage times static current over all physical electrical branches, including contact losses. Displacement currents, numerical GMIN shunts and phase-control rows contribute zero heat. At DC an unloaded junction reaches θ = P·RTH. All electrical branches see the resulting temperature at each Newton iterate.

Shot spectra are one-sided `2q·|I|`. The collector source uses If, and the B–E junctions use their respective diode currents. Avalanche enters the DC current and small-signal gain. The measured base noise excludes shot noise proportional to the net avalanche-modified B–C current. Sansho also includes the parasitic transport and pocket junction sources. Each finite lead contributes `4kT·g`, with bias-dependent base conductance and the low-field conductance for the epitaxial lead. At BI–EI and BX–EI the device flicker power for each branch is `M·KFN·|AREA·Ibe|^AFN/f^BFN`, with Ibe evaluated per unit area. The two sources are independent in Sansho. No thermal-port fluctuation source or avalanche excess-noise factor has been fitted.

## Outputs and diagnostics

The Gummel–Poon output set is available as `@q1[ic]`, `ib`, `ie`, `is`, `gm`, `go`, `gpi`, `gmu`, `cpi` and `cmu`. Terminal currents include their charge derivatives in transient. The current sum cancels across the four electrical terminals even with internal lead drops. The conductances and capacitances describe intrinsic partial derivatives at the operating point. VBIC also exposes intrinsic `vbe`, `vbc` and dissipated power `p`.

Both values of UNKNOWNFIT trigger an ignored-key warning in 44.2. All three explicit EBBE values are accepted silently. With IBBE=1 µA, increasing EBBE from zero to 1e−6 shifts i(Vb) at zero bias by −1 pA. Sansho matches both recognition and current. Numeric VERS and VREF are metadata. Independent AREAB and AREAC are outside the §27.5.2 instance interface. Invalid model selectors, nonpositive AREA/M or invalid emission coefficients give `E-ELAB-PARAM`. A nonpositive operating junction temperature or a request for higher-order DISTO derivatives gives `E-ANALYSIS`. [Diagnostic catalog](diagnostics.md)

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

### Remaining differences

- The 200 ps TD reference steps from −8.204 µA at the operating point to picoamps at the first transient sample. It aborts at 2.00054 ns. The 1 ns reference aborts at 2.96875 ps. Both logs name the internal branch and a 3.125e−23 s timestep. Sansho completes both runs with continuous initial transport. Neither trace establishes a delayed turn-on waveform.
- The output deck retains a 0.706 mV worst base-voltage deviation at Vc=0.17 V. Interpolation between adaptive thermal timesteps leaves a 5.09 µA collector difference on the pulse edge. Small DC and AC residuals persist around nonlinear operating points.
- Reverse-only noise, the separate pocket sources, and flicker correlations with split base drive need independent readouts. AC cannot distinguish an additive equilibrium constant in QCO. Parasitic TR and the QTF width factor require probes that excite those terms separately.
- The existing sweeps do not isolate positive AJC with ART, zero HRCF deep in the high-field regime, shared XRB precedence, all parasitic activation energies, or hot GAMM. External heating with zero RTH, untested card aliases, small-signal output conventions and invalid parameter domains remain open. Sansho retains its stated parameter checks and exponential/positive-domain continuations outside the measured range.

The parameter table and terminal conventions come from ngspice 44 §§7.3.1, 7.3.4 and 27.5.2. The physical basis is C. C. McAndrew et al., “VBIC95, the vertical bipolar inter-company model,” IEEE JSSC 31(10), 1996, and “BJT modeling with VBIC, basics and V1.3 updates,” MSM 2003. Epitaxial transport follows G. M. Kull et al., IEEE TED 32 (1985). R15b uses the original project probes and cached black-box outputs as its numerical evidence. [Probe comparison and next measurements](../verify/probes/README.md#r15b-comparison)
