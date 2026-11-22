# XSPICE A elements

An A card connects ordered signal ports to a named model. The last field is the model name. A model can follow its instances. Local model names and port nodes follow subcircuit scope.

```spice
A1 %vd(in ref) intermediate buffer
A2 [intermediate bias] out weighted
.model buffer gain(gain=2 in_offset=0.1 out_offset=-0.2)
.model weighted summer(in_gain=[1 -0.5] in_offset=[0 0])
```

A scalar voltage port refers to ground by default. Use `%v` or `%i` for a single-ended voltage or current, and `%vd(p n)` or `%id(p n)` for a differential port. A bracketed port has one type for the entire vector. For example, `%vd[a b c d]` supplies the two differences a−b and c−d. A current input imposes zero volts and measures the current entering its positive terminal. A current output drives current from its positive terminal to its negative terminal. A voltage output imposes the specified voltage. The analog switch accepts conductance and resistance ports (`%g`, `%gd`, `%h`, `%hd`). Use an explicit differential modifier for a two-node switch output. The parser recognizes `~` and `NULL` as disconnected ports. Individual model signatures specify which analog ports accept NULL. Digital storage models allow disconnected asynchronous controls and complementary outputs.

## Built-in inventory before R13

| Domain | Built-in model names |
|---|---|
| Analog | `aswitch`, `climit`, `d_dt`, `divide`, `filesource`, `gain`, `int`, `limit`, `mult`, `pwl`, `s_xfer`, `sine`, `slew`, `square`, `summer`, `triangle` |
| Hybrid | `adc_bridge`, `dac_bridge`, `d_osc` |
| Digital | `d_and`, `d_buffer`, `d_dff`, `d_dlatch`, `d_fdiv`, `d_inverter`, `d_jkff`, `d_lut`, `d_nand`, `d_nor`, `d_open_c`, `d_open_e`, `d_or`, `d_pulldown`, `d_pullup`, `d_source`, `d_srff`, `d_srlatch`, `d_tff`, `d_tristate`, `d_xnor`, `d_xor` |
| Real event nodes | None |

Comparing that baseline with the ngspice 44 user manual gives the following additions. Together these two tables and the external exclusions below cover all 68 model entries in §§8.2–8.4. Sansho also supplies the open-collector and open-emitter gates.

| Domain | Manual entries absent before R13 |
|---|---|
| Analog (§8.2) | `pwlts`, `xfer`, `delay`, `hyst`, `ilimit`, `multi_input_pwl`, `zener`, `core` (PWL and hysteretic), `lcouple`, `oneshot`, `cmeter`, `lmeter`, `pswitch`, `sidiode`, `potentiometer`, `memristor`, `table2d`, `table3d` |
| Digital | `d_state`, `d_ram`, `d_genlut` |
| Real and hybrid (§8.3) | `bidi_bridge`, `d_pwm`, `real_gain`, `real_delay`, `real_to_v`, `d_to_real` |

## Excluded external models

Sansho runs built-in XSPICE models in the simulator. Its security policy excludes models that launch another process or load executable code from a shared library. A `d_process` or `d_cosim` model declaration and the `codemodel` loading command each produce a source-located `E-UNSUPPORTED` error. This also applies to unused model cards.

## Transfer functions

The table gives Sansho’s defaults. R12 timing and threshold defaults measured against 44.2 are recorded separately below. Numeric array lengths must match the corresponding vector width. A missing required parameter produces `E-ELAB-PARAM` with the model name, parameter name and model-card location. Unsupported parameters also produce an error.

| Model | Ports in order | Parameters and behavior |
|---|---|---|
| gain | input, output | Adds `in_offset` (0), multiplies by `gain` (1), then adds `out_offset` (0). |
| summer, mult | input vector, output | Apply `in_offset` (zeros) and `in_gain` (ones) per input. Sum or multiply the adjusted inputs, then apply `out_gain` (1) and `out_offset` (0). |
| divide | numerator, denominator, output | Independent `num_offset`, `den_offset`, `num_gain`, `den_gain` and output gain and offset. Gains default to one and offsets to zero. `den_lower_limit` (1e−10) floors the denominator magnitude. `den_domain` (1e−10) sets the smoothing half-width. Zero selects the positive sign. |
| limit | input, output | `in_offset` and `gain` precede `out_lower_limit` (0) and `out_upper_limit` (1). The quadratic transition matches the flat and sloping tangents over a half-width of `limit_range` (1e−6). With `fraction=true` this width scales with the rail spacing. |
| climit | input, upper rail, lower rail, output | The same transfer uses signal-dependent rails, reduced by `upper_delta` and raised by `lower_delta` (aliases `out_upper_delta` and `out_lower_delta`) (both zero). Crossed rails give their midpoint. |
| pwl | input, output | Requires an increasing `x_array` and a matching `y_array` defining line segments. The outer segments extrapolate. Interior corners have tangent-matched quadratic transitions. `input_domain` (0.01) gives the half-width. With `fraction=true` (default), it scales with the shorter adjacent segment. |
| d_dt | input, output | Differentiates the gain-scaled input. Supports `gain` (1), `in_offset` (0) and `out_offset` (0). Requires `out_lower_limit` and `out_upper_limit` on the offset output. Inside the linear range the AC transfer is gain times jω. |
| int | input, output | Integrates the gain-scaled, offset input. Supports `gain` (1), `in_offset` (0) and `out_ic` (0). DC and the transient operating point use the initial output. Requires `out_lower_limit` and `out_upper_limit`. The internal integral continues accumulating while the output is clipped. Inside the linear range the AC transfer is gain divided by jω. |
| s_xfer | input, output | Requires `num_coeff` and `den_coeff` in descending powers. The numerator degree must be at most the denominator degree, with a nonzero leading denominator coefficient. Evaluates the rational function at s / `denormalized_freq` (1 rad/s). Supports `gain` (1), `in_offset` (0) and one `int_ic` per state (zeros). UIC uses the supplied states. A regular operating point solves the steady-state equations when the denominator has a nonzero constant term. |
| slew | input, output | Tracks the input with `rise_slope` and `fall_slope` in signal units per second (both 1e9). Each accepted step moves toward the input by at most the applicable rate times elapsed time. A breakpoint locates where the ramp reaches the input. DC and AC have unity gain. |
| sine, triangle, square | frequency control, output | `cntl_array` ([0 1]) and `freq_array` ([1k 2k]) map control to frequency in Hz by linear interpolation, holding the endpoint frequencies outside the table. Integrates phase in cycles. `out_low` and `out_high` default to −1 and 1. Triangle and square accept `duty_cycle` (0.5). Square accepts `rise_time` and `fall_time` (1 ns). At phase zero the square starts low. Its rise starts at phase `1 − duty_cycle` and its fall starts at the next cycle boundary. At constant frequency the rise and fall last their specified times in seconds. AC output is zero. The time solver limits the step using the maximum frequency and locates waveform corners from phase crossings. |
| aswitch | control, resistive port | `cntl_off` (0) and `cntl_on` (1) select `r_off` (1 TΩ) and `r_on` (1 Ω). Resistance interpolates logarithmically for `log=true` (default), linearly otherwise. With `limit=true` the endpoint resistances bound the output. |
| filesource | output vector | Reads `file` (default `filesource.txt`) with one time column and one amplitude column per output. Interpolates linearly, holds endpoint values and schedules sample times as breakpoints. `amplstep=true` holds each sample until the next one. Supports `timescale` (1), `timeoffset` (0), `timerelative` (false), `amplscale` (ones) and `amploffset` (zeros). AC excitation is zero. |

## File input

```spice
Adata [reference drive] stimulus
.model stimulus filesource(file="stimulus.dat" amplscale=[1 2])
```

The file belongs under the main deck directory. A model in an included deck resolves its path relative to that deck. Absolute paths, parent components, a symlink at the filename and destinations outside the root produce `E-XSPICE-FILE`. Linux checks the open regular-file handle before reading it. Other platforms restrict the file to the root directory itself. Text-only parsing disables file input. Every row must have the same number of finite numeric columns. With relative times, the time column gives increments. Scaled sample times must increase strictly.

## Coverage

DC, AC and transient share the port equations. Algebraic small-signal gains come from forward differentiation at the operating point. Supported scalar gains and offsets on explicit model cards participate in sensitivity. Continuous A models supply no intrinsic noise sources. The R13 tables below describe the magnetic, current-limited and sampled models.

## Verification against ngspice 44.2

With the oscillator deck at 1 kHz and duty cycle 0.4, the reference stays low through 600 µs and reaches 1 V at 610 µs. The falling edge measures 1, 0.5 and 0 V at 1000, 1010 and 1020 µs. The slew output reaches 1 V at 800 µs and returns to zero at 1250 µs, with slopes of +5000 and −4000 V/s and no overshoot. Across 208 reference samples the maximum errors are below 4 × 10⁻¹⁴ V for both square and slew.

The arithmetic, aswitch, curves, file and oscillator decks pass the cached comparison. The operator and ramp decks use explicit ±10 V limits and pass analytic checks. Their updated reference waveforms are pending. [Verification decks](../verify/README.md#r11-analog-code-models)

### R13c residuals

This section records the R13c baseline. The [R13d measurements](#r13d-measured-laws) below supersede its provisional laws.

R13c compares Sansho at the time or bias of each sample in the maintainer's ASCII raws.
Nine decks pass at their original tolerances. The real-event chain has a known
end-to-end gain difference. The raws save analog voltages and currents only.

**General LUT (§8.4.23).** In ns, the first output starts its rising and falling
ramps at 5.6 and 15.5. The second starts at 5.8, 10.8 and 15.8. Both DACs slew
at 5 V/ns in magnitude. At 20 ns, input A becomes U while B falls. A alone
determines the unknown result. Both outputs take A's 0.2 ns input delay.
The previously low output adds its 0.4 ns rise delay and the previously high
output adds its 0.5 ns fall delay. They start moving at 20.6 and 20.7 ns
and settle at 0.45 V. Sansho applies the directional partial-transition rule
used by the R12 gates. For a known address it uses the longest changed-pin delay.
For an unknown address it considers changes to unknown bits. The original
deck has only one such bit. The two-unknown probe tests which arrival controls
when their delays differ. The same probe measures releases and strength-only
changes between U and Z, for which Sansho retains the rising-delay convention.

**One-shot (§8.2.26).** The clock crosses 0.5 V at 1.50 and 7.50 ns.
The neighboring 44.2 samples are 0.45 and 0.55 V at ±0.05 ns from each
crossing. Output timing starts at the first active sample on each edge.
At 0.5 V control the interpolated pulse width is 10 ns. The first ramp
rises from 0 to 1 V over 2.55–4.55 ns. Retriggering extends the plateau
through 7.55 + 1 + 2 + 10 + 1 = 21.55 ns. The output then falls to zero
at 23.55 ns. Sansho samples the clock on the transient grid and schedules
the pulse corners once it recognizes the trigger.

**RAM (§8.4.20).** With a 1 ns read delay, the bare DAC starts moving to 1
and 0 V at 3 and 11 ns, during the writes. Ending those writes at 6 and
14 ns produces no voltage change. Reading address zero at 18 ns starts a
rise at 19 ns. Deselecting at 22 ns starts a fall to 0.45 V at 23 ns.
All four ramps have full-swing slope magnitude 5 V/ns. Sansho now carries
the addressed word's level on the selected output during a write. Its
existing write-time drive strength is provisional. A lone DAC senses the
logic level even on a released line. The probe observes both the event symbol
and a copy of the same RAM with a weak pulldown.

**Real chain (§§8.3.5–8).** The two digital codes map to 0.25 and 1.25.
The specified real gain gives `2 × (x + 0.25) − 0.5`, or 0.5 and 2.5.
With clock-to-output delay 0.5 ns and bridge gain 0.5, Sansho ramps to
0.25 V over 4.5–4.9 ns and to 1.25 V over 20.5–20.9 ns. The reference
instead approaches 0.5 and 2.5 V. Its first nonzero sample is 0.0175 V
at 4.528 ns. From 4.728 ns the remaining error shrinks by a factor of
0.75 per 0.1 ns step, and the second transition has the same tail.
The plateau-to-plateau swing contradicts the product of the specified gains.
The saved voltage alone cannot identify the responsible stage or establish
a general transition law. The deck records this difference and Sansho applies
the specified gains.

**Zener (§8.2.13).** Let `V` be positive reverse bias and
`I = −I(Vs) = (Vs − V)/220 Ω`. The leakage shunt is 23.5 MΩ. After
subtracting the shunt and saturation current, the 4.3 and 4.5 V supply
samples give an e-fold voltage of 30.0001 mV. The secant through the
6.8 and 7.0 V supply samples gives 2.996 Ω around the 10 mA rating.
An exponential through the rated point with this incremental resistance has
`S = Ib × Rb` and avalanche term `Ib × exp((V − Vb)/S)`. Sansho subtracts
the exponential's zero-bias value so the net current vanishes at zero, and
adds the forward junction and parallel leakage. The zero-bias correction
is negligible in this deck. Direct voltage drive with varied resistance
tests the continuation for broader knees. Across all 91 loaded points,
the largest residuals are 0.546 mV and 2.48 µA at 5.2 V supply.

The probe driver expands one parameter per deck. The original five passing
decks (hyst, ilimit, multi_pwl, state and transformer) still pass.
[R13c probe settings and observables](../verify/probes/README.md#r13c-xspice-residuals)

### R13d measured laws

The maintainer supplied 17 ngspice 44.2 sweeps. All ten original R13 batch decks
now pass the cached verifier at their original tolerances. The largest normalized
error across the ten decks is 0.487 on the hysteresis waveform. The probe logs
also expose event states that the batch analog comparisons cannot distinguish.

**General LUT.** An unknown address produces `Uu`. For two pins becoming
unknown together, the delay comes from the first unknown in address order.
Varying pin zero through 0.1, 0.5 and 0.9 ns while holding pin one at 0.4 ns
gives those same three input delays. On recovery to a definite address the
input delays are 0.4, 0.5 and 0.9 ns, the maxima over changed pins. Add the
output rise delay for low → U or U → high and the fall delay for high → U or
U → low. A direct strong high → Z release emits `Uz` after the fall delay.
A release from an unknown address emits `0z`. Sansho preserves the released
level while subsequent addresses still select Z. With a 1 ns rise and a 0.2 ns fall delay,
Uu → 0z adds 0.4 ns and 0z → Uu adds 0.6 ns. The R13e fall-delay sweep
identifies the contribution carried from one transition to the next.

**RAM.** Selection and high write enable make the addressed word transparent
to the data. The output carries the stored bit at high impedance (`0z` or
`1z`). Ending the write drives that bit strongly. Deselection emits `Uz`,
and reselection reads the retained bit. Each of these output events, including
a data change during a write, follows its cause by exactly `read_delay`.
The weak pulldown resolves to `0r` throughout a write. Sansho records changes
to the resolved node. The oracle also prints repeated `0r` entries when only
the released driver's level changes.

**Real gain and transition time.** The digital converter produces 0.25 and
1.25. The real amplifier produces 0.5 and 2.5, which the clocked delay
publishes 0.5 ns after its rising clocks. For a positive bridge transition time
`T`, let `x` be the signed real input during the interval, `s` the internal
state at its start, and `g` the bridge gain. For the measured steps (Δt < T),

```text
increment = (Δt / T) × (x − s)
output    = s + g × increment
next s    = s + increment
```

At an event the elapsed interval ends with the old input. The new input acts
over the following interval. Gain zero exposes the preceding internal state,
gain one exposes the new state, and gain two gives their linear extrapolation.
For steps shorter than T, the steady voltage equals the held real value for all four gains. The
variable steps at the clock corners distinguish this law from scaling only
the first increment after an event. A real bridge re-anchors its internal
state once per accepted analog step. Repeated Newton evaluations at the
same time retain the emitted sample and rejected trials roll back both values.
A finite transition supplies a slope and introduces no fixed completion
breakpoint. In an algebraic circuit coupled only through such output bridges,
landing on an event preserves the analog step proposal. The original deck
then agrees at all 288 reference times within 3.27 × 10⁻¹¹ V.

The three transition probes measure constant-step error factors 0.75, 0.875
and 0.9375 at Δt = 0.05 ns. The same equation fits the rising and falling
edges on the shorter corner steps. The gain probe's `scaled` event node starts at 0,
reaches 0.5 at 0.8 ns and gives 2.5 at 10.225 ns. R13e varies the
amplifier delay and IC and covers bridge steps longer than T.

**Zener.** Use positive reverse bias `V`, rated voltage `B`, rated avalanche
current `Ib`, and `S = Ib × r_breakdown`. Equally spaced high-bias *current
differences* remove the offset and give e-fold voltages of 15, 30, 60 and
1000 mV. At 100 Ω the reverse branch passes through zero and the rating.
Its continuation into forward bias has the tangent slope at zero. With
`A = Ib / expm1(B/S)`, the reverse-oriented avalanche contribution is

```text
V ≥ 0    A × expm1(V/S)
V < 0    A × V/S
```

Add the parallel leakage `V × i_rev / B` and the forward junction with
the appropriate current sign. At 100 Ω, `A = 91.787605 µA`, the zero-bias
avalanche slope is 91.787605 µS, and the avalanche incremental resistance
at the rating is `r_breakdown × (1 − exp(−B/S))`, or 99.0905 Ω. The narrow
knees approach the specified resistance. Factoring the reverse exponential
about the rating avoids forming `exp(B/S)` for narrow knees.

[Probe measurements and follow-up decks](../verify/probes/README.md#r13d-probe-results)

### R13e measured laws

**General LUT.** Let R and F denote one output's rise and fall delays.
Each row gives the output component of the lag from the current input event.
Add the applicable input delay once per event, including on the return from Z.

| Output sequence | Successive output delays |
|---|---|
| `1s → Uu → 0z → Uu → 1s` | F, 2F, 3F, R |
| `0s → Uu → 0z → Uu → 0s` | R, R + F, R + 2F, F |
| `1s → Uz → 1s` at definite addresses | F, R |

The Uu → 0z and 0z → Uu legs each add F to the preceding output delay.
The direct release carries an unknown level at high impedance. The release
after an unknown address carries low at high impedance for either starting
level. Selecting another Z entry preserves that symbol and its delay history.

**Real gain startup.** At the initial transient operating point, `d_to_real`
publishes the converted value immediately and `real_gain` drives its IC.
For amplifier delay D and a settled startup input, the first computed output
arrives at 2D. Later changes arrive D after the real input event. The
converter's own delay applies to later digital changes. Standalone OP and DC
analyses in Sansho continue to evaluate the static gain.

**Real-to-analog bridge.** The operating-point output is `g × x` and the
internal state starts at x. For T = 0 the bridge directly drives `g × x`.
For T > 0 let x be the signed input at the start of the accepted interval.

| Interval | State at end | Analog output at end |
|---|---|---|
| Δt < T | s + δ, where δ = (Δt / T) × (x − s) | s + g × δ |
| Δt > T | x | g × x |

Even at constant input a change in step size can change the output when
g differs from one. The internal state in the direct branch is the unscaled
input for the next interval.

**Open items.** Both branches occur at nominal Δt = T in the printed
traces. Sansho compares the actual floating-point interval and selects
direct gain at exact equality. The logs do not resolve the oracle's equality
test. At positive T the first two transient samples retain the DC gain
in 44.2. Their common startup grid leaves the condition for that hold
undetermined. Sansho applies the interval law from its first step. The
rounded analog and event times also leave the ordering of individual
coincident samples open, including the falling edge at T = 0.
[Replay measurements](../verify/probes/README.md#r13e-probe-results)

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
| `d_osc` | Analog control, digital output | Uses increasing `cntl_array` (default [0 1]) and matching positive `freq_array` (default [1M 2M]) in Hz. Interpolates linearly and holds endpoint frequencies. Starts at `0u`. At constant frequency f, the first strong high arrives after (1 − duty_cycle)/f and the first strong low at 1/f. Samples the control at each edge. Duty defaults to 0.5. The model accepts `rise_delay` and `fall_delay` and drives its output at the phase crossings even when these parameters are nonzero. |

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
| `d_source` | Digital output vector. Reads `input_file` (default `source.txt`) under the same path and open-file restrictions as analog filesource. Each row gives a nonnegative time followed by one state per output, for example `3n 0s 1r Uu`. Times increase strictly. An output stays unknown until its first sample. |
| `d_lut` | Digital input vector, one output. Requires a quoted `table_values` string of exactly 2ⁿ levels. The first input is the least significant address bit. An unknown address yields unknown. |

### Event timing and output

The operating point alternates analog solves and digital settling with zero propagation time. A transient begins from this resolved state. A flip-flop remembers the last definite clock level across the ADC's unknown window and triggers on arrival at high after low. The ADC uses the same inclusive thresholds in DC and transient. A self-fed buffer settles to `0s` in DC. A closed latch retains its IC and an open D latch takes its data during settling.

Sansho schedules each output change at its propagation deadline. A new change preserves earlier pending arrivals on that driver and replaces arrivals due at or after its own deadline. Equal delays therefore pass narrow pulses through a buffer or inverter. With a 2 ns rising delay and a 0.25 ns falling delay, a 0.70 ns input pulse is suppressed because the falling transition overtakes the rise. At a given time Sansho first applies all due driver changes, resolves the affected nodes together, then evaluates their fanout in instance order. A response with zero delay enters the next delta iteration. In particular, a pulse ending exactly when its pending event matures can propagate. Only the settled state at that time enters the output.

Strong drivers take precedence over resistive drivers, which take precedence over high impedance. Disagreement at the strongest active strength gives unknown. Undetermined drive can contend at any strength. A definite strong driver fixes the resolved strength even when contention makes the level unknown.

Scheduled events, file samples, oscillator edges and DAC ramp endpoints constrain analog steps. An ADC samples the converged analog solution at a trial time and schedules propagation from that time. The accepted time grid therefore affects its edge times. A rejected step discards its digital state and queue changes. Digital and mixed analog/digital feedback have a limit of 256 iterations per settling loop. Exceeding it reports `E-XSPICE-OSCILLATION` with the simulation time. The ring deck uses an enable input to establish a known start. The coarse counter has one 10 ps ADC sampling difference. At a 20 ps maximum step the complete counter histories agree. Changing TRTOL from 7 to 1 gives identical Sansho grids on this charge-free circuit. [Counter measurements](../verify/probes/README.md#counter-timestep-and-trtol)

CLI JSON and the session API return an `events` map for OP, DC and transient plots. Each entry contains the initial settled state and successive changes as coordinate/symbol pairs. MCP `get_waveform` accepts a bare digital node name and returns the symbols plus a numeric code (level index + 3 × strength index, in the orders listed above). A point budget selects evenly spaced event indices. `edisplay` lists event nodes and `eprint node ...` prints their histories. Sansho raw files carry the analog vectors, including DAC voltages. All five comparison decks save analog outputs explicitly.

The event subsystem covers operating point, DC sweep and transient behavior. Setup and hold timing checks are outside coverage. Oscillator frequency changes within a scheduled half-cycle take effect at the next edge. ADC crossing location depends on the analog solver sampling both sides of the threshold.

## R13 analog models

The section numbers below refer to the ngspice 44 user manual. Ports appear in instance order. The model card supplies the default terminal types, including the implicit pairs in `Acore mp mn iron`. The named-source modifier `%vnam Vsense` reads the existing voltage-source branch current. File-backed models use the same open-file confinement as `filesource`.

| Section and model | Ports | Behavior and parameters |
|---|---|---|
| §8.2.8 `pwlts` | Output | Evaluates the required `x_array` and `y_array` against simulation time. Uses the PWL smoothing parameters and schedules the knees and their smoothing boundaries. `limit=true` holds the endpoint values. The default extends the endpoint slopes. |
| §8.2.10 `multi_input_pwl` | Differential input vector, differential output | Requires two or more inputs and tables `x`, `y`. Looks up the smallest input for `model="and"` (default), or the largest for `model="or"`. A decreasing output table implements inversion. |
| §8.2.12 `pswitch` | Control conductance, differential load conductance | The control draws current through `r_cntl_in` (1 TΩ). Between `cntl_off=0` and `cntl_on=1`, the resistance changes from `r_off=1e12` to `r_on=1` Ω. `log=true` interpolates log resistance. |
| §8.2.13 `zener` | Differential conductance, anode first | Requires `v_breakdown`. The avalanche exponential has an e-fold voltage of `i_breakdown × r_breakdown` and is normalized to the rated current at the rated voltage. The forward-side continuation uses the zero-bias tangent. The default breakdown current is 20 mA. A parallel conductance contributes `i_rev/v_breakdown`. The forward junction uses `i_sat=1e-12` A, `n_forward=1` and the circuit temperature. `limit_switch=true` activates Newton junction limiting. |
| §8.2.14 `ilimit` | Voltage command, positive supply, negative supply, output | The target is `gain × (input + in_offset)`, bounded by connected supplies. Either supply may be NULL. The voltage error drives the load through `r_out_source` or `r_out_sink` (1 Ω each). Optional positive current magnitudes `i_limit_source` and `i_limit_sink` bound the two directions. Supply equations account for the load current. `v_pwr_range` defaults to 1 µV, `r_out_domain` to 1 nV and both current smoothing widths to 1 nA. |
| §8.2.15 `hyst` | Input, output | The rising transfer spans `in_low+hyst` to `in_high+hyst`, and the falling transfer uses the same limits minus `hyst`. Input endpoints default to 0 and 1, hysteresis to 0.1 and output rails to 0 and 1. `input_domain=0.01`, `fraction=true` round the knees. |
| §8.2.19 `xfer` | Input, output | AC gain comes from exactly one of `table` and a Touchstone `file`. `span=3` sets the record width. `offset=1` locates the first value after frequency. `r_i`, `db` and `rad` select coordinates or units. File options supply units and representation when the model leaves them implicit. DC and transient output are zero. |
| §§8.2.21–22 `lcouple`, `core` | Winding pair and MMF pair for the coupler, one magnetic pair for the core | `lcouple` imposes MMF = turns × winding current and induces voltage from the flux derivative. `num_turns` defaults to 1. Core mode 1 requires `length`, `area`, `H_array`, `B_array` and returns flux as area × B(MMF/length). Mode 2 uses the hysteresis characteristic directly in MMF and flux units. Both modes accept `input_domain` and `fraction`. |
| §8.2.26 `oneshot` | Clock, optional width control, optional clear, output | The active clock edge passes `clk_trig=0.5` in the direction set by `pos_edge_trig=true`. The control selects width from `cntl_array=[0 1]`, `pw_array=[1e-6 0.9999999]` seconds. `retrig=true` extends an active pulse. A rising clear terminates it. Rails default to 0 and 1. Rise and fall delays and transition times each default to 1 ns. |
| §§8.2.27–28 `cmeter`, `lmeter` | Sense, output | Outputs `gain` (default 1) times the total of attached explicit capacitances in farads or inductances in henries. Uses elaborated, temperature-adjusted component values. |
| §8.2.29 `memristor` | Differential conductance | Starts at `rinit=7000` Ω. Voltage changes resistance at the slope `alpha=0` inside ±`vt=0` V and `beta=1` outside. The two rate segments join continuously. Integrates on transient steps and clamps resistance to `rmin=10` and `rmax=10000` Ω. |
| §§8.2.30–31 `table2d`, `table3d` | Two or three scalar inputs, output (current by default) | The file starts with the axis sizes, then increasing axis coordinates, then samples with x varying fastest. Defaults are `2D-table-model.txt` and `3D-table-model.txt`. Multilinear values use `offset + gain × sample` (defaults 0, 1). `order=3` controls the ENO derivative stencil. `verbose` is accepted. |
| §8.2.32 `sidiode` | Differential conductance | The off-region line has slope 1/`roff`. Above `vfwd` and below −`vrev`, the slopes become 1/`ron` and 1/`rrev`. `epsilon` and `revepsilon` round the knees. `ilimit` and `revilimit` saturate the current smoothly. Defaults are 1 Ω for `ron`, the same resistance for omitted `roff`, 0 and 1e30 V for the thresholds, 0 for rounding and 1e30 A for both limits. |
| §8.2.33 `delay` | Input, voltage output, optional control | `delay` defaults to zero. With `has_delay_cnt=true`, control 0–1 selects `delmin`–`delmax` (both default to zero). `buffer_size=1024` counts accepted samples. Transient reads interpolate the history. DC passes the input and AC applies the propagation phase at the bias delay. |
| §8.2.34 `potentiometer` | Lower end, wiper, upper end | The lower resistance is `r × position` for linear taper and `r × 10^(-position × log_multiplier)` for log taper. The upper part is the remainder of `r`. Defaults are 100 kΩ, position 0.5, linear taper and multiplier 1. End stops are displaced inward by one billionth of full travel. |

## R13 event models

Real nodes carry signed floating-point samples. Multiple drivers sum in instance order. A tilde on a scalar real terminal reverses its sign. The real and digital domains share event deadlines, delta settling and rejected-step rollback. Numeric real histories are available through the session API, `eprint` and `edisplay`. See the ngspice 44 user manual §§8.5.2, 21.3 and 22.2.1.3.

| Section and model | Ports | Behavior and parameters |
|---|---|---|
| §8.3.3 `bidi_bridge` | Analog vector, digital vector, optional direction vector | `direction=0` drives analog, 1 senses analog, and 2 (default) consults the direction pins. Supports `in_low/in_high` (0.1/0.9 V), reversed thresholds for hysteresis, `out_low/out_high` (0/3.3 V), `strength`, `smooth`, `input_load`, independent analog transition times and digital propagation delays. Strong output uses `drive_low/high` (20 mA) and `r_stl/sth` (20 Ω). Resistive output uses `r_low/high` (10 kΩ). |
| §8.3.5 `d_to_real` | Digital data, optional enable, real output | Converts the two known levels to `zero=0` and `one=1` after `delay=1n`. NULL enable keeps the bridge active. |
| §8.3.6 `real_delay` | Real data, digital clock, real output | Samples on the rising clock edge and publishes the held value after `delay=1n`. |
| §8.3.7 `real_gain` | Real input, real output | Drives `ic` at transient startup and publishes the initial `out_offset + gain × (input + in_offset)` at twice its delay. Later input changes propagate after one delay (default 1 ns). Gain defaults to 1, offsets and IC to 0. |
| §8.3.8 `real_to_v` | Real input, analog output | Steps shorter than `transition_time=1n` use a fractional state update and apply `gain=1` to the reported increment. Longer steps, zero transition time and DC apply gain to the input. Supports single-ended and differential voltage and current outputs. |
| §8.3.9 `d_pwm` | Analog control, digital output | Maps `cntl_array=[-1 1]` to `dc_array=[0 1]` by linear segments. Takes `frequency=1e6` Hz, `init_phase=0` degrees and independent rise and fall propagation delays of 1 ns. |
| §8.4.18 `d_state` | Optional input vector, clock, optional reset, output vector | `state_file` defaults to `state.txt`. Each header gives a state number, output symbols, input pattern, separator and target state. Continuations give another pattern and target. The first bit corresponds to port index zero. `reset_state=0` sets the DC and asynchronous reset state. Rising clock and reset delays default to 1 ns. Accepts `input_load`, `clk_load` and `reset_load`. |
| §8.4.20 `d_ram` | Data input vector, equal-width output vector, address vector, write enable, select vector | Stores words when selected and write enable is high. `select_value=1`, `ic=2` (unknown), `read_delay=100n`. An unknown write address invalidates the RAM. An unknown select deselects it, and an unknown write enable selects reading. Accepts `data_load`, `address_load`, `enable_load` and `select_load`. |
| §8.4.23 `d_genlut` | Input vector, output vector | Requires a `table_values` string with one block of 2ⁿ symbols per output. Input zero is the least significant bit. Supports 0, 1, X and Z, per-input `input_load/input_delay` (1 pF/0 s), and per-output `rise_delay/fall_delay` (1 ns each). |

### Choices for reference probes

- **Hysteresis and PWL.** Hysteresis starts on the rising branch. On partial reversal it holds the last accepted output until the opposite envelope reaches it. Fractional smoothing uses the input span. At equal multi-input values the first input supplies the AC derivative. The outer table segments extrapolate.
- **Switches, diodes and current limiting.** The `pswitch` knee radius is 1% of the control span. The zener table omits defaults for the resistance and leakage mentioned in the description. Sansho uses 1 Ω and 1 µA for those defaults. The simple diode takes the default zero `rrev` as `ron` and applies tanh to each polarity's unsaturated current. The current limiter uses tangent quadratics at the bounds, a cubic resistance blend near zero error and a continuous split of positive and negative supply currents.
- **Magnetics, memory and meters.** The magnetic-port current entering a coupler is the negative of the flux delivered to a core. The sign of secondary voltage therefore depends on the orientation of both magnetic ports. The memristor uses the endpoint voltage over each accepted interval, with DC and AC resistance fixed at the initial value. A single-ended meter sums adjacent explicit components. A differential meter sums components directly between its terminals. This excludes intrinsic semiconductor storage and remote network impedance.
- **Pulse and history timing.** A one-shot uses the clock threshold for clear. A NULL width input selects control zero. Width is the high plateau following the rise, with the fall delay added before the down-ramp. Retriggering on the plateau extends its deadline. The analog delay holds the oldest sample for prehistory or buffer exhaustion, clips the control to [0,1] and allows up to 1,048,576 retained samples per instance.
- **Tables and frequency response.** Grid values extrapolate the outer cells. The derivative uses up to `order` neighboring points per axis (at least two), extending toward the smaller divided difference. Ties extend left. `verbose` adds no simulator output. `xfer` interpolates rectangular complex gain and holds the two frequency endpoints. The Touchstone reader covers numeric records, option lines and comments. Touchstone 2 keyword sections are outside its grammar.
- **Bridges and oscillators.** Automatic bidirectional mode considers drivers other than the bridge itself. A known strong or resistive bus drive selects analog output. Otherwise the analog side supplies the digital value. One direction pin can control all lanes. The bridge ramps its drive weights, and smoothing levels 1 and 2 use 1% and 2% of the strong current limit at the knees. PWM starts high at zero phase, samples control at each edge and clips duty to [1e−9, 1−1e−9].
- **Real events.** A disabled digital-to-real driver, unknown data or unknown enable contributes zero to the real-node sum. The clocked real delay initializes to zero and captures the current input on each rising edge. The real bridge carries its internal state across changes to the target. [Measured gain and transition law](#r13e-measured-laws)
- **State, RAM and general LUT.** State rows match in file order. An unmatched input holds state. U matches an unknown level, and X or a dash matches any level. Unwritten RAM words have the selected IC. Sansho retains high impedance during write and deselection. The write probe resolves bare and pulled RAM output states. The LUT probe distinguishes simultaneous unknown inputs and high-impedance releases. [R13d measurements](#r13d-measured-laws)

The ten R13 decks have independent Sansho checks and cached 44.2 waveform comparisons. Distortion derivatives for the nonlinear code models, custom event-node definitions, integer-node models and automatic bridge insertion remain outside this library. [R13 verification decks](../verify/README.md#r13-code-model-decks)
