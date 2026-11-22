# R14 classical transistor models

The reference measurements are the maintainer's ngspice 44.2 terminal waveforms and logs. The equations below describe Sansho's independent reconstruction. Absolute temperatures are in kelvin unless an input unit is given. The MOS currents use forward, polarity-normalized intrinsic voltages. Reverse conduction exchanges the two channel ends. No acceptance tolerances were changed. [Acceptance decks](../verify/README.md#r14-classical-models) · [Measurement decks and rerun commands](../verify/probes/README.md#r14b-reference-runs)

The R14c cache covers 484 completed waveform settings in the current probe templates. The unmodified R14b binary passed 411. Sansho now passes 457, including every previous pass. The four control settings and three rejected JFET2 settings are checked separately. The acceptance suite has nine passes and two known inverter current differences.

| Probe family | Passing / completed | Outside waveform tolerance |
|---|---:|---|
| JFET2 | 120/120 | |
| MOS9 | 45/45 | |
| BSIM1 | 98/110 | 12 noise settings |
| BSIM2 | 164/179 | 12 noise and three coarse transient settings |
| VDMOS | 30/30 | |

## Junction FET level 2

Parker and Skellern, “A realistic large-signal MESFET model for SPICE,” IEEE T-MTT 45(9), 1997, describes dispersive gate feedback, different linear and saturation powers, junction charge and heating. Manual §§7.4.1 and 7.4.5 identify the parameters. The numerical conventions below come from the P, Q, XI, Z, transfer, admittance and heating sweeps.

Let v and w be gate-to-source and gate-to-drain voltages after orientation, d = v − w ≥ 0, and s and t their respective first-order low-pass values with time constant TAUG. At DC, s = v and t = w.

```text
low  = LFGAM − LFG1·s + LFG2·t
high = HFGAM − HFG1·v + HFG2·w
eta  = −HFETA + HFE1·w − HFE2·v
u = v − VTO − low·t + high·(t − w) + eta·(v − s)
h = VST·(1 + MVST·d)
U = h·log(1 + exp(u/h)) for h > 0, otherwise max(u,0)
B = VBI − VTO
L = (Q/P)·U·(U/B)^(Q−P)
k = 1/(1 + U/(B·XI))
Icold = BETA·AREA·M·U^Q·[1 − (1−e)^Q]
```

For positive drive and drain bias the smaller positive root e satisfies
`[(k/e)²−1]·[(d/(L·e))²−1] = Z/(1+Z)`. Set the current to zero at cutoff and use the continuous small-drain limit at reversal. One can recover e directly from each measured current by taking the Qth root of the residual saturation fraction. At U = 0.6 V, XI of 0.5 and 2 gives k of 0.68421 and 0.89655. At U = 1.1 V, P of 1.4 and 2.6 gives L of 0.93788 and 1.41774 V. The product on the left is 0.2, 1/3 and 0.5 for Z of 0.25, 0.5 and 1. The omitted Z follows the Z = 1 curve. At VST = 0.06 V and MVST = 0.3/V, the inferred subthreshold widths are 0.06216, 0.08016 and 0.09816 V at drain biases 0.12, 1.12 and 2.12 V.

TAUD filters the normalized cold power `H = DELTA·Icold·d/(AREA·M)`. The hot current is `Icold/(1+H)`. At U = 1.6 V and d = 2.12 V, raising DELTA from zero to 80 reduces the normalized current from 2.507695 to 1.932395. The AC sweeps distinguish gate from drain excitation. For example, HFG2 = 0.03 changes high-frequency drain conductance from +9.57 to −89.36 µS.

The junction current apart from GMIN is `IS(T)·expm1(v/(N·VT)) − IBD·expm1(−v/VBD)`, scaled by AREA·M. At −4.8 V with VBD = 4.5 V the increment per IBD is −1.90568. Model and instance TEMP are Celsius. TNOM is the junction measurement temperature. The IS ratio is `exp[1.11·(T/Tnom−1)/(N·VT)]`. An instance at 85 °C gives 1.043e−11 A from 1e−14 A nominal. Model TEMP = 300 gives 7.542e−6 A. RD and RS divide by AREA·M. The 44.2 logs flag AFAC, CDSS, LD, LG, LS, NFING and RG as unrecognized, and the available setting pairs give identical curves. Sansho warns and ignores those inputs.

For charge, define the splitting width `wq = hypot(d, B/(2·(1+1/XI)))`. The upper and lower gate voltages are `(v+w±wq)/2 + ACGAM·d`. Apply the Statz rounded cutoff with width 0.2 V to the upper voltage, then the square-root CGS junction primitive with the FC forward continuation. CGD contributes linearly at the lower voltage. The two gate branches integrate the change in the primitive along the two rectangular paths between accepted bias points and average their contributions. At Vgs = 0.45 V and Vds = 1.8 V the lower term sends 0.884268 of its incremental charge to the drain. With CGS = 3 pF and CGD = 1 pF the total gate capacitance is 5.55430, 6.18909 and 6.71026 pF at FC of 0.25, 0.5 and 0.75. In the drain-excited probe at Vgs = −0.6 V, ACGAM of 0, 0.1 and 0.3 gives Cgd of 1.22494, 0.78560 and −0.28602 pF. For upper voltage h, set `p=(XC−1)(h−VTO)` and evaluate the CGS primitive at `h+(p+hypot(p,0.2 V))/2`. At Vgs = −1.65 V, the gate-driven total is 3.100317, 3.111849 and 3.238156 pF for XC = 0, 0.2 and 0.6. The same law gives the much smaller response at Vgs = −0.6 V. [Parker–Skellern charge equations, pp. 4–5](https://ngspice.sourceforge.io/external-documents/models/psfet.pdf)

All three ID=PF1 runs stop in parameter substitution at model line 8 with undefined parameter PF1. They produce no device measurements. Sansho now sends ID through normal model expression resolution and reports the undefined name at its source location. These logs establish the bare-token behavior only.

## MOS level 9

Manual §§7.6.2.6 and 27.6.5 and the paired MOS3/MOS9 experiments support using Sansho's MOS3 equations with `Leff = L + XL − 2·LD` and `Weff = W + XW − 2·WD`. The mask offsets are in metres. DELVTO (alias DELVT0) adds to the signed card VTO before polarity normalization. Both polarity probes and the output acceptance deck confirm this convention. With VMAX = 0 both devices show the same channel shortening. Divide the unshortened triode current by the reference current and subtract the result from one. The inferred fractional length loss scales as the fourth power of d below saturation. Above the knee, squaring the inferred loss recovers a linear function of d. With `s=Vsat` and `h=XD·sqrt(KAPPA)/Leff` (XD here denotes the derived depletion length), the branches are `h·sqrt(s/8)·(d/s)^4` and `h·sqrt(d−s+s/8)`. Equating the two slopes fixes the offset s/8. The four gate sweeps give offsets of 0.1250–0.1252 times s. The paired output residual is below 54 pA.

Each of XD, ALPHA and INPUT_DELTA caused 44.2 to reject the level-9 card. Sansho reports a model error for these inputs. The KAPPA = 0 run parsed successfully and stopped after three DC samples in the paired circuit. Its log identifies the dependent-source branch. Positive KAPPA settings remain valid model inputs. The repaired NSUB, TOX, DELTA and positive KAPPA sweeps all pass against their fixed MOS3 controls.

## BSIM1 and BSIM2

The channel and charge constructions follow Sheu, Scharfetter, Ko and Jeng, IEEE JSSC 22(4), 558–566 (1987), and Jeng, UCB/ERL M90/90. The ngspice 44 manual §§7.6.3 and 27.6.6–7 provide input meanings and units. The following parameter conventions and numerical examples were inferred from paired black-box experiments.

TOX, DL and DW are in micrometres. A binned parameter is `p0 + lp/Leff_um + wp/Weff_um`. At L = 1.2 µm a 0.2u change of LVFB shifts threshold by 0.1667 µV. Doubling L halves the shift. The oxide capacitance per area is `3.453e−11/TOX_m`. Let b, d and g denote body, drain and gate bias relative to the source. BSIM1 caps b at zero. BSIM2 uses `max(b,2·VBB)`, `min(d,2·VDD)` and `min(g,2·VGG)`. Omitted VBB gives +5 V and hence a +10 V body coordinate on the acceptance cards. For instance the relative changes of reciprocal current with UAB, UBB and UB0 at g = 1.8 V, d = 0.2 V give b = 10 V and overdrive 1.567034 V. At external b = −0.25 V the VBB = −1 and −0.25 curves coincide while VBB = −0.1 clamps b to −0.2 V.

```text
S = phi − b,                 r = sqrt(S)                  for b ≤ 0
S = phi/(1+b/phi),           r = sqrt(phi)/(1+b/(2·phi))  for b > 0
u = g − VFB − phi − K1·r + K2·S + eta·d
A = 1 + K1/(2r)·[1 − 1/(1.744 + 0.8364·S)]
Vsat = 2·(drive/A)/(1 + sqrt(1 + 2·F·drive/A))
v = min(d,Vsat)
Idrift = beta(d)·v·(drive − A·v/2)/denominator
```

Solving the measured triode and saturation currents for A gives `1/[1−(A−1)·2r/K1]` of 2.329480, 2.622220 and 2.914960 at S of 0.70, 1.05 and 1.40 V. This determines the intercept and slope above. Beta includes mobility, oxide capacitance, effective aspect ratio and multiplier.

**BSIM1.** Above threshold, drive = max(u,0), vertical = max(1,1+U0·drive), F = max(0,U1)/Leff_um and denominator = vertical·(1+F·v). The X2 coefficients multiply b. The X3E and X3U1 coefficients multiply d−VDD. Eta is nonnegative. The mobility is the quadratic through the body-adjusted MUZ and MUS at 0 and VDD with endpoint derivative max(0,X3MS), followed by the endpoint tangent. MUS defaults to zero. A separate diffusion current adds to drift at all gate biases. With beta0 based on MUZ, it is
`4.5·beta0·VT²·x/(4.5+x)`, where `x=(1−exp(−d/VT))·exp(1.8+u/(max(0.5,N0+NB·b+ND·d)·VT))`. For the output acceptance card this leaves about 0.6514 µA at VDD even in strong inversion. Both DC decks agree within 3 pA.

The original inverter exposed two effects. With MUS omitted, the drift mobility falls quadratically to zero at VDD, producing negative differential conductance. Near *zero* drain bias the diffusion term crosses to its plateau over a voltage of order `4.5·VT·exp(−1.8−u/(n·VT))`. With a strongly on PMOS, this is below the spacing of representable node voltages near the 3 V rail. The fitted equations are continuous in real arithmetic. In 44.2 the original deck failed stepping, found a transient operating point and aborted at 3.0849 ps. Sansho on the original card fails its initial operating point with out = 2.998367 V. Specifying MUS alone also failed the Sansho operating point. The revised deck sets MUS to the measured low-field mobility on each card and adds a symmetric 1 MΩ load to each rail. The 3 µA rail load exceeds the limited diffusion current. Both simulators complete the revised 12 ns transient.

The rail sweep gives 0.651528 µA at +0.25 µV and 0.651626 µA at +0.50 µV. The finite-difference slope is 391 µS for all three MUS settings. Extrapolation gives the 0.65143 µA diffusion plateau. On the sparse card with VDD and MUS omitted, Vg = 1.2 and 2.4 V give the same 0.748452 µA plateau from the first positive sample (25 mV). Sansho uses the zero-endpoint limit of the mobility fit for omitted BSIM1 VDD. These data bound the endpoint below the first resolved drain step. They cannot distinguish zero from a smaller positive endpoint.

**BSIM2.** The low and high mobility fits use MU0/MU0B and MUS0/MUSB. The high intercept has a floor of 1.01·MU0, with the high body slope bounded so high ≥ low at VBB. Let `p(d) = d·(MU3−MU4·d)`. Mobility is the positive part of `low + [high−low−p(VDD)]·tanh(MU2·d/Vsat) + p(d)`. MU2, MU3 and MU4 have independent constant, b and g terms. Their gate coordinate is the gate voltage. The curves with omitted MU20, MU30 and MUS0 are consistent with 1.5, 10 and 500 respectively. For MU0 = 470 and VDD = 3 the residual tanh coefficient vanishes and the apparent mobility is 470 + 10·d. Unequal endpoints expose the tanh argument.

Vertical = max(0.2,1+UA·drive+UB·drive²). The saturation field uses F = max(0,U10+U1B·b)/vertical. The triode denominator is `max(0.5,vertical + d·(U10+U1B·b)·[1−U1D·(1−d/Vsat)²])`. Above saturation it is vertical·(1+F·v). The hot-electron multiplier above pinch-off is `1 + AI·exp(−BI/(d−Vsat))` with separate body fits. Negative BI in the extreme BIB sweep gives currents exceeding 1e59 A. At gate 0.65 V and drain 0.2 V the current magnitude is 3.36315e59 A. Evaluating the representable exponential reproduces the sweep. Nonfinite channel currents or derivatives generate a numerical diagnostic.

The diffusion current is `beta(d)·VT²·exp(VOF+u/(n·VT))·(1−exp(−d/VT))`, with `n=N0+NB/r+ND·d`, default NB = 0.5, and `VOF=VOF0+VOFB·b+VOFD·d`, default VOF0 = 1.8. The measured exponential slope gives `VT=(TEMP+273.15)/11600` for model extraction temperature in Celsius. Below VGLOW the effective drive is `sqrt(2A)·VT·exp((VOF+u/(n·VT))/2)`. Between VGLOW and VGHIGH a cubic joins the weak and strong endpoints and their slopes. Its lower endpoint includes the square root of the diffusion drain factor. Omitted joins are −0.15 and +0.2 V. Both DC decks pass. At 1.7 V drain bias on the sparse VFB = −0.3 V card, successive weak-inversion current ratios give n−0.5/r = 1.4001. The independent sparse drain sweeps give saturated currents proportional to 450 + 10·d, consistent with VDD = 5 V and a vanishing residual mobility coefficient. Dividing the triode numerator by measured current gives denominators 1.192225 and 1.194725 at 25 and 50 mV for g = 1.2 V. Increasing g to 2.4 V adds exactly 0.240000. With the measured threshold these identify UA0 = 0.2/V, UB0 = 0 and U10 = 0.1/V. N0 = 1.4 and ND = 0 reproduce the diffusion slope and both mirrored sparse transfers. The repaired signed U1B sweep with VBB = −1 V also passes.

**Terminal charge.** Let C = Cox·W·L, U be the positive electrostatic overdrive, and z the drain bias divided by its charge saturation voltage, clipped at one. Integrating the inversion sheet gives the magnitude of total mobile charge `C·U·(1−z+z²/3)/(1−z/2)`. The source receives the balance after the drain partition. The gate balances the mobile and bulk charges at every bias. The XPART = 0 and 1 saturation probes give the same partition within each level.

- BSIM1 pins the surface at inversion onset `g=VFB+phi+K1·r`. Its charge saturation voltage is U/A. Below onset, the gate and bulk follow the oxide accumulation or square-root depletion primitive. The 0/100 drain charge magnitude is `C·U·(1−z)²/(2−z)`. Its first derivative goes to zero at pinch-off. The bulk shares the channel-potential drop, giving an incremental gate-to-bulk magnitude of C·(A−1)/(3A) in saturation. At 1.5 V drain bias the measured gate capacitance goes from 1.51636 fF at g = 0.6 V to 4.74256 fF at 0.8 V, with 4.41984 fF assigned to source. [Berkeley ERL M85/85, chapter 4, equations 4.14–4.43](https://www2.eecs.berkeley.edu/Pubs/TechRpts/1985/ERL-85-85.pdf)
- BSIM2 uses the transport threshold, `U=max(u,0)`, and the strong-inversion saturation voltage evaluated at U. The drain fraction follows the first spatial moment of the inversion sheet and reaches 40% at saturation. With `F=VFB+phi−S` and `H=Vth−F>0`, the gate-to-bulk capacitance in depletion is `C·(1−(g−F)/H)²`. Integrating from flat band gives a bulk-charge magnitude of C·H/3 at threshold. In accumulation the incremental capacitance is C. At g = −0.2 and 0 V the gate sends all 6.62976 fF to bulk. At 0.2 V the capacitance is 0.289121 fF. At 0.4 V the drain and source receive 1.767936 and 2.651904 fF. At d = 0.2 V and g = 1.4 V the drain-driven gate, drain, source and bulk derivatives are −6.026948, +4.181604, +1.818825 and +0.026519 fF. These measurements distinguish the charge saturation field from the long-channel pinch-off voltage and exclude mobile storage from the weak-current spline below threshold.

**Junctions.** The six repaired DC runs cover −20, 27 and 85 °C with model TEMP = 27. Both currents at every bias coincide across temperatures within each level. The forward slope and amplitude at Vb = 0.50 and 0.55 V give VT = k·300.15 K/q and a 1 fA per-device saturation-current floor. At 0.55 V the bulk source measures −1.717824 µA on both levels. The reverse-bias AC slope is IS/VT + GMIN. The reference DC reverse slope includes an additional approximately 1 pS per junction at the fixture's GMIN. The available temperature sweep measures DC current. Temperature dependence of junction capacitance is open. With MJSW omitted the PBSW sweeps give a constant 5 fF on both levels.

**Open noise difference.** On both levels and at each of 0.4, 1.4 and 2.4 V gate drive, the 1 Hz output is 7083.628994 V/√Hz for KF = 1e−24 and AF = 1.1. With KF = 0 it is 2.78127624e−7 V/√Hz. Only the input-referred spectrum tracks the gain. The earlier KF, AF and frequency sweeps give power proportional to KF/f. For AF of 0.6, 1 and 1.4 the 1 Hz outputs are 5008.88, 6609.26 and 8720.97 V/√Hz. The inferred drain noise power density on this fixture is KF·4^AF/(f·W·L·Cox²), using W and L in metres and Cox in F/cm². Using the documented channel-noise equations and 1 kΩ transimpedance would require 4 A and 7 S at every one of those biases. The measured AC gain changes by a factor of over 3200 for BSIM1. No physical bias law or general topology dependence for those reference prefactors is established. Sansho keeps operating-point current and gm in its noise model.

**Inverter supply current.** The following endpoint and last-interval mean currents are from the same first rising ramp in the BSIM2 reference. The mean is the average of the two accepted currents bounding the final integration interval.

| Maximum step | Trap endpoint / mean (µA) | Gear endpoint / mean (µA) | Sansho vs reference current score, trap / Gear |
|---|---:|---:|---:|
| 25 ps | 248.31 / 164.94 | 199.14 / 134.06 | 9.346 / 5.248 |
| 5 ps | 100.80 / 165.16 | 165.20 / 165.16 | 0.215 / 1.266 |
| 1 ps | 172.98 / 165.20 | 165.20 / 165.20 | 0.019 / 0.010 |

The PMOS gate ramp supplies C·dVin/dt = 165.74 µA less approximately 0.54 µA static supply current. Trapezoidal integration constrains the sum of consecutive currents through `(i[n]+i[n−1])/2=ΔQ/Δt`, allowing an alternating component on each accepted grid. At 8.300 ns on the coarse grids the reference and Sansho endpoints are +248.31 and +44.01 µA, but their means are +164.94 and +165.16 µA. With the input subsequently fixed the 44.2 current still alternates between +6.325 and −7.150 µA about the settled −0.413 µA.

On the revised BSIM1 deck the largest residual after the charge fix is 127.9 µA at 11.600 ns. The last two accepted currents are −533.52 and −428.69 µA in 44.2, and −405.00 and −556.55 µA in Sansho. Their means differ by 0.33 µA over final intervals of 12.50 and 12.77 ps. At 11.6025 ns both give −377.32 µA. The same pattern occurs on the first falling edge. This establishes a grid-phase difference on the coarse decks. Both voltage comparisons pass. The BSIM1 and BSIM2 waveform markers retain the instantaneous current scores.

## VDMOS drain resistance

Manual §7.7 supplies the two resistance exponents. The R9 and R14 controls distinguish an omitted TEXP0 from an explicit value. With RQ = 0, RD = 0.24 Ω, Vg = 6 V and Vd = 1 V, raising the ambient to 87 °C gives 3.244911, 2.608806 and 2.073695 A for TEXP0 of 0, 1.5 and 3. With RD = 0 the access branch is absent. Every RQ-only TEXP1 setting gives 13.5 A at this bias.

With explicit RTHJC, for finite RD let `f=|Vds_external|/(VQ+|Vds_external|)`. The mixed-branch thermal acceptance measurements support the following law for a single device.

```text
base  = RD·(Tj/Tnom)^a·(Tj/Tamb)^a   when TEXP0 = a is supplied
base  = RD·(Tj/Tamb)^1.5             when TEXP0 is omitted
drift = RQ·f·(Tj/Tnom)^TEXP1·(Tj/Tamb)^TEXP1
Rdrain = (base+drift)/M
```

At 84.441 µs the reference junction rise is 33.182 K. Inverting the electrical square law gives Rdrain of 0.61687 Ω against 0.61703 Ω above or 0.54156 Ω with a single temperature factor. At 404.441 µs the corresponding values are 0.72832 and 0.72844 Ω. The thermal deck passes. At a 60 K junction rise with RD alone, omitting RTHJC gives 3.244911 A at 1 V for all explicit exponents. Supplying RTHJC = 5 gives 3.244911, 2.073695 and 1.276289 A at exponents 0, 1.5 and 3. Sansho keeps the ambient RD when RTHJC is omitted. With both resistances present and the junction clamped, RTHJC of 1, 5 and 20 K/W gives the same 0.894446 A within 0.09 µA. Both mixed-resistance exponent sweeps pass under ambient and junction heating. [R9 thermal measurements](r8-semiconductors.md#thermal-terminals)

## Control text

The four repaired NUMDGT runs reproduce all numeric listing records and the complete wrdata files byte-for-byte. They distinguish the sign-dependent print precision from wrdata, the six-place measurement mantissa and six-significant-digit echo substitution. [Formatting and examples](control.md#batch-number-formatting)
