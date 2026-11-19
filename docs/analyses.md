# Circuit analyses

A netlist can contain several analysis cards. CLI JSON includes each plot, its scale,
the variable names and units, and the real or complex data columns. The session API and
MCP `simulate` accept the same cards as overrides. Use the plot index and vector name
with `get_waveform`. The CLI writes all plots to one raw file with `spx run deck.cir -r out.raw`. ASCII is the default. Add `--raw-format binary` for a text header followed by `Binary:` and native-endian f64 samples in point order. A complex plot stores a real/imaginary pair for every variable, including its frequency scale.

| Card | Result |
|---|---|
| `.op` | One DC operating point, with node voltages and branch currents. |
| `.dc src1 start stop step [src2 start stop step]` | DC solutions versus independent source value or ambient temperature (`temp`) on either axis. The outer loop advances src2. Each complete inner sweep follows the previous one in the same plot, with the inner value as scale. |
| `.ac dec\|oct\|lin n start stop` | Complex node voltages and branch currents versus frequency in Hz. Sources use their AC magnitude and phase. |
| `.tran step stop [start [maxstep]] [uic]` | Node voltages and branch currents versus time in seconds. The integrator adapts its internal step. |
| `.noise v(out[,ref]) source dec\|oct\|lin n start stop [interval]` | A noise spectrum and a separate plot of integrated RMS noise. |
| `.tf outvar source` | DC incremental gain and input and output resistance. |
| `.sens outvar [ac dec\|oct\|lin n start stop]` | DC or complex AC change in the selected output per unit change in each reported parameter. |
| `.four frequency outvar …` | DC and nine positive harmonics of the first transient analysis, with phase and total harmonic distortion. |
| `.pz in+ in- out+ out- vol\|cur pol\|zer\|pz` | Finite poles, transmission zeros, or both, in radians per second. |

An output probe can be a node voltage, a differential voltage, or a voltage-source
branch current (`i(vsense)`). Noise uses a voltage output. All small-signal analyses
first solve the nonlinear DC bias. The frequency grid uses points per decade or octave,
or a total count for a linear sweep.

An outer TEMP axis rebinds temperature-dependent devices once per inner curve. An inner TEMP axis rebinds at every point and reapplies the outer source bias. Netlist parameter expressions retain their preprocessing-time values.

## Measurements

`FIND` interpolates the selected signal at a fixed sweep value or crossing.
`DERIV` returns the slope of the bracketing linear segment. At a sampled point it uses
the preceding segment in traversal order, with a forward difference at the first point.
`WHEN v(a)=v(b)` locates zeros of the sample-by-sample difference. RISE, FALL and CROSS
accept a positive count or LAST. TD discards earlier events. TRIG and TARG can each use
a fixed AT value. MIN, MAX, MIN_AT, MAX_AT, PP, AVG, RMS and INTEG clip to FROM and TO
with interpolated endpoints. MIN_AT and MAX_AT choose the first extremum in sweep order.
For extrema the ngspice manual specifies the FROM/TO interval and leaves endpoint
interpolation unspecified. On the R4 linear ramp, ngspice 44.2 took the largest
interior sample (0.7928 V), while spx included the interpolated upper edge
(0.8 V). A window bounded by PWL breakpoints gives both simulators the same
endpoint samples. Measurements on a repeated DC scale report an ambiguous curve.

Section 11.4.11 documents both `DERIV` and `DERIVATIVE` with `AT=value` or
`WHEN signal=value` / `WHEN signal=signal`. The 44.2 reference control command
reported failure for both R4 derivatives. For TRAN the verification harness
computes a derivative vector in ngspice and uses FIND on that vector at the
specified point or crossing. [ngspice user manual, §11.4](https://ngspice.sourceforge.io/docs/ngspice-manual.pdf#page=348)

```spice
.meas tran t1 WHEN v(out)=v(ref) RISE=1 TD=1n
.meas tran slope DERIV v(out) AT=2n
.meas tran high MAX v(out) FROM=1n TO=5n
.meas tran low MIN v(out) FROM=1n TO=5n
.meas tran swing PARAM='high-low'
```

PARAM evaluates scalar arithmetic and built-in math functions in card order.
An unknown or failed measurement used by a later expression produces W-MEAS-FAILED.
Measurements operate on TRAN, AC or DC results.

## BJT transport delay

With positive TF and PTF the forward transport delay is TF × PTF × π/180 seconds.
In AC and noise the forward transport gain acquires the factor exp(−jωτ). In transient
analysis spx advances the second-order Weil–McNamee approximation using accepted
timesteps. PZ diagnoses the exponential delay. XCJC divides the collector depletion
charge between the internal and external base. Reverse diffusion charge stays on the
internal junction. The base spreading resistance uses the operating base current and
normalized base charge. Its bias conductance also sets the base resistor thermal noise.

## Noise

`onoise_spectrum` is output voltage amplitude density in V/√Hz. `inoise_spectrum`
divides that density by the magnitude of the transfer from the named independent
source. Its unit is V/√Hz for a voltage source or A/√Hz for a current source. spx
uses a unit perturbation of that source, independent of other sources' AC settings.
A zero gain makes input referral undefined and produces `E-ANALYSIS`.

At each frequency the engine factors the complex AC matrix and solves its transpose
once for the output probe. The projection of each source onto that solution gives
its complex output transfer. Independent contributions add in power. Within one
generator the signed coefficients of all current, voltage and charge equations
combine coherently. Each generator's output and input power are integrated over the
actual frequency grid by fitting a power law between adjacent positive samples.
Intervals adjoining a zero use a trapezoid. Square roots give `onoise_total`
and `inoise_total` in V or A RMS. A single frequency has zero bandwidth.

spx exposes `onoise.<instance>` spectra and `onoise_total.<instance>` RMS values on
every run. These names are spx extensions. The session/MCP summary ranks contributors
by integrated output noise. A positive summary interval prints device densities
on the CLI at the first point and every interval thereafter. Zero suppresses this
point-by-point listing. The interval has no effect on integration.

| Device | Independent generators |
|---|---|
| Resistor | Johnson–Nyquist noise with the effective resistance and instance temperature. `noisy=0` disables it. |
| Diode | Junction shot noise, KF/AF flicker proportional to the junction current raised to AF, and series-resistance thermal noise. |
| BJT | Collector and base shot noise, base-current KF/AF flicker, and RC/RB/RE thermal noise at the internal terminals. |
| MOS1, MOS2 and MOS3 | Channel thermal noise with conductance 2·abs(gm)/3, gate-referred KF/AF flicker projected through gm, and drain/source series-resistance noise. |
| BSIM3v3.3 | noiMod 1–4 select SPICE2 or inversion-charge thermal noise and empirical or trap-density flicker. Diffusion sheet resistances contribute independent thermal sources. [Mode mapping and units](bsim3.md) |
| BSIM4 | Charge-based channel thermal noise (tnoiMod 0), partitioned channel noise (1), empirical KF/AF/EF flicker (fnoiMod 0), and unified trap-density flicker (1). Enabled diffusion, electrode gate and body resistances contribute at their internal nodes. Gate-to-source, gate-to-drain and gate-to-body leakage currents contribute shot noise. |
| Verilog-A | `white_noise`, `flicker_noise` and numeric `noise_table` pairs in current or voltage contributions. Analog evaluation propagates the source through assignments and arithmetic at the converged bias. Positive table samples interpolate in log frequency and log power, with constant endpoints. |

For MOS1, the oxide capacitance of one gate is `Cg = (εox / TOX) W (L − 2 LD)`
in farads, with lengths in metres. For one device the gate-voltage power density
is `KF / (Cg f^AF)` and the drain-current power density is gm² times that value.
At AF = 1, KF has units V²·F. For general AF its units include Hz^(AF−1).
Independent parallel copies add in power. The oxide thickness for noise defaults
to 100 nm. The R2 MOS1 reference uses AF = 1.

In BSIM4, tnoiMod defaults to 0. Mode 1 uses the incremental gm, gmb and gds
of the intrinsic channel and the ratio Vdseff/Ids. RNOIA/TNOIA set the total
noise factor and RNOIB/TNOIB set the source-side partition (equations 10.11–14).
The partition is a series voltage at the electrical source, which swaps ends in
reverse operation. With finite diffusion resistance it has a Norton equivalent.
At zero resistance its transfer follows from the source-terminal derivatives
of the device currents and charges. NTNOI applies to mode 0. RNOIC and TNOIC
apply to mode 2. The 4.8.2 appendix A.10 default for RNOIB is 0.5164, while
the prose below equation 10.14 gives 0.37. spx uses the appendix value.

fnoiMod defaults to 1. Equations 10.2–8 combine inversion and subthreshold
trap noise using NOIA/B/C, EF, EM, the local depletion capacitance and effective
mobility. LINTNOI shortens the inversion noise length. The total conducting
width includes NF, and M multiplies the device power. Thermal and trap noise use
the operating temperature in kelvin. Gate and substrate resistance networks
default off. Enabled physical branches have Johnson noise, including any
drain/source diffusion resistance. Gate shot noise uses the enabled IGC/IGB
currents (both selectors default off). Chapter 10 specifies no separate
source/body or drain/body diode shot generator. The R2 source measurements
and the outstanding flicker discrepancy are in the [verification notes](../verify/README.md#r2-noise-audit).

BSIM4 uses the existing spx channel, leakage, charge and network equations, including
their documented approximations. tnoiMod 2 correlated induced gate/channel noise
is unsupported. Behavioral sources and ideal switches have no intrinsic generators.
Verilog-A file-backed noise tables and transient random-noise waveforms are outside
this implementation. [BSIM4 coverage](bsim4.md)

## Transfer and sensitivity

The transfer-function vectors are `transfer_function`, `<source>#input_impedance`,
and `output_impedance_at_v(...)` or `<sense-source>#output_impedance` for a branch
current output. All other independent source increments are zero. Resistance is in
ohms. An open port measured through a voltage source is reported as 1e20 Ω for a
current response below 1e-20 A per volt.

DC sensitivity solves one transposed DC Jacobian. At fixed bias, central parameter
perturbations evaluate the change in each individual residual stamp before
contraction with the adjoint. Each figure is an absolute derivative in output units
per parameter unit. R, C, L and independent source values use their instance names.
Ideal C and L values have zero DC derivative. At a zero GAMMA, KP, BETA or IS, the perturbation uses the positive side of the physical domain. A shared model perturbation affects
all its instances together and uses the spx name `model:parameter`.

Supported continuous model-card entries include IS, N, BF, BR, NF, NR, VAF, IKF,
ISE, VTO/VT0, KP, GAMMA, LAMBDA, PHI, U0/UO, VTH0, K1, K2, VSAT, TOX, TOXE,
TNOM, TC1, TC2 and positive RS/RD/RB/RC/RE where the built-in model uses them.
The basic diode IS/N, BJT IS/BF/BR and MOS1 threshold, body, channel-length and
gain defaults are also considered. An implicit mobility-derived KP retains its
dependence on oxide thickness and mobility. Discrete selectors and topology-changing zero-resistance perturbations are outside
this coverage.

AC sensitivity stores complex derivatives versus frequency. It solves the nominal
forward and adjoint equations with one LU per frequency. For each parameter a
positive and negative perturbation re-establish the bias and linearization.
Contracting the matrix difference with the nominal solutions includes the bias
dependence of conductances and capacitances. The AC source amplitudes and phases
stay fixed when differentiating a source's DC value. Two extra bias solves per
parameter can make this analysis expensive for large models.

## Fourier

The period ends at the final stored time. spx linearly interpolates 200 equally
spaced samples, including the start and excluding the duplicated endpoint.
`magnitude(outvar)` reports signed DC and peak harmonic amplitudes,
`phase(outvar)` reports sine-reference degrees from the start of that period,
and `thd(outvar)` is 100 times the root sum of squared harmonic amplitudes 2–9
divided by the fundamental. The scalar percentage repeats along the frequency
axis. A fundamental at numerical roundoff leaves THD undefined and omits that
column. The full report is available as a Fourier plot, including for a card
preceding `.tran`. The engine requires at least one complete recorded period.
spx uses fixed harmonic count, grid and interpolation order. Use a standalone
`.four` card alongside transient `.meas` cards.

## Pole-zero

The input and output are two-terminal ports. A current drive leaves the input open
for the natural-mode calculation. A voltage drive adds a zero-volt constraint at
the input. For transmission zeros the extra column injects an unknown input
current and the extra row constrains the output voltage to zero. The dense real
generalized Schur (QZ) calculation solves the resulting pencil `-G x = s C x`.
It handles singular C matrices without inverting C. Row and column equilibration
and a characteristic frequency scale precede factorization.

`pole(1)`, `zero(1)` and subsequent vectors each hold a complex root. Results
are sorted by real and then imaginary part. Algebraic modes at infinity are
discarded. Roots beyond the scaled numerical resolution can also be lost.
The limit is 256 circuit unknowns plus the port constraint. The method retains
common pole-zero factors, including unobservable modes. A redundant ideal input
short, identically zero transfer, or singular pencil can prevent extraction.

## Distributed lines and classical FETs

A lossless line enforces equal differential port voltages and opposite entering
currents at DC. In AC and noise each outgoing wave reaches the other port with
factor exp(−j2πf TD). The ideal line adds no noise power of its own.
Transient solutions interpolate the two outgoing waves at the retarded time
on the accepted grid. The maximum internal step is bounded by the shortest
line delay. Source corners, delay boundaries and accepted switch transitions
schedule further arrivals. PZ reports E-ANALYSIS for a distributed delay
because its exponential cannot be represented by the finite G + sC pencil.

DC and AC sensitivity include `line:z0`, `line:td` and polynomial coefficients
`instance:p0`, `instance:p1`, etc. The TD derivative is zero at DC. JFET bias,
gate junction charge and classical MOS linearizations participate in TF, SENS
and lumped PZ. The JFET noise generators are RD/RS thermal noise, channel
thermal noise with conductance 2|gm|/3 and drain-current flicker power
KF·|Id|^AF/f for a single copy. Levels 2 and 3 reuse the MOS1 thermal and
flicker conventions with the selected channel's gm.
