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

## TXL and CPL

**Compatibility.** Sansho implements the Y/TXL and P/CPL interfaces using the uniform RLGC telegrapher equations. DC and AC use the full matrix solution. Transient analysis uses the physical flight times, attenuated fronts and a numerical approximation to the continuous tail. The ngspice 44 manual §6.4 describes Y and P as transient-only. For these distributed elements, agreement with a 44.2 waveform is a compatibility result. The equations and independent line controls determine correctness when the two disagree. This policy applies specifically to lines. [Measured limits in 44.2](#r18c-loss-coupling-and-settling)

`Y1 a 0 x 0 wire LEN=2` connects a grounded TXL. For three conductors, `P1 a b c ref1 x y z ref2 bundle` pairs a with x, b with y, and c with z. Each port voltage is measured relative to its own reference. Branch currents point into the line at both ends. Sansho accepts one through eight CPL conductors. Both card types require a finite positive model LENGTH. Instance LEN overrides it for that instance. All four per-unit-length quantities use the same length unit. Sansho multiplies each by the effective length once. These conventions follow the ngspice 44 manual §§6.4.1–6.4.2 and the parameter tables in §§27.4.1 and 27.4.4.

TXL takes scalar R, L, G and C. CPL takes symmetric Maxwell matrices, preserving the signs of mutual entries. Supply the upper triangle one row at a time, equivalently the lower triangle one **column** at a time. For three conductors the sequence is **11, 12, 13, 22, 23, 33**. R and G may also be given as N diagonal entries. Omitted loss is zero. L and C must be positive definite. R and G may be positive semidefinite, including all-zero matrices. The wave models require positive inductive and capacitive storage. An RC diffusion line is available through the O element.

### DC and AC

The frequency solution uses the full matrices, including noncommuting R, L, G and C. With length included, write `Z = R+sL` and `Y = G+sC`. The near-end voltage and forward current follow from the far-end state by the exponential of the block matrix `[[0,Z],[Y,0]]`. For one conductor this is the hyperbolic ABCD solution with diagonal entries `cosh(sqrt(ZY))` and off-diagonal entries `Z sinhc(sqrt(ZY))` and `Y sinhc(sqrt(ZY))`, where `sinhc(x) = sinh(x)/x`.

Sansho evaluates a short section in wave coordinates and doubles its length by cascading scattering matrices. The short section has generator norm at most 0.5 and uses 16 Taylor terms beyond the identity. This keeps the growing backward wave out of the long-line calculation and allows ideal shorts, zero loss and lossless resonances without dividing by series resistance or by a sine of the electrical length. DC uses the same equations at s = 0. With G = 0 the total series drop is R times the through current. With R = 0 both end voltages are equal and the sum of incoming currents is G times the voltage. With both losses present their distribution enters through the full exponential.

The ngspice 44 manual §6.4 explicitly limits both KSPICE elements to transient simulation. Sansho also provides the physical AC response. Across all 35 frequencies in each R18 AC deck, an independent 64-section ABCD calculation in voltage/current coordinates reproduced the far-end voltages within 3.3e−14 V for TXL and 2.8e−14 V for CPL. At 100 MHz the coupled victim voltage is 0.0122168 + j0.0920836 V.

### Modal transient

Choose B such that `L = B Bᵀ`. Orthogonal eigenvectors Q of `Bᵀ C B` give squared flight times τ². The voltage transform is `T = B Q` and the current transform is its inverse transpose, preserving port power. For a mode, the series loss rate a is the diagonal of `T⁻¹ R T⁻ᵀ`. The shunt rate b is the diagonal of `Tᵀ G T` divided by τ². For repeated flight times (within 32 machine epsilons in τ²), Sansho diagonalizes the sum of these two rate matrices within that eigenspace. Simultaneously diagonal modal losses admit independent scalar filters. R18b retains off-diagonal losses in the transient of asymmetric bundles using the coupled propagation described below.

After normalizing the high-frequency characteristic admittance to one, each port satisfies `A(s)v − i = H(s)[A(s)v_remote + i_remote]`, where

- `A(s) = sqrt((s+b)/(s+a))`
- `H(s) = exp(−τ sqrt((s+a)(s+b)))`

For b > a the implementation filters the reciprocal characteristic impedance by exchanging voltage and current and reversing the sign of the incoming term. This keeps the local filter bounded even for pure shunt loss. Equal rates give an undistorted wave with attenuation `exp(−τa)`.

The implementation extracts the pure delay `exp(−sτ)`. For a > b, 32 midpoint Chebyshev nodes on the finite interval [b,a] give rates `p = b+(a−b) cos²(θ/2)`, with `θ = π(k+1/2)/32`. The local filter is a sum of first-order terms obtained by averaging `(s+b)/(s+p)`. Its residues are normalized to give the exact DC and high-frequency admittances. Multiplying the same approximation by (s+a) gives the delay-extracted propagation exponent as a constant plus terms proportional to `s/(s+p)`. Positive weights proportional to `(a−p)(p−b)/p` share the total excess attenuation `τ(√a−√b)²/2`.

For each propagation factor write `F = E exp(uP)`, where `E = exp(−u)` and `P = p/(s+p)`. Sansho retains the direct term E and the first relaxation uEP. It represents the remaining DC weight `m = 1−E−uE` by `mP q/(s+q)`, with `q = pm/(u−1+E)`. This gives F its correct DC and high-frequency gains, its derivative at zero and the first continuous impulse-response value immediately behind the front. A series evaluates m and the rate ratio for small u. The cascade includes the DC gain `exp(−τ√(ab))`. Its direct wavefront therefore has weight `exp(−τ(a+b)/2)`. The intermediate-frequency fit and the late diffusion tail depend on the finite order. Sansho integrates each pole for a linearly varying input over the actual trial interval. The local endpoint coefficient enters the Newton matrix. Rejected steps leave pole states and delay buffers intact.

The smallest flight time caps the timestep. Source corners and detected events launch arrival breakpoints for each mode. Each same-mode reflection schedules its next arrival. Reflections converted between different modes at a termination use linear interpolation on the accepted grid. Each modal buffer retains the bracketing samples back to one flight time. Without UIC, the exact DC solution supplies constant prehistory and the filters act on deviations from that bias. With UIC, the line interior and filters start quiescent, and the circuit's initialized terminal voltages and currents launch the time-zero waves. The manual specifies no Y/P instance IC tuple.

The distortionless 50 Ω acceptance line launches 0.5 V, arrives after 5 ns and settles at `0.5 exp(−0.2)` V. Its maximum error from the analytic ramp was 4.3e−15 V. The lossless TXL far end agreed with its T control within 7e−18 V on their shared time grid. An independent R8 LTRA transient control with 50 Ω total series resistance, 250 nH, 100 pF, a 50 Ω source and a 75 Ω load differed from TXL by at most 1.15 µV over a 22 ns pulse simulation with a 70 ps step ceiling. The probes exercise unequal losses, finite order, coincident delays, timestep refinement and startup. [R18 probe index](../verify/probes/README.md#r18-kspice-lines)

### Non-diagonal modal losses

In power-normalized wave coordinates the storage blocks both equal the diagonal delay matrix D. The series and shunt loss blocks are `Rₘ = √D T⁻¹ R T⁻ᵀ √D` and `Gₘ = D⁻½ Tᵀ G T D⁻½`. If either contains an off-diagonal entry larger than 32 machine epsilons times that block's largest absolute entry, Sansho retains both full blocks. R18's diagonal projection could produce a steady victim voltage even for insulated conductors with diagonal DC resistance.

The uniform line is split into N equal cells. Each cell propagates lossless modal waves for half a cell, crosses a passive static junction, and propagates for another half cell. The junction is the scattering form of the exponential with off-diagonal blocks Rₘ/N and Gₘ/N. This symmetric product converges to the full telegrapher exponential as the cell length tends to zero. At DC the delays are identities and the N static exponentials multiply to the exact DC solution. The product approximation concerns the spatial distribution of loss at finite frequency.

N is the ceiling of the longest modal flight divided by the circuit's step ceiling at line setup, bounded to 1–256. Each trial spans at most the shortest half-cell flight. Directly transmitted waves are interpolated once from their launch history. Reflected and converted waves use linear interpolation between adjacent junctions. Only accepted steps update either history. Very fast edges can resolve the discrete junctions, particularly at the 256-cell limit.

For the 12/60 Ω asymmetric pair with 0.3 mS diagonal shunt conductance, a 100 MHz sine compared with the loaded full-matrix AC solution over 80–100 ns gives maximum driven/victim far-end errors of 602/334, 213/154 and 93/72 µV for requested step ceilings of 250, 125 and 62.5 ps. With G set to zero the 100 ns step response gives 0.547445255474 V on the driven load (75/137 V analytically) and 0.52 pV on the undriven load. The old independent-mode projection assigns the mean resistance of 36 Ω to both conductors, predicting a 75/161 V driven divider.

### R18b measurements against ngspice 44.2

The seven original decks give one waveform pass and six known differences. Three of the six have a recorded reference failure and no waveform. The acceptance tolerances are unchanged.

| Deck | 44.2 evidence and physical control |
|---|---|
| `txl_mismatched` | Pass. With a 50 Ω source, a 180 Ω load and a 1 V step, the first wave has amplitude `(180/230) exp(−0.18) = 0.653690 V`. The 1 ns source edge takes 5 ns to arrive. The 44.2 samples at 6.025 and 6.075 ns give a 5.001 ns flight by the half-amplitude crossing. At 6.125 ns it gives 0.654274 V including the developing tail. The maximum far-end error on the reference grid is 6.522 mV (score 0.923). |
| `txl_ac` | All 35 reference phasors have zero imaginary part on all three signals. The near end stays at 0.500000 V, the far end at −1.53846 pV and the source current at −10.000 mA. These values resemble a 50 Ω input with negligible transmission for this circuit. The distributed DC divider gives 0.497466 V at the far end. At 100 kHz Sansho gives 0.497460 − j0.002456 V. The cache establishes the terminal behavior on this deck. The manual provides no AC stamp. |
| `cpl_ac` | The reference exits with status 1 and reports a singular matrix during the unsupported AC analysis. The grounded resistive loads and full-matrix ABCD control give a finite physical solution. The log gives no further cause for the singularity. |
| `txl_lossless` | The model omits R. Setup specifically reports missing series resistance although §6.4.1 lists a zero default. Sansho uses the documented default and agrees with the ideal T control. The explicit-zero control below runs in 44.2. |
| `txl_matched` | With R/L = G/C = 40 million/s the reference reports that all setup pivots are below 10⁻²⁸. The same diagnostic occurs at 4 mS in the shunt sweep. The manual gives no exclusion for equal rates. The physical ramp reaches 0.409365 V after the 5 ns flight and 0.1 ns rise. |
| `cpl_crosstalk` | The first negative far victim pulse reaches −301.291 mV in 44.2. At 6.685 ns Sansho gives −260.442 mV. A lossless control gives a −301.296 mV first plateau. |
| `cpl_three` | At 9 ns the reference far-end voltages (x,y,z) are (600.589, 4.967, −0.996) mV. Removing loss in an independent run of the modal equations gives (600.585, 4.968, −0.996) mV. With the deck's 7/9/13 Ω series resistances the full-loss calculation gives (563.768, 10.147, 0.934) mV. Refining the 60 ps ceiling to 30 ps changes each of those values by less than 0.2 µV. |

**CPL timing and attenuation.** For the symmetric pair, the odd and even impedances are 42.8478 and 59.8200 Ω. Their flight times `√(LₘCₘ)` are 5.48452 and 6.10164 ns. Estimating the half-height of each cached ramp by linear interpolation between actual raw points, then subtracting the source midpoint at 1.080 ns, gives 5.48552 and 6.10264 ns. The modal front transmission is `exp[−(Rₘ/Zₘ + GₘZₘ)ℓ/2]` for physical scalar per-length modal parameters. This gives 0.863762 and 0.896485. The observed reference increments divided by the lossless increments give 0.99998 and 0.99999. At 2 ns the near victim is +41.615 mV in 44.2 and +40.553 mV in Sansho. Sweeping the second resistance through 12/24/60 Ω changes the reference peak absolute far victim only from 301.297 to 301.303 mV. Sansho gives 261.254, 244.341 and 201.335 mV. Refining the reference step ceiling from 140 to 35 ps leaves the original pair's maximum far-voltage discrepancy near 70 mV.

For three conductors the half-height flights measured on z, y and x are 5.612706, 6.460488 and 7.181195 ns. The square roots of the LC eigenvalues give 5.612711, 6.460489 and 7.181195 ns. The predicted high-frequency modal transmission factors with the specified losses are 0.88985, 0.93217 and 0.92386. The corresponding measured ramp increments divided by the lossless controls are 0.99992, 0.99999 and 0.99999. The reference's absolute peaks on the two near victims are 37.454 and 11.265 mV, and on the far victims 115.917 and 9.066 mV. Changing L13 through 0/20/45 nH produces signed largest far-z excursions of +115.231/−9.732/−155.939 mV. Relative to the original reference trajectories, zeroing R and G only in Sansho gives maximum errors on the far traces of 1.94 mV (pair) and 0.70 mV (three lines). These results establish almost lossless fronts on these decks. They leave the simulator's internal approximation undetermined.

**TXL tails and startup.** With G = 0 and a 75 Ω load the direct-wave amplitude is `0.6 exp(−R/100)` for the 1-unit, 50 Ω line. At R = 4/40/200 Ω the predicted steps are 576.474/402.192/81.201 mV. The first reference samples after the 0.1 ns ramp, at 6.12 ns, are 576.523/402.936/83.766 mV including dispersion. In R18b, over the 150 ns pulse train the Sansho TXL far voltage differed from a separate Sansho LTRA convolution by at most 0.02/0.78/35.2 µV on the TXL grid. Comparing the 44.2 TXL to the same convolution on the reference grid gives 5.77/4.99/23.46 mV. With 0.2 mS shunt loss, one uniform line and two identical half-sections have the same exact transfer matrix. The 44.2 cascade probe gives maximum differences of 30.1 µV, 0.573 mV and 12.08 mV at 10, 100 and 300 Ω. This measures the approximation directly, without choosing either implementation as the reference.

At the first UIC sample (0.5 ps) the 44.2 far voltage is 0, 78.934 and 157.867 mV for 0, 0.5 and 1 V near-end ICs. A quiescent interior in Sansho gives zero before the 5 ns flight. The reference already initializes a remote response. The manual does not specify a Y/P interior initial profile. For all three settings of `r18_cpl_equal_delay`, including zero mutual R, the reference reports a forbidden CPL parameter combination after initialization. Their raw headers declare zero points. The existing inputs vary mutual R but keep both equal velocities and diagonal L/C. The R18c results below distinguish these conditions. [Probe definitions and replay commands](../verify/probes/README.md#r18b-reference-follow-ups)

### R18c loss, coupling and settling

All measurements below use the supplied 44.2 raws and logs. Timed voltage samples use linear interpolation on each simulator's own time axis. A front amplitude is the coefficient of an ideal incident step. A finite source ramp also excites the continuous tail.

#### TXL

For scalar per-length parameters, `Z₀ = √(L/C)`, `τ = ℓ√(LC)` and the direct front transmission is `A = exp[−ℓ(R/Z₀ + GZ₀)/2]`. For a unit source step the load sees `[Z₀/(Rs+Z₀)] A [2RL/(RL+Z₀)]` at the first arrival. When R/L = G/C the characteristic impedance stays constant and the entire forward wave is a scaled delay. For unequal rates the tail and reflections determine the later voltage.

| R (Ω), G = 0 | Physical front (V) | 44.2 TXL at 6.12 ns (V) | 44.2 TXL at 45 ns / DC divider (V) | 44.2 TXL / LTRA at 90 ns (mV) |
|---:|---:|---:|---:|---:|
| 4 | 0.576474 | 0.576523 | 0.581439 / 0.581395 | −0.007 / −0.001 |
| 40 | 0.402192 | 0.402936 | 0.454775 / 0.454545 | −0.338 / +0.136 |
| 200 | 0.081201 | 0.083766 | 0.232884 / 0.230769 | −23.438 / +0.418 |

The 50 Ω source drives a 75 Ω load. The edge starts at 1 ns, rises for 0.1 ns, and begins to fall at 46.1 ns. The physical DC level during a sustained high input is `75/(50+R+75)` V. At 200 Ω the 44.2 TXL near voltage reaches 0.88438 V at 45 ns, versus a DC target of 0.84615 V. Within the same 44.2 run, the maximum far-end TXL–LTRA differences are 0.059, 0.474 and 23.861 mV. The full-wave score also detects sampling of delayed corners. At 4 Ω both 44.2 near traces turn down before the reflected falling edge can arrive at 56.1 ns. At 56.09 ns they read 80.38 and 80.37 mV, while both Sansho controls give 93.29 mV.

At 200 Ω Sansho's revised scalar filter gives +0.0191 mV at 90 ns. Against the independent Bessel-kernel LTRA on the same time grid, its maximum far-end residual over the full 90 ns fell from 34.95 to 1.04 µV. At 4 and 40 Ω it is 0.016 and 0.078 µV. The correction preserves the first relaxation term of the propagation exponential as well as its DC moment.

* **Explicit R and equal rates.** The 44.2 TXL voltage and time columns are identical at R = 0, 1 µΩ and 0.1 Ω. Its 135 Ω load reaches 0.729730 V in all three cases. The ideal T control gives the same plateau, with a 7.297 mV within-run difference on the rising ramp at 6.07 ns. With the specified 0.1 Ω, Sansho gives 0.729335 V at 18 ns, consistent with the divider 135/185.1. The cached equality covers these three settings. With G = 3.99 and 4.01 mS, the matched 50 Ω circuit gives 0.409468 and 0.409263 V at 18 ns in both simulators. At exactly 4 mS, 44.2 reports a TXL setup pivot below 10⁻²⁸ and produces an empty raw. The analytic limit and Sansho give 0.409365 V.
* **DC with both losses.** For `r18b_txl_ac_ports`, ℓ = 1.5 gives total R = 24 Ω and G = 1.2 mS. For source resistances 25/50/100 Ω, the 44.2 OP gives far voltages 0.620155/0.519481/0.392157 V. Each is the series-only divider `80/(Rs+24+80)`, including the instance length. With the specified leakage, let `k = √(RG)` using line totals. The distributed solution is `Vfar = RL / [(RL+Rs)cosh(k) + (R+Rs RL G)sinh(k)/k]`. It gives 0.601311/0.497466/0.369755 V. Both Sansho port voltages and the source current agree with this solution for each source resistance.
* **AC port behavior.** In each of those runs all nine 44.2 phasors from 100 kHz to 1 GHz are real and constant. The near/far pairs are 0.902067/0.394734, 0.795779/0.330654 and 0.631045/0.249611 V. The apparent input resistances `Vnear/(−Ivin)` are 230.28/194.83/171.04 Ω. These data rule out a common 50 Ω input or a single fixed loaded two-port explanation for the whole sweep. At Rs = 50 Ω Sansho gives 0.497460 − j0.002456 V at 100 kHz and −0.469775 + j0.000321 V at 1 GHz. Evaluating the scalar ABCD hyperbolic functions independently for all three source resistances at OP and across the AC sweep gives maximum Sansho residuals of 1.4 × 10⁻¹⁵ V near, 5.7 × 10⁻¹⁵ V far and 4.7 × 10⁻¹⁷ A at the source.

#### CPL

For a symmetric pair the in-phase and opposite-phase drives excite the even and odd scalar lines separately. Their L and C are the self terms plus or minus the mutual terms. A single-ended drive is half the sum of the two drives. On the matched modal probe, the direct front voltages are `0.5 exp(−0.109274 k)` and `0.5 exp(−0.146458 k)`, where k scales both R and G. The table samples the even mode at 7.30 ns and the odd mode at 6.68 ns, just after their respective ramps.

| Loss scale k | Physical even / odd fronts (V) | 44.2 CPL even / odd (V) | 44.2 scalar TXL controls (V) | Sansho CPL even / odd (V) |
|---:|---:|---:|---:|---:|
| 0.01 | 0.499454 / 0.499268 | 0.499946 / 0.499977 | 0.500000 / 0.499269 | 0.499454 / 0.499268 |
| 1 | 0.448242 / 0.431881 | 0.500018 / 0.500008 | 0.448296 / 0.431988 | 0.448278 / 0.431962 |
| 4 | 0.322955 / 0.278322 | 0.500003 / 0.500002 | 0.323417 / 0.279218 | 0.323369 / 0.279148 |

At k = 1, the 0.25 V crossings in 44.2, less the launch midpoint, give 6.101637 and 5.484523 ns. The LC flights are 6.101639 and 5.484524 ns. The reference gets the mode polarities and delays right across the loss sweep. Its CPL level stays within 55 µV of the lossless 0.5 V after each ramp, even at four times the losses. In Sansho the CPL traces coincide with their scalar TXL controls to within 4 × 10⁻¹⁵ V on the entire probe.

| Undriven series R (Ω) | 44.2 far victim minimum (V) | Sansho far victim minimum (V) | 44.2 driven far at 100 ns (V) | 44.2 far victim at 100 ns (µV) |
|---:|---:|---:|---:|---:|
| 12 | −0.301295 | −0.262840 | 0.599993 | −3.216 |
| 24 | −0.301296 | −0.245798 | 0.599996 | −0.103 |
| 60 | −0.301295 | −0.202564 | 0.599992 | −1.507 |

This settling deck keeps the driven resistance at 12 Ω, G = 0 and the source high through 100 ns. The reference settles close to the lossless driven divider `75/(50+75) = 0.6` V on both ends. The physical near and far levels are `87/137 = 0.635036496` and `75/137 = 0.547445255` V, with both victim ends at zero. At 100 ns Sansho is within 0.19 µV on all four voltages for equal losses and within 1 pV for the two asymmetric settings. The victim pulse is real reactive coupling. Its disappearance at steady state follows from diagonal R and zero G. The 44.2 loss sweep shows little change in either its driven divider or its peak victim response.

At mutual C = −19/−20/−21 pF, with mutual L = 50 nH, **all three** reference runs succeed. The 44.2 signed far-victim peaks are −126.184 mV, +0.0845 µV and +126.302 mV. At −20 pF the two flights coincide at √24 ns. Equal delays cancel the lossless far-end pulses for these terminations. With 10 Ω series loss, the even/odd impedances are 61.237/40.825 Ω and the unequal attenuations leave a direct victim step of +9.123 mV (half the difference of the two loaded modal fronts). Sansho peaks at +9.067 mV with the finite ramp. Changing L22 in the **uncoupled** control produces the opposite setup outcome. At 225, 250 and 275 nH, 44.2 reports a forbidden parameter combination after initialization. Every raw declares zero points and contains only the initialization row. Sansho gives identically zero far-victim voltage and 0.454545 V at the driven load to within 1 µV by 18 ns. Repeated delay alone explains neither acceptance nor rejection.

#### Verification status

The seven acceptance decks give one waveform pass and six documented differences, including three reference setup or analysis errors. Across the 48 existing R18 and R18b settings there are 11 passes, 29 measured waveform differences and 8 reference rejections. All 48 run in Sansho. In the supplied comparison four of the rejections were counted as failures. The replay recognizes them by exact diagnostic and checks for a finite native result. Known waveform differences retain their measured scores and the original tolerances. [Probe definitions and cached replay](../verify/probes/README.md#r18b-reference-follow-ups)
