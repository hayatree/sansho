# ngspice compatibility

The scope below applies to the built-in parser and engine used by the CLI, session API and MCP. **Supported** covers the forms named in the row. **Partial** identifies the implemented subset. Numerical comparison with ngspice 44 is tracked separately in the [verification notes](../verify/README.md).

## Elements and models

| Feature | Status | Scope |
|---|---|---|
| R, C, L | Partial | Explicit values, passive model temperature coefficients, instance temperature, multiplicity and C/L initial conditions. Geometry-derived passives and arbitrary nonlinear charge or flux expressions are outside this syntax. |
| K | Supported | Mutual flux of two named positive inductors, signed coupling from −1 through +1. |
| V, I | Partial | DC, AC magnitude/phase, PULSE, SIN, EXP and PWL. Random, SFFM and AM waveforms are unsupported. |
| E, F, G, H | Supported | Linear gains and SPICE2 POLY(n). E/G use differential node pairs. F/H use branch currents. A single coefficient for one input denotes linear gain. Otherwise coefficients start with the constant term and advance by total degree, with the first variable varying first in the list of degree-one terms. |
| B and nonlinear E/G | Partial | Voltage and current expressions, branch probes, time, temperature, conditionals and math functions. E/G VALUE and endpoint-clamped TABLE are supported. |
| S, W | Partial | SW and CSW resistance states, threshold, hysteresis and initial on/off. spx uses the magnitude of the hysteresis width. |
| T | Partial | Four terminals, Z0 and TD, or F and optional NL (default ¼ wavelength). DC, AC and transient propagation are supported. Line IC tuples and distributed pole-zero extraction are unsupported. |
| O, U, Y and coupled lossy lines | Not supported | LTRA, uniform distributed RC and multiconductor models. |
| D | Partial | Level-one junction current, series resistance, transit and depletion charge, temperature and noise. Reverse breakdown uses one exponential. |
| J | Partial | Level-one NJF/PJF. VTO, BETA, LAMBDA, B, RD/RS, CGS/CGD, PB, IS, FC, KF/AF, TNOM, BEX and VTOTC. Instance area, M, OFF, TEMP and DTEMP. Gate junctions carry current and depletion charge. Level two and instance voltage IC tuples are unsupported. |
| Q | Partial | Level-one NPN/PNP Gummel–Poon model with optional substrate, high injection, transit and junction charges, temperature and noise. Base resistance is constant. Excess phase and split base–collector capacitance are unsupported. |
| M, level 1 | Partial | NMOS/PMOS square-law channel, threshold body effect, explicit channel-length modulation, RD/RS, Meyer and overlap capacitances, bulk junctions, temperature and noise. |
| M, level 2 | Partial | Charge-sheet current integrated along the channel, pinch-off, XJ charge sharing with NSUB, UCRIT/UEXP mobility reduction, VMAX and explicit LAMBDA. The drain-depletion calculation for automatic channel-length modulation and NEFF scattering corrections are gaps. |
| M, level 3 | Partial | Body-charge slope, DELTA narrow-width shift, ETA drain feedback, THETA mobility reduction, VMAX and depletion-length correction with NSUB/KAPPA. Empirical XJ short-channel threshold scaling and the full velocity-dependent saturation-region length correction are gaps. |
| M, level 54 | Partial | Built-in silicon BSIM4.8.2. Includes binned geometry, stress, gate and body resistance networks and quasi-static terminal charges. NQS, material models and correlated tnoiMod 2 noise are outside coverage. [BSIM4 coverage](bsim4.md) |
| Other built-in transistor families | Not supported | Includes MESFET, VDMOS, MOS6/9, BSIM1/2/3, BSIM-SOI, VBIC and HICUM model cards. Independently supplied Verilog-A modules can implement some of these families. |
| X, subcircuit instances | Supported | Nested flattening, parameters, local models and hierarchical names. |
| Verilog-A via N and .hdl | Partial | Compiled analog subset with an interpreter and optional JIT. [Language coverage](verilog-a.md) |

Levels 2 and 3 share the classical MOS terminal assembly. Both accept explicit VTO, KP, PHI, GAMMA, TOX, U0/UO, LD, DELTA and NFS. NSUB can supply PHI and GAMMA, and the threshold calculation can use TPG and NSS. The same model supports RD/RS or RSH with NRD/NRS, CGSO/CGDO/CGBO, IS/JS, CBD/CBS, CJ/CJSW, PB, MJ/MJSW, FC, KF/AF, and the MOS1 temperature translation. Level 3 also uses WD. Unsupported model parameters produce a warning. The R3 decks exercise a subset of these parameters.

## Analyses, commands and options

| Feature | Status | Scope |
|---|---|---|
| .op, .ac | Supported | DC operating point and complex AC on linear, decade or octave frequency grids. |
| .dc | Partial | One independent voltage source, current source or TEMP sweep. Nested sweeps are unsupported. |
| .tran | Partial | Adaptive trapezoidal or Gear/BDF2 integration, TSTART, TMAX and UIC. Waveforms contain accepted timepoints. |
| .noise | Partial | One-sided small-signal source projection, amplitude spectra and integrated RMS noise. Device and selector coverage is listed in the [analysis notes](analyses.md). |
| .tf | Supported | Incremental DC transfer and port resistances for a named independent input. |
| .sens | Partial | DC and AC adjoints for selected continuous parameters. Exported vectors include named model parameters and polynomial coefficients. |
| .pz | Partial | Finite poles and zeros for lumped voltage/current port networks up to 256 circuit unknowns. |
| .four | Partial | DC and nine harmonics of the final period of the first transient, sampled at 200 points. |
| .disto, .sp, .pss and periodic/HB analyses | Not supported | No corresponding engine analysis. |
| .meas / .measure | Partial | TRAN, DC and AC. TRIG/TARG, WHEN, FIND, MIN, MAX, PP, AVG, RMS and INTEG. |
| .include / .inc, .lib, .subckt / .ends, .global | Supported | File-relative includes, selected library sections and scoped hierarchy. Relative paths resolve through the file loader. |
| .param, .func, .if / .elseif / .else / .endif | Partial | Scoped numeric expressions, functions, comparisons and conditional expansion. The expression dialect and statistical bindings have specific [preprocessing rules](architecture.md#10-compatibility). |
| .model | Partial | Built-in types above, selected MOS geometry bins and models bound to a loaded VA module. |
| .end, .title | Partial | .end terminates input. The first netlist line supplies the title. The parser ignores a later .title card. |
| .step, .alter | Not supported | Parameter sweeps and changes are available through the session API. |
| .ic, .nodeset, .temp | Partial | Node-voltage initial conditions and guesses. The first .temp value sets operating temperature. Use .dc temp for a sweep. |
| .save, raw output | Partial | Select node voltages and branch currents, or save all. CLI raw output is ASCII. Arbitrary internal device vectors, .print and .plot output formatting are unsupported. |
| .options | Partial | RELTOL, ABSTOL, VNTOL, CHGTOL, TRTOL, GMIN, ITL1, ITL4, RSHUNT, GMINSTEPS, SRCSTEPS, TEMP, TNOM, SCALE, DEFL/DEFW/DEFAD/DEFAS, TRAP/GEAR and BYPASS. Some listing flags have no effect. Unknown options warn. |
| .control / .endc and Nutmeg | Not supported | The preprocessor skips control blocks with W-PARSE-CONTROL. No interactive commands, loops, plot algebra or scripting interpreter. |
| XSPICE and CIDER | Not supported | No A-device code-model runtime, digital event engine or numerical semiconductor solver. |
| OSDI / pre_osdi | Not supported | Native simulation loads VA source through .hdl. OSDI appears in the external verification workflow. |

## Numerical differences

- E TABLE uses straight segments and the right-hand slope at an interior knot. ngspice 44 smooths the corners. This affects values and incremental gains near a breakpoint.
- BSIM4 KU0 mobility stress follows manual equation 14.5. The cached 44.2 control gave the same drain current with and without KU0. For unified flicker noise, spx uses the appendix NOIC default of 8.75. The reference spectrum behaves like an explicit 8.75e9. Explicit NOIC removes that discrepancy. [BSIM4 comparisons](bsim4.md)
- A switch changes state after the continuous solution crosses a threshold. spx interpolates a bracketed crossing, retries the step and restarts integration history. Different accepted grids can shift switching time. A small TMAX resolves narrow pulses. [Switch verification](../verify/README.md#r1-device-and-temperature-decks)
- In the cached RC AC sensitivity deck the ngspice R1 vector is zero above the first frequency. spx reports the complex derivative of the RC transfer at every frequency. The SKY130 free-running ring also shows phase drift in the body-node traces. [Reference comparisons](../verify/README.md)
- Geometry binning also applies to classical MOS models. The classical MOS noise implementation uses the existing MOS1 gate-referred flicker convention. Differential-equation integration and waveform interpolation use the spx solver tolerances. [Analysis conventions](analyses.md)
