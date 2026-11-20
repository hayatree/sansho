# Analog A elements

An A card connects ordered signal ports to a named model. The last field is the model name. A model can follow its instances. Local model names and port nodes follow subcircuit scope.

```spice
A1 %vd(in ref) intermediate buffer
A2 [intermediate bias] out weighted
.model buffer gain(gain=2 in_offset=0.1 out_offset=-0.2)
.model weighted summer(in_gain=[1 -0.5] in_offset=[0 0])
```

A scalar voltage port refers to ground by default. Use `%v` or `%i` for a single-ended voltage or current, and `%vd(p n)` or `%id(p n)` for a differential port. A bracketed port has one type for the entire vector. For example, `%vd[a b c d]` supplies the two differences a−b and c−d. A current input imposes zero volts and measures the current entering its positive terminal. A current output drives current from its positive terminal to its negative terminal. A voltage output imposes the specified voltage. The analog switch accepts conductance and resistance ports (`%g`, `%gd`, `%h`, `%hd`). Use an explicit differential modifier for a two-node switch output. The parser recognizes `~` and `NULL` as disconnected ports. All implemented models require their ports to be connected.

## Transfer functions

The table gives Sansho’s defaults. A full audit against the ngspice 44.2 parameter tables is pending. Numeric array lengths must match the corresponding vector width. A missing required parameter produces `E-ELAB-PARAM` with the model name, parameter name and model-card location. Unsupported parameters also produce an error.

| Model | Ports in order | Parameters and behavior |
|---|---|---|
| gain | input, output | Adds `in_offset` (0), multiplies by `gain` (1), then adds `out_offset` (0). |
| summer, mult | input vector, output | Apply `in_offset` (zeros) and `in_gain` (ones) per input. Sum or multiply the adjusted inputs, then apply `out_gain` (1) and `out_offset` (0). |
| divide | numerator, denominator, output | Independent `num_offset`, `den_offset`, `num_gain`, `den_gain` and output gain and offset. Gains default to one and offsets to zero. `den_lower_limit` (1e−10) floors the denominator magnitude. `den_domain` (1e−16) sets the smoothing half-width. Zero selects the positive sign. |
| limit | input, output | `in_offset` and `gain` precede `out_lower_limit` (−1) and `out_upper_limit` (1). The quadratic transition matches the flat and sloping tangents over a half-width of `limit_range` (1e−6). With `fraction=true` this width scales with the rail spacing. |
| climit | input, upper rail, lower rail, output | The same transfer uses signal-dependent rails, reduced by `out_upper_delta` and raised by `out_lower_delta` (both zero). Crossed rails give their midpoint. |
| pwl | input, output | Requires an increasing `x_array` and a matching `y_array` defining line segments. The outer segments extrapolate. Interior corners have tangent-matched quadratic transitions. `input_domain` (0.01) gives the half-width. With `fraction=true` (default), it scales with the shorter adjacent segment. |
| d_dt | input, output | Differentiates the gain-scaled input. Supports `gain` (1), `in_offset` (0) and `out_offset` (0). Requires `out_lower_limit` and `out_upper_limit` on the offset output. Inside the linear range the AC transfer is gain times jω. |
| int | input, output | Integrates the gain-scaled, offset input. Supports `gain` (1), `in_offset` (0) and `out_ic` (0). DC and the transient operating point use the initial output. Requires `out_lower_limit` and `out_upper_limit`. The internal integral continues accumulating while the output is clipped. Inside the linear range the AC transfer is gain divided by jω. |
| s_xfer | input, output | Requires `num_coeff` and `den_coeff` in descending powers. The numerator degree must be at most the denominator degree, with a nonzero leading denominator coefficient. Evaluates the rational function at s / `denormalized_freq` (1 rad/s). Supports `gain` (1), `in_offset` (0) and one `int_ic` per state (zeros). UIC uses the supplied states. A regular operating point solves the steady-state equations when the denominator has a nonzero constant term. |
| slew | input, output | Tracks the input with `rise_slope` and `fall_slope` in signal units per second (both 1e9). Each accepted step moves toward the input by at most the applicable rate times elapsed time. A breakpoint locates where the ramp reaches the input. DC and AC have unity gain. |
| sine, triangle, square | frequency control, output | Required `cntl_array` and `freq_array` map control to frequency in Hz by linear interpolation, holding the endpoint frequencies outside the table. Integrates phase in cycles. `out_low` and `out_high` default to −1 and 1. Triangle and square accept `duty_cycle` (0.5). Square accepts `rise_time` and `fall_time` (1 ns). At phase zero the square starts low. Its rise starts at phase `1 − duty_cycle` and its fall starts at the next cycle boundary. At constant frequency the rise and fall last their specified times in seconds. AC output is zero. The time solver limits the step using the maximum frequency and locates waveform corners from phase crossings. |
| aswitch | control, resistive port | `cntl_off` (0) and `cntl_on` (1) select `r_off` (1 MΩ) and `r_on` (1 Ω). Resistance interpolates logarithmically for `log=true` (default), linearly otherwise. |
| filesource | output vector | Requires `file` and reads a time column and one amplitude column per output. Interpolates linearly, holds endpoint values and schedules sample times as breakpoints. `amplstep=true` holds each sample until the next one. Supports `timescale` (1), `timeoffset` (0), `timerelative` (false), `amplscale` (ones) and `amploffset` (zeros). AC excitation is zero. |

## File input

```spice
Adata [reference drive] stimulus
.model stimulus filesource(file="stimulus.dat" amplscale=[1 2])
```

The file belongs under the main deck directory. A model in an included deck resolves its path relative to that deck. Absolute paths, parent components, a symlink at the filename and destinations outside the root produce `E-XSPICE-FILE`. Linux checks the open regular-file handle before reading it. Other platforms restrict the file to the root directory itself. Text-only parsing disables file input. Every row must have the same number of finite numeric columns. With relative times, the time column gives increments. Scaled sample times must increase strictly.

## Coverage

DC, AC and transient share the port equations. Algebraic small-signal gains come from forward differentiation at the operating point. Supported scalar gains and offsets on explicit model cards participate in sensitivity. Continuous A models supply no intrinsic noise sources. Nonlinear A-model distortion coefficients, magnetic models, `multi_input_pwl`, `ilimit`, `hyst`, zener, varactor and current_limit are outside this implementation. Digital and bridge families give `E-XSPICE-MODEL`.

## Verification against ngspice 44.2

With the oscillator deck at 1 kHz and duty cycle 0.4, the reference stays low through 600 µs and reaches 1 V at 610 µs. The falling edge measures 1, 0.5 and 0 V at 1000, 1010 and 1020 µs. The slew output reaches 1 V at 800 µs and returns to zero at 1250 µs, with slopes of +5000 and −4000 V/s and no overshoot. Across 208 reference samples the maximum errors are below 4 × 10⁻¹⁴ V for both square and slew.

The arithmetic, aswitch, curves, file and oscillator decks pass the cached comparison. The operator and ramp decks use explicit ±10 V limits and pass analytic checks. Their updated reference waveforms are pending. [Verification decks](../verify/README.md#r11-analog-code-models)
