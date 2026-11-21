# Batch control scripts

`sansho deck.cir` and `sansho run deck.cir` execute the deck's `.control` blocks in order. When a block is present it owns the analysis schedule. Analysis cards outside it execute only when a command calls `run`. A deck containing only analysis cards follows the usual batch path. The command line accepts `--json`, `-j N`, `--output-dir DIR` and `--iteration-limit N`.

```spice
Divider at three loads
V1 in 0 1
R1 in out 1k
R2 out 0 1k
.control
foreach load 1k 2k 4k
  alter r2 = $load
  dc v1 0 1 .25
  meas dc endpoint find v(out) at=1
end
.endc
.end
```

The three measurements are 0.5, 0.666667 and 0.8 V. Each belongs to its own DC plot. Reusing a measurement name preserves all three entries in the transcript.

## Analyses and plots

| Command | Behavior |
|---|---|
| `op dc ac tran noise tf sens pz sp disto` | Use the corresponding analysis-card grammar and solver. Each result becomes current as it is produced. Noise and distortion can produce several plots. |
| `four`, `fourier` | Evaluate the requested fundamental and outputs on the current transient. |
| `run` | Execute the loaded analysis cards in card order, with the current circuit values. |
| `setplot [name]` | List plots with the newest first, or select a named plot. `setplot new` creates an empty vector workspace. `$curplot` expands to the current name. |
| `destroy [name …]` | Default to the current plot. `destroy all` keeps the permanent `const` plot. Removing the current plot selects the newest survivor. |
| `linearize [vector …]` | Create a transient plot on the transient card's uniform TSTART/TSTEP/TSTOP grid. Both endpoints are included when the interval is an integer multiple of TSTEP. |

The first workspace is `const`. Analysis names use independent, increasing counters such as `dc1`, `ac1`, `dc2`. Destroying a plot preserves its counter. A bare name searches the current plot, then the constants. Use `dc1.v(out)` to select a particular plot. A `let` before the first analysis is a convenient place for a persistent loop counter. Assign it again in `const` or use a qualified assignment.

## Expressions and state

- `let name = expression` assigns a vector. `let name[i] = scalar` replaces an element. `unlet name …` removes vectors. Indices start at zero. A range `name[i,j]` includes both ends and can run backwards.
- Expressions accept SPICE numeric suffixes, parentheses, `+ - * / ^ %`, comparisons, `and or not` and their symbolic forms. Power associates to the right. Arithmetic broadcasts a scalar over a vector. Two nonscalar operands must have equal lengths. A condition succeeds if any component is nonzero.
- Read voltages with `v(a)` or `v(a,b)`, branch currents with `i(v1)`, and the independent scale by name, for example `frequency`, `time` or `v-sweep`. Read an explicitly set model parameter as `@model[param]`. Instance reads include primary R/C/L values, source DC values and device parameter-list entries. Literal references such as `@m1[gm]` also request supported bias-dependent quantities when signals are saved by default. Use `save all @m1[gm]` before simulation for names built by substitution.
- Constants include `pi`, `e`, `i` (imaginary unit), `c`, `boltz`, `planck`, `echarge` and `kelvin`. A top-level `.csparam name={expression}` evaluates circuit parameters during preprocessing and creates a read-only scalar in `const`.
- Pointwise functions include `mag abs ph cph db real imag j sqrt exp ln log log10 sin cos tan asin acos atan sinh cosh tanh asinh acosh atanh floor ceil round sgn pos`. Phase and trigonometric angles default to radians. Use `set units=degrees` to change the angular unit. `cph` unwraps adjacent samples. `ln` is natural logarithm. `log` and `log10` use base ten. `db` returns 20 log10 of magnitude. `j(x)` multiplies by the imaginary unit.
- `mean` returns the arithmetic mean. `avg` returns cumulative arithmetic means. `length`, `vecmax`, `vecmin`, unary `max` and unary `min` return scalars. Two-argument `max` and `min` compare elementwise. Extrema, ordering, rounding and remainder use real components. `norm` divides by the largest magnitude. `vector(n)` produces 0 through n−1 and `unitvec(n)` produces ones.
- `deriv` uses centered secants and one-sided endpoints. `integ` accumulates trapezoids from zero. `interpolate(plot.vector)` resamples onto the current scale. Linear interpolation holds the nearest endpoint outside the input interval. These numerical choices can differ from polynomial interpolation in ngspice.
- `setseed N` or `set seed=N` initializes a local deterministic generator. `rnd(x)` returns integer draws below each positive bound. A missing seed is an error. The sequence is specific to Sansho.
- `set name`, `set name=value`, `set name = ( a b c )` and `unset name …` manage script variables. Substitution occurs at execution time. `$name`, `${name}`, `$#name`, one-based `$name[i]` and `$&vector` are available. Lists use whitespace-delimited words. `$plots` lists names newest first.
- `if … [else …] end`, `repeat`, `while`, `dowhile`, and `foreach name words …` nest. Each loop ends with `end`. `dowhile` tests after the body. `break` and `continue` apply to the enclosing loop. A `goto` target must be a `label` in the same statement list. `quit` and `exit` end the script.
- `alter r1 = expression`, `alter @m1[w] = expression` and `altermod model parameter = expression` use the session alteration machinery. `option` and `options` use the engine's option grammar. `save` selects signals for subsequent analyses.
- `reset` restores the circuit as it stood at script entry. Existing plots and script variables survive. `alterparam global = expression` stages a numeric override for the next reset. Global parameter re-elaboration currently requires a self-contained deck. It uses the in-memory original input and a loader that denies filesystem access. Session export preserves the original self-contained parameterized script until a session alteration changes its circuit. Flat exports after such an alteration resolve parameter expressions. Retain the source for subsequent alterparam replay.

## Output and security

| Command | Output |
|---|---|
| `echo words` | One transcript line after substitution. |
| `print [line\|col] expressions` | Named values or indexed columns, plus structured numeric columns. Use parentheses around a greater-than comparison to distinguish it from file redirection. |
| `showmod` | Print the current model cards and their explicit parameters to the transcript. |
| `edisplay`, `eprint [node ...]` | List digital nodes or print their successive axis coordinates, names and resolved symbols in the current plot. Both write to the transcript. |
| `print expressions > file` | Formatted text in a file. The numeric columns remain in the transcript. |
| `meas analysis name …` | The existing measurement grammar and engine applied to the current plot. Successful measurements also create scalar vectors. |
| `wrdata file expressions` | Whitespace-delimited columns. Each vector has a scale and real value or complex pair. `set wr_singlescale` writes one shared scale. `set wr_vecnames` adds a header. |
| `write file [vectors …]` | One raw plot with its scale. With no arguments the filename is `rawspice.raw`. Default binary native doubles. `set filetype=ascii` selects ASCII. |
| `plot`, `gnuplot`, `hardcopy` | A named graphics warning. |

CLI output paths resolve against the deck directory or an existing `--output-dir`. The default policy rejects absolute paths, parent components and destinations outside the allowed root. Linux pins the resolved parent directory while publishing. Other platforms restrict confined writes to the root directory itself. Each write exclusively creates a temporary sibling, flushes it, and renames it onto the destination. Existing symlinks and special files are rejected. Replacing an existing hard link leaves its other names intact. The appendwrite option produces an error. The trusted caller may explicitly permit external paths with `--allow-external-paths` or `ControlOptions::allow_external_paths`. A script cannot change this policy.

The evaluator dispatches a fixed command set. Host commands, `!`, `shell`, `cd`, arbitrary `source`, system queries, interactive circuit editing, wildcard expansion, pipelines and command substitution have no execution facility. An unsupported command produces a named diagnostic when reached. The first execution error stops the block and subsequent blocks, preserving earlier results. Ordinary deck includes retain the caller's existing netlist-loader policy.

All loops and jumps share a default budget of 10,000 iterations per execution. Block nesting is limited to 64 levels, expression nesting to 48 and expression token count to 256. Vector constructors and uniform-grid generation permit up to one million points.

## Batch number formatting

Set `numdgt` inside the control block. The four 44.2 reference runs cover 4, 7, 12 and −6. A deck-level `.options NUMDGT` is a parse error. Sansho supports positive integer settings through 17. The observed negative setting restores command defaults.

| Output | Precision and field layout | Example at numdgt = 4 |
|---|---|---|
| Indexed print | Default six fractional places. A minus sign consumes one place. Each component ends with a tab, and a complex real component also has a comma before the tab. The index is eight columns wide in the header. A real vector takes 16 header columns, a complex vector 32. | `3.3333e-01` and `-3.333e-01` |
| Print line | Same numeric fields. Vector values are tab-separated within parentheses and wrap onto continuation lines. Complex pairs use a comma between components. | `3.3333e-01,-1.000e+00` |
| Wrdata | Default eight fractional places. Both signs retain all places. The scale and each real or imaginary component have their own field, a sign column and a trailing blank. Header labels start one space into the same fields. | `-3.3333e-01` |
| Measurement | Left-aligned name in 20 columns, equals sign, two spaces, six fractional places independent of numdgt. | `endpoint            =  -3.333333e-01` |
| Echo vector substitution | Six significant digits in general format, with trailing fractional zeros removed. | `measured -0.333333` |

Scientific fields use lowercase e and a signed exponent of at least two digits. Text redirection uses the same listing as the transcript. Structured vectors, subsequent script calculations and raw files keep their numerical precision. The byte comparison covers numeric records, names, whitespace and complete wrdata files. The reference simulator also prints run banners and dates. [Precision probe](../verify/probes/r14_control_numdgt.cir)

## Library and MCP

`Session::simulate(id, None)` executes a fresh script with file output disabled. Pass explicit analysis cards to run just those analyses. An embedding can supply an output directory and iteration limit with `Session::simulate_with_control`. The loaded session circuit is reusable after a script alters its private copy.

CLI JSON and MCP job summaries expose `control.lines`, `prints`, `measurements`, `plots`, `current_plot` and `written_files`. Each print has expression names and columns of real/imaginary pairs. Plot names index the surviving analyses in creation order. Waveform queries use the usual job and analysis index. MCP exposes no control file capability. A file command returns `E-CONTROL-OUTPUT` along with the in-band work completed before it.

The compatibility target is the ngspice user manual's interactive interpreter, expressions, commands and control structures. This is a batch subset. Multidimensional arrays, command aliases, shell quoting and command chaining are outside its grammar. A source line holds one command. Existing netlist comment and continuation conventions apply.
