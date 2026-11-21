# Native HICUM/L2 bipolar model

Sansho binds NPN and PNP cards with `LEVEL=8` to a native charge-based device. The interface targets HICUM/L2 **2.4.0**, the version in the supplied ngspice 44.2 parameter listing. R16c passes six acceptance decks and 276 probe settings against cached 44.2 waveforms. The isolated BC diode and its NPN mirror establish a known difference for the PNP acceptance deck.

## Interface and clean-room evidence

```spice
Q1 c b e s tj h area=1.5 m=2
.model h npn(level=8 version=2.4.0 flsh=2 rth=250 cth=20n)
```

The three-terminal form grounds the substrate. The fourth terminal exports the substrate and the fifth exports junction temperature rise in kelvin. An explicit ground clamps the rise to zero. When heating is enabled, an omitted thermal terminal allocates an internal heat-balance node. AREA scales a unit cell and M places independent copies in parallel. Model TNOM, instance TEMP and circuit TEMP are in °C. Without a model TNOM the circuit nominal temperature applies. The three original temperature-probe logs all reject instance DTEMP at parse time. In the repaired decks a fixed instance TEMP takes precedence at all 16 circuit temperatures. Collector-source currents stay at −2.95849 µA, −121.242 µA and −1.40393 mA for instance temperatures of −20, 27 and 70 °C.

The sources are the ngspice 44 manual §§7.3.2, 7.3.5 and 11.5.3.2, particularly circuit Fig. 7.3, M. Schröter and A. Chakravorty, *Compact Hierarchical Bipolar Transistor Modeling with HICUM* (2010), and S. M. Sze and K. K. Ng, *Physics of Semiconductor Devices*, 3rd ed. R16b used the supplied showmod/OP listing, independently written circuits and black-box terminal waveforms. A matching terminal measurement does not identify every internal parameter independently. [Nominal card](../crates/sansho-devices/src/hicum/params.rs)

The nominal 27 °C, 0.8 V BE, 1 V CE listing gives 2.70488 mA collector current, 27.083 µA base current, 2.96642 mA critical current and 2.49397 times the nominal intrinsic BE capacitance. Its 102.413 mS GMI also reveals a difference between 44.2's small-signal slope and the derivative of its DC curve, discussed below.

## Charge control and critical current

All scalar junction voltages in the physical model use forward-positive polarity. The two directional injection numerators are `Jf = (C10/QP0) exp(Vbe/(MCF VT))` and `Jr = (C10/QP0) exp(Vbc/VT)` at nominal temperature. The normalized weighted hole charge w divides both. Positive ICH enhances forward transport to `If (1+If/ICH)`. At 0.88 V the ICH=0 reference gives 59.62358 mA. Substitution of 1 mA predicts 3.6146 A in the ICH sweep. Its interaction with substantial forward storage remains unverified.

Define the rounded positive part `P(x,b)=(x+sqrt(x²+b²))/2`. At the origin, `P(0,2w ln 2)=w ln 2`, the value of the thermal softplus. This same rounding explains independent depletion, depleted-hole-charge and low-field measurements. With equilibrium charge e in units of QP0,

```text
wd   = e + (Hje Qje + HJCI Qjc)/QP0
base = 0.05 e + P(wd − 0.05 e, 0.1 e ln 2)
w    = base + (HF0 τ0 If + HFE Qem + [(1−FTHC) HF0 + FTHC HFC] Qhigh + TR Ir)/QP0
If   = Jf/w                       Ir = Jr/w
Qf   = τ0 If + Qem + Qhigh        Qr = TR Ir
```

At e=wd=1 the rounding gives 1.001263. Independently, dividing the ideal numerator by the observed collector current at 0.72 V gives 1.001263. The depleted-width sweeps approach the lower limit from positive charge. The solver uses a positive quadratic root for linear storage and implicit differentiation for nonlinear storage. For finite AHJEI, the AHJEI=1 and 4 curves independently give the average exponential weight `expm1(AHJEI x)/(AHJEI x)`. At 0.76 V the extracted depletion displacement x is 0.5942. The implementation estimates x as one minus the remaining normalized depletion width, with a rounded positive BE drop and a rounded distance to Vdei of thermal widths VT and RHJEI·VT. All three RHJEI settings pass the crossed heterojunction sweep. The independent DVGBE effect remains open. [Charge solution](../crates/sansho-devices/src/hicum/charge.rs)

For positive RCI0 and VPT, the nominal critical-current law is

```text
U   = VT + P(Vce − VCES − VT, 2 VT ln 2)
Ick = U / [RCI0 (1+(U/VLIM)^DELCK)^(1/DELCK)]
        × [1 + P((U−VLIM)/VPT, sqrt(AICK))]
```

This gives 2.966416 mA at the listing's bias. The DELCK=1 and 2 sweeps share the same emitter-storage law. Dividing the inferred critical currents removes the reach-through factor and recovers U. At Vce=−0.05, 0.10 and 0.50 V it gives approximately 27.7, 35.0 and 400.8 mV. Across the low-field crossover, `(U−VT)(U−Vce+VCES) ≈ 0.0003214 V² = (VT ln 2)²`. AICK controls the reach-through rounding. Its sweep changes the knee even at large forward Vce.

With c the normalized intrinsic BC capacitance, `τ0=T0+DT0H(1/c−1)+TBVL(c−1)`. The AC phase slope at Vbe=0.76 V gives 8.000 ps at zero correction, 8.498 ps at DT0H=4 ps and 5.786 ps at TBVL=20 ps. Emitter storage is `Qem=TEF0 r If (If/Ick)^GTFE/(1+GTFE)`, where r is the absolute temperature ratio. The low-VLIM measurements distinguish a thermal floor `VLIMeff = VT [1+ln(1+exp(VLIM (1−ALVS ΔT)/VT−1))]`. At nominal 0.10 V this gives 0.101432 V. After removing this knee from the hot-knee sweep, the inferred emitter-storage coefficient divided by r agrees across all three ALVS settings at 120 °C to the displayed precision. These temperature probes use GTFE=1. For the measured base push-out component,

```text
z     = 1 − Ick/If
d     = (z + sqrt(z²+AHC)) / (1+sqrt(1+AHC))
Qhigh = THCS If d²                          (zero collector spreading contribution)
```

The squared fraction reproduces the storage extracted by inverting the FTHC=0 Gummel curve below, at and above the knee. FTHC defaults to zero in the listing. All three HFC and LATB settings give identical reference currents at default FTHC. The lateral polynomial used for finite collector partition remains an approximation. TBHREC diverts the excess base charge `(1−FTHC)Qhigh` into BE recombination. The ICBAR settings at fixed VCBAR and ACbar give identical output currents throughout their sweep. The native evaluator applies no additional barrier attenuation in this round.

## Junctions, base resistance and substrate

* Each BE branch sums ideal and recombination diodes. Separate intrinsic and peripheral BC diodes terminate on the internal collector. GMIN shunts are per instance.
* For grading z, ceiling A and built-in potential Vd, set `k=Vd(1−A^(−1/z))`, `x=k−P(k−v,2 VT ln 2)`. The junction charge is the integral of the graded capacitance up to x plus `A C0 (v−x)`. Differentiating this expression gives 2.493975 times C0 for the listing's BE point. The BC reference at Vbc=0.96 V gives 2.39996 times C0. The BC/SC ceiling is 2.4. The nominal VPT=100 V card uses the ordinary graded junction. With finite reach-through, the exponent changes from z to z/4 near `kpt=Vd−VPT`. Write J and H for the two forward-limited charge integrals. Scale H by `(Vd/VPT)^(3z/4)` to match the two graded capacitances at kpt. With `b=4 VT + (VPT−Vd)/10` and `x=kpt+b ln(1+exp((v−kpt)/b))`, the charge is `J(x)+H(v)−H(x)` up to its bias-independent reference. Its voltage derivative is `H′(v)+x′[J′(x)−H′(x)]`. The sharp limit applies when b is nonpositive. Both collector and substrate use this integral and its derivative.
* At VPTSP=1 V and Vd=0.6 V, the transition spans about 0.14 V. At substrate biases of −0.55, −0.40 and −0.25 V the measured capacitances are 0.76454, 0.79027 and 0.83977 pF. The integral gives 0.76474, 0.79065 and 0.84043 pF. The independent −0.35 V point gives 0.80357 pF measured and 0.80404 pF calculated. These measurements constrain the nominal transition width. Its variation with temperature remains unverified.
* FBCPAR and FBEPAR specify the fraction attached inside the external base resistance. At FBEPAR=0 the high-frequency base current includes the full external 2 pF. At FBEPAR=1 the base resistance limits that current. The peripheral share of `CJCX0+CBCPAR` takes depletion first. At FBCPAR=0.5 with CJCX0=1 pF and CBCPAR=0.5 pF, the external contact gets 0.25 pF of nominal depletion and the full 0.5 pF of overlap. This resolves the middle setting as well as both endpoints.
* The reverse BE current magnitude is `IBETS (−v/Vd) F exp(−ABET/F)`, with `F=(1−v/Vd) C(v)/C0`. The ratio of the ABET=2 and 6 runs gives F=2.3332 at −4 V. Dividing out the exponential gives 10.37 times IBETS, equal to `(4/0.9)F`. TUNODE=0 places the branch at the intrinsic base and 1 at the peripheral base.
* For weak avalanche, put `u=Vdc−Vbc`, `u0=QAVL/Cjc0`, and `a=QAVL/Cjc(Vbc)`. The measured multiplier divided by FAVL is `u exp(−a/u)` for positive u up to u0. Above u0 it is `exp(−a/u0)[u0+(1+a/u0)(u−u0)]`. At Vc=5 V the 10 fC probe gives 1.5241 V and the 30 fC probe gives 1.0325 V. The ALFAV sweep ratios follow `exp(ALFAV ΔT)`. Let g be this weak multiplication including FAVL. Avalanche carriers add to the primary current through the KAVL feedback, giving the denominator `1−KAVL g` before regularization. For positive KAVL the measured denominator is `D=P(1−KAVL g,0.1)`, and `Iav=If g/D`. This rounding can be recovered independently at each bias. Subtract the matched base injection from the source readout, form `D=Iav(0)/Iav(KAVL)`, then compute `D(D−1+KAVL g)`. It gives 0.00250 on both the 0.5 and 2 sweeps over 0.7–5 V. At 5 V the base-source currents grow from 0.81753 mA to 0.84784 and 0.96221 mA. KAVL=0 retains weak multiplication directly. The data sample positive feedback below breakdown at nominal temperature.
* With `Rsheet=RBI0(T)(1+FDQR0)/(b+(Qf+Qr)/QP0+FDQR0)`, where b is the rounded unweighted depletion charge, and `x=FGEO Ibei Rsheet/VT`, the measured spreading resistance follows `FQI Rsheet ln(1+x)/x`, taking the limit at zero. At FQI=0 the intrinsic and peripheral base collapse. At the highest FCRBI probe bias the observed resistance is 0.3490 RBI0 and the logarithmic factor gives 0.3497. All three FCRBI settings agree. The low-frequency base and collector gains in the 200 Ω FDQR0 sweep imply internal DC resistances of approximately 67.6, 86.7 and 100.9 Ω at its three settings. All three FDQR0 settings also pass the independent DC bias sweep.
* The lateral transistor carries `Isub [exp(Vbx/(MSF VT))−exp(Vsc/(MSF VT))]` between peripheral base and internal substrate. At nominal temperature, the effective saturation coefficient divided by ITSS is 0.160705, 0.133920 and 0.107140 at MSF=1, 1.2 and 1.5. This identifies the nominal logarithmic temperature derivative of saturation current, `(ZETACT+Eg/VT)/(MSF T)`, with no fitted prefactor. R16b extends the derivative to operating temperature, using the mean collector/substrate gap. All three MSF settings pass the independent temperature sweep. ISCS sets the separate SC diode, TSF stores forward substrate diffusion charge on peripheral BC, and RSU and CSU connect the internal substrate to its contact.

## Temperature, dynamics and noise

With `r=T/Tnom`, the measured transport saturation multiplier is `r^ZETACT exp[VGB(r−1)/VT]`. The ideal forward BE multiplier also uses VGB, with a further `exp(−ALB ΔT)`. Recombination divides the activation and log-power term by its ideality. At 120 °C and default parameters the BE current divided by its nominal prefactor and activation gives 2.24732, equal to r³. The F1VG sweeps leave this base current invariant. Regional gaps and F1VG/F2VG enter junction potentials through `Eg(T)=Vg+F1VG T ln(T)+F2VG T`. A zero-bias capacitance scales as `(Vd0/Vd(T))^z`. Inverting the gap and regional-gap transport sweeps gives `e=1+(1−Vdei(T)/Vdei0)/4` at the listed ZEI=0.5. The implementation scales this correction with ZEI/2. Across the three ALVS settings and 16 temperatures the maximum absolute Ick discrepancy is 49.1 pA. The ZEI dependence of the equilibrium charge remains unverified.

The NQS time is the differential transit time `Tf=∂Qf/∂If` at fixed field. With `L=ALIT Tf`, the transport transfer is `1/[1+sL+(sL)²/3]`. At 3.162 GHz with T0=50 ps and ALIT=1, the reciprocal transfer is 0.671013+j0.993459. Forward charge has the first-order transfer `1/(1+s ALQF Tf)`. The implementation uses two real states for transport and one for charge. Positive T0 activates the delay. The original 425-point FLNQS=0 and 1 references are identical. The fT acceptance card separates differential time (about 8.286 ps) from storage divided by current (8.143 ps).

Positive RTH and FLSH enable `CTH dΔT/dt+ΔT/RTH=P`. Mode 1 uses main intrinsic dissipation and mode 2 sums electrical branch powers. The heat port scales with AREA·M. At 0.88 V with an omitted pin, the 0, 200 and 500 K/W references draw 16.472, 17.817 and 20.409 mA. With CTH omitted, dividing the residual `P−ΔT/Reffective` by the first backward difference of ΔT after the falling corner gives 0.99982 and 0.99989 pJ/K for RTH=200 and 500 Ω. The native heat balance uses 1 pJ/K when CTH is zero. Charge LTE alone can accept large voltage increments on a pulse edge. For a linear voltage ramp an exponential current's relative midpoint interpolation error is `cosh(ΔV/(2 nVT))−1`. The transient sampler bounds forward junction increments using RELTOL. On the thermal acceptance deck this resolves the 21.104 µs collector-current sample while retaining adaptive steps on the plateau.

One-sided current-noise PSDs use `2q|I|`. The transport source uses net transfer, BC and SC shot terms include the junction shunt currents, and the lateral transfer adds no separate shot source in the substrate readout. In the zero-IBEIS probe the two reverse BC shunts account for the 0.392 pV/√Hz output density through 1 kΩ. Base flicker uses the sum of intrinsic and peripheral base injection at the peripheral base. The CFBE spectra agree for both settings. Parallel copies sum independently. Noise referral uses the upper-frequency gain for each integrated output-power interval. This reconstructs 0.3236775 V RMS in the shot probe and 45.1438 µV RMS in the substrate probe from their measured spectra. [Noise analysis](analyses.md)

## PNP polarity and small-signal evidence

The published device uses forward-positive BE and BC voltages and signs each terminal current with transistor polarity. Sansho retains that law. The 44.2 PNP breaks complementary symmetry even for a single BC diode. In the paired probe C10 and IBEIS are zero, IBCIS is 10⁻¹⁶ A and the two devices have equal and opposite applied voltages. At reverse BC bias of magnitude 0.6 V the reference gives −53.8926 µA on the PNP base sense source and −0.4001 pA on its NPN mirror. Sansho gives +0.4001 pA and −0.4001 pA. Tripling IBCIS raises the 44.2 PNP magnitude to 161.678 µA. At zero IBCIS both simulators agree on the shunts.

The original acceptance sweep at Vc=−1 V also has a recognizable terminal signature. With `u=Vb−Vc`, `j=AREA M IBCIS exp(u/VT)` and w the nominal rounded hole charge, the base-source current follows `j(2u/VT−1)`. The collector-source current follows `−i(Vb)−(j/w)(2u/VT−1−2 Vbe²)` with voltages in volts. The maximum errors across its 25 rows are 2 pA and 0.66 nA respectively. These empirical expressions describe the 44.2 readout along that bias line.

With both base junction currents disabled and zero emitter resistance, the 44.2 collector-source current goes from −53.445 µA to +1.170 pA as forward emitter bias increases from 0.40 to 0.88 V at fixed −1 V collector bias. Three-, four- and five-pin devices give identical reference curves in each pin-count sweep. At −0.4 V drive the 100 Ω load gives a base voltage of −0.39279194 V. The resulting 72.0806 µA resistor current agrees with the 44.2 source readout, so the discrepancy affects the solved circuit. The 1 kΩ reference stayed in the operating-point fallback for nine hours and the operator killed it before the first row. The comparison records oracle non-termination.

At 0.76 V BE the zero-storage NPN gives 21.8364 mS at 1 Hz and 22.2744 mS from the centered DC difference. For the electrical AC and noise tangent the BE contribution to `d ln(Qbase)` is `Vbe dVbe`. The BC contribution follows the GICCR depletion-charge derivative. Increasing CJCI0 from 5 to 15 to 30 fF gives incremental logarithmic slopes of about 0.235, 0.799 and 2.00 per volt in the matched avalanche-on/off experiment. Including this derivative in the gain also reproduces the input-referred noise from the same branch shot powers. DC and transient use the derivative of the charge balance.

## Interface diagnostics and verification

The three ratings VBE_MAX, VBC_MAX and VCE_MAX are checked with `.option warn=1` on accepted OP, DC and transient samples. The SOA logs identify VCS_MAX as an unrecognized model parameter. Sansho emits W-MODEL-IGNORED for it. Explicit FLCOMP, FLCONO and ZETACX are accepted. Their sampled settings produce identical reference waveforms within each family. The device keeps positive-area, finite-parameter and positive-temperature diagnostics. The probe matrix covers measured parameter acceptance and leaves untested ranges open.

Terminal outputs include signed `@q[ic]`, `ib`, `ie` and `is`, internal voltage drops, junction currents, `gm`, `gmi`, `go`, `cpi`, `cmu`, `ick`, differential `tf` and dissipation `p`. Transient terminal outputs include discharge and charging currents. DISTO reports E-ANALYSIS.

R16c has six passing acceptance decks and one quantitative PNP known difference. Of 287 probe settings, 276 pass, ten have measured PNP differences and one has no completed oracle reference. The zero-injection control, mirrored NPN signals and all completed non-PNP settings are scored normally. In the knee-temperature logs 44.2 reports four columns and 16 rows. Its raw writer labels the time vector `v(@q1[tf])` while Sansho writes `@q1[tf]` in seconds. Comparing the actual samples gives a maximum absolute Tf error of 0.316 ps across the three sweeps. The KAVL temperature dependence, the punch-through width away from nominal temperature and feedback near breakdown remain open. [Reproduction and timeout handling](../verify/probes/README.md#r16c-cached-results)
