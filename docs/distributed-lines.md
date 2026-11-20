# Distributed lines

An O element connects two differential ports. Its LTRA card specifies R, L, G and C per unit length and a positive LEN. The equations below use totals for the entire line. An RC or RLC line has series resistance and zero shunt conductance. An RG line has shunt leakage and no reactive storage. An LC line uses the lossless T-element propagation stamp.

## Frequency domain

Let `Z = R + sL`, `Y = G + sC`, `z = sqrt(ZY)` and `y = z/Z`. Both diagonal entries of the two-port admittance matrix are `y coth(z)`. Both off-diagonal entries are `−y csch(z)`. Port currents point into the line. The AC implementation uses the attenuating exponential for long lines and a series in ZY near zero. The same two-port participates in AC sensitivity and in the transfer of noise generated elsewhere in the circuit. Distributed resistor noise generation is outside current coverage.

At DC, an RC or RLC line carries the port voltage difference divided by its total resistance. The RG line uses its static hyperbolic admittance in all analyses.

In the 44.2 raw for `r8_ltra_ac`, all 41 frequencies from 100 kHz to 1 GHz give the real DC divider 75/(50+60+75) = 0.4054054054 at the far end and zero phase. With 500 nH and 200 pF in the line this misses the 10 ns flight time. Independently cascading the source, line ABCD matrix and load gives −0.334617 + j0.024240 V at 50.119 MHz. Over the complete sweep the ABCD solution agrees with Sansho within 4.7e−15 V for V(far) and 1.8e−17 A for the combined source current. The separate RG leakage voltage agrees with 44.2 within 1.6e−13 V. The deck records the AC reference as a known difference. An analytic regression exercises both loaded phase and source current.

## Transient convolution

The RC stamp convolves port voltage increments with the diffusion step response. For elapsed time t, the excess driving-point step admittance is the sum of `(2/R) exp(−n²π² t/(RC))` over positive n. The transfer kernel includes the additional factor `(−1)ⁿ`. The implementation switches to the image expansion below `t/(RC) = 0.2`. Short-time integration in the square root of elapsed time removes the initial singularity. At long times the code integrates each exponential analytically.

For RLC, let `a = R/(2L)`, `τ = sqrt(LC)`, and `y₀ = sqrt(C/L)`. The outgoing wave propagates by `H(s) = exp(−τ sqrt(s(s+2a)))`. Each port equation uses the local characteristic admittance `y₀ sqrt(s/(s+2a))`, the delayed remote current, and the product of H with the characteristic admittance multiplying the remote voltage. The wavefront has weight `exp(−aτ)`. Scaled modified Bessel functions evaluate the continuous impulse-response tails without forming an exponentially growing intermediate.

Each trial interpolates the accepted port histories linearly. Eight-point Gauss quadrature integrates each smooth interval. The direct wavefront is evaluated separately. Coefficients multiplying the unknown trial endpoint enter the Jacobian. Only accepted steps extend history. UIC uses V1,I1,V2,I2 when specified. Otherwise the initial solution supplies constant prehistory.

RLC defaults to a maximum step of one flight time. NOSTEPLIMIT enables interpolation into the trial interval. With STEPLIMIT, source breakpoints also schedule delayed arrivals. With NOSTEPLIMIT the circuit controls the time grid. RC follows the circuit step controls. The RC series stops at terms below 1e−16 or its 16-term bound. The time history is retained for the entire run, so storage grows linearly and total convolution work quadratically with accepted step count. LININTERP and the default STEPLIMIT are supported. Quadratic and mixed interpolation, history compaction, and the reference simulator's line-specific truncation-error controls remain gaps. Unsupported options produce W-MODEL-IGNORED. Pole-zero extraction of a dynamic distributed line remains unsupported.

## Uniform RC ladder

A U element takes two ends, a common substrate, a URC model, instance length L and optional integer N. The series resistances sum to L·RPERL and the shunts to L·CPERL. Both follow symmetric geometric weights with ratio K toward the centre. N is the number of series sections in each half. N=1 gives two equal resistors and one centre shunt.

With positive ISPERL the shunt diode anodes face the taps and the cathodes face the common substrate. Both IS and zero-bias capacitance scale with the normalized shunt weight w. The end diode access resistance is N·L·RSPERL, divided at other taps by their area relative to the end diode (w/w_end). At 1 V input the 44.2 auto output is 0.37548 V at 31 ns. Reversing the shunts leaves this output near the resistive divider voltage of 0.71429 V.

For automatic segmentation let R and C be the total line resistance and capacitance. The count per half is
`N = max(3, floor(log(2π FMAX R C ((K−1)/K)²) / log(K)))`.
An explicit N bypasses the estimate and can be as small as one. Sansho accepts up to 4096.

The 44.2 FMAX probe compares the automatic phasor directly with seven explicit ladders at the same AC frequency. At design frequencies of 0.10, 0.70, 0.85, 1.00, 1.30, 1.50 and 1.70 GHz the matches are N = 3, 4, 4, 5, 5, 5 and 6. In particular, the estimate 5.886 at 1.50 GHz still selects five. On the diode card, N = 1, 2, 4, 5 and 8 give end resistances of 0.6, 1.2, 2.4, 3.0 and 4.8 Ω. Across these five ladders the simulated far voltage and source current agree with the reference within 1.7e−13 V and 2.2e−15 A. In the combined `r8_urc` deck the worst AC voltage error is below 1e−13 V. The worst transient score is 0.652 on input current. [Probe measurements](../verify/probes/README.md#r8b-epitaxial-collector-and-urc)
