# XSPICE A elements

An A card connects ordered signal ports to a named model. The last field is the model name. A model can follow its instances. Local model names and port nodes follow subcircuit scope.

```spice
A1 %vd(in ref) intermediate buffer
A2 [intermediate bias] out weighted
.model buffer gain(gain=2 in_offset=0.1 out_offset=-0.2)
.model weighted summer(in_gain=[1 -0.5] in_offset=[0 0])
```

A scalar voltage port refers to ground by default. Use `%v` or `%i` for a single-ended voltage or current, and `%vd(p n)` or `%id(p n)` for a differential port. A bracketed port has one type for the entire vector. For example, `%vd[a b c d]` supplies the two differences a−b and c−d. A current input imposes zero volts and measures the current entering its positive terminal. A current output drives current from its positive terminal to its negative terminal. A voltage output imposes the specified voltage. The analog switch accepts conductance and resistance ports (`%g`, `%gd`, `%h`, `%hd`). Use an explicit differential modifier for a two-node switch output. The parser recognizes `~` and `NULL` as disconnected ports. Analog models require connected ports. Digital storage models allow disconnected asynchronous controls and complementary outputs.

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

DC, AC and transient share the port equations. Algebraic small-signal gains come from forward differentiation at the operating point. Supported scalar gains and offsets on explicit model cards participate in sensitivity. Continuous A models supply no intrinsic noise sources. Nonlinear A-model distortion coefficients, magnetic models, `multi_input_pwl`, `ilimit`, `hyst`, zener, varactor and current_limit are outside this implementation.

## Verification against ngspice 44.2

With the oscillator deck at 1 kHz and duty cycle 0.4, the reference stays low through 600 µs and reaches 1 V at 610 µs. The falling edge measures 1, 0.5 and 0 V at 1000, 1010 and 1020 µs. The slew output reaches 1 V at 800 µs and returns to zero at 1250 µs, with slopes of +5000 and −4000 V/s and no overshoot. Across 208 reference samples the maximum errors are below 4 × 10⁻¹⁴ V for both square and slew.

The arithmetic, aswitch, curves, file and oscillator decks pass the cached comparison. The operator and ramp decks use explicit ±10 V limits and pass analytic checks. Their updated reference waveforms are pending. [Verification decks](../verify/README.md#r11-analog-code-models)

## Digital nodes and bridges

An event node carries a level (`0`, `1`, `U`) and a drive strength (strong `s`, resistive `r`, high impedance `z`, undetermined `u`). For example, `1r` is a resistively driven high. Model signatures infer digital terminals, which also accept explicit `%d`. A net shared by an analog and a digital terminal gives `E-XSPICE-PORT`. Use a bridge to cross domains. Event nodes occupy no analog matrix rows. A circuit containing only digital elements can run without an electrical ground.

```spice
Aread [clk_v] [clk] threshold
Aflip one clk NULL NULL q NULL toggle
Apull one pull
Awrite [q] [out] levels
.model threshold adc_bridge(in_low=0.4 in_high=0.6)
.model toggle d_tff(ic=0)
.model pull d_pullup
.model levels dac_bridge(out_low=0 out_high=1.2 t_rise=1n t_fall=2n)
```

The tables give Sansho’s model parameters. The five ngspice 44.2 batch raws contain time and the saved analog voltages. They establish timing with explicit delays and show the DAC voltage during contention. Seventeen 44.2 probe runs also cover default timing, internal strengths, interrupted ramps and oscillator startup. [Digital-history measurements](../verify/probes/README.md#r12-digital-histories)

| Model | Ordered ports | Implemented parameters |
|---|---|---|
| `adc_bridge` | Analog input vector, digital output vector of equal width | `in_low=0.1`, `in_high=0.9` volts. Test low first (V ≤ in_low), then high (V ≥ in_high). The open interval gives unknown. With coincident thresholds, equality selects low from either direction. `rise_delay` and `fall_delay` default to 1 ns. |
| `dac_bridge` | Digital input vector, analog voltage output vector of equal width | `out_low=0`, `out_high=1`, `out_undef` defaults to their midpoint. `t_rise` and `t_fall` default to 1 ns for a full swing. At a new event the ramp continues from the voltage reached. Duration scales with the remaining voltage change divided by the full swing. Zero duration gives an instantaneous change. `input_load` defaults to 1 pF. |
| `d_osc` | Analog control, digital output | Requires increasing `cntl_array` and matching positive `freq_array` in Hz. Interpolates linearly and holds endpoint frequencies. Starts at `0u`. At constant frequency f, the first strong high arrives after (1 − duty_cycle)/f and the first strong low at 1/f. Samples the control at each edge. Duty defaults to 0.5. The model accepts `rise_delay` and `fall_delay` and drives its output at the phase crossings even when these parameters are nonzero. |

The 44.2 boundary probe and Sansho both give low at 0.1 V, unknown at 0.101 and 0.899 V, and high at 0.9 V with default ADC thresholds. With DAC rails of −1 and +3 V, both settle at +1 V for unknown. At 100 MHz and duty 0.25 the oscillator rises at 7.5 ns and falls at 10 ns with explicit 2 ns rise and 0.25 ns fall delays. [Boundary measurements](../verify/probes/README.md#r12d-boundary-measurements)

### Digital code models

Unless specified separately, output propagation uses `rise_delay` and `fall_delay`, both 1 ns by default. A low → unknown transition uses the rise delay and a high → unknown transition uses the fall delay. In the boundary probe these measured 2 ns and 0.25 ns respectively for both the ADC and the buffer. Input loads (1 pF by default) add once per connected pin, including repeated vector entries. `Circuit::digital_load(name)` returns the total in farads. Changing a buffer input load from 1 to 100 pF gave identical upstream and downstream event times in the load probe. The circuit equations include no electrical shunt for those loads.

| Model | Ports and behavior |
|---|---|
| `d_buffer`, `d_inverter` | One input and one output. Regenerate the input level at strong drive, complementing it for the inverter. Accept `input_load`. |
| `d_and`, `d_nand`, `d_or`, `d_nor`, `d_xor`, `d_xnor` | An input vector such as `[a b c]` followed by one output. AND and OR respect controlling low and high inputs respectively. XOR propagates an unknown on any input. Accept `input_load`. |
| `d_tristate` | Data, active-high enable, bus. Disabling releases the driver to high impedance. Indeterminate enable selects undetermined drive. Accepts `delay`, `input_load` and `enable_load`. |
| `d_pullup`, `d_pulldown` | One bus connection. Drive a constant resistive high or low. Accept `load`. |
| `d_open_c`, `d_open_e` | Input, output. The collector drives a strong low and the emitter drives a strong high. The opposite level releases the driver. Accept `delay` and `input_load`. |
| `d_dff`, `d_tff`, `d_jkff`, `d_srff` | D or T, J/K or S/R, then rising-edge clock, asynchronous set, asynchronous reset, Q, complementary Q. `NULL` disables a set or reset pin or disconnects one output. Simultaneous active set and reset give unknown. `ic` is 0, 1 or 2 (unknown), default 0. Control delays `clk_delay`, `set_delay` and `reset_delay` default to 1 ns and add to the output propagation delay. |
| `d_dlatch`, `d_srlatch` | D or S/R, then active-high enable, asynchronous set, asynchronous reset, Q and complementary Q. Enabled D tracks data. Enabled SR holds with S=R=0 and gives unknown with S=R=1. While transparent, data changes use `data_delay` (D) or `sr_delay` (SR). These and `enable_delay` default to 1 ns. Both latch families also accept set and reset delays. |
| `d_fdiv` | Clock, output. Count rising edges modulo `div_factor` (2). Start at `i_count` (0). For a partial duty cycle, output is high at counts 1 through `high_cycles` (default 1) inclusive and low at count zero. The initial count sets the DC level. A full-width high phase holds high. |
| `d_source` | Digital output vector. Requires `input_file` under the same path and open-file restrictions as analog filesource. Each row gives a nonnegative time followed by one state per output, for example `3n 0s 1r Uu`. Times increase strictly. An output stays unknown until its first sample. |
| `d_lut` | Digital input vector, one output. Requires a quoted `table_values` string of exactly 2ⁿ levels. The first input is the least significant address bit. An unknown address yields unknown. |

### Event timing and output

The operating point alternates analog solves and digital settling with zero propagation time. A transient begins from this resolved state. A flip-flop remembers the last definite clock level across the ADC's unknown window and triggers on arrival at high after low. The ADC uses the same inclusive thresholds in DC and transient. A self-fed buffer settles to `0s` in DC. A closed latch retains its IC and an open D latch takes its data during settling.

Sansho schedules each output change at its propagation deadline. A new change preserves earlier pending arrivals on that driver and replaces arrivals due at or after its own deadline. Equal delays therefore pass narrow pulses through a buffer or inverter. With a 2 ns rising delay and a 0.25 ns falling delay, a 0.70 ns input pulse is suppressed because the falling transition overtakes the rise. At a given time Sansho first applies all due driver changes, resolves the affected nodes together, then evaluates their fanout in instance order. A response with zero delay enters the next delta iteration. In particular, a pulse ending exactly when its pending event matures can propagate. Only the settled state at that time enters the output.

Strong drivers take precedence over resistive drivers, which take precedence over high impedance. Disagreement at the strongest active strength gives unknown. Undetermined drive can contend at any strength. A definite strong driver fixes the resolved strength even when contention makes the level unknown.

Scheduled events, file samples, oscillator edges and DAC ramp endpoints constrain analog steps. An ADC samples the converged analog solution at a trial time and schedules propagation from that time. The accepted time grid therefore affects its edge times. A rejected step discards its digital state and queue changes. Digital and mixed analog/digital feedback have a limit of 256 iterations per settling loop. Exceeding it reports `E-XSPICE-OSCILLATION` with the simulation time. The ring deck uses an enable input to establish a known start. The coarse counter has one 10 ps ADC sampling difference. At a 20 ps maximum step the complete counter histories agree. Changing TRTOL from 7 to 1 gives identical Sansho grids on this charge-free circuit. [Counter measurements](../verify/probes/README.md#counter-timestep-and-trtol)

CLI JSON and the session API return an `events` map for OP, DC and transient plots. Each entry contains the initial settled state and successive changes as coordinate/symbol pairs. MCP `get_waveform` accepts a bare digital node name and returns the symbols plus a numeric code (level index + 3 × strength index, in the orders listed above). A point budget selects evenly spaced event indices. `edisplay` lists event nodes and `eprint node ...` prints their histories. Sansho raw files carry the analog vectors, including DAC voltages. All five comparison decks save analog outputs explicitly.

The event subsystem covers operating point, DC sweep and transient behavior. It does not implement digital timing checks, tabular state machines (`d_state`), RAM (`d_ram`) or generalized multi-output tables (`d_genlut`). Oscillator frequency changes within a scheduled half-cycle take effect at the next edge. ADC crossing location depends on the analog solver sampling both sides of the threshold.
