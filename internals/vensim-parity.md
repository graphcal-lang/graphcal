# Vensim Parity Gap Analysis

> [!WARNING]
> This research note was written by an AI agent (created 2026-07-30; official-
> source audit 2026-07-31; Graphcal repo state `bdb597a` /
> v0.0.1-alpha.17) and must be verified by a human developer before being relied
> on for planning. Vensim claims now cite Ventana's current 10.5-era official
> documentation and website in Appendix B. The online help is a living manual
> containing some legacy pages and edition inconsistencies; recheck edition
> gates before making a purchasing decision. Secondary sources remain only for
> explicitly identified third-party ecosystem context.

Goal: use Graphcal as a Vensim replacement for System Dynamics (SD) modeling.
This note inventories what Vensim provides, maps where Graphcal already stands,
and tiers the remaining gaps. It revives and updates the analysis from the
removed design doc `design/11-system-dynamics.md` (recoverable at
`d9236b35^:design/11-system-dynamics.md`), which settled the core stance:
**system dynamics is a pattern over existing primitives (coordinate time axes +
`unfold`/`scan`), not first-class `stock`/`flow` keywords.**

## 1. Scope and verdict

"Vensim parity" decomposes into four layers with very different costs:

| Layer | What it means | Distance today |
|---|---|---|
| A. Equation language | Express a Vensim model: stocks, flows, delays, lookups, subscripts, data, noise | Substrate exists (`unfold`, coordinate indexes, dimension checking); missing the SD vocabulary and two structural ergonomics (named flows, coupled stocks) |
| B. Analysis workflow | Multi-run compare, sensitivity, calibration, Reality-Check-style testing | Almost entirely missing; single-shot `eval` only |
| C. Interactive UX | SyntheSim sliders, live graphs, gaming | LSP re-eval loop exists; no interactive control surface |
| D. Ecosystem | Reader/publishing, DLL embedding, molecules library, XMILE | Different strategy (git, WASM, Python interop); mostly non-goals near-term |

Verdict: Graphcal's foundations are genuinely suitable — a typed, unit-checked,
git-native SD tool would *exceed* Vensim on model quality assurance (Vensim's
own marketing pillars, units checking and Reality Check, map to areas where
Graphcal is structurally stronger). But a practicing Vensim modeler cannot move
today: the SD function vocabulary (INTEG/DELAY/SMOOTH/LOOKUP/STEP/PULSE), the
two structural ergonomics gaps (§4.1), exogenous time-series data, and every
multi-run workflow are absent. Layer A is compiler/stdlib work and is the
critical path; Layer B is mostly CLI work layered on A; Layer C is LSP/editor
work; Layer D should be deliberately reframed (text + git + WASM instead of
binary packages).

## 2. What Vensim is: capability map

Condensed from Ventana's official 10.5-era documentation. PLE/PLE+/Pro/DSS are
edition tiers. The current release-notes page is titled
[10.4.X–10.5.X](https://www.vensim.com/documentation/vensim-10_4_x.html), and
Ventana announced [10.5 in June
2026](https://vensim.com/2026/06/vensim-ventity-news-june-2026/). Edition labels
below follow the official
[comparison chart](https://vensim.com/comparison-chart-for-vensim-configurations/)
and per-feature availability notes; those two official sources occasionally
disagree, which is called out where material.

### 2.1 Modeling language

Official anchors: [equations](https://www.vensim.com/documentation/ref_equations.html),
[equation components](https://www.vensim.com/documentation/22025.html),
[variable types](https://www.vensim.com/documentation/ref_variable_types.html),
[computational sequence](https://www.vensim.com/documentation/computationalsequence.html),
and the [function summary](https://www.vensim.com/documentation/22300.html).

- **Everything is an equation.** A model is an order-independent set of
  elements written in `.mdl` form as
  `name = expr ~ units [min,max,increment] ~ comment |`. Units and free-text
  documentation are equation fields. Ranges on dynamic variables generate
  runtime warnings rather than clamps; ranges on Constants also supply
  SyntheSim slider bounds and increments. Vensim derives a causal computation
  order rather than using source order (except that a macro must be defined
  before it is called).
- **Executable types are inferred from definitions.** The official type list
  distinguishes Auxiliary, Constant, Data, Initial, Level, Lookup, and Time
  Base, alongside structural/metadata categories such as Group, String,
  Subscript Element/Range, and Units. `==` defines an Unchangeable Constant,
  which cannot be overridden or selected for a slider, sensitivity run, or
  optimization. The Equation Editor additionally presents Reality Check as an
  equation category and Gaming/with Initial/Simultaneous as subtypes. A Rate is
  an Auxiliary *role*, not a separate language type.
- **Stocks & initialization.** `INTEG(rate, initial)` defines a Level and the
  initial expression may reference other variables. Vensim recursively chases
  initialization dependencies until it reaches Constants/Data or detects a
  circle. `ACTIVE INITIAL(active, initial)` supplies a separate initialization
  expression to break such a circle; it does not itself solve an equilibrium.
  Intentional active algebraic loops can instead use iterative `SIMULTANEOUS`
  or the iteration-bounded, Broyden-style `FIND ZERO` root solver (Pro/DSS).
- **Time control and integration.** `INITIAL TIME`, `FINAL TIME`, `TIME STEP`,
  and `SAVEPER` are mandatory model variables with matching time units; they
  may be arbitrary equations, though they are usually Constants. `Time` is
  built in. A run chooses Euler, Diff, RK2 fixed/auto, or RK4 fixed/auto.
  Automatic RK uses *global* `ABSOLUTE TOLERANCE` and `RELATIVE TOLERANCE`
  model constants (default `.001`) and checks every Level; these are not
  per-Level tolerances. The official guidance prefers Euler for ordinary SD
  models and requires Euler or Diff for difference-equation models, while RK
  is useful for physical/oscillatory systems.
- **More than 100 predefined function/keyword entries** appear in the official
  summary (some entries represent whole families); Appendix A is a
  family-level disposition, not a byte-for-byte catalog. The vocabulary spans
  integration and constrained `SINTEG`; material, information, fixed,
  conveyor, batch, and profile delays; `SMOOTH`/`TREND`/`FORECAST`; test inputs;
  sample-and-hold; lookup accessors; guarded division; arrays and matrix
  operations; allocation/market clearing; finance; queues; data access; and
  runtime messages.
- **Delay semantics distinguish conserved material from information.** The
  official modeling guide classifies `DELAY1`, `DELAY3`, `DELAY N`, and
  `DELAY MATERIAL` as quantity-conserving, while `SMOOTH`, `SMOOTH3`,
  `SMOOTH N`, and `DELAY INFORMATION` preserve the input's range when delay
  time varies. `DELAY FIXED` uses only the initial delay time. Several dynamic
  functions are implemented as hidden macro-expanded Levels so causal tracing
  can expose them.
- **Test inputs have grid semantics.** `STEP`, `RAMP`, `PULSE`, and
  `PULSE TRAIN` change only at `TIME STEP` boundaries regardless of integration
  method. `STEP` and `PULSE` use an internal `Time + TIME STEP/2` convention to
  avoid boundary-rounding errors. Runge–Kutta also holds exogenous data, these
  test inputs, and pure delays constant during an integration interval.
- **Randomness is reproducible by default.** The current `RANDOM ...` family
  has eleven distribution/PDF forms (Beta, Binomial, Exponential, Gamma,
  Lookup, Negative Binomial, Normal, Poisson, Triangular, Uniform, Weibull),
  plus obsolete `RANDOM 0 1` and separate autocorrelated `RANDOM PINK NOISE`.
  Most forms truncate to explicit minimum/maximum bounds. Calls supply a
  stream ID; calls may intentionally share a stream, and the docs recommend
  one or a few streams rather than one per call site. Stream 0 is initialized
  by the `NOISE SEED` model variable, and `NOISE RNG` selects the generator
  (SIMD-oriented Mersenne Twister by default).
- **Subscripts** (Pro/DSS) support up to eight dimensions, generated element
  ranges (`(A1-A10)`), overlapping subranges, positional mappings (`->`),
  family copies (`<->`), `:EXCEPT:` equations, disjoint multiple definitions,
  constant arrays, subscripted lookups, and `!` axis marking for reductions.
- **Units** are equation-attached names combined through expressions and
  compared literally, with model-specific synonym groups. Numeric literals are
  treated as dimensionless unless context makes their units apparent;
  "strictest testing" disables that contextual assumption. Unit-check errors
  are reported but explicitly do not prevent simulation.
- **Macros** (`:MACRO:` … `:END OF MACRO:`, Pro/DSS) define reusable structures
  that may contain Levels. Definitions may call previously defined macros and
  are recursively expanded. They cannot define internal subscripts; expanded
  variables inherit the caller's left-hand-side subscripts.
- **Data variables** are exogenous series supplied by datasets or `GET`
  functions. Their equation-level modes are `:INTERPOLATE:` (default),
  `:HOLD BACKWARD:`, `:LOOK FORWARD:`, and `:RAW:`. `:=` data equations run
  during setup and include transforms such as `CUMULATE`, `TIME SHIFT`, and
  `X IF MISSING`. `:NA:` is a recognized numeric missing-value sentinel: raw
  gaps can be tested, plots omit/interpolate them, and tables render `—`.
- **Numerics** use double precision in every edition since Vensim 8 (2019).
  Division by zero and floating-point overflow abort a normal run and identify
  the equation/time. `ZIDZ`/`XIDZ` intentionally substitute 0/an alternate when
  `abs(denominator) < 1e-6`. A normal lookup linearly interpolates and holds the
  first/last y value outside its x range, but this endpoint hold emits a
  warning—it is implicit, not silent.

### 2.2 Structural and run-analysis tools

Official anchors: the current [Tree
Diagram](https://www.vensim.com/documentation/treetool.html), [Loops](https://www.vensim.com/documentation/loops_tool.html),
[Document](https://www.vensim.com/documentation/documenttool.html), [Runs
Compare](https://www.vensim.com/documentation/runscompare.html), and
[Table](https://www.vensim.com/documentation/23865.html) documentation.

- **Causal Tracing®** includes expandable Causes/Uses trees, strip graphs, and
  Causal Chain route finding. The redesigned 10.5 Tree can overlay a thumbnail
  behavior graph on each node; it treats a subscripted variable as the union of
  all element-level causes/uses.
- **Loops tool** lists feedback loops through the workbench variable, shortest
  first, and can highlight/place a loop on the sketch. It is not literally
  unbounded: only loops shorter than 32 variables are searched, and the UI asks
  before displaying more than the first 50.
- **Document tool** (rebuilt in 10.5) provides filterable grid/list views of
  equations, comments/definitions, units, values, and causes/uses. The
  **Causal Table** (added in 10.1) is an influence adjacency matrix preserving
  arrow formatting.
- **Table tool** displays run/data values and can perform equilibrium testing:
  report Levels whose first saved value differs from the initial value beyond
  both configured absolute and relative tolerances.
- **Units check** (Ctrl+U) reports literal-name dimensional inconsistencies with
  model-managed synonyms (`Dimensionless`, `Dmnl`, `Fraction`, `Nil`, and `1`
  are built-in equivalents). Errors do not block simulation.
- **Model check** (Ctrl+T) checks syntax/completeness, duplicate and subscript
  definitions, variable-type usage, and whether active/initial equations can be
  causally ordered; undefined references are reported and then treated as Data.
- **Runs Compare** is broader than an assumptions diff. It can compare selected
  variable types in the first two loaded datasets. For Constants it reports
  values; for dynamic series it reports first difference/time, counts,
  mean/maximum absolute and percentage differences, and slow/fast difference
  heuristics, with configurable absolute+relative thresholds.
- **Stats tool** (Pro/DSS) reports count, min/max, mean, median, standard
  deviation, normalized deviation, percentiles, and optional R² against the
  first dataset. RMSE, MAE/MAPE, residual autocorrelation, and Theil
  mean/variance/covariance decomposition belong to the optimization **payoff
  report**, not the Stats tool.

### 2.3 Simulation UX and scenario workflow

- **SyntheSim** is more than sliders. Every changeable Constant gets a slider
  and dynamic variables get behavior thumbnails; dragging continuously
  re-simulates.
  Lookups are editable point-by-point with immediate feedback, and almost any
  variable can be temporarily replaced by a constant or time-shaped exogenous
  input, explicitly cutting a feedback link. Since **6.5**, SyntheSim can also
  run a configured sensitivity ensemble; ordinary runs occur while dragging,
  and the ensemble runs on slider release for sensitivity range/bar objects.
- **Gaming mode** (PLE+ and above) advances to a user-selected time, pauses for
  changes to `GAME` variables, can load/export timestamped `.gin` decisions,
  and can back up to an earlier time. Backup is unavailable when the model uses
  pure delay or queue functions—a material limitation omitted by the earlier
  draft.
- **Runs are datasets.** A normal simulation creates and loads a named
  `.vdf/.vdfx` dataset. The official development guide documents up to eight
  loaded datasets, which need not come from the same model. Dataset-aware
  graphs overlay loaded runs; other analysis tools intentionally use the first,
  first two, or all loaded datasets depending on the tool. A Save List can
  reduce the variables stored (all are stored by default for causal tracing).
- **Scenarios are reusable.** Constant/Lookup overrides can be saved in text
  changes files (conventionally `.cin`), layered on prior scenarios, or based
  on a previous run. A run may optionally **Resume** from a prior run's ending
  state. The DSS Run Configuration Manager (added in 9.4) stores multiple
  configurations and runs any subset/all with one click.
- **Reference Modes** are up to eight user-drawn/imported behavior-over-time
  traces representing observed, expected, feared, or desired behavior. They can
  be overlaid with simulation output and can also seed an exogenous driver.
- **Custom graphs, strip graphs, tables, Gantt/bar displays, and embedded I/O
  objects** provide presentation surfaces; datasets and tool output can be
  exported to text/CSV/spreadsheet and graphics to formats including SVG.

### 2.4 Reality Check

[Reality Check®](https://www.vensim.com/documentation/usr14.html) is available
in every Vensim edition, including PLE—not a premium product-tier advantage.
Constraints use `:THE CONDITION:` / `:IMPLIES:` and optional named
`:TEST INPUT:` equations. In an **active** check, equality/Test Input conditions
replace a variable's equation; a false inequality is made true by assignment;
the consequence is then tested. Other constraints are checked **passively**
without intervention during that Reality Check run (not during an ordinary
simulation). Dynamic `RC STEP/RAMP/...` inputs and corresponding `... CHECK`
functions support onset, duration, and grace periods.

`Test All` activates every Constraint in turn; Boolean condition structure can
require more than one simulation for one Constraint. Vensim temporarily
restructures and reorders the model, always runs these tests interpreted rather
than compiled, and can create a new algebraic loop that prevents the check from
running. Results are reported but `Test All` does not retain the simulation
runs. It is therefore both a regression facility and an equation-level
intervention engine—not merely implication syntax.

### 2.5 Estimation & optimization (Pro/DSS)

- A bounded, modified **Powell** search is the default optimizer, configured by
  a `.voc` control file; random/re-randomized and grid multi-start modes are
  also available.
- **Payoff definitions** (`.vpd`) combine calibration terms (model/data error or
  likelihood terms, including priors) and policy terms (weighted model
  performance). The payoff report now includes fit diagnostics such as R²,
  Durbin–Watson/residual autocorrelation, RMSE and Theil decomposition, and
  MAE/MAPE.
- Parameter bounds can be explored with payoff sensitivity. For a properly
  weighted sum of squares/log-likelihood, payoff changes yield approximate
  confidence bounds; this is more precise than the earlier note's unsupported
  phrase "likelihood profiling."
- **MCMC** samples the posterior/likelihood surface and reports chain
  diagnostics, marginal/joint distributions, samples, and parameter bounds;
  the same engine supports **simulated annealing** optimization. Vensim 10.5
  advertises substantial MCMC/optimization performance and output improvements.
- An extended nonlinear **Kalman filter** adjusts model states from available
  data during a run. **Stochastic optimization** optimizes the aggregate payoff
  (a sum) over a sensitivity ensemble of random or file-specified scenarios.
- Since Vensim 10, most sensitivity, optimization, and MCMC work can run in
  parallel. The official comparison chart assigns multicore execution to DSS,
  so do not assume the same entitlement in Pro without checking the license.

### 2.6 Sensitivity (PLE+ and up)

The official control format supports **Univariate, Multivariate Monte Carlo,
Latin Hypercube, Latin Grid, and File-driven** designs, with explicit seeds and
per-parameter distributions (uniform, normal, triangular, Weibull, and others).
Output includes individual traces, percentile/confidence-band time graphs, and
histograms at a selected time—not only end-time histograms—and is exportable.

Vensim 10 added **Sensitivity2All**: for selected outputs it increases and
decreases every changeable Constant by 10% (by `0.1` when the base is zero),
respects declared bounds, runs the cases, and ranks/displays influence. This is
not the same as distribution-based global sensitivity, but it is an effective
one-at-a-time screening workflow.

### 2.7 Data connectivity

- Import/conversion commands include `CSV2VDF`, `TAB2VDF`, `XLS2VDF`,
  `DAT2VDF`, relational `DLIST2VDF`, and tidy-data `TIDY2VDF`.
- `GET XLS *` links to Excel; `GET DIRECT *` reads `.xls/.xlsx` or delimited
  text without Excel. Families populate CONSTANTS, DATA, LOOKUPS, and
  SUBSCRIPT definitions during model check/simulation setup. DSS additionally
  supports ODBC.
- Imported datasets can both drive Data variables and serve as comparison data.
  Using the same series/variable name makes graphs and tables overlay history
  and simulation automatically; interpolation policy lives on the Data
  equation.
- Runs export through `VDF2TAB`, `VDF2CSV`, `VDF2XLS/XLSX`, data-list, and
  related commands, with save lists, time ranges, and output frequency options.

### 2.8 Automation, embedding, publishing (mostly DSS)

- Linear `.cmd` **command scripts** batch long command sequences and can make
  Vensim act as a batch processor; the official examples explicitly say they
  do not support iteration or branching. The DSS Action Recorder can generate
  scripts from simulation actions.
- The **Vensim DLL** can load packaged models, run/game them, retrieve data,
  and display tools/sketches (full Windows DLL). Official examples/prototypes
  cover C/C++, Visual Basic/VBA/Excel, Java, and Delphi/Pascal; a server variant
  provides multiple isolated contexts.
- **Compiled simulations** order equations, emit C, and compile a platform
  `.dll`/`.dylib`; this is intended to accelerate simulation, sensitivity, and
  optimization. The earlier `~3×` figure had no support in current official
  documentation and is removed. DSS also supports arbitrary native external-
  function DLLs.
- **Venapps** are screen-oriented decision-support/flight-simulator interfaces
  composed from buttons, sliders, graphs, games, and Vensim commands.
- Packaged `.vpm/.vpmx` models and `.vpa/.vpax` applications run in the free
  **Model Reader**. Web publishing emits model C and compiles it with Emscripten
  to WASM, exposes a JavaScript API, and can generate a SyntheSim-like web app.
  Vensim 10.5 adds simpler templates, dashboards, and packaged exogenous data;
  file-reading data functions such as `GET XLS DATA` still cannot execute in
  the browser. Current release notes group web-publishing work under Pro/DSS,
  while the older comparison chart marks publishing DSS-only—an official
  edition discrepancy to verify.

### 2.9 Files, interchange, and ecosystem

- `.mdl` is documented plain text, but one file contains equation groups,
  macros, equations, generated **Sketch Information**, and settings. Ventana's
  official Git/SVN guidance is more favorable than the earlier draft admitted:
  since 8.2, Vensim can externalize user-specific settings to `.mdls`, preserve
  equation order, and stabilize zoom/recentering to reduce noisy diffs. The
  generated sketch records remain less reviewable/mergeable than equations,
  but "almost impossible to merge" is not an official-source-supported claim
  and has been removed.
- Since 7.2 Vensim attempts to read **XMILE** models (7.3.4 still called support
  experimental), and an older `stel2ven` utility translates Stella/iThink
  equation exports. `MDL2XMILE` is not present in the current official command
  list, so the earlier claim of official XMILE export is removed rather than
  inferred from a forum post.
- Third-party ecosystem context (not Ventana claims): **PySD** translates
  `.mdl` to Python; **SDEverywhere** targets JavaScript/C/WASM and is used by
  En-ROADS/C-ROADS; SDM-Doc, venpy, readsdr, and stanify provide other analysis
  and interoperability paths.
- **Molecules of Structure** is a separately licensed collection of small,
  reusable complete model structures included with current installations.
  Vensim supplies an in-app mechanism to browse, copy/replicate, replace, and
  extend that collection; the molecules are content, not language builtins.

## 3. Where Graphcal already stands

### 3.1 Concept mapping

| Vensim | Graphcal today | Status |
|---|---|---|
| Auxiliary variable | `node` | ✅ |
| Constant / Unchangeable Constant | `param` / `const node` | ✅ (richer: required vs default, domain constraints) |
| Model + views | files, modules, `dag` blocks | ✅ different shape, comparable power |
| `INTEG(rate, init)` (Level/stock) | `unfold(Axis, init, \|prev, t0, t1\| prev + rate*(t1-t0))` | 🟡 expressible, Euler only, by hand |
| `SINTEG` constrained/quantized stock | conditional logic inside `unfold` | 🟡 semantics by hand; no explicit saturating integrator (§4.2) |
| Flow (Rate—an Auxiliary role) | expression inside the `unfold` body | 🟡 cannot be a *named* equation participating in feedback (§4.1) |
| TIME STEP / INITIAL/FINAL TIME | `index T = range(t0, t1, step: dt)` | 🟡 typed & exact, but compile-time only (§4.5) |
| TIME | loop coordinate `t` in `for`/`unfold` | ✅ |
| SAVEPER / Save List | — | ❌ no time decimation or reusable run-output projection (§4.5, §5.1) |
| Integration methods (Euler/Diff/RK2/RK4) | hand-written Euler | ❌ `integrate` proposed (#351) |
| Subscripts | `index` + `T[I]`, tables, `for` | ✅ core; ❌ subranges, mappings, `:EXCEPT:` (§4.8) |
| Lookup / graphical function | — | ❌ (#891) |
| DELAY1/3/N, SMOOTH*, TREND, ... | — | ❌ (§4.3) |
| STEP / PULSE / RAMP | `if`/`else` over `t` by hand | 🟡 sugar and Vensim's exact grid-boundary semantics missing |
| RANDOM * / NOISE SEED | — | ❌ deterministic by design; needs seeded-pure design (§4.7) |
| IF THEN ELSE | `if cond { } else { }` | ✅ (exhaustive `else` required) |
| ZIDZ / XIDZ | `if` guard | 🟡 divergent philosophy: error-by-default is safer; explicit guard is easy |
| Units check | dimensions as types, rational exponents | ✅ **stronger** (see §3.2) |
| Data variables (exogenous series) | `--input` JSON / literal tables | ❌ no series binding, alignment, or interpolation modes (§4.6) |
| `.cin` changes / run configurations | `--set` / `--input`; case tables proposed | 🟡 overrides exist; no named multi-run scenario artifact (#897, §5.1) |
| Datasets / Runs Compare | — | ❌ single-shot eval; no behavioral diff metrics (§5.1) |
| Based On / Resume a run | — | ❌ no checkpoint/restart state (§5.1) |
| Reference Modes | literal series + plots, assembled manually | 🟡 no draw/import/overlay workflow (§6.1) |
| Sensitivity / optimization / MCMC / Kalman | — | ❌ (§5.2, §5.3) |
| Reality Check | `assert` (boolean, tolerance, indexed) | 🟡 checking exists; equation-level forcing/intervention does not (§5.4) |
| Causes/Uses trees, chains, loops | `graphcal graph` (DOT, experimental), LSP references | 🟡 (§6.2) |
| SyntheSim | LSP inlay values, 300 ms debounced re-eval | 🟡 recompute loop exists; no sliders, lookup editor, overrides, sensitivity UI, or live charts (§6.1) |
| Gaming, Venapp, Reader, publish-to-web | — | ❌ different strategy (§6.3) |
| Document tool | `graphcal eval`/`check` output, LSP hover | 🟡 |
| Molecules library | — | ❌ content awaits an SD stdlib; packages can be git dependencies |
| `.mdl` text format | `.gcl` text | ✅ cleaner semantic diffs: no generated sketch records; exact-rev lockfile |

### 3.2 Where Graphcal is already stronger

Worth stating because it defines the pitch — a Vensim replacement that keeps
these properties is *better*, not just equivalent:

- **Dimensional safety.** Vensim units are literal names plus model-maintained
  synonym groups, and official documentation says unit-check errors do not
  prevent simulation. Graphcal dimensions are compile-time types with rational
  exponents; `Velocity ≡ Length/Time` structurally. There is no synonym drift.
- **Explicit failure policy.** Vensim's `ZIDZ`/`XIDZ` silently substitute at
  `abs(denominator) < 1e-6`; Graphcal's default is an error with per-node fault
  isolation. Vensim's endpoint lookup hold is *not* silent—it warns—but its
  policy is implicit; Graphcal should require the policy at the call/type
  boundary (§4.4).
- **No broadcasting.** Vensim subscript equations replicate/broadcast a scalar
  or mapped expression implicitly; Graphcal requires explicit `for`, reducing
  the NumPy/Excel class of accidental-shape bugs.
- **Cleaner semantic version control.** Vensim genuinely supports text `.mdl`
  and, since 8.2, provides Git-oriented settings that externalize user state and
  reduce diff noise. It still mixes equations with generated sketch records.
  `.gcl` keeps semantic source free of diagram layout and adds an exact-revision
  dependency lockfile, which Vensim does not provide.
- **Typed data model.** Algebraic types, exhaustive `match`, `Fin(N)`, and
  phantom-typed frames go beyond Vensim's numeric scalars/subscripts and String
  Variables.
- **Ordinary typed assertions.** Graphcal assertions compose directly with
  indexed Booleans and tolerance forms. Reality Check is available in all
  Vensim editions and has stronger intervention machinery, so the advantage is
  language integration—not product tier or current feature breadth.
- **Sandboxed extensibility.** WASM plugins are sha-pinned, fuel-metered, pure;
  Vensim external functions are arbitrary native DLLs (DSS only).

## 4. Tier 0 gaps — expressing a Vensim model at all

These block porting even a *toy* Vensim model. Ordered by structural depth,
not effort.

### 4.1 Named flows and coupled stocks (the structural gap)

**Vensim:** a model is a web of small named equations. Stocks and their flows
reference each other freely; feedback loops *between* named variables are the
whole point of the paradigm. The engine schedules within a time step
(auxiliaries in dependency order, then integrates), so `stock → flow → stock`
is the normal case, and Vensim even detects and reports genuinely simultaneous
(same-instant algebraic) loops.

**Graphcal:** cycle detection is at whole-node granularity
(`exec_plan.rs` toposort), and since #1029 *any* `@self` reference inside an
`unfold` body is a cycle — the only back-reference is the `prev_state` lambda
binding. Consequences:

- A flow cannot be a named `node` if it reads the stock it feeds
  (`stock ↔ flow` is a node-level cycle). It must be inlined in the closure.
- Two stocks that read each other cannot be two `unfold` nodes. They must be
  packed into one struct state in one `unfold` (#889 notes this workaround and
  its cost).
- There are **no expression-local bindings** (no `let` in `grammar.ebnf`), so
  the inlined flows can't even be *named locally* — every flow is repeated
  inline or the model becomes one opaque expression. (The removed design doc
  assumed `let` in closure bodies; it was never implemented. #349 tracks
  closure-body syntax.)

This — not delays or lookups — is the deepest parity gap: the Vensim modeler's
unit of thought is the named equation, and today every coupled subsystem
collapses into one giant anonymous closure. The design doc's answer was
per-`(node, t)` DAG granularity so `A[t] → B[t-1]` cross-references work; the
lighter-weight alternative is `let` bindings (or DAG-style bodies, #349) plus
struct-state packing (#889). Either way this needs a design decision before the
SD stdlib (§4.3) can even be written, because `DELAY3`/`SMOOTH` are themselves
stocks that must compose inside a user's recurrence.

A related, smaller gap is **initialization structure**. Vensim recursively
resolves arbitrary acyclic initial expressions at t₀. Many steady-state
initializations (derive stock from parameters so inflow equals outflow) need no
special function. `ACTIVE INITIAL(active, init)` does something narrower: it
supplies a distinct initialization expression to cut a circular initial
dependency, and may produce a warning when the first active value differs. It
is not an equilibrium solver. `SIMULTANEOUS` iterates an intentional active
algebraic loop, while `FIND ZERO` solves a scaled root system. Graphcal
`unfold` init expressions already cover the acyclic case and can directly
supply a separate initial expression; genuinely self-referential equilibrium
or active algebraic constraints need the bounded solve/fixpoint primitive
proposed in #888.

Tracked: #889 (indexed state in unfold/scan), #349 (closure body syntax),

## 888 (bounded solve/fixpoint — covers `FIND ZERO`/`SIMULTANEOUS`/equilibrium

init). Untracked: `let`/local bindings; per-`(node, t)` dependency granularity
decision.

### 4.2 Stocks and integration quality

`unfold` gives hand-written explicit Euler. Vensim offers six per-run methods:
Euler, Diff (Euler with previous-save-time Auxiliary reporting), RK2/RK4 fixed,
and RK2/RK4 auto. Auto RK uses the two **global** model tolerances against every
Level, halves a failed step down to `TIME STEP/256`, and never steps above
`TIME STEP`; it is not controlled by per-Level tolerances. Vensim recommends
Euler for most ordinary SD models and Euler/Diff for difference equations,
while RK is useful for physical and oscillatory systems. RK4 matters for models
such as predator–prey or supply-chain oscillation, where Euler at a practical
`dt` can visibly distort amplitude.

The settled Graphcal proposal is
`integrate(init, method: RK4, |state, t| derivatives)`, with the compiler
checking `derivative_dim * time_dim == state_dim` (#351). Two semantics need to
be designed rather than inferred:

- RK stages generally evaluate flows between output coordinates. Vensim,
  however, freezes exogenous inputs, `STEP`/`RAMP`/`PULSE`, and pure delays
  within each RK integration interval. Graphcal needs an explicit stage-input
  policy (hold, interpolate, or evaluate continuously) for external series and
  test inputs; merely requiring every input to be a continuous closure would
  not reproduce Vensim.
- Vensim modelers commonly guard outflows with `MAX`, and Pro/DSS also has
  `SINTEG(rate, initial, min, max, quantum, special, threshold)`, an explicitly
  non-conservative saturating/quantizing integrator. Graphcal domain
  constraints error rather than clamp. Non-negativity and saturation are valid
  model semantics, so they need an explicit constrained-integrator policy, not
  accidental reuse of error constraints.

Tracked: #351. Untracked: RK stage-input semantics and constrained stock
(non-negativity/saturation/quantization) semantics.

#### 4.3 The SD function library (delays, smooths, test inputs)

The recurring Vensim vocabulary (family-level disposition in Appendix A) is:

- **Exponential material delays** — `DELAY1`/`DELAY1I` and
  `DELAY3`/`DELAY3I` expand to continuous internal Levels; `DELAY N` is a
  defining state function whose output is held within each `TIME STEP`. All are
  first-/third-/Nth-order forms that conserve the time-integral of material
  when delay time changes.
- **Pure/discrete transit delays** — `DELAY FIXED` holds the delay time's
  initial value and needs history storage; `DELAY MATERIAL` conserves quantity
  under a varying delay time; `DELAY INFORMATION` instead discards/holds
  samples to preserve the signal range. `DELAY CONVEYOR`, `DELAY BATCH`, and
  `DELAY PROFILE` add ordering, batch, leakage, and arbitrary transit-profile
  behavior. These are not one homogeneous "conserving family."
- **Information smoothing** — `SMOOTH`/`SMOOTHI`,
  `SMOOTH3`/`SMOOTH3I`, `SMOOTH N`, `TREND`, and `FORECAST` preserve signal
  semantics rather than material conservation.
- **Test inputs** — `STEP(h, t)`, `RAMP(s, t0, t1)`, `PULSE(t, w)`, and
  `PULSE TRAIN(...)`; sine forcing uses `SIN`. Exact parity includes Vensim's
  `TIME STEP`-quantized and midpoint boundary behavior (§4.2), not just the
  mathematical shape.
- **Sample/hold, restart, and discrete helpers** — `SAMPLE IF TRUE`, `INITIAL`,
  `REINITIAL`, `ACTIVE INITIAL`, `QUANTUM`, `MODULO`, `INTEGER`, and
  `SHIFT IF TRUE` (aging chains).
- **Financial dynamics** — `NET PRESENT VALUE`, `NPV`, `NPVE`, internal rate of
  return, and depreciation functions.
- **Later tier** — FIFO queues with age/attribute statistics, allocation by
  priority, and market clearing.

A practical first tranche of delays, smooths, test inputs, sample/hold, and NPV
would be roughly 20 functions. That is a scope recommendation, not a measured
claim about the proportion of published models.

None exist in Graphcal (`builtin.rs` has 64 functions at the audited revision:
math, aggregation, conversions, and datetime, but nothing temporal). Vensim
itself implements many dynamic functions (`DELAY1/3`, `SMOOTH/3`, `TREND`,
`FORECAST`, `NPV/E`) as macro-expanded hidden Levels. Two Graphcal shapes are
viable:

1. **Stdlib dags/macros over a time axis** — semantically honest, because delays
   are state, but blocked on §4.1 ergonomics (a delay must compose inside a user
   recurrence) and generic-over-axis dag instantiation.
2. **Builtins/intrinsics in unfold/integrate bodies** — stateful special forms
   lowered to explicit extra state, analogous to Vensim's expanded Levels.

WASM plugins **cannot** host these: plugins are pure whole-array transforms; a
stateful function inside a feedback loop must step in lockstep with its caller
(verified against the plugin ABI: no retained buffers, result arrays only over
input-bound index variables).

Tracked: #99 (stdlib, generic). Untracked: SD function library specifically.

#### 4.4 Lookup tables (graphical functions)

Vensim lookups are ubiquitous nonlinear "effect of X on Y" relationships:
named piecewise-linear x/y tables with editor display ranges, inline
`WITH LOOKUP`, and `LOOKUP EXTRAPOLATE`, `FORWARD`, `BACKWARD`, `INVERT`,
`AREA`, and `SLOPE` accessors. A normal lookup holds the first/last y value
outside the table and **warns**; extrapolation is opt-in and suppresses that
warning. Lookups can be subscripted (one curve per element) and loaded from
files. Vensim expects the ordinary lookup input to be dimensionless and warns
on a dimensioned x argument, while the output carries the Lookup variable's
units.

Graphcal has nothing (`table` is map-literal sugar, not interpolation; verified
no `interp`/`lookup` anywhere). #891 proposes `interp(xs, ys, x)` plus a
breakpoint-table type. Parity should improve on Vensim with:

- dimensioned x and y axes;
- an explicit out-of-range policy (hold/clamp, extrapolate, or error)—the issue
  with Vensim is implicit policy, not a silent warning-free clamp;
- a literal syntax pleasant enough for 20-point curves (the `table` layout is a
  good start);
- use inside `unfold`/`integrate` bodies (again §4.1).

Tracked: #891.

#### 4.5 Time-axis control at run time

Vensim's `FINAL TIME`, `TIME STEP`, `INITIAL TIME`, `SAVEPER` are *ordinary
model equations* (`FINAL TIME = INITIAL TIME + 100` is legal) — changeable
per-run from the UI, changes files, or scripts. Graphcal coordinate-index bounds
must be **compile-time constants** (runtime values rejected; verified in
`ir/lower.rs`); `--set` cannot touch an index; `pub(bind) index` moves the
choice to another *file*, still compile time. Consequences: no "run to 2100
instead of 2050" without editing source; no dt-halving convergence check from
the CLI; blocks sweep tooling (§5.2) from varying horizons. Also missing:
`SAVEPER` time thinning—a 100k-step run currently prints/plots 100k points and
only the 1M cardinality cap bounds it—and Vensim-style Save Lists for reusable
variable projection in run artifacts.

Untracked: runtime-bound coordinate indexes (or CLI index overrides), SAVEPER
analogue (output decimation policy), and reusable output save lists.

#### 4.6 Exogenous time series (data variables)

Vensim: data variables are first-class citizens — imported from
CSV/XLS/ODBC, carrying interpolation modes (`:INTERPOLATE:`, `:RAW:`,
`:HOLD BACKWARD:`, `:LOOK FORWARD:`), graphable against model runs, drivable
into calibration payoffs. Graphcal: params can be indexed, so a series *can*
enter as a literal `table` or `--input` JSON keyed by named labels — but
(verified) JSON indexed-param entries work for **named-label** indexes, there
is no CSV/spreadsheet ingestion, no binding of an external series onto a
*coordinate* axis, no resampling/alignment (no index mapping at all), and the
1 MiB default `--input` cap is sized for parameters, not datasets.

Needed for parity: bind external series → `D[T]` params (with explicit
alignment/interpolation policy against the model's axis), CSV/XLSX readers at
the CLI boundary (keeping the core pure), and export of any `D[T]` node back
out. Tracked: #892 (documenting the binding path), #353 (spreadsheet
import/export), #355 (input files), #350 (table autofill). Untracked:
interpolation-mode semantics, coordinate-axis JSON/CSV binding.

#### 4.7 Stochastics with reproducibility

Vensim has eleven current `RANDOM ...` distribution/PDF functions—Uniform,
Normal, Poisson, Beta, Binomial, Negative Binomial, Gamma, Weibull,
Exponential, Triangular, and Lookup—plus obsolete `RANDOM 0 1` and separate
`RANDOM PINK NOISE`. Most distribution functions stretch/shift an underlying
distribution and rejection-sample into explicit min/max bounds; failure after
2,000 rejected draws returns `:NA:` and can stop downstream computation.

Contrary to the earlier "per-site stream" wording, the last argument is a
**stream ID**, and multiple call sites may deliberately consume the same
stateful stream. Ventana recommends only one or a few streams unless noise
sources need independent control. ID 0 uses `NOISE SEED` (default `4487556` if
absent); `NOISE RNG` defaults to the SIMD-oriented Mersenne Twister. Same-input
runs are reproducible unless a model deliberately randomizes a stream with
`GET TIME VALUE`.

Graphcal is deterministic **by design** (the plugin ABI bans entropy imports;
tzdb is pinned; bit-identical behavior is a goal). The parity path should be a
*seeded, pure* PRNG—preferably a splittable counter-based design where an
explicit `param` seed plus typed draw-site identity and coordinate derive each
sample. That preserves Vensim's reproducibility while avoiding order-sensitive
shared-stream consumption: refactoring an unrelated draw should not perturb a
series. Distribution sampling then enables §5.2 Monte Carlo.

Untracked entirely.

#### 4.8 Subscript conveniences

Graphcal indexes cover Vensim's core subscript uses (multi-dim variables —
Vensim caps at 8 dims, Graphcal has no rank cap — tables, per-element
equations) with stronger typing. Missing Vensim conveniences (all verified
absent in Graphcal): subranges (`Europe: FR, DE, ...` within `Country`, with
equations over the parent covering the subrange), family mappings (`->`) and
copies (`<->`), `:EXCEPT:` partial equations plus multiple definitions over
disjoint subranges, aggregation over *all* axes at once (Vensim
`SUM(x[d1!,d2!])`; rank≥2 aggregation is `D021` "not yet defined"), `PROD`,
element-position arithmetic (`VECTOR ELM MAP`; Graphcal named labels are
deliberately not values — `Fin(N)` covers part of this), sorting/ranking
(`VECTOR SORT ORDER`/`RANK`/`REORDER`), masked reduction (`VECTOR SELECT`),
aging-chain shifting (`SHIFT IF TRUE`), and any index-to-index zip/mapping.
`ALLOCATE AVAILABLE`-style allocation and market clearing have no counterpart
(rarely blocking; niche).

Tracked: #894 (dynamic/data-driven indexes), #893 (keyed lookup/group-by),

## 890 (linear algebra). Untracked: subranges/mappings, multi-axis aggregation

(has an error-message placeholder only).

### 5. Tier 1–2 gaps — the analysis workflow

Everything here is CLI/tooling layered on Tier 0; none of it needs language
changes except where noted.

#### 5.1 Runs, scenarios, and restartable artifacts

Vensim separates several concepts that the earlier draft collapsed:

- every run is a named `.vdf/.vdfx` result dataset; dataset-aware tools can
  overlay runs even when they came from different models;
- text `.cin` files persist Constant/Lookup assumptions, while the DSS Run
  Configuration Manager stores and batch-runs named configurations;
- Runs Compare can report both assumption and dynamic-series differences with
  thresholds and summary metrics;
- **Based On** reuses assumptions from a prior run, and **Resume** continues
  from its ending simulation state; and
- `SAVEPER` plus Save Lists control time and variable output density.

Graphcal has no counterpart: `graphcal eval` is single-shot, plots see one
evaluation, and source-level double inclusion is the only scenario comparison.
A credible design needs (1) a versioned machine-readable run artifact with
outputs, typed inputs, model/revision identity, time/integration settings, data
source identity, and diagnostics; (2) a separate reusable scenario artifact;
(3) `eval --save-run`, overlay, and compare commands whose behavioral diff is
at least as useful as Vensim's; and (4), later, an optional checkpoint carrying
all visible and hidden state (delays, samples, and PRNG position) for restart.
JSON/CSV can keep the public boundary inspectable; checkpoint state need not be
promised in M1.

Untracked (adjacent: #45 parallel eval, #897 case tables/sweeps).

#### 5.2 Sweeps, sensitivity, Monte Carlo

Vensim supports univariate, multivariate MC, Latin Hypercube, combinatorial
Latin Grid, and file-fed samples, with explicit distribution and seed control.
Graphcal domain constraints already declare ranges "for sweeping/sampling"
(verified in `types.rs` docs), but nothing consumes them. #897 (case tables) +

## 45 (parallel eval) are the deterministic half; distribution-driven MC also

needs §4.7. Confidence-fan plots need multi-series percentile bands (#896
covers band/heatmap kinds). A `Sensitivity2All` equivalent is cheap and
high-value once sweeps exist: perturb each param ±10% (use ±0.1 at zero),
respect its domain, and rank a selected output metric. Label it one-at-a-time
screening, not global sensitivity analysis.

### 5.3 Calibration & optimization

Vensim Pro's Powell/payoff/MCMC/Kalman stack is its deepest analytical moat.
Graphcal's stated direction — Python interop (#174, README vision; also #265
optimization-friendliness, #281 autodiff) — is the right call: expose
fast batch eval to scipy/PyMC rather than reimplementing an optimizer.
Prerequisites: stable machine-readable run I/O (§5.1), parameter metadata
(domain constraints exposed — done in eval API), cheap re-eval (#45, #360).
A native likelihood/payoff DSL and Kalman filtering should be explicit
non-goals for now.

#### 5.4 Reality-Check-style conformance testing

Graphcal `assert` covers the non-intervening check: typed/indexed Booleans
(`Bool[T]` checks every coordinate), tolerance forms, and `#[expected_fail]`.
A first useful milestone is named test scenarios containing param/external-input
overrides plus assertions, runnable in batch—the `graphcal test` direction in

## 656. Vensim's Table equilibrium test similarly maps to an assertion comparing

the initial and first simulated stock values once §4 lands.

That milestone is **not full Reality Check parity**. Vensim can replace an
endogenous variable's equation, force a false inequality true, apply timed
RC inputs with grace periods, pass the replacement through downstream
feedback, and run multiple interventions for one Boolean condition. Implementing
that safely needs a typed intervention target and an evaluation-plan
transformation, not merely `--set` on params. It also needs clear behavior when
an intervention creates an algebraic loop and a policy for retaining or
throwing away generated runs. Treat equation-level intervention as a separate,
later design after scenario tests.

Tracked: #656 (scenario/test runner). Untracked: typed endogenous intervention
and multi-run Constraint planning.

### 5.5 Output/export

Vensim exports runs through tab, CSV, XLS/XLSX, data-list, and related formats,
with variable lists, time range, and frequency controls. Graphcal has text/JSON
stdout only. CSV export of indexed nodes (long or wide), selected-node lists,
and output decimation are table stakes for the Excel-adjacent audience; this
ties into #353 and the run-artifact design.

### 6. Tier 3 gaps — interactive & structural UX

#### 6.1 SyntheSim and Reference Modes

The recompute loop already exists (LSP: 300 ms debounced whole-graph re-eval,
inlay values—a "live worksheet"). A browser live view with param sliders from
domain bounds plus auto-updating Vega-Lite plots would be a credible **first
slice**, and #352 (live view), #100 (`graphcal watch`), and #43 (WASM
playground) point in that direction.

Official-source review makes clear that full SyntheSim parity is wider:

1. behavior thumbnails on the model/dependency view;
2. continuous Lookup point editing;
3. temporary time-shaped override of an arbitrary endogenous variable, visibly
   cutting its feedback links;
4. sensitivity ensembles and range/histogram objects after a control change;
5. save/load of the resulting assumptions; and
6. Reference Mode drawing/import and overlay for expected or observed behavior.

These should be staged, not hidden under "add sliders." The WASM plugin/eval
architecture makes an in-browser engine plausible, but endogenous overrides
share the typed plan-transformation problem in §5.4.

Tracked: #352, #100, #43, #512. Untracked: lookup manipulation, endogenous
SyntheSim overrides, sensitivity controls, and Reference Mode authoring.

#### 6.2 Causal tracing & loop visibility

Causes/Uses trees and Causal Chains are dependency-graph walks;
`graphcal graph` (DOT) plus LSP references cover that static half crudely, and

## 512 (DAG visualizer) / #24 (`graphcal tree`) are the tracked shape. Current

Vensim also puts behavior thumbnails directly on an expandable causal tree.
Two SD-specific wrinkles remain: (1) behavioral tracing needs per-node
multi-run time series, cheap once runs are artifacts (§5.1); (2) **feedback
loops are invisible in Graphcal's DAG** because all SD feedback is buried in
closure state. A Loops tool must analyze data flow inside `unfold`/`integrate`
(or a future per-`(node,t)` graph, §4.1). Whichever named-flow mechanism lands
must keep flow→stock edges reconstructable for tooling. Vensim's own loop search
has a <32-variable limit, so practical bounded enumeration is parity enough.

### 6.3 Gaming, apps, publishing — reframe, don't copy

Vensim offers gaming, Venapps, packaged models/apps plus a free Reader, and a
10.5 web publisher whose templates run WASM models at SyntheSim-like speed with
packaged exogenous data. Graphcal equivalents should be: git repo = package;
`graphcal eval` = reader (FOSS, no license split); WASM playground (#43) / live
view (#352) = published interactive model. Gaming (step–pause–decide, including
restartable state and decision logs) is genuinely distinct; defer unless a
concrete user appears. A Molecules-style collection becomes a normal
`graphcal` package once §4.3 exists (#99, #154).

### 7. What NOT to copy (hold the line)

- **Implicit dimensionless & unit synonyms** — Vensim needs `Dmnl = Fraction`
  lists because units are strings. Keep dimensions structural.
- **Silent divide substitution or implicit lookup policy** — `ZIDZ` is silent;
  endpoint hold warns but is still selected implicitly. Keep normal arithmetic
  fallible and make any substitute/hold/extrapolate behavior explicit.
- **Order-sensitive shared RNG streams** — require a seed param and derive
  independent draws from typed site/coordinate identity (§4.7).
- **Binary run/dataset formats** (`.vdf`) — public runs should use inspectable,
  documented JSON/CSV (a separate opaque restart checkpoint can be optional).
- **Sketch-position records interleaved with equations** — Vensim 8.2's Git
  settings reduce noise, but Graphcal should keep any future layout in a
  separate file so semantic source remains independently mergeable.
- **Equation-attached-to-diagram as the only entry path** — text-first with
  rendered views, not diagram-first with generated text.

### 8. Suggested sequencing

**M1 — "port a textbook SD model"** (SIR, Bass diffusion, inventory
oscillation, World3-style stock webs):

1. §4.1 named flows / local bindings / coupled-state decision (#349, #889 + new
   design) — *first, everything composes with it*
2. `integrate` + Euler/RK4 and explicit stage-input semantics (#351)
3. Lookups with explicit range policy (#891)
4. SD function set: delays/smooths/test inputs (§4.3, new)
5. Runtime time-axis bounds + SAVEPER/save-list output control (§4.5, new)

**M2 — "believe the model"**: run + scenario artifacts and behavioral compare
(§5.1, new) → CSV/series data in & out (#353/#892/#355) → sweeps (#897/#45)
→ seeded RNG + MC + fan charts (§4.7/#896) → `graphcal test` scenarios (#656).
Checkpoint/resume and endogenous Reality Check interventions follow only if a
real workflow requires them.

**M3 — "estimate & optimize"**: Python bridge (#174) with batch eval, exposed
domain constraints, autodiff later (#281/#265).

**M4 — "show it live"**: live view with sliders (#352) + WASM playground (#43)

- DAG/loop visualization (#512/#24/§6.2), then Lookup editing, overrides,
sensitivity controls, and Reference Modes (§6.1).

Gaps with no tracking issue yet (candidates to file): local `let` bindings /
flow naming inside recurrences; per-`(node,t)` granularity decision; SD
function library; constrained stock semantics; RK stage-input policy; runtime
index bounds; SAVEPER/save lists; data-variable interpolation modes; run and
scenario artifacts + behavioral compare; checkpoint/resume; typed endogenous
interventions; seeded PRNG + distributions; CSV export; Reference Modes;
loop-aware tooling over closure bodies; `.mdl`/XMILE importer (worth a look as
an adoption funnel—PySD's parser is prior art).

### Appendix A — Vensim function families → Graphcal disposition

This is a family-level planning catalog, not an exhaustive transcription of the
official [100+ entry function
summary](https://www.vensim.com/documentation/22300.html).
Status: ✅ exists · 🟡 expressible as a pattern today · ❌ missing.
Path: `builtin` = add to the closed builtin set · `stdlib` = SD library (§4.3,
blocked on §4.1) · `CLI` = tooling boundary (§4.6/§5) · `plugin` = viable WASM
plugin today · `non-goal` / `divergent` = deliberate difference.

| Vensim | Semantics | Graphcal today | Path |
|---|---|---|---|
| **Stocks & init** | | | |
| `INTEG(rate, init)` | stock | 🟡 hand-Euler `unfold` | `integrate` #351 |
| `SINTEG(r,i,min,max,q,s,t)` | saturating/quantized/snap-to-special INTEG; explicitly non-conservative | 🟡 conditional `unfold` by hand | constrained integrator design (§4.2) |
| `INITIAL(expr)` | freeze expr at t₀ | 🟡 separate node feeding init | — (works) |
| `REINITIAL(expr)` | recompute INITIAL when resuming a run | ❌ | follows checkpoint/resume (§5.1) |
| `ACTIVE INITIAL(a, i)` | distinct active/init equations to break an initial loop | 🟡 only inside one packed recurrence; not across named nodes | §4.1; #888 only if a solve is required |
| `GAME(expr)` | gaming override hold | ❌ | non-goal (§6.3) |
| `A FUNCTION OF(...)` | placeholder RHS from sketch | ❌ | #505 `todo` marker |
| `==` unchangeable constant | non-overridable constant | ✅ `const node` | — |
| `[min,max,incr]` bounds | runtime warnings + slider ranges | ✅ domain constraints | divergent: error vs warn — needs a policy look |
| **Delays** | | | |
| `DELAY1`/`1I`, `DELAY3`/`3I`, `DELAY N` | quantity-conserving exponential material delays | ❌ | stdlib (hidden stocks) |
| `DELAY FIXED` | pure delay using initial delay time | ❌ | stdlib (history/ring-buffer state) |
| `DELAY MATERIAL` | discrete varying-time delay that conserves quantity | ❌ | stdlib, later |
| `DELAY INFORMATION` | discrete varying-time delay that preserves signal range | ❌ | stdlib, later |
| `DELAY CONVEYOR/BATCH/PROFILE`, `DELAYP` | ordered/leaky conveyor, batch, arbitrary-profile, pipeline transit | ❌ | defer |
| **Smoothing/trend** | | | |
| `SMOOTH(I)/3(I)/N` | exponential information smoothing | ❌ | stdlib |
| `TREND(x, at, it)` | fractional growth rate | ❌ | stdlib |
| `FORECAST(x, at, h)` | trend extrapolation | ❌ | stdlib |
| **Test inputs** | | | |
| `STEP`, `RAMP`, `PULSE`, `PULSE TRAIN` | `TIME STEP`-quantized forcing functions with defined boundary rules | 🟡 mathematical shapes via `if`; exact Vensim boundaries absent | stdlib + stage-input policy (§4.2) |
| **Sample/discrete** | | | |
| `SAMPLE IF TRUE(c, x, i)` | hold unless condition | 🟡 `unfold` state | stdlib |
| `SHIFT IF TRUE` | aging-chain shift | ❌ | stdlib after #889 |
| `INTEGER(x)` | truncate | ✅ `trunc` (dimensionless-only) | divergent (by design, #1039) |
| `QUANTUM(a, b)` | truncate toward zero to a multiple of `b` (`b<=0` returns `a`) | 🟡 `b*trunc(a/b)` for `b>0` | maybe builtin |
| `MODULO(x, m)` | remainder | ✅ `%` operator | — |
| **Lookups** | | | |
| lookup def + `name(x)` | piecewise-linear; endpoint hold + warning | ❌ | #891 |
| `WITH LOOKUP` | inline anonymous lookup | ❌ | #891 (literal form) |
| `LOOKUP EXTRAPOLATE/FORWARD/BACKWARD` | out-of-range / step policies | ❌ | #891 (explicit policy enum) |
| `LOOKUP INVERT/AREA/SLOPE`, `VECTOR LOOKUP` | inverse/integral/derivative | ❌ | later |
| **Logic & guards** | | | |
| `IF THEN ELSE(c, a, b)` | conditional | ✅ `if {} else {}` | — (typed, exhaustive) |
| `:AND: :OR: :NOT:` | logic on 0/1 | ✅ `&& \|\| !` on `Bool` | divergent: no short-circuit; distinct Bool type |
| `= <> < <= > >=` | comparisons return 1/0 | ✅ (return `Bool`) | — |
| `ZIDZ(a,b)` / `XIDZ(a,b,x)` | silent div-zero guards (\|b\|<1e-6) | 🟡 `if` guard | divergent: keep errors loud; sugar debatable |
| **Math** | | | |
| `ABS EXP LN SQRT` | — | ✅ `abs exp ln sqrt` | — (sqrt is dimension-aware) |
| `LOG(x, b)` | log base b | ✅ `log(x, base)` | — |
| `POWER` / `^` | power | ✅ `^` | divergent: dimensioned base needs exact rational exponent |
| `MIN(a,b)` / `MAX(a,b)` | binary min/max | ✅ `least`/`greatest` | — |
| `GAMMA LN` | ln Γ | ❌ | plugin |
| `SIN COS TAN` (radians) | trig | ✅ `sin cos tan` on `Angle` | — (stronger typing) |
| `ARCSIN/ARCCOS/ARCTAN` | inverse trig | ✅ `asin acos atan` (+`atan2`, which Vensim lacks) | — |
| `SINH COSH TANH` | hyperbolic | ✅ + inverses | — |
| **Random** | | | |
| `RANDOM UNIFORM/NORMAL/POISSON/EXPONENTIAL/BETA/BINOMIAL/GAMMA/NEG BINOMIAL/TRIANGULAR/WEIBULL` | bounded/truncated distribution family | ❌ | §4.7 seeded-pure design |
| `RANDOM LOOKUP`, `RANDOM PINK NOISE` | arbitrary PDF; autocorrelated noise | ❌ | later |
| stream ID, `NOISE SEED`, `NOISE RNG` | reproducible shared streams; seed and PRNG selection | ❌ | explicit seed + typed counter identity (§4.7) |
| **Vector/array** | | | |
| `SUM VMIN VMAX` over `d!` | reductions (multi-axis allowed) | ✅ `sum minimum maximum` rank-1; ❌ multi-axis | extend aggregation (D021) |
| `PROD` | product reduction | ❌ | builtin |
| `ELMCOUNT` | axis cardinality | ✅ `count` | — |
| `VECTOR SELECT` | masked reduction | 🟡 `for`+`if`+`sum` | maybe stdlib |
| `VECTOR ELM MAP` | positional element access | ❌ (labels aren't values; `Fin(N)` partial) | design needed |
| `VECTOR SORT ORDER/RANK/REORDER` | sorting/permutation | ❌ | builtin/stdlib |
| `INVERT MATRIX` | matrix inverse | ❌ | #890 |
| **Solvers** | | | |
| `FIND ZERO`, `SIMULTANEOUS` | nonlinear/fixpoint solve | ❌ | #888 |
| **Allocation** | | | |
| `ALLOCATE AVAILABLE/BY PRIORITY`, `ALLOC P`, `MARKETP`, `DEMAND/SUPPLY AT PRICE`, `FIND MARKET PRICE` | scarce-resource & market clearing | ❌ | defer (niche) |
| **Financial** | | | |
| `NPV`, `NPVE` | running discounted value | 🟡 `unfold`/`scan` | stdlib |
| `INTERNAL RATE OF RETURN`, `DEPRECIATE *` | IRR; depreciation streams | ❌ | stdlib/plugin, later |
| **Queues** | | | |
| `QUEUE FIFO (ATTRIB)`, `QUEUE AGE/ATTRIB *` | FIFO cohorts + age stats | ❌ | defer (discrete-event tier) |
| **Data-side** | | | |
| Data variables + `:INTERPOLATE:/:RAW:/:HOLD BACKWARD:/:LOOK FORWARD:` | exogenous series w/ alignment policy | ❌ | §4.6 |
| `:=` data equations, `CUMULATE(F)`, `TIME SHIFT`, `X IF MISSING` | pre-sim series transforms | 🟡 ordinary nodes (no phase distinction needed) | §4.6 |
| `GET DATA AT TIME/BETWEEN TIMES/MEAN/STDV/MIN/MAX/FIRST/LAST/TOTAL POINTS` | series statistics/access | ❌ | follows §4.6 + aggregations |
| `:NA:` | recognized numeric sentinel omitted in plots/rendered `—` in tables | ❌ (by design: no magic number) | model missing data as an ADT (`Option`-style) at the data boundary |
| `GET TIME VALUE`, `TIME BASE` | calendar access; alternate axes | 🟡 `Datetime` builtins (`year`…`day_of_year`, scale conversions) | calendar time axes = open design |
| **External readers** | | | |
| `GET XLS/DIRECT/123/VDF DATA/CONSTANTS/LOOKUPS/SUBSCRIPT` | spreadsheet/dataset ingestion in-language | ❌ | CLI boundary (#353, #892); keep the core pure |
| `TABBED ARRAY` | inline array literal | ✅ `table` (typed, total) | — (stronger) |
| **Reality Check** | | | |
| `:TEST INPUT:`, `:THE CONDITION:`/`:IMPLIES:`, `RC *` | passive checks + equation-level forced conformance runs | 🟡 `assert` covers checking; param scenarios cover only a subset | #656 first; typed intervention later (§5.4) |
| **Misc** | | | |
| `MESSAGE` | runtime warnings/prompts | 🟡 asserts + per-node errors | — |
| `:MACRO:` | user components w/ internal stocks | 🟡 `dag` blocks (no stateful-over-axis story yet) | §4.1 + #99 |
| String type | file names/labels | ❌ | #149 |
| External function DLLs | native extension (DSS) | ✅ WASM plugins | — (stronger: sandboxed, pinned, pure) |

### Appendix B — Sources and research method

#### B.1 Source discipline

The 2026-07-31 audit read/extracted the pages below from **Ventana's official
`vensim.com` site**; site-restricted web search was used to discover URLs and
locate relevant passages. Every Vensim semantic/product citation now points to
an official page. This replaces the earlier secondary-source caveat and removes
the unsupported Vengine, `~3×` compilation, "since v8" SyntheSim sensitivity,
XMILE-export, merge-quote, and Reality-Check-tier claims.

The official help is a living manual: old and new UI pages coexist, some pages
retain historic edition names, and the current comparison chart conflicts with
10.4–10.5 release notes on web-publishing entitlement. Semantic claims were
preferred from the dedicated reference page; release claims from release notes;
edition claims from a function's Availability line or the comparison chart,
with discrepancies stated in the text.

Third-party names in §2.9 are ecosystem context only. PySD, SDEverywhere,
simlin, forum posts, and community inventories were **not** used to establish
Vensim language or product semantics in this revision.

#### B.2 Official Vensim sources

| Topic | Official sources read |
|---|---|
| Current release & editions | [Vensim 10.5 announcement](https://vensim.com/2026/06/vensim-ventity-news-june-2026/); [10.4.X–10.5.X notes](https://www.vensim.com/documentation/vensim-10_4_x.html); [configuration comparison](https://vensim.com/comparison-chart-for-vensim-configurations/) |
| Equation/type model | [Equations](https://www.vensim.com/documentation/ref_equations.html); [Equation Components](https://www.vensim.com/documentation/22025.html); [Variable Types](https://www.vensim.com/documentation/ref_variable_types.html); [Computational Sequence](https://www.vensim.com/documentation/computationalsequence.html); [ACTIVE INITIAL](https://www.vensim.com/documentation/fn_active_initial.html); [FIND ZERO](https://www.vensim.com/documentation/fn_find_zero.html); [SIMULTANEOUS](https://www.vensim.com/documentation/fn_simultaneous.html) |
| Time, integration & numerics | [Simulation Control Parameters](https://www.vensim.com/documentation/ref_sim_control_params.html); [Integration](https://www.vensim.com/documentation/integration.html); [Runge–Kutta](https://www.vensim.com/documentation/rungekutta.html); [TIME STEP](https://www.vensim.com/documentation/ref_time_step.html); [Simulation Errors](https://www.vensim.com/documentation/ref_sim_errors.html); [SINTEG](https://www.vensim.com/documentation/fn_sinteg.html); [Vensim 8 double precision](https://www.vensim.com/documentation/vensim_8_0_-_june_2019.html) |
| Functions, delays & lookups | [Function Overview](https://www.vensim.com/documentation/ref_functions.html); [Summary List](https://www.vensim.com/documentation/22300.html); [Material and Information Delays](https://www.vensim.com/documentation/mgu09_material_and_information_delays.html); [Using Lookups](https://www.vensim.com/documentation/22820.html); [WITH LOOKUP](https://www.vensim.com/documentation/fn_with_lookup.html) |
| Test inputs & randomness | [STEP](https://www.vensim.com/documentation/fn_step.html); [RAMP](https://www.vensim.com/documentation/fn_ramp.html); [PULSE](https://www.vensim.com/documentation/fn_pulse.html); [RANDOM family](https://www.vensim.com/documentation/fn_random.html); [Stream ID](https://www.vensim.com/documentation/stream_id.html); [NOISE SEED](https://www.vensim.com/documentation/noise_seed.html); [NOISE RNG](https://www.vensim.com/documentation/noise_rng.html) |
| Units, arrays & reuse | [Units Check](https://www.vensim.com/documentation/ref_units_check.html); [Units Equivalents/strict mode](https://www.vensim.com/documentation/ref_units_equiv.html); [Subscripts](https://www.vensim.com/documentation/ref_subscripts.html); [Subranges](https://www.vensim.com/documentation/22100.html); [Mappings](https://www.vensim.com/documentation/ref_subscript_mapping.html); [Macros](https://www.vensim.com/documentation/macros.html) |
| Data & run I/O | [Preparing/Using/Exporting Data](https://www.vensim.com/documentation/ref_data.html); [Data Equations](https://www.vensim.com/documentation/dataequations.html); [`:NA:`](https://www.vensim.com/documentation/na.html); [GET DIRECT DATA](https://www.vensim.com/documentation/fn_get_direct_data.html); [Dataset Export](https://www.vensim.com/documentation/data_export.html) |
| Analysis tools | [Tree Diagram](https://www.vensim.com/documentation/treetool.html); [Loops](https://www.vensim.com/documentation/loops_tool.html); [Causal Chain](https://www.vensim.com/documentation/causal-chain.html); [Causal Table](https://www.vensim.com/documentation/causal-table.html); [Document](https://www.vensim.com/documentation/documenttool.html); [Table](https://www.vensim.com/documentation/23865.html); [Stats](https://www.vensim.com/documentation/ref_stats.html); [Runs Compare](https://www.vensim.com/documentation/runscompare.html); [Payoff Reporting](https://www.vensim.com/documentation/extended_payoff_reporting.html) |
| Interactive/scenario UX | [SyntheSim](https://www.vensim.com/documentation/synthesim.html); [SyntheSim + Sensitivity](https://www.vensim.com/documentation/synthesim__sensitivity_analysi.html); [Lookup editing](https://www.vensim.com/documentation/ref11_changing_lookups.html); [Behavior Overrides](https://www.vensim.com/documentation/ref__changing_behavior.html); [Gaming](https://www.vensim.com/documentation/ref_gaming.html); [Datasets](https://www.vensim.com/documentation/datasets.html); [Changes/Based On/Resume](https://www.vensim.com/documentation/23320.html); [Save Lists](https://www.vensim.com/documentation/ref_savelist.html); [Run Configuration Manager](https://www.vensim.com/documentation/run-configuration-tool.html); [Reference Modes](https://www.vensim.com/documentation/usr20.html) |
| Reality Check | [Overview](https://www.vensim.com/documentation/usr14.html); [Test Inputs](https://www.vensim.com/documentation/20965.html); [Constraints](https://www.vensim.com/documentation/20970.html); [Active Checking](https://www.vensim.com/documentation/20980.html); [Simulation/Restructuring](https://www.vensim.com/documentation/20975.html); [Running Reality Check](https://www.vensim.com/documentation/21005.html) |
| Sensitivity & estimation | [Sensitivity](https://www.vensim.com/documentation/sensitivity.html); [Control Format](https://www.vensim.com/documentation/sensitivitycontrol.html); [Sensitivity2All](https://www.vensim.com/documentation/sensitivity2all.html); [Optimization](https://www.vensim.com/documentation/ref_optimization.html); [Payoffs](https://www.vensim.com/documentation/ref_payoff.html); [MCMC/SA](https://www.vensim.com/documentation/mcmc_sa.html); [Kalman](https://www.vensim.com/documentation/ref_kalman.html); [Stochastic Optimization](https://www.vensim.com/documentation/stochastic_optimization.html); [Parallel Simulation](https://www.vensim.com/documentation/parallel-simulation.html) |
| Automation & publishing | [DSS Supplement](https://www.vensim.com/documentation/dss_supplement.html); [Command Scripts](https://www.vensim.com/documentation/dss_command.html); [Command-script examples/limits](https://www.vensim.com/documentation/25710.html); [DLL](https://www.vensim.com/documentation/dss_dll.html); [Compiled Simulations](https://www.vensim.com/documentation/dss_compiled.html); [Web Publishing](https://www.vensim.com/documentation/publishing-a-model-to-the-inte.html); [Web limitations](https://www.vensim.com/documentation/limitations.html) |
| Files, Git, interchange & library | [`.mdl` structure](https://www.vensim.com/documentation/_mdl_model_files.html); [Sketch records](https://www.vensim.com/documentation/ref_sketch_format.html); [SVN/Git guidance](https://www.vensim.com/documentation/svn_git.html); [File-format controls](https://www.vensim.com/documentation/file-format.html); [7.2 XMILE import](https://www.vensim.com/documentation/vensim_7_2_-_november_2017.html); [current supported commands](https://www.vensim.com/documentation/ref_cmd_supported.html); [Molecules](https://www.vensim.com/documentation/mgu10_getting_and_using_the_molecules.html) |

#### B.3 Graphcal sources

Graphcal facts use this repository at `bdb597a`: `grammar.ebnf`,
`docs/language/*`, `crates/graphcal-compiler/src/builtin.rs`,
`registry/builtins.rs`, `tir/dim_check/infer/hir.rs`,
`crates/graphcal-eval/src/{exec_plan.rs,eval_expr/hir_eval.rs,host_fns.rs}`,
`crates/graphcal-cli/src/{main.rs,overrides.rs,json_input.rs,plot.rs}`,
`crates/graphcal-plugin-host/src/host.rs`, `tests/fixtures/`, the issue tracker,
and the removed design doc `design/11-system-dynamics.md` (recoverable from
revision `d9236b35^`).
