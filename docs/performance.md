# Simulator performance

The benchmark runs the IHP SG13G2 decks with the release Cranelift JIT. The
comparison is against ngspice 44.2 with the corresponding OSDI models. The
campaign goal is at most 0.80× reference time for each of three measurements
on every deck.

## Method

Build the unchanged CLI from the stated Git revision, save its executable
outside the release directory, then build the candidate with the same flags.
The driver alternates the two binaries on the same host. It reports the median
for each executable over seven or more pairs. Analysis is measured inside a
single-worker process. Wall time comes from separate launches with profiling
disabled and includes process startup, reading the deck, preparation, output
and exit. Each executable and each timing mode has its own private cache.
Cold means an empty Sansho cache. The OS file cache can be warm on both launches.

The reference timings come from the maintainer's simultaneous measurements of
ngspice and the baseline. For each deck and worker setting, the estimated ratio
is (candidate median / baseline median) × (maintainer baseline / reference).
The default wall estimate uses the default baseline wall time. CPU frequency
and competing work vary enough to make direct division of sandbox timings by
an older reference unreliable. Each round uses its own reference calibration.

## Round 6 results

The baseline is `63b70bd`. The maintainer measured 56.2 ms for its ring21
analysis against a 64 ms ngspice reference. Both the simulator and the
reference ran single-threaded for the analysis comparison.

Warm analysis and the two uninstrumented wall measurements each use 41
interleaved pairs with default CPU affinity. Times are local medians in ms.
The three ratios apply the maintainer's round 5 calibration to each A/B
measurement. Ring5 uses the supplied approximate baseline analysis ratio
of 0.64×.

| Deck | ngspice analysis / elapsed | Analysis −j1 HEAD → R6 | Wall −j1 HEAD → R6 | Wall default HEAD → R6 | Analysis ratio | −j1 wall ratio | Default wall ratio |
|---|---:|---:|---:|---:|---:|---:|---:|
| nmos_idvg | 2.3 / 39 | 0.510 → 0.502 | 18.480 → 17.813 | 18.167 → 18.694 | 0.205× | 0.512× | 0.591× |
| inverter_dc | 3.4 / 45 | 1.148 → 1.150 | 21.569 → 22.061 | 20.108 → 19.710 | 0.333× | 0.552× | 0.460× |
| inverter_tran | 4.45 / 47 | 1.856 → 1.881 | 21.719 → 21.870 | 22.037 → 22.269 | 0.414× | 0.563× | 0.574× |
| ring5 | 18–23 / 65 | 13.580 → 12.146 | 36.883 → 36.639 | 31.479 → 31.934 | 0.572× | 0.682× | 0.548× |
| ring21 | 64 / 127 | 53.107 → 45.937 | 82.717 → 77.206 | 60.499 → 58.424 | 0.760× | 0.650× | 0.493× |

With both executables pinned to CPU 2 for 31 pairs, ring21 analysis fell
from 51.775 to 45.802 ms (0.777× ngspice) and ring5 fell from 13.016 to
11.829 ms (approximately 0.582×). Both affinity settings meet the 0.80×
target. The three short decks use the ordinary kernels. Their measured
warm changes range from a 3.6% improvement to a 2.9% regression.

Cold wall medians include first compilation and use nine fresh-cache pairs.

| Deck | −j1 HEAD → R6 (ms) | Default HEAD → R6 (ms) |
|---|---:|---:|
| nmos_idvg | 129.5 → 134.2 | 125.8 → 118.8 |
| inverter_dc | 161.9 → 161.8 | 143.7 → 147.4 |
| inverter_tran | 171.4 → 162.6 | 149.9 → 149.1 |
| ring5 | 249.4 → 229.8 | 219.8 → 213.7 |
| ring21 | 288.3 → 267.1 | 239.6 → 226.4 |

### Work counts and exact reuse

Both builds took the same number of Newton iterations and timepoints. Each
ring ran seven Newton iterations at initialization. Of the 306 accepted
steps, four took two iterations and 302 took three. The initial solution brings the reported sample
count to 307. The ngspice accounting reports 920 iterations and 308
timepoints for each ring. The step controller, convergence tolerances and
predictor are unchanged.

| Deck | Newton iterations | Accepted / rejected steps | Group assemblies | Instance visits | Native evaluations HEAD → R6 | Exact stamp hits HEAD → R6 |
|---|---:|---:|---:|---:|---:|---:|
| ring5 | 917 | 306 / 0 | 1,224 | 12,240 | 9,180 → 9,190 | 3,060 → 3,050 |
| ring21 | 917 | 306 / 0 | 1,224 | 51,408 | 38,556 → 38,598 | 12,852 → 12,810 |

HEAD reused 25% of stamps. The initial switch of derivative sets costs one
additional evaluation per instance in round 6. Among the remaining
ring21 inputs, a baseline trace of all 36,720 attempted SIMD lanes found
just 62 repeated NMOS bias tuples and 63 repeated PMOS tuples across instances and time. Even
a history large enough to retain the entire trace would recover only 0.34%
of those lanes. Fusing the two-lane output scatter with flat slot writes also
slowed ring21, from 50.90 to 51.34 ms over 15 pinned pairs.

### Fixed supply voltage in transient Newton solves

An ideal voltage source connected to ground owns the equation
`±V = Vsource`. Once the iterate matches a constant source voltage bit for
bit, the branch equation gives a zero voltage increment. If this voltage
is the last column in the simplicial LU ordering, the source row is zero
in all previous columns. Its last pivot is ±1. The upper entries in that
column multiply a zero increment in back substitution. The packed kernel
can therefore omit the local forward-AD seeds for this voltage while
still computing all currents, charges and derivatives used by the solve.
Source current remains an unknown. Each instance retains its usual stamp
order and output layout.

The optimization requires a positive transient time, unit source scaling,
and, for a Newton solve, zero integration history in the source equation.
A negative-zero iterate selects the ordinary path to preserve the sign of
an exact zero update. Full Jacobian callers, including small-signal
analysis, keep their ordinary kernels. Packed images and cached stamps
distinguish the projected and full derivative sets. The internal transient
Jacobian differs in the constrained column. If a model has a nonfinite
derivative solely in that column, the projected evaluation can succeed where
a full derivative evaluation reports an error. Every observed IHP output
bit agrees with HEAD.

Nine separate interleaved single-worker profiles put ring21 native evaluation
at 42.95 → 37.33 ms and LU at 2.96 → 2.97 ms. Ring5 native evaluation
fell from 11.25 to 9.97 ms. The remainder of group evaluation (group wall
minus the evaluation wrappers, divided by all visits) was 69.0 → 70.6 ns
per visit for ring21 and 67.1 → 67.2 ns for ring5. These intervals include
gathering local inputs and copying stamps to group slots. Global gather and
factorization fall outside the group interval.

### Packed block experiments

A 15-run PC sample located 169 of 380 baseline native samples and 131 of
287 candidate samples in the two hottest blocks. Static x86-64 disassembly
gave the following counts of instructions with SIMD and stack operands.
Execution frequency is measured separately by the sampler.

| Packed block 1 | Code bytes HEAD → R6 | Stack reads HEAD → R6 | Stack writes HEAD → R6 |
|---|---:|---:|---:|
| NMOS (3,182) | 40,465 → 40,465 | 1,467 → 1,467 | 622 → 622 |
| PMOS (4,116) | 69,106 → 40,757 | 2,715 → 1,472 | 1,162 → 620 |

On x86-64 the widest blocks reached 53 (NMOS) and 110 (PMOS) live tangent
SSA values after common-subexpression elimination. Splitting at the live
peak and scheduling partial derivatives in two-column passes gave ring21
medians of 53.55, 54.83 and 56.78 ms for 128-instruction, 512-instruction
and whole-block scheduling windows. Their respective baselines were 50.34,
50.54 and 50.91 ms, with 15 pinned pairs each. For the 512-instruction
window, static reads/writes rose to 2,250/1,097 on NMOS and 3,966/1,574
on PMOS. A separate 15-pair device profile used the same 38,556 native
bias evaluations in both builds and measured 42.46 → 46.43 ms.

Explicitly storing live values across long gaps also failed to give a
repeatable win. Tiling across
pairs and immediately storing completed fragment outputs moved analysis
by less than 1%. Rematerializing repeated SIMD literals changed ring21
from 50.68 to 50.49 ms and ring5 from 12.82 to 12.94 ms. These experiments
were removed.

The most promising remaining target is the NMOS hot block. It now has
the largest individual sample share, and its 1,467 static SIMD stack reads and
622 writes were unaffected by supply-column specialization. Profiling
individual live ranges around math calls would guide a narrower change.
The roughly 71 ns of group overhead per ring21 visit sets the scale of
possible adapter improvements.

The offline default and all-feature test and Clippy runs passed. The cached
reference verifier passed 14 of 14 ordinary decks and 10 of 10 VA decks.
The full-vector comparison against HEAD covered all ten VA decks plus
ring21, with independent cold/warm caches and one/default workers. Every
binary64 result and worker-count cache artifact comparison passed.

## Round 5 results

Warm analysis uses 41 interleaved pairs at one worker. Wall uses nine
interleaved pairs with the default CPU affinity. All local values are ms.
The last three columns are estimated fractions of ngspice time.

| Deck | ngspice analysis / elapsed | Analysis HEAD → R5 | Wall −j1 HEAD → R5 | Wall default HEAD → R5 | Analysis ratio | −j1 wall ratio | Default wall ratio |
|---|---:|---:|---:|---:|---:|---:|---:|
| nmos_idvg | 2.5 / 42 | 0.483 → 0.492 | 23.47 → 22.21 | 26.64 → 19.64 | 0.204× | 0.554× | 0.391× |
| inverter_dc | 3.1 / 42 | 1.167 → 1.139 | 29.86 → 21.75 | 24.01 → 19.80 | 0.507× | 0.543× | 0.522× |
| inverter_tran | 4.5 / 47 | 2.516 → 1.864 | 29.71 → 24.25 | 27.68 → 23.36 | 0.466× | 0.551× | 0.548× |
| ring5 | 17.8 / 60 | 14.977 → 14.094 | 53.34 → 42.62 | 43.90 → 31.30 | 0.793× | 0.599× | 0.505× |
| ring21 | 57 / 120 | 55.120 → 53.526 | 110.63 → 82.09 | 67.94 → 62.96 | 1.026× | 0.597× | 0.548× |

Cold wall medians include first compilation.

| Deck | −j1 HEAD → R5 (ms) | Default HEAD → R5 (ms) |
|---|---:|---:|
| nmos_idvg | 128.8 → 120.9 | 132.4 → 129.5 |
| inverter_dc | 162.0 → 160.5 | 154.9 → 134.4 |
| inverter_tran | 172.1 → 169.1 | 159.6 → 157.3 |
| ring5 | 263.6 → 241.2 | 235.9 → 215.7 |
| ring21 | 309.0 → 288.4 | 259.6 → 250.6 |

A second 41-pair run pinned both binaries to CPU 2. Its estimated analysis
ratios, in deck order, were 0.201×, 0.526×, 0.618×, 0.838× and 1.045×.
Ring5 crosses 0.80× between these runs. Ring21 misses the target on both.
The shorter nine-pair analysis estimates ranged as high as 0.835× on
inverter_tran and as low as 0.986× on ring21. All warm wall ratios were
under 0.80× on both the nine-pair release pass and the earlier seven-pair pass.

## Where the time goes

On seven warm, interleaved single-worker profiles, the ring21 median for
reading and restoring the typed VA cache fell from 15.84 to 10.67 ms,
binding from 8.39 to 2.43 ms and JIT preparation from 5.24 to 3.21 ms.
Netlist preprocessing, flattening and parsing on the candidate took 0.40,
2.05 and 1.87 ms respectively. On the inverter DC deck those phases took
0.36, 1.85 and 0.18 ms. Its typed VA cache restore took 10.24 ms, down
from 13.97 ms. These are separately instrumented phase medians.

Nine interleaved device profiles on ring21 attributed 42.90 ms out of
53.15 ms of candidate analysis to native device evaluation. LU took 3.01 ms,
including a median 2.23 µs per factorization (HEAD 2.33 µs). Both builds
performed 38,556 native instance evaluations, reused 12,852 stamps and took
the same number of Newton iterations. On ring5, factorization fell from
0.70 to 0.65 µs. The LU schedule handles independent single-entry pivots
directly and skips empty lower columns in the forward solve. Explicit
unrolling of short column updates increased factor time on both rings.

The packed kernels read repeated literals from a single table indexed by bit
pattern and precompute derivative factors with constant divisors. Experiments
that reordered independent operations around math calls gave mixed or slower
results. A guarded two-region kernel trained near each supply rail completed
only 11,121 of 18,360 ring21 pair attempts. Its median analysis rose from
53.25 to 80.80 ms over 15 pairs. The general packed kernel completed 18,307.

A 15-run PC sample of the final packed kernels placed 94 and 64 of 345 native
samples in block 1 of the 4,116- and 3,182-instruction programs, respectively.
Delaying pure operations within 8–256-instruction windows gave ring21 medians
of 50.78–53.13 ms against 50.83 ms without reordering. A math-call ABI that
preserves more SIMD registers slowed both rings by about 3%. The next
experiment is to split the two hottest packed blocks at measured
peaks in live tangent count. Count loads and stores around those points and
measure device evaluation with the same paired biases. A second opportunity
is a compact typed-module cache representation to reduce the remaining
restore cost on the short decks.

## Reproduction

Install the local IHP test inputs following [the compiler guide](verilog-a.md#11-va-4b-production-circuits).
In a clean baseline checkout, build and save the executable. Run the remaining
commands in the candidate checkout, using the same Rust toolchain. Use `63b70bd` for round 6 and `c49c242` for round 5.

```sh
cargo build --offline --release -p sansho-cli --features jit
cp target/release/sansho /tmp/sansho-baseline
# In the candidate checkout
cargo build --offline --release -p sansho-cli --features jit
python3 benches/run.py --baseline /tmp/sansho-baseline --runs 9
python3 benches/run.py --warm-analysis --baseline /tmp/sansho-baseline --runs 41
taskset -c 2 python3 benches/run.py --warm-analysis --baseline /tmp/sansho-baseline --runs 31 --deck benches/psp_ring21.cir --deck verify/va-decks/psp_ring5.cir
python3 benches/run.py --warm-wall --baseline /tmp/sansho-baseline --runs 41
python3 benches/run.py --warm-wall --baseline /tmp/sansho-baseline --runs 41 --jobs default
python3 benches/run.py --devices --baseline /tmp/sansho-baseline --runs 9 --deck benches/psp_ring21.cir
python3 benches/run.py --profile --runs 5
python3 benches/compare.py /tmp/sansho-baseline target/release/sansho
python3 benches/blocks.py --runs 15 --spills benches/psp_ring21.cir
python3 benches/ordering.py
```

The comparison removes each deck's save projection and compares all returned
samples, scales, internal unknowns and complex components by their binary64
bits. It also compares cold and warm execution, one and default workers, and
the contents of cache artifacts produced under both worker settings.

## Round history

| Stage | Change and measured result |
|---|---|
| VA-5 | Binding-specialized register interpretation reduced ring5 / ring21 default wall from 4.410 / 22.604 s to 0.248 / 0.468 s. |
| VA-5b | Native scalar AD reduced single-worker ring5 / ring21 analysis from 267.4 / 1,251.1 ms to 66.6 / 241.9 ms on the same pass. |
| Round 1 | Persistent typed, specialized and native images reduced single-worker ring21 wall from 1,309 to 687 ms warm. Cold wall was 1,273 ms during that pass. |
| Round 2 | Circuit-aware tangent and stamp specialization, direct writes and exact stamp reuse sped up single-worker ring5 / ring21 analysis by 2.32× / 2.40× over the preceding build. |
| Round 3 | Pivot-reuse LU and SIMD across pairs of equal VA bindings sped up ring5 / ring21 analysis by 1.56× / 1.34×. |
| Round 4 | Guarded branch predictions, packed ring5 kernels and active-coefficient LU plans sped up ring5 / ring21 analysis by 1.36× / 1.30× on the seven-pair pass. On repeat passes ring21 estimated ratios ranged from 1.027× to 1.065×. The later idle-host calibration placed it at 60.2 / 57 ms. |
| Round 5 | Packed literals and constant derivative factors, independent LU pivots, shared binding work, hardware-accelerated cache checksums and smaller worker pools. See the round 5 table. |
| Round 6 | Project the constant supply voltage out of the packed transient Jacobian when its Newton increment is zero and it is eliminated last. |

The baseline ring5 / ring21 matrices have dimensions 37 / 149. On the first
Newton matrix they contain 108 / 444 active coefficients and 155 / 626 active
factor entries including fill. Alternative Markowitz orderings changed solution
bits in the first Newton update. Both rings take 306 accepted transient steps,
917 factorizations and 1,224 assemblies in the baseline profile.

## Internal noise equations

The specialized PSP register programs retain the internal voltage NOI, the gate
voltage GP and the branch **current** NOII for each device. The NOII equation
sets the current to zero when the noise drive vanishes. That current also enters
the NOI row and terminal flow stamps. The NOI row includes a conductance term
and a charge proportional to its voltage. The same voltage enters other terminal
charge expressions. Consequently neither noise unknown has an isolated Jacobian
row **and** column. Removing both would change the structural matrix used for
ordering, as well as the observable unknown list. Both stay in the solve.
A trial retained the matrix and specialized just the primals on exact, guarded
rail and zero inputs. Over 31 pinned pairs it moved ring21 from 51.81 to
51.69 ms and ring5 from 12.81 to 12.93 ms. The production kernel retains its
general input loads.


## BSIM-CMG 111.2.1 transients

Release build on an AMD Ryzen AI Max+ 395, one worker, warm disk cache.
Each number is the median of five runs after one warm-up. Analysis time
comes from `SANSHO_PROFILE=1` with device profiling disabled. Per-evaluation
cost comes from separate runs with `SANSHO_PROFILE_DEVICES=1`. It includes
native and interpreted evaluator time, divided by executed instance
evaluations. Exact stamp reuse is excluded from that denominator.

| Deck | JIT-enabled analysis | JIT-enabled µs/eval | Interpreter analysis | Interpreter µs/eval |
|---|---:|---:|---:|---:|
| `cmg_inverter_tran` | 208.3 ms | 4.659 | 344.9 ms | 7.484 |
| `cmg_ring5` | 227.7 ms | 0.764 | 417.4 ms | 6.918 |

With JIT enabled the inverter recorded 12,469 native and 18,085 interpreted
evaluations plus 5,662 exact reuses. The ring recorded 23,620 native
evaluations and 6,060 reuses. All five CMG decks agree between backends on
the binary64 values of every scale and output vector.

To reproduce, build `sansho-cli` with `--offline --release --features jit` and
run `target/release/sansho run -j1 verify/va-decks/cmg_ring5.cir` or the inverter
deck. Set `SANSHO_VA_JIT=0` for the interpreter. In the device profile,
`sansho-device` reports native, interpreted and reused counts followed by native
nanoseconds. `sansho-interpreted-ns` reports the interpreted nanoseconds per group.

## Large circuits

M5-1 uses five generated transient decks. The two multipliers share 256
partial products, a row-wise carry-save array and a final ripple adder.
The implementation uses 2,336 NAND2s, 32 NOR2s and 304 inverters at 1.8 V.
The digital check samples all 32 product bits just before the next operand
change and at the end of the run. Valid lows are −0.18 to 0.36 V and valid
highs are 1.44 to 1.98 V. Four unsigned operand pairs exercise low and high
bits and the final carries. MOS1 uses 2 ns per vector. BSIM4 needs 8 ns per
vector to settle the high-order carries with the generic verification cards.
The 2 ns BSIM4 pilot produced incorrect products at the sampling times.

The MOS1 ring scales the parallel-assembly test fixture to 1,001 stages. The
1,000-stage BSIM4 chain starts in its quiescent state and launches an input
pulse. The 300 × 300 grid includes 90,000 local capacitors, horizontal and
vertical resistors, a feed resistor and three pulsed loads.

### Reproduction

Generate the five large decks in a fresh checkout before running the benchmark or accuracy drivers.

```sh
python3 benches/large/gen.py
cargo build --offline --release -p sansho-cli
python3 benches/large/run.py --runs 3
python3 benches/large/run.py --check-bits
```

The driver launches a fresh release CLI for each repetition and reports
median milliseconds. It times from process launch to exit with
`SANSHO_PROFILE=1` and includes writing the projected ASCII raw file. Product
checks run outside that interval. The optional `--wall-binary` obtains wall
time from a second executable with profiling disabled. The phase columns are
independent medians. Setup includes file reading, preprocessing, flattening,
parsing and checking/elaboration. Device measures wall time across all
evaluation workers. Assembly includes gathering slots and constructing the
Newton matrix and RHS. Control covers transient integration coefficients,
prediction initialization, charge history and LTE. Output covers CLI raw
serialization and summary. Other costs include Newton convergence tests,
waveform retention, pool setup and process teardown. A pivot count denotes
a factorization with a fresh pivot search in at least one block. Refactor
counts assemblies where all blocks reused their pivot sequences. Identical
counts solves that reused the entire numeric factorization.

The baseline is revision `137dc4d`, built with only the aggregate timing
probes and the factor outcome exposed. The candidate uses the same release
profile and generic cards. The host is a Ryzen AI Max+ 395 with 16 physical
cores and 32 available logical CPUs. In the default columns the CLI received
no worker argument. HEAD used 32 workers. The candidate used two for the ring,
three for the chain, one for the grid, five for the MOS1 multiplier and twelve
for the BSIM4 multiplier. Independent long deck runs overlapped on this host,
so the wall times include CPU and memory contention.

An independent wall-clock check ran the untouched release executable from
`137dc4d` and the candidate with profiling disabled. These are individual
one-worker runs in seconds. Both multipliers passed all 128 product-bit
checks on each executable.

| Deck | Untouched HEAD | M5-1 |
|---|---:|---:|
| Ring MOS1 | 0.131 | 0.107 |
| Chain BSIM4 | 1.153 | 0.746 |
| RC grid | 464.61 | 4.62 |
| Multiplier MOS1 | 535.67 | 9.39 |
| Multiplier BSIM4 | 508.14 | 46.65 |

The generated Boolean gate network also agreed with unsigned multiplication
on 537 edge and pseudorandom operand pairs. Enabling approximate device bypass
for the cached BSIM4 and SKY130 suites gave 26 passes and the four declared
known differences.

### M5-1 measurements

Times are milliseconds. Each candidate row and each inverter baseline row is the median of three runs. The grid and multiplier baseline rows are individual full runs. Small differences on the short chain are within the variation observed on this shared host.

| Deck | Workers | Build | Wall | Setup and parse | Device | Assembly | Symbolic | Numeric | Solve | Control | Output |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Ring MOS1 | 1 | HEAD | 122.73 | 22.90 | 62.34 | 21.95 | 0.19 | 4.57 | 1.99 | 0.57 | 0.25 |
| Ring MOS1 | 1 | M5-1 | 117.59 | 21.25 | 68.45 | 9.27 | 0.26 | 4.99 | 2.04 | 0.47 | 0.22 |
| Ring MOS1 | default | HEAD | 257.85 | 29.22 | 180.96 | 34.97 | 0.18 | 5.75 | 2.18 | 1.21 | 0.30 |
| Ring MOS1 | default | M5-1 | 152.66 | 19.93 | 75.44 | 28.47 | 0.23 | 4.74 | 2.00 | 0.47 | 0.20 |
| Chain BSIM4 | 1 | HEAD | 1,358.46 | 249.29 | 891.76 | 152.45 | 0.19 | 7.08 | 3.29 | 12.61 | 0.18 |
| Chain BSIM4 | 1 | M5-1 | 741.69 | 249.83 | 424.60 | 41.79 | 0.17 | 4.70 | 2.61 | 10.89 | 0.19 |
| Chain BSIM4 | default | HEAD | 1,064.33 | 238.65 | 667.37 | 118.93 | 0.16 | 6.51 | 3.11 | 13.41 | 0.26 |
| Chain BSIM4 | default | M5-1 | 1,069.53 | 281.34 | 616.19 | 119.29 | 0.20 | 7.50 | 4.66 | 12.77 | 0.18 |
| RC grid | 1 | HEAD | 479,758.61 | 469,685.24 | 139.05 | 244.59 | 112.54 | 8,416.87 | 872.20 | 40.64 | 0.21 |
| RC grid | 1 | M5-1 | 4,710.13 | 684.23 | 156.82 | 181.63 | 46.96 | 2,239.67 | 1,026.17 | 54.28 | 0.34 |
| RC grid | default | HEAD | 1,340,635.91 | 447,830.16 | 159.03 | 379.03 | 81.12 | 890,213.61 | 1,625.00 | 52.49 | 0.31 |
| RC grid | default | M5-1 | 4,781.72 | 765.69 | 159.00 | 172.59 | 58.13 | 2,201.06 | 1,009.04 | 55.52 | 0.58 |
| Multiplier MOS1 | 1 | HEAD | 643,436.07 | 386.07 | 3,845.27 | 4,326.41 | 20.26 | 616,172.99 | 18,315.49 | 20.15 | 1.11 |
| Multiplier MOS1 | 1 | M5-1 | 9,169.21 | 80.00 | 2,889.16 | 1,831.55 | 5.07 | 3,858.89 | 403.89 | 17.54 | 2.59 |
| Multiplier MOS1 | default | HEAD | 5,017,460.00 | 303.35 | 3,885.22 | 3,995.45 | 20.12 | 4,979,916.95 | 27,053.67 | 26.36 | 1.92 |
| Multiplier MOS1 | default | M5-1 | 14,753.89 | 92.84 | 5,221.22 | 4,151.00 | 10.03 | 4,637.81 | 502.22 | 21.03 | 7.75 |
| Multiplier BSIM4 | 1 | HEAD | 544,171.47 | 1,554.16 | 43,750.24 | 11,704.24 | 14.05 | 471,772.97 | 15,002.21 | 280.69 | 1.40 |
| Multiplier BSIM4 | 1 | M5-1 | 49,240.17 | 1,269.04 | 38,801.64 | 4,265.60 | 5.37 | 3,944.31 | 493.65 | 306.99 | 3.15 |
| Multiplier BSIM4 | default | HEAD | 4,585,350.00 | 1,511.77 | 10,354.48 | 7,958.15 | 11.88 | 4,537,470.17 | 27,678.54 | 273.99 | 1.53 |
| Multiplier BSIM4 | default | M5-1 | 46,392.89 | 1,310.23 | 29,449.35 | 10,381.15 | 8.07 | 4,275.91 | 575.75 | 295.27 | 2.96 |

Counts are for the complete transient, including initialization. Points include the initial sample. Both multipliers passed all product checks on HEAD. The historical worker comparison flagged different multiplier Newton counts and different grid waveforms. The table includes timings from those completed, profiled CLI processes. The candidate gave identical counts and full-precision waveforms across worker settings.

| Deck | Build | Workers | Newton | Points | Rejected | Pivot search | Fixed pivots | Identical Jacobian |
|---|---|---|---:|---:|---:|---:|---:|---:|
| Ring MOS1 | HEAD | both | 206 | 66 | 0 | 2 | 204 | 0 |
| Ring MOS1 | M5-1 | both | 206 | 66 | 0 | 2 | 204 | 0 |
| Chain BSIM4 | HEAD | both | 290 | 70 | 0 | 1 | 289 | 0 |
| Chain BSIM4 | M5-1 | both | 290 | 70 | 0 | 1 | 289 | 0 |
| RC grid | HEAD | both | 63 | 63 | 0 | 63 | 0 | 0 |
| RC grid | M5-1 | both | 63 | 63 | 0 | 20 | 0 | 43 |
| Multiplier MOS1 | HEAD | 1 | 2724 | 206 | 27 | 2724 | 0 | 0 |
| Multiplier MOS1 | HEAD | default | 2680 | 206 | 27 | 2680 | 0 | 0 |
| Multiplier MOS1 | M5-1 | both | 2664 | 206 | 27 | 1428 | 1236 | 0 |
| Multiplier BSIM4 | HEAD | 1 | 2344 | 222 | 33 | 2344 | 0 | 0 |
| Multiplier BSIM4 | HEAD | default | 2346 | 222 | 33 | 2346 | 0 | 0 |
| Multiplier BSIM4 | M5-1 | both | 2373 | 222 | 32 | 1023 | 1350 | 0 |

### Profile-guided changes

The grid spent 469.7 seconds in setup and parsing at one worker on HEAD.
Element duplicate detection scanned every previous element. A name set
preserves duplicate diagnostics with expected linear total lookup time.
Elaboration now checks repeated terminals of the current device in constant
time even on a high-fanout rail. The final grid makes 63 solves with 20
factorizations. Exactly repeating Jacobians account for the other 43.
An intermediate build that retained general faer's pool-dependent frontal
kernels produced different grid waveforms across worker settings. Explicitly
sequential kernels and reusable scratch give identical full-precision results.

At one worker the MOS1 multiplier spent 616.2 seconds in numeric LU on
HEAD. BTF separates the rail and input constraints before ordering the main
block. The first BTF trial took 7.18 seconds in numeric factorization. Retaining
a valid pivot prefix during a partial refactor brought that phase to 4.35 seconds.
A source-ordered serial gather reduced the assembly phase from 4.06 to 2.38
seconds in consecutive experiments. In a sweep on this host, four, eight,
sixteen and thirty-two unrestricted workers took 13.41, 14.98, 17.27 and
21.07 seconds. The engine now budgets workers using device workload and can
gather conductance and charge in parallel.

The BSIM4 chain's main cost was intrinsic device evaluation. An eight-nanosecond
hold in the multiplier exposed the same cost over a larger switching wavefront.
An optional one-worker trial with `.options bypass=1` completed the multiplier
in 34.38 seconds, including 24.56 seconds in device evaluation, against 47.69
and 37.47 seconds respectively in the preceding exact-reuse build. All 128
bit checks passed in both runs. The approximate option changed the Newton
work count and remains off by default.

All five candidates produced identical round-trip JSON across one and default
workers with the save projections removed. The cached verifier reported 14
ordinary passes, 15 VA passes, 19 BSIM4 passes plus three known differences,
and seven SKY130 passes plus one known difference. Ngspice 44.2 was unavailable
as an executable in the offline build environment, and its Docker socket denied
access. Its runtime and fresh compatibility checks remain for the maintainer.

The next single-worker targets are exact BSIM4 evaluation and the remaining
pivot searches in the 5,034-column multiplier block. Splitting work across the 67 small companion
blocks leaves almost all factorization in the large block.

A diagnostic build measured the first successful fixed-pivot update of the
large block during startup. Both models had 526 dependency levels, including
253 singleton levels, and a maximum width of 707 columns. Counting one unit
per column, input entry, stored lower or upper entry and elimination update
gave the MOS1 block 956,168 work units with 592,618 on its critical path.
The BSIM4 counts were 968,314 and 603,458. For this cost model, even unlimited
workers bound ideal speedup to about 1.6× on these two startup factorizations.
Sampling a reusable pivot sequence every 500 factor calls over each complete
transient gave 521–529 levels across 11 samples. The ideal column-parallel
speedup ranged from 1.57× to 1.62× before any synchronization cost. These
measurements favor reducing repivot and BSIM4 evaluation costs before L3.
The BSIM4 assembly cost also rose from 4.27 s at one worker to 10.38 s at
the default, offsetting most of the device evaluation savings.

### M5-2 transient accuracy

The gold runs use ngspice 44.2, `reltol=1e-4`, `vntol=1e-7`, `chgtol=1e-16`
and a 2 ps maximum step. For each raw file, linearly interpolate all 32 product
voltages at 4,000 uniformly spaced times including both endpoints. Average
the absolute differences over all 128,000 bit/time pairs. Count transitions
between the two sides of 0.9 V on each waveform's original time grid, taking
equality as high. Sum the absolute count difference for each bit. This counts
a missed pulse on one bit even if another bit has an extra pulse.

```sh
cargo build --offline --release --features jit -p sansho-cli -p sansho-verify
target/release/sansho run benches/large/mult16_mos1.cir -r /tmp/mos1.raw
python3 benches/large/accuracy.py mult16_mos1 /tmp/mos1.raw
target/release/sansho run benches/large/mult16_bsim4.cir -r /tmp/bsim4.raw
python3 benches/large/accuracy.py mult16_bsim4 /tmp/bsim4.raw
python3 benches/large/run.py --runs 1
python3 benches/large/run.py --check-bits
```

Generate a tighter candidate from a temporary copy of either deck. For example,

```sh
sed 's/^\.options /.options reltol=1e-4 /' benches/large/mult16_mos1.cir > /tmp/mos1-tight.cir
target/release/sansho run /tmp/mos1-tight.cir -r /tmp/mos1-tight.raw
python3 benches/large/accuracy.py mult16_mos1 /tmp/mos1.raw /tmp/mos1-tight.raw
```

The comparison reads gold and ngspice-default data from `verify/out/large`.
It accepts additional candidate paths or uses the cached Sansho raw when invoked
with just the deck name. The `reltol=1e-4` Sansho runs change only that option.

| Deck | Run | Points | Mean absolute error (mV) | Crossings | Crossing difference |
|---|---|---:|---:|---:|---:|
| MOS1 | ngspice gold | 9,301 | 0 | 502 | 0 |
| MOS1 | ngspice default | 657 | 15.628 | 490 | 36 |
| MOS1 | Sansho at `6ef1df7` | 206 | 63.113 | 278 | 224 |
| MOS1 | Sansho M5-2 default | 1,560 | 4.338 | 486 | 24 |
| MOS1 | Sansho M5-2 `reltol=1e-4` | 3,682 | 0.772 | 502 | 4 |
| BSIM4 | ngspice gold | 21,414 | 0 | 556 | 0 |
| BSIM4 | ngspice default | 934 | 36.771 | 554 | 30 |
| BSIM4 | Sansho at `6ef1df7` | 222 | 95.089 | 302 | 254 |
| BSIM4 | Sansho M5-2 default | 2,750 | 4.834 | 558 | 6 |
| BSIM4 | Sansho M5-2 `reltol=1e-4` | 7,624 | 0.191 | 556 | 0 |

At the old defaults the charge-error allowance was at least 70 fC. A full
1.8 V transition on a 3 fF load moves only 5.4 fC. The controller now applies
`reltol` to the charge scale including its `chgtol` floor. The corresponding
minimum allowance is 0.07 fC. It also uses the variable-step BDF2 error
coefficient and monitors individual capacitors and MOS1 branch charges before
they can cancel in the nodal sum. The default step ceiling on both decks was
already the documented value. [Transient integration](architecture.md#53-transient-analysis)

### M5-2 wall time and profile

One release CLI process per cell on a Ryzen AI Max+ 395 with 32 logical CPUs,
using the built-in device models, the JIT-enabled build and the original save
projections. The runs include profiling and ASCII raw output. Large decks ran
one at a time for this table. Times are seconds. The maintainer measured the
ngspice 44.2 comparison times separately.

| Deck | Sansho one worker | Sansho default | Default workers | ngspice default |
|---|---:|---:|---:|---:|
| ring1001_mos1 | 0.098 | 0.092 | 2 | OP failed |
| inv_chain1000_bsim4 | 0.600 | 0.349 | 3 | 0.33 |
| rc_grid_300 | 3.589 | 3.515 | 1 | exceeded 1,200 |
| mult16_mos1 | 14.014 | 8.307 | 5 | 82 |
| mult16_bsim4 | 127.649 | 37.933 | 12 | 51 |

With the corrected tolerance and variable-step coefficient, an initial BSIM4
multiplier run took 196.81 s on one worker. Device evaluation accounted for
158.86 s across 2,746 points. The final run used 95.47 s for evaluation
across 2,750 points. Model binding now precomputes bias-independent channel
and junction factors. The overlap stamp stores charge and capacitance.
A device chunk can reuse intrinsic evaluations at exactly equal biases for
instances with a shared parameter binding. The default still evaluates
changed biases exactly.

The chain's initial profile measured 196.74 ms in elaboration and 4.95 ms
in reading, preprocessing, flattening and parsing. Its transistors name two
static model cards directly and use fixed instance dimensions. The generic
bin search and model-expression evaluator have no instance-specific model
expressions to resolve on this deck. Caching the validated model and network
by selected card and ordered parameter values reduced total setup and parse
time to 14.00 ms on one worker and 10.12 ms at the default.

Native assembly streams the per-group slot maps in source order. Conductance
and capacitance use independent tasks, and dense zero overlap-current stamps
have been removed. Multiplier assembly took 2.32 s on one worker and 1.20 s
on five workers for MOS1, and 13.75 s on one worker and 7.46 s on twelve for BSIM4.
At the BSIM4 default, device evaluation still took 13.27 s and numeric
factorization took 12.59 s. On one worker BSIM4 remains slower than the
maintainer's ngspice default. The Sansho run takes nearly three times as many
points and resolves more of the gold waveform's glitches.

The release JIT-enabled cached verifier preserved all four suites' verdicts.
The ordinary and Verilog-A suites had 14 and 15 passes. BSIM4 had 19 passes
and three known differences, and SKY130 had seven passes and one known
difference. All 128 settled product checks passed on each multiplier at both
worker settings.
With save projections removed, every time point and internal waveform in
the full-precision JSON matched across one and default workers on all five decks.

### M5-3 single-worker BSIM4

On the BSIM4 multiplier the single-worker release run took 41.87 s, including
setup and raw output. With 2,750 points the cost fell from 46.93 to 15.22 ms
per point. The maintainer's ngspice 44.2 reference took 51 s and produced
934 points, with 36.771 mV mean error and a crossing difference of 30.
A separate run with all profiling disabled took 41.95 s and produced the
same projected waveform and work counts as the 41.87 s aggregate profile.

The following runs used the same 32-logical-CPU Ryzen AI Max+ 395, the
release JIT-enabled CLI and the original save projections. Large simulations ran sequentially. Each multiplier cell is one process, and
each shorter-deck cell is the median of three processes. Wall includes startup
and ASCII output. The denominator for wall per point includes the initial
point. Newton iterations per accepted step below exclude that point from the
denominator and include work on rejected attempts in the numerator.

| Deck | Revision | Points | Mean error (mV) | Crossing difference | Wall -j1 (s) | Wall default (s) | Default workers | -j1 ms/point |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| BSIM4 multiplier | 2366370 | 2,750 | 4.834 | 6 | 129.06 | 37.52 | 12 | 46.931 |
| BSIM4 multiplier | M5-3 | 2,750 | 4.834 | 6 | 41.87 | 14.79 | 12 | 15.224 |
| BSIM4 chain | 2366370 | 221 | 6.5e-08 | 0 | 0.581 | 0.297 | 3 | 2.629 |
| BSIM4 chain | M5-3 | 221 | 6.5e-08 | 0 | 0.152 | 0.133 | 3 | 0.690 |
| SKY ring | 2366370 | 1,508 | 0.673 | 0 | 0.232 | 0.238 | 1 | 0.154 |
| SKY ring | M5-3 | 1,508 | 0.673 | 0 | 0.235 | 0.249 | 1 | 0.156 |
| MOS1 multiplier | 2366370 | 1,560 | 4.338 | 24 | 14.19 | 8.31 | 5 | 9.097 |
| MOS1 multiplier | M5-3 | 1,560 | 4.338 | 24 | 10.14 | 5.10 | 5 | 6.503 |

Multiplier errors use the M5-2 tight gold files and the original 4,000-sample
comparison. For the short decks the reference is the cached ngspice-default
waveform. The same interpolation and 0.9 V crossing rule use the reference
time interval, chain nodes n0, n500 and n1000, and ring nodes a through e.
The cached verifier separately compares the SKY internal nodes.

**Phase times in seconds.** Setup includes parsing and elaboration. Control
covers the integration and step controller. The process wall also includes
unattributed work. Each phase median for the short decks is independent.

| Deck | Revision | Workers | Setup | Device | Assembly | Symbolic | Numeric LU | Solve | Control | Output |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| BSIM4 multiplier | 2366370 | 1 | 0.0704 | 96.5867 | 13.8507 | 0.0066 | 13.5586 | 1.8682 | 2.7146 | 0.0153 |
| BSIM4 multiplier | 2366370 | 12 | 0.0574 | 12.7142 | 7.3174 | 0.0046 | 12.7245 | 1.5750 | 2.7048 | 0.0184 |
| BSIM4 multiplier | M5-3 | 1 | 0.0591 | 29.5385 | 2.3864 | 0.0122 | 5.7371 | 0.8379 | 2.9699 | 0.0148 |
| BSIM4 multiplier | M5-3 | 12 | 0.0519 | 3.5455 | 1.5640 | 0.0134 | 5.4133 | 0.8422 | 3.0106 | 0.0195 |
| BSIM4 chain | 2366370 | 1 | 0.0129 | 0.4375 | 0.0748 | 0.0002 | 0.0102 | 0.0062 | 0.0304 | 0.0005 |
| BSIM4 chain | 2366370 | 3 | 0.0122 | 0.1767 | 0.0516 | 0.0002 | 0.0102 | 0.0062 | 0.0319 | 0.0007 |
| BSIM4 chain | M5-3 | 1 | 0.0105 | 0.0608 | 0.0230 | 0.0001 | 0.0082 | 0.0050 | 0.0355 | 0.0006 |
| BSIM4 chain | M5-3 | 3 | 0.0096 | 0.0397 | 0.0225 | 0.0001 | 0.0083 | 0.0050 | 0.0380 | 0.0006 |
| SKY ring | 2366370 | 1 | 0.1287 | 0.0395 | 0.0027 | 0.0000 | 0.0024 | 0.0012 | 0.0015 | 0.0092 |
| SKY ring | M5-3 | 1 | 0.1301 | 0.0339 | 0.0021 | 0.0000 | 0.0020 | 0.0009 | 0.0018 | 0.0095 |
| MOS1 multiplier | 2366370 | 1 | 0.0509 | 6.0158 | 2.3859 | 0.0046 | 4.7684 | 0.5992 | 0.1540 | 0.0128 |
| MOS1 multiplier | 2366370 | 5 | 0.0536 | 1.5031 | 1.2065 | 0.0068 | 4.5674 | 0.5946 | 0.1615 | 0.0133 |
| MOS1 multiplier | M5-3 | 1 | 0.0511 | 5.8165 | 1.3553 | 0.0122 | 2.0030 | 0.5624 | 0.1491 | 0.0130 |
| MOS1 multiplier | M5-3 | 5 | 0.0511 | 1.3811 | 0.8871 | 0.0129 | 1.8783 | 0.5395 | 0.1571 | 0.0153 |

**Work counts and LU cost on one worker.** Fresh includes the first
factorization and factorizations requiring pivot search. Reuse includes
successful fixed-pivot numeric refactors. The nanosecond counters accumulate
the entire factor call, including a failed fixed-pivot attempt when a fresh
search follows. The table divides these totals by calls of that type.
Assembly counts one gathered contribution to an MNA entry. The baseline
counters come from a separately instrumented build of 2366370.

| Deck | Revision | Rejected | Newton iterations | Newton/accepted | Fresh LU | Fixed LU | µs/fresh | µs/fixed | Gathered contributions | ns/contribution |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| BSIM4 multiplier | 2366370 | 527 | 12,860 | 4.678 | 3,047 | 9,813 | 1756.9 | 892.1 | 8,013,353,068 | 1.813 |
| BSIM4 multiplier | M5-3 | 527 | 10,257 | 3.731 | 2,122 | 8,135 | 1079.2 | 423.7 | 3,241,436,741 | 0.736 |
| BSIM4 chain | 2366370 | 19 | 793 | 3.605 | 1 | 792 | 151.3 | 13.0 | 89,971,930 | 0.838 |
| BSIM4 chain | M5-3 | 19 | 659 | 2.995 | 1 | 658 | 177.4 | 12.2 | 37,272,590 | 0.618 |
| SKY ring | 2366370 | 0 | 4,540 | 3.013 | 3 | 4,537 | 16.9 | 0.6 | 4,355,740 | 0.718 |
| SKY ring | M5-3 | 0 | 3,051 | 2.025 | 3 | 3,048 | 16.1 | 0.8 | 2,978,415 | 0.854 |

**Device evaluation on one worker.** 2366370 visits include the converged
charge sample. The combined terminal stamp has a separate exact scalar charge
path for that sample. Devices with internal resistances, including those in
the SKY ring, retain the general evaluation path. A shared hit takes an exact
model-and-bias match. Bypass hits below count changed biases reused within
the configured voltage tolerance. The remaining hits used an unchanged bias
at the same instance.

| Deck | Revision | Newton/general visits | Full intrinsic evaluations | Shared hits | Bypass hits | Charge-only visits | Scalar evaluations |
|---|---|---:|---:|---:|---:|---:|---:|
| BSIM4 multiplier | 2366370 | 162,660,960 | 127,491,240 | — | — | — | — |
| BSIM4 multiplier | M5-3 | 103,390,560 | 33,838,783 | 473,930 | 69,077,735 | 33,032,160 | 32,310,902 |
| BSIM4 multiplier | M5-3 bypass=0 | 103,410,720 | 99,946,284 | 2,285,593 | 0 | 33,032,160 | 32,274,856 |
| BSIM4 chain | 2366370 | 2,066,000 | 364,603 | — | — | — | — |
| BSIM4 chain | M5-3 | 1,318,000 | 23,168 | 1,872 | 1,292,960 | 480,000 | 156,199 |
| SKY ring | 2366370 | 60,480 | 60,480 | — | — | — | — |
| SKY ring | M5-3 | 45,590 | 45,562 | 0 | 18 | 0 | 0 |

The detailed probe samples every 1,024th full evaluation in each group.
The final intrinsic column times all four terminal currents and charges,
including enabled gate terms. AD overhead is the difference from replaying
the same intrinsic equations with zero derivative lanes. Overlap/junction
and gate columns are separate probes, so the columns do not sum to device
wall time. Charge-only measures the actual scalar charge calculation on
sampled cache misses. Zero gate cost denotes disabled tunneling. The 2366370 auxiliary probe timed the original branch evaluation.
A dash marks a measurement unavailable in that build or a path unused by the deck.

| Deck | Revision | Intrinsic ns | AD overhead ns | Overlap/junction ns | Enabled gate ns | Charge-only ns |
|---|---|---:|---:|---:|---:|---:|
| BSIM4 multiplier | 2366370 | 558.7 | — | 180.4 | — | — |
| BSIM4 multiplier | M5-3 | 356.9 | 145.8 | 82.0 | 0.0 | 149.0 |
| BSIM4 chain | 2366370 | 587.3 | — | 115.6 | — | — |
| BSIM4 chain | M5-3 | 301.6 | 111.3 | 61.6 | 0.0 | 138.2 |
| SKY ring | 2366370 | 711.0 | — | 223.7 | — | — |
| SKY ring | M5-3 | 584.9 | 268.0 | 141.1 | 0.0 | — |

The matched probes measured 80.83 s with exact-bias reuse and
42.35 s with default bypass. Exact-bias reuse produced
4.834 mV mean error and a crossing difference of 6. The default avoids
66,107,501 full evaluations in these runs. All four
terminals must stay within the existing `vntol` of the original linearization.
The converged LTE charge sample uses the actual voltage. Solver and charge
tolerances are the M5-2 defaults.

**Measured changes.** To compare intermediate builds, the BSIM4 multiplier
was also run to 8.15 ns with the original 320 ps maximum step, one worker
and 70 output points. The following builds all rejected 11 steps. Each row
is a single timing, so small differences include timing variation.

| Change included | Wall (s) | Device (ms) | Assembly (ms) | Numeric LU (ms) | Newton |
|---|---:|---:|---:|---:|---:|
| 2366370 | 2.77 | 1982 | 287 | 301 | 315 |
| Voltage-bounded bypass | 2.28 | 1537 | 268 | 285 | 312 |
| Linear accepted-voltage predictor | 2.24 | 1531 | 250 | 267 | 285 |
| Minimum-degree ordering of the large block | 2.14 | 1510 | 238 | 210 | 285 |
| Combined stamp, three AD lanes and scalar charge sampling | 1.61 | 1091 | 157 | 185 | 279 |
| Quadratic accepted-voltage predictor | 1.55 | 1043 | 144 | 178 | 276 |
| Detailed probes implemented, inactive | 1.69 | 1182 | 152 | 180 | 276 |
| Prepared zero-body electrostatics | 1.59 | 1082 | 148 | 185 | 276 |
| Two AD lanes for wired source and body | 1.58 | 1076 | 141 | 184 | 276 |
| Electrostatics-only charge path, with experimental shared bypass | 1.44 | 938 | 139 | 180 | 275 |
| Mobility-0 specialization, shared powers and compact instance data | 1.39 | 908 | 130 | 173 | 275 |

Both multiplier cards select mobmod=0, capmod=2, igcmod=igbmod=0 and
rbodymod=0. Preparing bias-independent quantities and sharing repeated
powers reduces the cost of the IV evaluation. Compact batches store terminal
indices, model bindings and off flags in separate arrays. The MNA receives
one 4×4 stamp per device, and the LTE test retains all 12 component charges.

On the complete multiplier an ordering-only comparison kept 10,455 Newton
iterations in both builds. Numeric LU fell from 12.19 to 7.60 s and solve
from 1.70 to 0.93 s. Fresh factorizations fell from 2,555 to 2,479. A later
build using two 16-bit indices for each representable gather entry reduced
assembly from 6.47 to 3.11 s. Contiguous derivative copies and sparse
zero-update skips brought the next complete run to 42.25 s, including
2.38 s of assembly.

The 5,034-column block had 206,151 factor entries and 4,042 occupied positions
in its trailing 64×64 square. A dense trailing kernel increased median numeric
time on the prefix from 171.0 to 181.0 ms across three interleaved runs. A
previous-LU predictor correction required 2,987 additional solves to avoid
1,151 Newton factorizations. Approximate sharing between instances saved
about 1% of full evaluations. All three experiments were removed. Exact
cross-instance sharing and the existing pivot stability threshold are retained.

A worker sweep on an intermediate multiplier build took 19.11, 18.64, 15.89
and 14.91 s at 4, 6, 8 and 12 workers. The chain took 0.16, 0.12 and 0.12 s
at 1, 2 and 3 workers. The existing default selects 12 for the multiplier,
3 for the chain and 1 for the ring. Parsing and elaborating the SKY model
library still take about 0.13 s per process.

Run the release binary as follows to reproduce the counters and inspect the matrix shape.

```sh
SANSHO_PROFILE=1 SANSHO_PROFILE_BSIM4=1 target/release/sansho run -j1 benches/large/mult16_bsim4.cir -r /tmp/mult16.raw
SANSHO_PROFILE=1 SANSHO_PROFILE_MATRIX=shape target/release/sansho run -j1 benches/large/mult16_bsim4.cir -r /tmp/mult16-shape.raw
python3 benches/large/accuracy.py mult16_bsim4 /tmp/mult16.raw
python3 benches/large/run.py --check-bits --deck mult16_mos1 --deck mult16_bsim4
```

The cached release JIT verifier reported 55 passing decks, four known
differences and zero skipped. The SKY ring retained its known internal body
node difference (10.57 µV maximum).
Both multipliers passed all 128 settled product checks at each worker setting.
With the save projections removed, the SHA-256 digests of every JSON waveform
and time point matched at one and default workers on both multipliers and
the BSIM4 chain.

| Full-precision comparison | Matching SHA-256 |
|---|---|
| mult16_bsim4, 1 and 12 workers | `130ab727c5e26c4a7156b4bd9e5ccc0f7db4a3fd61b4abd06f4c9012bcd29a7f` |
| mult16_mos1, 1 and 5 workers | `39e09dabb59609e26c98704a96e01987e11f25b581dfda889eb6fb96621ab935` |
| inv_chain1000_bsim4, 1 and 3 workers | `f5739483db7c3f68cb55c9fc4c7031745f92fd5d958c3c0678d9b820fb8274eb` |

### M5-4 column-parallel numeric refactor

The BSIM4 multiplier missed the 9 s target. With profiling disabled,
three interleaved default-worker runs per build gave a median of **23.96 s
at a37d005 and 20.28 s at M5-4**, a 15.4% wall reduction. The ranges
were 23.54–25.21 and 19.12–20.54 s. The full-precision waveform matches
M5-3, and every deck met the single-worker guard in the profiled comparison.

Both multiplier decks still have 68 BTF blocks. The 5,034-column block sets
the cost. In the fixed-pivot factorization, an upper entry at row `i` of
column `j` reads the completed lower column `i`. This gives a DAG directly
from the stored factors. Disjoint components of its prefix can run on the
existing pool with private scatter vectors. After the join, the remaining
columns run in order. A failed pivot preserves every valid earlier column
for the serial search. The triangular solves retain their original ordering.

The M5-1 estimate refers to an earlier column ordering. For the M5-3
minimum-degree ordering, counting columns, inputs, L and U entries and
scalar updates gives 7,551,526 work units and a 7,024,156-unit critical path
for the startup factorization. A transient sample after 350 attempts had
59,861 factor entries, 212 levels, 446,136 units of work and an ideal speedup
of 1.71. These are structural estimates. They exclude synchronization,
zero-update skips and fresh pivoting.

**Ordering and pivot experiments.** A recursive breadth-first separator
trial on the block graph kept minimum-degree order within leaves of fewer
than 128 columns. The startup factor grew from 206,151 to 493,498 entries.
The estimated ideal speedup rose from 1.08 to 1.19. On a 9 ns transient
with the original 320 ps step limit, numeric time rose from 1.19 to 14.03 s
and the waveform changed. Reducing the fixed-pivot acceptance threshold
from 0.1 to 0.01 cut fresh searches from 274 to 87 and numeric time to
0.91 s on this interval. It changed the waveform and took 952 Newton
iterations against the original 951. The shipped ordering and threshold
preserve the M5-3 waveform.

**Incremental measurements.** Each pair below compares the affected phase
on the same 9 ns deck. Times are median milliseconds across three interleaved
processes per executable. Each row is its own experiment. Host load varied
between and within experiments, especially in the gather trial.

| Change | Phase | Before (ms) | After (ms) |
|---|---|---:|---:|
| Copy contiguous charge slots and parallelize the maximum LTE reduction | Control | 687.5 | 262.5 |
| Schedule independent fixed-pivot subtrees | Numeric | 1,285.4 | 1,253.1 |
| Replay verified reach during fresh pivot search | Numeric | 1,039.7 | 956.0 |
| Gather native stamps into disjoint destination tiles | Assembly | 510.2 | 307.8 |
| Rotate accepted charge buffers and compute local derivatives with LTE | Control | 198.0 | 119.6 |

The last control comparison used a 16-worker trial budget. A backward solve
trial processed the serial separator first and then solved independent upper
subtrees in reverse column order. Copying the pivot RHS into per-task scratch
and gathering solutions took the solve phase from 138 to 139 ms. A four-column
packed separator increased numeric time from 474 to 561 ms. A contiguous
lower-factor run kernel took 531 versus 506 ms, and a 96-column dense frontal
trial took 796 versus 791 ms. The fixed-pivot kernel keeps the original
scalar update for each column. Testing for unchanged individual columns gave
1,015 versus 1,021 ms with extra bookkeeping. These trials were discarded.

The MOS1 batch caches voltage-independent geometry and reuses the junction
limiting voltage for devices with the same peak saturation current. The
converged-point sampler evaluates terminal charges and updates Meyer histories
without writing current or Jacobian stamps. In two interleaved full-deck
single-worker comparisons, the MOS1 multiplier took 12.02 and 12.74 s with
these changes, against 15.31 and 14.43 s at HEAD. Unit coverage compares
every charge and history slot across both device polarities, reversed drain
bias and series resistance.

**Complete decks.** The comparison below uses two release CLIs on the same
32-hardware-thread host. Each process ran on its own, without affinity.
For each deck and worker setting the M5-3 and M5-4 builds alternated order.
Times are medians of three runs per build for the multipliers and generated
RC grid and seven for the shorter decks. The host was shared. Wall includes
CLI startup and raw output, and the phase timers were enabled in both builds.
The 3% guard uses the single-worker wall medians.

| Deck | Default workers | M5-3 -j1 (s) | M5-4 -j1 (s) | -j1 change | M5-3 default (s) | M5-4 default (s) |
|---|---:|---:|---:|---:|---:|---:|
| mult16_bsim4 | 12 | 56.87 | 53.90 | −5.2% | 25.85 | 19.11 |
| mult16_mos1 | 5 | 14.11 | 12.31 | −12.8% | 8.42 | 7.54 |
| rc_grid_300 | 1 | 4.87 | 4.10 | −15.9% | 4.32 | 4.67 |
| inv_chain1000_bsim4 | 3 | 0.1960 | 0.1909 | −2.6% | 0.1988 | 0.1463 |
| ring1001_mos1 | 2 | 0.1135 | 0.1078 | −5.0% | 0.1338 | 0.0911 |
| sky_ring5 | 1 | 0.3022 | 0.2810 | −7.0% | 0.2963 | 0.2803 |
| ring_osc | 1 | 0.0054 | 0.0053 | −1.8% | 0.0052 | 0.0047 |

All seven decks met the single-worker guard in this comparison. The RC grid
used one worker in both modes, and its two series moved in opposite
directions. The grid retains the general sparse backend. The earlier M5-3
reference measured 41.87 s at one worker and 14.79 s at twelve on the BSIM4
multiplier. The same-session baseline above is the denominator for speedups.

**BSIM4 worker sweep.** Times below are seconds. The one- and twelve-worker
rows are medians from the paired experiment. Other rows are single
processes. Phase medians are calculated independently. Wall also includes
engine work outside the phase timers. The production CLI treats `-j` as
a ceiling. To measure sixteen actual workers, the timing build raised only
the native single-analysis budget floor to sixteen and ran with `-j16`.
The production worker heuristic and affinity policy are unchanged.

| Build | Actual workers | Wall | Setup + parse | Device | Assembly | Symbolic | Numeric LU | Solve | Step control | Output |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| M5-3 | 1 | 56.871 | 0.074 | 39.191 | 3.546 | 0.016 | 8.132 | 1.243 | 4.249 | 0.031 |
| M5-4 | 1 | 53.902 | 0.061 | 38.447 | 3.318 | 0.019 | 7.162 | 1.218 | 3.212 | 0.040 |
| M5-4 | 4 | 23.064 | 0.056 | 11.770 | 2.213 | 0.016 | 5.800 | 1.309 | 1.445 | 0.033 |
| M5-4 | 8 | 19.007 | 0.091 | 7.315 | 2.096 | 0.024 | 6.469 | 1.369 | 1.161 | 0.029 |
| M5-3 | 12 | 25.848 | 0.077 | 6.799 | 3.169 | 0.017 | 8.701 | 1.386 | 5.256 | 0.036 |
| M5-4 | 12 | 19.106 | 0.090 | 6.426 | 2.460 | 0.023 | 6.888 | 1.457 | 1.212 | 0.037 |
| M5-4 | 16 | 20.248 | 0.051 | 6.667 | 2.484 | 0.015 | 7.476 | 1.532 | 1.364 | 0.078 |

The complete BSIM4 transient required 10,257 Newton iterations and rejected
527 steps. Of its 10,257 LU calls, 2,122 (20.7%) searched for a new pivot
and 8,135 completed with fixed pivots. At twelve workers the respective
times were 2.79 and 4.10 s. The replay cache reduces the cost of a search
while preserving the original pivot decisions. Eight and twelve workers
were within 0.1 s in this sweep. Sixteen increased wall by 1.14 s relative
to twelve, and a lighter-load trial favored twelve over eight. These runs
did not justify raising the default worker budget.

**Accuracy and determinism.** Each projected raw file at one and default
workers matched its HEAD counterpart byte for byte on all seven decks.
The four-, eight-, twelve- and sixteen-worker BSIM4 sweep also produced
the HEAD projection. Removing the save projection and hashing all internal
and exported binary64 time series reproduced the M5-3 SHA-256 values above
at one, four and default workers for both multipliers. Both passed the
per-bit product checks on every profiled run.

| Multiplier | Points | Mean absolute error against cached gold (mV) | Crossings | Crossing difference |
|---|---:|---:|---:|---:|
| BSIM4 | 2,750 | 4.834 | 558 | 6 |
| MOS1 | 1,560 | 4.338 | 486 | 24 |

The unit test for fixed-pivot L3 forces accepted refactors and pivot changes
on a random sparse block-banded matrix and a coupled ladder. It compares
the earliest failed column, factor index sequences, every factor value and
the triangular solution against the serial traversal.

| Verification | Result |
|---|---|
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets -- -D warnings`, default and all features | Both passed |
| `cargo test`, default and all features | 182 passed, 2 ignored / 193 passed, 3 ignored |
| Release JIT verifier with cached regular, VA, BSIM4 and SKY130 decks | 55/55 passed, 4 known, 0 skipped |
| `run.py --check-bits` on both multipliers | Both matched M5-3 at all three worker settings |

**Next measurements.** Repeat the paired default-worker runs on an idle host
to compare against the historical 14.79 s baseline. Instrument update counts
and time by column in the serial factor suffix, including the 2,122 calls
that repivoted. The 12-worker profile puts 13.31 s into numeric LU and
device evaluation together. For LU, evaluate SIMD across independent
destination rows of a single predecessor update. For device evaluation,
measure batching BSIM4 cache misses across instances with the same geometry.
Both experiments can preserve the existing operation order per output.

The checked-in runner accepts repeated worker limits. From the repository
root the following runs reproduce the normal CLI sweep and full-vector checks.

```sh
cargo build --release -p sansho-cli
python3 benches/large/gen.py
python3 benches/large/run.py --deck mult16_bsim4 --jobs 1 --jobs 4 --jobs 8 --jobs 12
python3 benches/large/run.py --check-bits --deck mult16_bsim4 --deck mult16_mos1
```
