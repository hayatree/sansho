# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions before 1.0 may change public interfaces.

## [Unreleased]

## [0.1.0] - 2026-11-22

First public release, distributed on GitHub.

### Analyses

- Compute operating points, nested DC and temperature sweeps, complex AC responses and adaptive transients with trapezoidal or Gear integration.
- Extract small-signal noise spectra and integrals, DC transfer and port resistances, DC/AC sensitivities, Fourier harmonics and finite poles and zeros for small lumped circuits.
- Run seeded Monte Carlo batches, summarize measurements and replay an individual draw.

### Devices and models

- Simulate passive and mutually coupled elements, independent and controlled sources, behavioral sources, switches and lossless transmission lines.
- Use junction diodes, Gummel–Poon BJTs, JFETs, classical MOS levels 1–3 and built-in quasi-static BSIM3v3.3 and BSIM4.8.2 models.
- Load the Verilog-A analog subset from source with a register interpreter and optional native JIT.

### Netlist dialect

- Expand parameterized subcircuits, library sections, model bins, functions and conditional includes. Set initial conditions, nodesets and simulator tolerances.
- Select signals with `.save` and extract event times, derivatives, windowed aggregates and derived scalars with `.meas`. Export ASCII or binary raw plots.

### Agent interfaces

- Run, check and batch netlists from the CLI with JSON results and structured diagnostics.
- Use the Rust session API or stdio MCP server to simulate, inspect waveforms, measure, alter parameters, sweep and export decks.

### Performance

- Reuse the matrix structure, apply block-triangular sparse LU and parallelize device evaluation and numeric refactorization with fixed accumulation order.
- Share topology across Monte Carlo samples. The performance notes report wall times, solver work and waveform error on large circuits. [Measurements](https://github.com/hayatree/sansho/blob/v0.1.0/docs/performance.md)

### Verification

- Compare original device, analysis, SKY130 and Verilog-A decks against black-box ngspice output, alongside analytic and worker-count reproducibility tests.
- Report known numerical differences separately and record compact-model behavior with one-variable probes. [Verification guide](https://github.com/hayatree/sansho/blob/v0.1.0/verify/README.md)

### Further models, analyses and performance

- Apply the BSIM4 Newton bypass (`.options bypass=1`) to the converged charge sample. When every terminal is within `vntol` of the last linearization, the terminal charges are expanded to first order instead of being evaluated again. Instructions per transient run drop by 8–29% on BSIM4 rings. Only three ngspice comparison waveforms change, by at most 1.7e-10 of their range. [Bypass rules](docs/bsim4.md#terminal-evaluation-and-bypass)

- Schedule Monte Carlo runs dynamically, one serial run per worker, instead of in fixed windows. Circuits with 256 or more nonlinear devices no longer fall back to two concurrent runs. BSIM3/BSIM4 bindings are also shared between Monte Carlo instances whose expression-bound cards are identical, instead of being prepared once per instance. A 402-transistor BSIM4 ring runs 10.9 times faster with 32 workers and 1.24 times faster serially, and the results are unchanged. [Scheduling](docs/architecture.md)

- Add an experimental mixed-precision real solver (`SANSHO_REAL_SOLVER=mixed`). It uses single-precision LU with double-precision iterative refinement and falls back to double factors whenever refinement does not reach double accuracy. [Measurements](docs/architecture.md)

- Add an optional CUDA batch for Monte Carlo (`sansho mc --gpu`, build feature `gpu`). Each run keeps its CPU Newton and step control; diode evaluations of all concurrent runs share one kernel launch, with results independent of the lane count. An experimental `--gpu-jacobian f32` uses single-precision derivatives inside Newton iterations only. [Design and measurements](docs/architecture.md#82-g1-gpu-batch-sansho-gpu-feature-gpu)

- Add two-dimensional NUMD/NBJT and four-terminal NUMOS with silicon/oxide meshes, electrodes, surface mobility and interface charge. Eight acceptance circuits cover diode C–V, NPN characteristics, MOS transfer/output, gate AC and a loaded inverter transient. [Geometry, analytic checks and open calibration choices](docs/cider.md#two-dimensional-devices)

- Add one-dimensional numerical silicon diodes and bipolar transistors with CIDER input cards, DC, frequency-dependent terminal admittance, carrier-storage transient and temperature. R20c resolves the silicon densities of states, transport and recombination parameters, and the bias-dependent NBJT base access. All six R20 acceptance decks pass against ngspice 44.2. [Equations and measured mesh convergence](docs/cider.md)

- Add shooting-Newton periodic steady state for autonomous oscillators and fixed-frequency driven circuits. The CLI, batch controls, session API and MCP expose one cycle and its harmonic spectrum. Six analytic decks cover RC, RLC, diode rectification, a sustained LC tank, a CMOS ring and a Van der Pol oscillator. [Algorithm and limits](docs/analyses.md#periodic-steady-state)

- Add TXL (Y) and coupled CPL (P) lines with length overrides, exact matrix DC and AC, and transient propagation with scalar modal filters or coupled loss sections. The scalar filter preserves the wavefront and the onset of the dispersive tail. Seven acceptance decks and 48 cached probe settings quantify 44.2 compatibility. [Equations and verification](docs/distributed-lines.md#txl-and-cpl)

- Add native silicon B4SOI 4.4 NMOS and PMOS at levels 10 and 58 with partial, unified and ideal full depletion, floating and contacted film, a back gate, self-heating, RF resistance networks and noise. R17c passes eleven of twelve ngspice 44.2 acceptance circuits and the 45 and 90 fF inverter probes. The original 80 ps inverter transient is a documented time-grid difference. [Model coverage and open choices](docs/bsimsoi.md)

- Add native level-eight HICUM/L2 NPN and PNP devices targeting 2.4.0, with charge control, high-injection storage, substrate, NQS, electrothermal and noise analyses. Six of seven acceptance decks pass against cached ngspice 44.2 data. The isolated BC mirror establishes a PNP known difference. R16c passes 276 cached probe settings. [Model and measurement status](docs/hicum.md)

- Add native level-four VBIC NPN and PNP devices with substrate transport, quasi-saturation, excess phase, a junction-temperature port and noise. Seven acceptance decks and isolated parameter sweeps cover the initial 1.2 card mapping. All seven decks and 219 complete black-box probe settings pass against ngspice 44.2. [Equations and open choices](docs/vbic.md)

- Add provisional JFET2, MOS9, BSIM1 and BSIM2 models with DC, charge, AC, temperature and noise. VDMOS accepts TEXP0 and TEXP1. Against ngspice 44.2 nine acceptance decks pass. The two inverter decks have quantified trapezoidal supply-current differences on coarse grids. [Equations, assumptions and probe index](docs/r14-models.md)
- Match measured NUMDGT formatting for signed real and complex print fields, wrdata, measurement and echo output. [Batch number formatting](docs/control.md#batch-number-formatting)

- Add the 27 remaining built-in analog, hybrid and digital model families in ngspice 44 §§8.2–8.4, including PWL and hysteretic magnetic cores, winding coupling, two- and three-dimensional file tables, a clocked state machine and RAM. Real nodes sum sampled drivers and share propagation deadlines with digital nodes. Ten new decks exercise the library with one and two workers. [Model behavior and reference-probe choices](docs/xspice.md#r13-analog-models)

- Report `E-UNSUPPORTED` for XSPICE external-process and co-simulation models and the `codemodel` library-loading command.

- Add event-driven digital A models and ADC/DAC bridges to OP, DC sweeps and transient simulation. Batch controls and waveform queries expose strength-resolved changes. Match the probed ADC window and equality behavior, preserve delayed transitions until overtaken, and correct divider phase, oscillator startup strength and DC feedback initialization. Four mixed-signal deck comparisons pass. The counter has a 10 ps coarse-grid sampling difference that disappears on the refined grid. [Digital models and limits](docs/xspice.md#digital-nodes-and-bridges)

- Simulate analog A cards with scalar and vector signal ports, confined file input, smooth curves, dynamic transfer functions, oscillators and a controlled resistor. Missing required parameters identify the model and field. Square waves start low and honor asymmetric edge times, and slew-limited outputs resolve the instant they reach the input. [Supported code models](docs/xspice.md)

- Execute batch control blocks from the CLI, session API and MCP. Scripts can run analyses, sweep circuit values, compute vectors, measure named plots and write raw or tabular results. The verifier compares script-generated files and repeated measurements. [Batch control language](docs/control.md)

- Add N-port S, Y and Z analysis and two-port noise parameters through netlists, the session API and MCP. A four-temperature matched-pad sweep confirms circuit-temperature noise normalization.
- Add single-tone and two-tone small-signal distortion with analytic current and charge derivatives for diodes, Gummel–Poon BJTs and level-one MOSFETs.
- Add the level-six power-law MOS channel and extend VDMOS with subthreshold shape, drain drift resistance, threshold temperature translation and a coupled thermal network using RTHJC, RTHCA and CTHJ. THERMAL requires five nodes and reports E-PARSE-ARITY for missing pins. Correct hot drain resistance and open-case startup against ngspice 44.2. The gate-pulse comparison and all twelve thermal follow-up sweeps pass. CTHJC gives an ignored-parameter warning.

- Reconcile VDMOS channel and capacitance defaults, incremental Statz gate charge and URC diode shunts with cached ngspice 44.2 measurements. Document the static LTRA AC reference and nonphysical bipolar QCO transient.
- Support diode TCV in V/K with BV(T) = BV − TCV·(T − TNOM). TBV1 and TBV2 produce `W-MODEL-IGNORED`, matching ngspice 44.2 behavior.
- Add electrical VDMOS and level-one MESFET models with gate charge, small-signal noise and terminal vectors.
- Add lossy O lines with distributed AC transfer and transient convolution, and graded U-element RC ladders with truncated FMAX lump counts and inverse-area diode access resistances.
- Implement Kull static quasi-saturation with field-dependent drift and base–collector diffusion area. Add epitaxial storage, independent bipolar junction areas and quadratic junction-diode temperature coefficients. [R8 parameter coverage](docs/r8-semiconductors.md)

- Correct the doping-derived body coefficients used by the default BSIM3 and BSIM4 cards, improving threshold voltage and drain current.
- Record device currents, power, small-signal parameters and MOS charge derivatives through `.save`, `.print`, `.meas`, session results and MCP waveform queries.
- Sweep numeric resistors on either DC axis, use signal-dependent R/C/L values and initialize semiconductor junctions and lossless lines under UIC.
- Add repeatable TRNOISE and TRRANDOM sources, noise-spectrum measurements, batch print tables, title overrides, Gear order selection and option diagnostics.

[Unreleased]: https://github.com/hayatree/sansho/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hayatree/sansho/releases/tag/v0.1.0
