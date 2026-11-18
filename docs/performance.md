# PSP103 performance

The benchmark runs the IHP SG13G2 decks with the release Cranelift JIT. The
comparison is against ngspice 44.2 with the corresponding OSDI models. The
round 5 goal is at most 0.80× reference time for each of three measurements
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
an older reference unreliable. For ring21 use the supplied 57 ms analysis
reference chosen by the maintainer from passes at 50–64 ms.

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
commands in the candidate checkout, using the same Rust toolchain. The round 5 baseline is `c49c242`.

```sh
cargo build --offline --release -p spx-cli --features jit
cp target/release/spx /tmp/spx-baseline
# In the candidate checkout
cargo build --offline --release -p spx-cli --features jit
python3 benches/run.py --baseline /tmp/spx-baseline --runs 9
python3 benches/run.py --warm-analysis --baseline /tmp/spx-baseline --runs 41
taskset -c 2 python3 benches/run.py --warm-analysis --baseline /tmp/spx-baseline --runs 41
python3 benches/run.py --devices --baseline /tmp/spx-baseline --runs 9 --deck benches/psp_ring21.cir
python3 benches/run.py --profile --runs 5
python3 benches/compare.py /tmp/spx-baseline target/release/spx
python3 benches/blocks.py --runs 15 benches/psp_ring21.cir
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
| Round 5 | Packed literals and constant derivative factors, independent LU pivots, shared binding work, hardware-accelerated cache checksums and smaller worker pools. See the current table above. |

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
