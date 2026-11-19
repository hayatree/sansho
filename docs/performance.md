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
Cold means an empty spx cache. The OS file cache can be warm on both launches.

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
cargo build --offline --release -p spx-cli --features jit
cp target/release/spx /tmp/spx-baseline
# In the candidate checkout
cargo build --offline --release -p spx-cli --features jit
python3 benches/run.py --baseline /tmp/spx-baseline --runs 9
python3 benches/run.py --warm-analysis --baseline /tmp/spx-baseline --runs 41
taskset -c 2 python3 benches/run.py --warm-analysis --baseline /tmp/spx-baseline --runs 31 --deck benches/psp_ring21.cir --deck verify/va-decks/psp_ring5.cir
python3 benches/run.py --warm-wall --baseline /tmp/spx-baseline --runs 41
python3 benches/run.py --warm-wall --baseline /tmp/spx-baseline --runs 41 --jobs default
python3 benches/run.py --devices --baseline /tmp/spx-baseline --runs 9 --deck benches/psp_ring21.cir
python3 benches/run.py --profile --runs 5
python3 benches/compare.py /tmp/spx-baseline target/release/spx
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
comes from `SPX_PROFILE=1` with device profiling disabled. Per-evaluation
cost comes from separate runs with `SPX_PROFILE_DEVICES=1`. It includes
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

To reproduce, build `spx-cli` with `--offline --release --features jit` and
run `target/release/spx run -j1 verify/va-decks/cmg_ring5.cir` or the inverter
deck. Set `SPX_VA_JIT=0` for the interpreter. In the device profile,
`spx-device` reports native, interpreted and reused counts followed by native
nanoseconds. `spx-interpreted-ns` reports the interpreted nanoseconds per group.

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

```sh
python3 benches/large/gen.py
cargo build --offline --release -p spx-cli
python3 benches/large/run.py --runs 3
python3 benches/large/run.py --check-bits
```

The driver launches a fresh release CLI for each repetition and reports
median milliseconds. It times from process launch to exit with
`SPX_PROFILE=1` and includes writing the projected ASCII raw file. Product
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
