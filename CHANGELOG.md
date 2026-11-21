# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions before 1.0 may change public interfaces.

## [Unreleased]

- Add native level-four VBIC NPN and PNP devices with substrate transport, quasi-saturation, excess phase, a junction-temperature port and noise. Seven acceptance decks and isolated parameter sweeps cover the initial 1.2 card mapping. All seven decks and 187 complete black-box probe settings pass against ngspice 44.2. [Equations and open choices](docs/vbic.md)

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

## [0.1.0] - 2026-11-20

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

[0.1.0]: https://github.com/hayatree/sansho/releases/tag/v0.1.0
