# ngspice compatibility

The scope below applies to the built-in parser and engine used by the CLI, session API and MCP. **Supported** covers the forms named in the row. **Partial** identifies the implemented subset. **Provisional** marks an implemented model whose parameter laws await reference measurements. Numerical comparison with ngspice 44 is tracked separately in the [verification notes](../verify/README.md).

## Compact-model compatibility

BSIM3, BSIM4 and SPICE3 diode model cards are extracted against the Berkeley reference implementation. Where its observed behavior differs from the printed manual, Sansho follows the behavior established by black-box experiments with ngspice 44.2, which embeds the Berkeley models. ngspice may carry its own modifications. A measured curve establishes external behavior for the tested conditions. An ambiguous result stays documented until an independent probe distinguishes the candidate laws. Model source code is never consulted.

Language and analysis conventions follow the manual. SFFM argument order, E-table corner interpolation and the anomalous AC sensitivity vector remain known differences. The model-specific measurements are in the [BSIM3 notes](bsim3.md#verification-status) and [BSIM4 notes](bsim4.md#mobility-stress).

## Elements and models

| Feature | Status | Scope |
|---|---|---|
| R, C, L | Partial | Explicit values, model and instance temperature coefficients, multiplicity, resistor AC resistance and C/L initial conditions. Named R, C and L expressions can depend on circuit signals, time and temperature. Geometry-derived values and explicit charge/flux expressions remain gaps. |
| K | Supported | Mutual flux of two named positive inductors, signed coupling from −1 through +1. |
| V, I | Partial | DC, AC magnitude/phase, DISTOF1/DISTOF2 magnitude and phase, numbered voltage ports with positive real Z0, PULSE, SIN, EXP, PWL, SFFM, delayed AM, TRNOISE and TRRANDOM types 1–4. Stochastic waveforms use the netlist seed. Flicker noise uses a finite bandwidth. |
| E, F, G, H | Supported | Linear gains and SPICE2 POLY(n). E/G use differential node pairs. F/H use branch currents. A single coefficient for one input denotes linear gain. Otherwise coefficients start with the constant term and advance by total degree, with the first variable varying first in the list of degree-one terms. |
| B and nonlinear E/G | Partial | Voltage and current expressions, branch probes, time, temperature, conditionals and math functions. E/G VALUE and endpoint-clamped TABLE are supported. |
| S, W | Partial | SW and CSW resistance states, threshold, hysteresis and initial on/off. Sansho uses the magnitude of the hysteresis width. |
| T | Partial | Four terminals, Z0 and TD, or F and optional NL (default ¼ wavelength). DC, AC and transient propagation are supported. UIC initializes port voltages, port currents and incident-wave history. Distributed pole-zero extraction remains unsupported. |
| O | Partial | LTRA with exact AC two-port and time-domain RLC or RC convolution, lossless LC delay and static RG. Linear interpolation, limited option coverage. [Distributed line equations and limits](distributed-lines.md) |
| U | Partial | Geometric RC ladder with explicit N or FMAX-based estimate, K, RPERL, CPERL and optional ISPERL/RSPERL diode shunts. Automatic segmentation truncates the logarithmic estimate with a minimum of three sections per half. [Ladder measurements](distributed-lines.md#uniform-rc-ladder) |
| Y and coupled lossy lines | Partial | TXL and up to eight CPL conductors with exact distributed DC/AC and delayed recursive convolution in transient. Non-diagonal modal losses use coupled scattering sections. 44.2 reproduces the tested LC delays. Its lossy CPL examples have nearly lossless fronts and settle near the lossless divider. Sansho retains physical attenuation, leakage and modal losses. [Card syntax](distributed-lines.md#txl-and-cpl) · [Measured 44.2 behavior](distributed-lines.md#r18c-loss-coupling-and-settling) |
| D | Partial | Level-one junction current, series resistance, transit and depletion charge, temperature and noise. BV and IBV set the avalanche operating point, and NBV sets its slope (defaults to N). ISR/NR, IKF, and PJ-scaled JSW/NS and CJP/PHP/MJSW/FCS are supported. TRS1/2, TTT1/2 and TM1/2 apply first- and second-order temperature factors. TCV (V/K) sets BV(T) = BV − TCV·(T − TNOM). Sansho and ngspice 44.2 both warn on unrecognized TBV1 and TBV2 and ignore them. Alternate temperature selectors remain unsupported. |
| J | Partial | Level-one NJF/PJF. VTO, BETA, LAMBDA, B, RD/RS, CGS/CGD, PB, IS, FC, KF/AF, TNOM, BEX and VTOTC. Instance area, M, OFF, TEMP and DTEMP. Gate junctions carry current and depletion charge. UIC accepts drain–source and gate–source voltages. |
| J, level 2 | Provisional | Parker–Skellern reconstruction with symmetric channel, gate-feedback and thermal relaxation, RD/RS, terminal charge, AC and noise. [Fitting choices](r14-models.md#junction-fet-level-2) |
| Q | Partial | Level-one NPN/PNP Gummel–Poon model with optional substrate, high injection, transit and junction charges, temperature and noise. RB/RBM/IRB current crowding, NKF roll-off, ISS/NS substrate current, explicit SUBS collector/base substrate connection, C2/C4 leakage factors, PTF forward excess phase, XCJC collector depletion partition and TF/XTF/VTF/ITF bias dependence. The default substrate junction connects to the internal collector. Separate AREAB/AREAC junction factors and a Kull epitaxial collector with RCO, VO, GAMMA and QCO. The static high-field law and vertical base–collector diffusion area agree with the three loaded RCO sweeps. |
| M, level 1 | Partial | NMOS/PMOS square-law channel, threshold body effect, explicit channel-length modulation, RD/RS, Meyer and overlap capacitances, bulk junctions, temperature and noise. |
| M, level 2 | Partial | Charge-sheet current integrated along the channel, pinch-off, XJ charge sharing with NSUB, UCRIT/UEXP mobility reduction, VMAX and explicit LAMBDA. With NSUB and omitted LAMBDA, the post-saturation depletion region shortens the channel using NEFF and the velocity-limited end field. Explicit zero LAMBDA selects constant length. The automatic branch awaits an independent waveform comparison. |
| M, level 3 | Partial | Body-charge slope, DELTA narrow-width shift, ETA drain feedback, THETA mobility reduction, VMAX and depletion-length correction with NSUB/KAPPA. With NSUB and XJ, the lateral depletion estimate reduces the body charge. With VMAX, the saturation current and its linear-region slope set the end field for channel shortening. These NSUB-dependent terms await an independent waveform comparison. |
| M, level 4 | Provisional | BSIM1 extracted geometry, threshold, field-dependent mobility, weak inversion, charge partition, AC, noise and temperature. [Equations and gaps](r14-models.md#bsim1-and-bsim2) |
| M, level 5 | Provisional | BSIM2 body/gate mobility fits, cubic inversion transition, hot-electron term and conservative terminal charge. [Equations and gaps](r14-models.md#bsim1-and-bsim2) |
| M, level 9 | Provisional | MOS3 equations with XL, XW, WD, DELVTO/DELVT0 and dimension sensitivities. [Manual coverage and comparison choices](r14-models.md#mos-level-9) |
| M, level 6 | Partial | Sakurai–Newton power-law saturation current (KC, NC) and voltage (KV, NV), body and drain bias shifts (GAMMA, GAMMA1, SIGMA), and output modulation (LAMBDA0/1). Uses the classical access, Meyer, junction and noise models. The level-specific defaults and temperature behavior await a 44.2 comparison. |
| M, levels 8 and 49 | Partial | Built-in BSIM3v3.3 NMOS and PMOS, mobMod 1–3, capMod 0–3, noiMod 1–4, geometry bins and temperature-dependent junctions. Quasi-static operation. [Version rules, choices and gaps](bsim3.md) |
| M, levels 10 and 58 | Provisional | Silicon B4SOI 4.4 with PD, unified, ideal FD and automatic mode selection. Four to seven terminals, electrothermal DC and transient, quasi-static AC and noise. Eleven of twelve R17 decks pass against 44.2. The inverter switching transient differs. [Coverage and fitting probes](bsimsoi.md) |
| M, level 54 | Partial | Built-in silicon BSIM4.8.2. Includes binned geometry, stress, gate and body resistance networks and quasi-static terminal charges. NQS, material models and correlated tnoiMod 2 noise are outside coverage. [BSIM4 coverage](bsim4.md) |
| Z | Partial | Level-one Statz NMF/PMF channel, Schottky gate, gate charge, RD/RS, thermal and flicker noise, device vectors and UIC. Gate charge uses the last accepted bias for its incremental partition. [Model details](r8-semiconductors.md) |
| M, VDMOS | Partial | N-channel and p-channel power MOSFET, Miller charge, body diode, subthreshold smoothing, drain drift resistance, temperature, noise and UIC. THERMAL requires explicit junction and case nodes and uses RTHJC, CTHJ and RTHCA. The cached 44.2 sweeps confirm the RTHCA boundary, hot channel transfer and body-diode heat including RB. TEXP0 and TEXP1 configure the fixed and bias-dependent drain resistance exponents. CTHJC gives W-MODEL-IGNORED. [Parameters and limits](r8-semiconductors.md) |
| Other built-in transistor families | Not supported | Includes the older SOI levels 55–57 and HICUM/L0 model cards. Independently supplied Verilog-A modules can implement some of these families. |
| A, analog code models | Partial | All 34 manual §8.2 families, including coupled cores, nonlinear passives, meters, file tables and analog delay. All ten R13 batch decks pass against cached ngspice 44.2 waveforms. [Ports, parameters and limits](xspice.md) |
| Q, level 4 | Native VBIC NPN/PNP, 3–5 terminals | DC, AC, noise, temperature and electrothermal transient. All seven R15 acceptance decks and 219 completed probe settings pass against ngspice 44.2. [Equations and coverage](vbic.md) |
| Q, level 8 | Native HICUM/L2 2.4.0 NPN/PNP, 3–5 terminals | OP, DC, AC, noise, temperature and transient including thermal feedback and NQS. Compared against R16 cached 44.2 waveforms. The 44.2 PNP violates complementary BC polarity in the mirrored-diode probe and carries a quantitative known difference. DISTO gives E-ANALYSIS. [Equations and limits](hicum.md) |
| X, subcircuit instances | Supported | Nested flattening, parameters, local models and hierarchical names. |
| Verilog-A via N and .hdl | Partial | Compiled analog subset with an interpreter and optional JIT. [Language coverage](verilog-a.md) |

Levels 2 and 3 share the classical MOS terminal assembly. Both accept explicit VTO, KP, PHI, GAMMA, TOX, U0/UO, LD, DELTA and NFS. NSUB can supply PHI and GAMMA, and the threshold calculation can use TPG and NSS. The same model supports RD/RS or RSH with NRD/NRS, CGSO/CGDO/CGBO, IS/JS, CBD/CBS, CJ/CJSW, PB, MJ/MJSW, FC, KF/AF, and the MOS1 temperature translation. Level 3 also uses WD. Unsupported model parameters produce a warning. The R3 decks exercise a subset of these parameters.

### Modulated independent sources

Voltage and current sources accept `AM(VO VMO VMA FM FC TD PHASEM PHASEC)`
and `SFFM(VO VA FC MDI FM TD PHASEM PHASEC)`. The first two values are
required. Both waveforms are zero through TD, including at TD itself.
After TD both phases are measured in degrees from the delay. AM adds VO to
the product of the carrier sine and the envelope `VMO + VMA sin(modulator phase)`.
SFFM adds `MDI sin(modulator phase)` to the carrier phase in radians.

Omitted FC and FM are 500/TSTOP and 5/TSTOP, respectively. TD and both
phases default to zero, VMA to one and MDI to 90. Section 4.1.5 limits MDI
to the range from zero to FC/FM for positive frequencies. Sansho places a
breakpoint at TD and limits the active timestep to resolve the modulation.
[ngspice user manual, §§4.1.5–4.1.6](https://ngspice.sourceforge.io/docs/ngspice-manual.pdf#page=99)

The cached ngspice 44.2 SFFM waveform is consistent with interpreting argument
three as FM and argument five as FC, including the resulting limit on MDI.
Sansho uses the manual's order. The dedicated SFFM deck records this known difference.

## Analyses, commands and options

| Feature | Status | Scope |
|---|---|---|
| .op, .ac | Supported | DC operating point and complex AC on linear, decade or octave frequency grids. |
| .dc | Partial | One or two distinct axes, each an independent voltage source, independent current source, numeric resistor or TEMP. The first axis varies fastest. A source may omit its DC value. Arbitrary parameter sweeps remain outside the netlist syntax. |
| .tran | Partial | Adaptive trapezoidal or Gear/BDF2 integration, TSTART, TMAX and UIC. Waveforms contain accepted timepoints. |
| .noise | Partial | One-sided small-signal source projection, amplitude spectra and integrated RMS noise. Device and selector coverage is listed in the [analysis notes](analyses.md). |
| .tf | Supported | Incremental DC transfer and port resistances for a named independent input. |
| .sens | Partial | DC and AC adjoints for selected continuous parameters. Exported vectors include named model parameters and polynomial coefficients. |
| .pz | Partial | Finite poles and zeros for lumped voltage/current port networks up to 256 circuit unknowns. |
| .four | Partial | DC and nine harmonics of the final period of the first transient, sampled at 200 points. |
| .sp | Partial | DEC/OCT/LIN N-port S, Y and Z matrices. Two-port DONOISE=1 reports the Cy noise matrix, NF, SOpt, NFmin and Rn. Source noise normalization uses circuit temperature. Requires finite, nonsingular port conversions. |
| .disto | Partial | HD2/HD3 and two-tone sum, difference and third-order intermodulation. Analytic diode, Gummel–Poon and MOS1 current and charge generators. Unsupported nonlinear devices and bipolar extensions produce an error. |
| .pss | Supported | Autonomous shooting and a fixed-frequency driven extension. The ngspice 44.2 oracle build lacks PSS. Verification uses analytic solutions and long transients. Harmonic balance (HB) is outside the ngspice analysis set. [PSS options and limits](analyses.md#periodic-steady-state) |
| .meas / .measure | Partial | TRAN, single-curve DC, AC and noise spectra. TRIG/TARG, WHEN, FIND, DERIV, AT, RISE/FALL/CROSS including LAST, TD, signal-to-signal comparisons, windowed MIN/MAX/MIN_AT/MAX_AT/PP/AVG/RMS/INTEG, and PARAM expressions using preceding successful measurements. A repeated DC scale produces an ambiguous-curve diagnostic. SP remains unsupported. |
| .include / .inc, .lib, .subckt / .ends, .global | Supported | File-relative includes, selected library sections and scoped hierarchy. Relative paths resolve through the file loader. |
| .param, .func, .if / .elseif / .else / .endif | Partial | Scoped numeric expressions, functions, comparisons and conditional expansion. The expression dialect and statistical bindings have specific [preprocessing rules](architecture.md#10-compatibility). |
| .model | Partial | Built-in types above, selected MOS geometry bins and models bound to a loaded VA module. |
| .end, .title | Supported | .end terminates input. A .title card overrides the title from the first netlist line. |
| .step, .alter | Not supported | Parameter sweeps and changes are available through the session API. |
| .ic, .nodeset, .temp | Partial | Node-voltage initial conditions and guesses. Element ICs under UIC cover C, L, D, Q, M, J and T. The first .temp value sets operating temperature. Use .dc temp for a sweep. |
| .save, raw output | Partial | Select node voltages, branch currents and named device quantities in OP, DC, TRAN and AC. SAVECURRENTS adds device current vectors. ASCII and binary raw, session results, CLI JSON and MCP waveforms share a vector catalog. See the [device output table](analyses.md#device-output-vectors). |
| .print, .plot, .width | Partial | OP, DC, TRAN, AC and noise print tables support voltage, current, named device outputs and AC projections. .plot warns and .width is accepted. Tables use the CLI layout. |
| .options | Partial | RELTOL, ABSTOL, VNTOL, CHGTOL, TRTOL, GMIN, ITL1, ITL2, ITL4, RSHUNT, GMINSTEPS, SRCSTEPS, TEMP, TNOM, SCALE, DEFL/DEFW/DEFAD/DEFAS, METHOD=TRAP/TRAPEZOIDAL/GEAR, MAXORD=1/2, SAVECURRENTS, SEED and BYPASS. ITL5 and listing flags are accepted. INTERP, unknown options and invalid option values warn. |
| .control / .endc, .csparam and Nutmeg | Partial | Batch analyses, bounded control flow, plot and vector algebra, measurements, circuit alterations and confined file output. Scientific text fields support NUMDGT. API and MCP return structured transcripts. [Commands and differences](control.md) |
| Digital XSPICE | Partial | Twelve-state and real events, bidirectional and sampled-real bridges, PWM, state machines, RAM and multi-output LUTs in OP/DC/transient. Raw comparisons use DAC voltages. [Digital timing and limits](xspice.md#digital-nodes-and-bridges) |
| External XSPICE models | Excluded | `d_process`, `d_cosim` and `codemodel` require external execution or shared-library loading and produce `E-UNSUPPORTED`. [Security policy](xspice.md#excluded-external-models) |
| CIDER | Provisional | Level-one and level-two NUMD/NBJT, and four-terminal NUMOS. Coupled OP/DC, AC, transient, temperature and terminal G/C/Y. Planar silicon/oxide devices include gate work functions, interface charge and surface transport. All eight R21 decks pass against 44.2. Signed Si/oxide interface-charge controls pass. Surface-field transport retains probe discrepancies, and two recessed-contact references are empty. [Cards, equations and verification](cider.md) |
| OSDI / pre_osdi | Not supported | Native simulation loads VA source through .hdl. OSDI appears in the external verification workflow. |

## Numerical differences

- In `r8_ltra_ac`, ngspice 44.2 reports the real DC divider 75/185 at all 41 AC frequencies from 100 kHz to 1 GHz. Sansho keeps the exact RLC two-port, giving −0.334617 + j0.024240 V at 50.119 MHz. [Loaded AC measurements](distributed-lines.md#frequency-domain)
- At 1 mA reverse current and 100 °C, ngspice 44.2 measured a +56.002876 mV terminal voltage shift for TCV = 0.0007 V/K and TNOM = 20 °C. The paired TBV1/TBV2 probe gave identical voltages at all four temperatures from −20 to 100 °C. [Diode probe measurements](../verify/probes/README.md#diode-breakdown-under-temp)
- The 44.2 QCO switching reference swings hundreds of volts in the 5 V fixture. Sansho retains the Kull storage charges and compares static quasi-saturation separately with QCO=0. [Collector measurements](r8-semiconductors.md#bipolar-junction-transistor-and-diode)

- Sansho applies the documented diode instance `ic=` junction voltage under UIC and initializes the junction charge at that voltage. For `ic=0.2`, CJO=10 pF and a 2 kΩ shunt, the node starts at 0.2 V and discharges. ngspice 44.2 gives zero at every saved time in the original R7 raw. The maintainer reproduced this with a single diode and with explicit area. [Diode UIC deck](../verify/decks/r7_diode_uic.cir)

- The Berkeley diode in ngspice 44.2 matches the avalanche knee to the bottom saturation current. Bottom and sidewall diffusion share the resulting exponential. At AREA=2 the R4 card draws 1.0500 mA at −BV. The six probe families also resolve the low-IBV clamp and the NBV slope. [Measured diode currents and matching law](../verify/probes/README.md#spice3-diode-breakdown)
- The BSIM3 unified flicker and thermal spectra agree within 0.018% across ten probe families. At the two weakest gate biases, integrated input RMS differs by 13.85% and 5.09%. Sansho integrates input-referred power using the gain at each endpoint. The 44.2 integrals use the upper endpoint's gain across the interval. [Spectra and integration measurements](../verify/probes/README.md#bsim3-noimod-2)
- E TABLE uses straight segments and the right-hand slope at an interior knot. ngspice 44 smooths the corners. This affects values and incremental gains near a breakpoint.
- A switch changes state after the continuous solution crosses a threshold. Sansho interpolates a bracketed crossing, retries the step and restarts integration history. Different accepted grids can shift switching time. A small TMAX resolves narrow pulses. [Switch verification](../verify/README.md#r1-device-and-temperature-decks)
- In the cached RC AC sensitivity deck the ngspice R1 vector is zero above the first frequency. Sansho reports the complex derivative of the RC transfer at every frequency. The SKY130 free-running ring also shows phase drift in the body-node traces. [Reference comparisons](../verify/README.md)
- Geometry binning also applies to classical MOS models. The classical MOS noise implementation uses the existing MOS1 gate-referred flicker convention. Differential-equation integration and waveform interpolation use the Sansho solver tolerances. [Analysis conventions](analyses.md)
