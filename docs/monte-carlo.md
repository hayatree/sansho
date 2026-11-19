# Monte Carlo analysis

A batch runs the deck's analyses for each of N statistical instances. Indices run from 0 through N−1. Index 0 is a draw. Use an ordinary `sansho run` for the nominal circuit.

## CLI and API

```sh
sansho mc -n 200 --seed 1 -j 2 verify/mc-decks/rc_delay.cir --json
sansho run --mc-seed 1 --mc-run 37 verify/mc-decks/rc_delay.cir --json
sansho run --mc-seed 1 --mc-run 37 verify/mc-decks/rc_delay.cir -r replay.raw
```

The master seed defaults to 1 for a batch. Replay accepts any unsigned 64-bit seed and index. It prepares the same circuit and runs only the requested index. The CLI count supplies the repetition count for the whole deck, including decks with multiple analysis cards. `--save-waves --json` includes the analysis results projected by the deck's `.save` cards. With no `.save` it includes every circuit signal. The default batch output discards each run's traces after measuring them.

Rust callers can prepare a reusable `sansho_api::MonteCarlo` with netlist text and a `sansho_netlist::SourceLoader`. Its `batch(seed, n, save_waves, &config)` and `run_with(seed, index, &config)` methods share the prepared topology. `Session::monte_carlo(text, base_directory, seed, n, save_waves)` runs a batch on the session's pool. A base directory enables relative includes. Use `None` for a self-contained deck.

The MCP `monte_carlo` tool accepts exactly one of `path` and `netlist`. Paths resolve on the server. For example, the following arguments run the SKY130 mirror.

```json
{"path":"verify/mc-decks/sky_mirror.cir","n":200,"seed":1}
```

MCP returns the scalar batch report as structured JSON. It accepts 1–256 runs and limits the serialized report to 256 KiB before MCP transport wrapping. A larger result returns a tool error. The CLI supports larger batches and waveform output.

## Parameter semantics

| Expression | Sample |
|---|---|
| `agauss(m, a, s)` | `m + a Z / s` |
| `gauss(m, r, s)` | `m + m r Z / s` |
| `aunif(m, a)` | `m + a U` |
| `unif(m, r)` | `m + m r U` |
| `limit(m, a)` | `m − a` or `m + a` with equal probability |

Here Z is standard normal and U is uniform between −1 and 1. For example, `agauss(1k, 90, 3)` has a 30 Ω standard deviation. A nonzero Gaussian spread requires a positive finite sigma divisor. A zero spread gives the nominal value. Ordinary runs evaluate the five functions at their first argument. The existing three-argument `limit(x, lo, hi)` continues to clamp a value.

A top-level `.param` defines one global draw per run. Every use of the parameter sees that value, including uses from instance parameters and model cards. A subcircuit-local parameter or default has one value per invocation. An expression written directly on a primitive, an X-line override or a primitive's selected model card draws for that instance. Two devices of equal size using the same model card therefore get independent mismatch. Calls within a user `.func` inherit the calling expression's identity. Use the same named parameter in two places to express correlation.

The counter is built from the master seed, run index and lexical call identity. Parameter definitions use the scope path and parameter name. Direct fields use the flattened instance path and token position in the numbered source files. Child positions in the expression tree distinguish repeated calls in the same field. Integer mixing produces two independent lanes for the Box–Muller transform. A run caches unit variates by call identity. Scheduling, lazy parameter lookup and replay order do not advance a random stream. Replay assumes the same deck, include contents, simulator build and math platform. Editing a call location can change its draw.

The batch prepares a valid nominal topology, including external nets, internal unknowns and device stamp locations. Geometry-dependent model bins are reselected when a sample changes their applicable bounds. A sample that changes the required topology fails with `E-MC-TOPOLOGY`. Statistical structural `.if` selection and statistical simulator options, including `.temp`, require separate fixed decks. Numeric fields on analysis, measurement, initial-condition and nodeset cards can depend on draws.

## Results

Each `runs` entry carries `seed`, `index`, simulation `success`, `measures` and `diagnostics`. A measurement records its name, analysis, unit, and value or evaluation error. Simulation failure leaves the other indices runnable. A failed measurement on a converged simulation retains its own error and `W-MEAS-FAILED`. The CLI exits with status 1 if any simulation failed, after printing the batch.

The `statistics` array follows measurement-card order. Each entry counts finite, successful measurements from completed runs. It reports the mean, sample standard deviation (N−1), minimum, maximum and interpolated 5th, 50th and 95th percentiles. For sorted observations the percentile position is `(count − 1) p`, with linear interpolation between adjacent values. Empty samples have null statistics. A single observation has null standard deviation. The independent reference check and the three original circuits are described in [the verification guide](../verify/README.md#monte-carlo-distributions).

## M5-5 validation and cost

Release CLI on an AMD Ryzen AI Max+ 395, with seed 1 and 200 successful samples per deck. Times are elapsed milliseconds per statistical instance, including CLI startup, preparation and output. Batch columns use the median of three complete batches with compact JSON output. The separate column times 200 sequential single-worker replay processes, with human output redirected to the null device. The executable and input files were warm.

| Deck | Mean | Sample SD | Batch, 1 worker | Batch, 2 workers | Separate processes |
|---|---:|---:|---:|---:|---:|
| RC half-charge delay | 692.543 ns | 36.874 ns | 0.102 ms | 0.052 ms | 1.028 ms |
| MOS1 falling delay | 260.698 ps | 14.647 ps | 0.540 ms | 0.291 ms | 1.420 ms |
| SKY130 mirror supply current | −20.2033 µA | 0.5440 µA | 0.546 ms | 0.428 ms | 55.054 ms |

The RC analytic mean is 693.147 ns and its standard deviation is 34.667 ns. For each deck the scalar JSON files matched byte-for-byte across one and two workers. Run 3 also reproduced the saved analysis arrays and measurements in a fresh CLI process. The cached nominal verification returned 55 passed, 4 known and 0 skipped. Fresh ngspice distribution comparisons and their elapsed times await the maintainer's black-box runs.
