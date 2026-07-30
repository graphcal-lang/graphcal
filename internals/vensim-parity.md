# Vensim Parity Gap Analysis

> [!WARNING]
> This research note was written by an AI agent (2026-07-30, repo state
> `bdb597a` / v0.0.1-alpha.17) and must be verified by a human developer before
> being relied on for planning. Vensim facts were gathered from Vensim 10.4-era
> documentation and secondary sources; items marked *(unverified)* could not be
> pinned to an official source.

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

Condensed from Vensim 10.4-era documentation. PLE/PLE+/Pro/DSS = edition tiers.

### 2.1 Modeling language

- **Everything is an equation.** A model is a flat, order-independent set of
  elements `name = expr ~ units [min,max,increment] ~ comment |`. Units and
  free-text documentation are per-equation fields; `[min,max]` bounds produce
  runtime *warnings* (and SyntheSim slider ranges), never clamps or errors.
  Vensim sorts everything by causal dependency.
- **Variable types are inferred from the defining operator/function**: Level
  (`INTEG(rate, init)`), Auxiliary, Constant (`=` number; `==` = unchangeable),
  Data (`:=` or dataset-supplied), Initial (`INITIAL(expr)`), Lookup (table),
  Subscript Range, Gaming (`GAME`), String, Time Base, Reality Check
  constraint/test input.
- **Stocks & initialization**: `INTEG(rate, initial)` where `initial` is an
  arbitrary expression; initial values are resolved by recursively chasing
  dependencies at t₀, with `ACTIVE INITIAL(active, init)` to break
  simultaneous initial-value loops (equilibrium initialization) and
  `FIND ZERO`/`SIMULTANEOUS` for genuine algebraic systems.
- **Time control**: `INITIAL TIME`, `FINAL TIME`, `TIME STEP`, `SAVEPER` are
  *ordinary model equations* (conventionally a `.Control` group) plus the
  builtin `Time`; all carry the model's time units. Integration method is
  chosen per-run: Euler, Diff (difference-equation mode), RK2/RK4 fixed,
  RK2/RK4 auto (adaptive between save points, per-Level absolute/relative
  tolerances). Docs steer discrete/random models back to Euler.
- **~150 built-in functions** — the SD vocabulary; full disposition table in
  Appendix A. Highlights: delay family (1st/3rd/nth-order, fixed pipeline,
  conserving material delay under varying delay times, conveyor, batch),
  smoothing (`SMOOTH`/`3`/`N`), `TREND`/`FORECAST`, test inputs
  (`STEP`/`RAMP`/`PULSE`/`PULSE TRAIN`), sample-and-hold (`SAMPLE IF TRUE`),
  lookups with six access variants, `IF THEN ELSE` + `ZIDZ`/`XIDZ` divide
  guards, 12 truncated random distributions with per-site stream IDs seeded by
  a `NOISE SEED` model constant, vector ops (`SUM`/`PROD`/`VMIN`/`VMAX` over
  `!`-marked axes, sort/rank/reorder, matrix inverse), allocation & market
  clearing, financial (`NPV`/`IRR`/depreciation), FIFO queues with age
  statistics, data accessors, spreadsheet readers (`GET XLS/DIRECT *`), and
  `MESSAGE` runtime diagnostics.
- **Subscripts**: up to 8 dimensions; element ranges (`(A1-A10)` runs),
  subranges, positional mappings (`->`), family copies (`<->`), `:EXCEPT:`
  exception equations, multiple definitions over disjoint subranges, constant
  value tables, subscripted lookups, `!` axis marking for reductions.
- **Units**: per-equation strings compared by name with user-maintained
  synonym lists; numeric literals are unit *wildcards* by default (an optional
  "strictest testing" mode disables the leniency); failures warn, never block
  simulation.
- **Macros** (`:MACRO:` … `:END OF MACRO:`, Pro/DSS): user-defined reusable
  components that may contain stocks — this is how custom delay/smooth gadgets
  are built; no internal subscript definitions, no recursion.
- **Data variables**: dataset-fed exogenous series with per-variable
  interpolation modes (`:INTERPOLATE:` default, `:RAW:`, `:HOLD BACKWARD:`,
  `:LOOK FORWARD:`), `:NA:` missing values, and pre-simulation `:=` data
  equations (`CUMULATE`, `TIME SHIFT`, `X IF MISSING`).
- **Numerics**: double precision everywhere since v8 (2019); division by zero
  and domain errors abort the run with variable+time; `ZIDZ`/`XIDZ`
  (|denom| < 1e-6 → 0/alternate) are the idiomatic *silent* guards; lookups
  clamp+warn out of range; `:NA:` propagates as missing.

### 2.2 Structural analysis tools

- **Causal Tracing** (trademarked): Causes Tree / Uses Tree / Causal Chain —
  click any variable, walk the influence chain across views.
- **Loops tool** — enumerate every feedback loop through a variable.
- **Document tool** — every equation with units, definition, usage (interactive
  since 10.4); **Causal Table** (10.1); **Table tool** with built-in
  equilibrium testing (flags Levels changing from initial time within
  tolerance).
- **Units check** (Ctrl+U) — name-equality with user-managed synonym lists
  (`Dimensionless = Dmnl = Fraction = 1`); `* / ( )` only.
- **Model check** (Ctrl+T) — syntax, undefined/unused variables, subscript
  inconsistencies, simultaneous-equation (algebraic loop) detection.
- **Runs Compare** — diff all Constants/Lookups between two runs.
- **Stats tool** — model-vs-data fit: RMSE-family stats, Theil inequality
  decomposition (bias/variance/covariance).

### 2.3 Simulation UX

- **SyntheSim** — the flagship: every Constant grows a slider, every dynamic
  variable shows a mini behavior graph on the diagram, dragging a slider
  re-simulates and redraws continuously. Since v8, Monte Carlo confidence bands
  render live behind the graphs.
- **Gaming mode** — advance the run in user-controlled intervals, changing
  `GAME`-declared variables at each pause; step backward; save/reload decision
  files. Basis of "management flight simulators".
- **Runs as datasets** — every simulation writes a named `.vdf/.vdfx` dataset;
  up to 8 loaded at once; *every* analysis tool (graphs, tables, trees, stats)
  automatically overlays all loaded datasets; datasets need not come from the
  same model (model-vs-model, model-vs-data).
- **Custom graphs / strip graphs / tables**, export to spreadsheet/CSV/SVG.

### 2.4 Reality Check

Automated conformance testing: constraint equations (`:THE CONDITION:` /
`:IMPLIES:`) plus `:TEST INPUT:` equations defining the manipulation space.
Vensim forces each condition to hold (overriding model values, running as many
simulations as needed) and verifies the implication; "Test All" is a regression
suite for model validity.

### 2.5 Estimation & optimization (Pro/DSS)

- Modified **Powell** search over bounded parameters (`.voc` control file).
- **Payoffs** (`.vpd`): calibration (weighted least squares / log-likelihood
  against data) and policy (maximize performance measures); mixable (priors,
  penalties).
- **Confidence bounds** via likelihood profiling; **MCMC** (credible
  intervals, burn-in); **simulated annealing**; **extended Kalman filtering**
  (state adjustment from data during simulation); **stochastic optimization**
  (expected payoff over noise ensembles); multi-start; multicore since v10.

### 2.6 Sensitivity (PLE+ and up)

Monte Carlo / Latin Hypercube / Latin Grid / file-driven designs; per-parameter
distributions (uniform, normal, triangular, Weibull, ...); seed control;
confidence-fan graphs and end-time histograms; `Sensitivity2All` automatic
±10% screening of *every* constant ranked by influence (10.x); results
exportable.

### 2.7 Data connectivity

- Converters to datasets: `CSV2VDF`, `TAB2VDF`, `XLS2VDF`, `DAT2VDF`, ...
- `GET XLS *` (live Excel link) and `GET DIRECT *` (file read) families for
  CONSTANTS / DATA / LOOKUPS / SUBSCRIPT at load/run time; ODBC in DSS.
- Data variables carry per-variable interpolation modes; graphing a variable
  from a data dataset and a simulation dataset together is the standard
  model-vs-history workflow.
- Export any run to tab/CSV/spreadsheet (`VDF2TAB`).

### 2.8 Automation, embedding, publishing (DSS)

- Command scripts (`.cmd` Venscripts, no control flow); **Vensim DLL**
  (VB/C/Excel/Java bindings, multicontext server variant); **compiled
  simulations** (model → C → DLL, ~3× speedup); **external function DLLs**;
  **Vengine** console engine for heavy batch optimization *(unverified
  details)*.
- **Venapp** — screen-based flight-simulator apps (sliders, buttons, graphs).
- **Publishing** — `.vpm/.vpmx` packaged models, `.vpa` packaged apps, free
  **Model Reader**; **Publish Web Files** compiles the model via Emscripten to
  WASM with a SyntheSim-style browser UI (data variables since 10.3).

### 2.9 Ecosystem

`.mdl` is documented plain text (equations diff well; the embedded sketch
section merges terribly — acknowledged by Ventana). Vensim reads XMILE/Stella
and exports via `MDL2XMILE`. Third-party: **PySD** (mdl→Python),
**SDEverywhere** (mdl→JS/C/WASM, powers En-ROADS/C-ROADS), SDM-Doc, venpy,
readsdr, stanify. **Molecules of Structure** — curated library of reusable SD
building blocks, browsable in-app.

## 3. Where Graphcal already stands

### 3.1 Concept mapping

| Vensim | Graphcal today | Status |
|---|---|---|
| Auxiliary variable | `node` | ✅ |
| Constant | `param` / `const node` | ✅ (richer: required vs default, domain constraints) |
| Model + views | files, modules, `dag` blocks | ✅ different shape, comparable power |
| `INTEG(rate, init)` (Level/stock) | `unfold(Axis, init, \|prev, t0, t1\| prev + rate*(t1-t0))` | 🟡 expressible, Euler only, by hand |
| Flow (Rate) | expression inside the `unfold` body | 🟡 cannot be a *named* equation participating in feedback (§4.1) |
| TIME STEP / INITIAL/FINAL TIME | `index T = range(t0, t1, step: dt)` | 🟡 typed & exact, but compile-time only (§4.5) |
| TIME | loop coordinate `t` in `for`/`unfold` | ✅ |
| SAVEPER | — | ❌ every step materialized in output |
| Integration methods (Euler/RK2/RK4/diff) | hand-written Euler | ❌ `integrate` proposed (#351) |
| Subscripts | `index` + `T[I]`, tables, `for` | ✅ core; ❌ subranges, mappings, `:EXCEPT:` (§4.8) |
| Lookup / graphical function | — | ❌ (#891) |
| DELAY1/3/N, SMOOTH*, TREND, ... | — | ❌ (§4.3) |
| STEP / PULSE / RAMP | `if`/`else` over `t` by hand | 🟡 sugar missing |
| RANDOM * / NOISE SEED | — | ❌ deterministic by design; needs seeded design (§4.7) |
| IF THEN ELSE | `if cond { } else { }` | ✅ (exhaustive `else` required) |
| ZIDZ / XIDZ | `if` guard | 🟡 divergent philosophy: error-by-default is *better*, sugar debatable |
| Units check | dimensions as types, rational exponents | ✅ **stronger** (see 3.2) |
| Data variables (exogenous series) | `--input` JSON / literal tables | ❌ no series binding, no interpolation modes (§4.6) |
| Datasets / run compare | — | ❌ single-shot eval (§5.1) |
| Sensitivity / optimization / MCMC / Kalman | — | ❌ (§5.2, §5.3) |
| Reality Check | `assert` (boolean, tolerance, indexed) | 🟡 checks exist; forcing/test-input machinery missing (§5.4) |
| Causes/Uses trees, loops | `graphcal graph` (DOT, experimental), LSP references | 🟡 (§6.2) |
| SyntheSim | LSP inlay values, 300 ms debounced re-eval | 🟡 recompute loop exists; no sliders, no in-editor charts (§6.1) |
| Gaming, Venapp, Reader, publish-to-web | — | ❌ different strategy (§6.3) |
| Document tool | `graphcal eval`/`check` output, LSP hover | 🟡 |
| Molecules library | — | ❌ becomes trivial once a stdlib exists: packages are git deps |
| .mdl text format | `.gcl` text | ✅ **stronger** (no embedded sketch blob, real lockfile) |

### 3.2 Where Graphcal is already stronger

Worth stating because it defines the pitch — a Vensim replacement that keeps
these properties is *better*, not just equivalent:

- **Dimensional safety.** Vensim units checking is string-name equality with
  user-maintained synonym lists, `* / ( )` expressions only, and warnings that
  teams routinely silence. Graphcal dimensions are compile-time types with
  rational exponents; `Velocity ≡ Length/Time` structurally. There is no
  "synonym drift".
- **No silent numerics.** Vensim's `ZIDZ` returns 0 on division by zero and
  lookups clamp at table ends by default — both silent-error factories.
  Graphcal surfaces NaN/inf/div-zero as per-node errors with fault isolation.
- **No broadcasting.** Vensim subscript equations broadcast implicitly;
  Graphcal requires explicit `for`, killing the NumPy/Excel bug class.
- **Version control.** `.mdl`'s sketch section makes merges "almost
  impossible" (Ventana's words); `.gcl` is pure text, plus an exact-rev
  lockfile — Vensim has no dependency story at all.
- **Typed data model.** Algebraic types, exhaustive `match`, `Fin(N)`,
  phantom-typed frames — Vensim has scalars and subscripts.
- **Assertions in-language** vs Vensim's separate Reality Check product tier.
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

A related, smaller gap: **initialization structure**. Vensim initial values
are arbitrary expressions resolved by recursive chasing at t₀, with
`ACTIVE INITIAL` for equilibrium initialization ("start the stock where
inflow = outflow, computed from the very parameters being simulated") and
`FIND ZERO`/`SIMULTANEOUS` for true algebraic systems. Graphcal `unfold` init
expressions can reference any non-cyclic node — which covers the common cases —
but equilibrium initialization *of a self-referential kind* and algebraic
constraints need the bounded solve/fixpoint primitive already proposed in
#888.

Tracked: #889 (indexed state in unfold/scan), #349 (closure body syntax),
#888 (bounded solve/fixpoint — covers `FIND ZERO`/`SIMULTANEOUS`/equilibrium
init). Untracked: `let`/local bindings; per-`(node, t)` dependency granularity
decision.

### 4.2 Stocks and integration quality

`unfold` is hand-written explicit Euler. Vensim offers six per-run methods —
Euler, Diff (difference-equation mode), RK2/RK4 fixed, RK2/RK4 auto (adaptive
step between save points, controlled by per-Level `ABSOLUTE TOLERANCE` /
`RELATIVE TOLERANCE`) — and its docs steer discrete/stochastic models back to
Euler. RK4 matters for oscillatory models (predator-prey, supply-chain
oscillation) where Euler at practical `dt` visibly distorts amplitude. The
settled proposal is `integrate(init, method: RK4, |state, t| derivatives)`
with the compiler checking `derivative_dim * time_dim == state_dim` (#351).
Notes:

- RK stages sample flows *between* grid points, so exogenous inputs must be
  functions of `t` (closures/lookups), not per-step arrays — another reason
  §4.1 and §4.4 precede this.
- Vensim's non-negative stock idiom (`MAX(0, ...)` outflow guards) maps to
  domain constraints — but Graphcal constraints *error* rather than clamp.
  Clamping semantics for stocks (a modeling choice, not an error) deserve an
  explicit design, e.g. a `clamp`ed integrator variant.

Tracked: #351. Untracked: stock non-negativity/saturation semantics.

### 4.3 The SD function library (delays, smooths, test inputs)

The working vocabulary of every Vensim model (full disposition in Appendix A):

- **Material delays** — `DELAY1[I]`, `DELAY3[I]`, `DELAY N` (each internally
  1..N cascaded stocks), `DELAY FIXED` (pipeline, needs a history ring
  buffer), and the discrete conserving family (`DELAY MATERIAL`,
  `DELAY INFORMATION`, `DELAY CONVEYOR`, `DELAY BATCH`, `DELAY PROFILE`).
- **Information smoothing** — `SMOOTH[I]`, `SMOOTH3[I]`, `SMOOTH N`, `TREND`
  (fractional growth rate vs. an internal exponential average), `FORECAST`.
- **Test inputs** — `STEP(h, t)`, `RAMP(s, t0, t1)`, `PULSE(t, w)`,
  `PULSE TRAIN(...)`; sine forcing is written from `SIN` directly.
- **Sample/hold & discrete** — `SAMPLE IF TRUE` (hold-unless), `INITIAL`,
  `ACTIVE INITIAL`, `QUANTUM`, `MODULO`, `INTEGER`, `SHIFT IF TRUE` (aging
  chains).
- **Financial accumulators** — `NPV`, `NPVE`, `INTERNAL RATE OF RETURN`,
  `DEPRECIATE *` (each internally a stock).
- **Later tier (defer)** — FIFO queues with age/attribute statistics
  (`QUEUE FIFO`, `QUEUE AGE *`), allocation & market clearing
  (`ALLOCATE AVAILABLE`, priority profiles, `FIND MARKET PRICE`).

The core set (delays, smooths, test inputs, sample/hold, NPV) is small —
roughly 20 functions covers the overwhelming majority of published models.

None exist in Graphcal (`builtin.rs` has 64 functions: math, aggregation,
conversions, datetime — nothing temporal). Two viable shapes:

1. **Stdlib dags/macros over a time axis** — honest (delays *are* stocks), but
   blocked on §4.1 ergonomics (a delay must nest inside a user recurrence) and
   on generic-over-axis dag instantiation.
2. **Builtins/intrinsics on unfold/integrate bodies** — special forms the
   compiler lowers to extra hidden state, the way Vensim macros expand to
   levels.

WASM plugins **cannot** host these: plugins are pure whole-array transforms;
a delay inside a feedback loop needs state interleaved with the caller's
stepping (verified against the plugin ABI: no retained buffers, result arrays
only over input-bound index variables).

Tracked: #99 (stdlib, generic). Untracked: SD function library specifically.

### 4.4 Lookup tables (graphical functions)

Vensim lookups are ubiquitous (nonlinear effects: "effect of X on Y"): named
piecewise-linear tables with definition-time x/y display ranges, inline
`WITH LOOKUP`, clamp-at-ends-with-warning default, and access variants
(`LOOKUP EXTRAPOLATE`, `LOOKUP FORWARD`/`BACKWARD` step modes,
`LOOKUP INVERT`, `LOOKUP AREA`, `LOOKUP SLOPE`); lookups can be subscripted
(one curve per element) and filled from spreadsheets. Graphcal has nothing
(`table` is map-literal sugar, not interpolation; verified no
`interp`/`lookup` anywhere). #891 proposes `interp(xs, ys, x)` + a
breakpoint-table type. Parity needs:

- dimensioned x and y axes (typed better than Vensim's);
- explicit out-of-range policy (clamp / extrapolate / error) — Graphcal should
  make Vensim's silent clamp *explicit*;
- a literal syntax pleasant enough for 20-point curves (the `table` layout is
  a good start);
- usable inside `unfold`/`integrate` bodies (again §4.1).

Tracked: #891.

### 4.5 Time-axis control at run time

Vensim's `FINAL TIME`, `TIME STEP`, `INITIAL TIME`, `SAVEPER` are *ordinary
model equations* (`FINAL TIME = INITIAL TIME + 100` is legal) — changeable
per-run from the UI, changes files, or scripts. Graphcal coordinate-index bounds
must be **compile-time constants** (runtime values rejected; verified in
`ir/lower.rs`); `--set` cannot touch an index; `pub(bind) index` moves the
choice to another *file*, still compile time. Consequences: no "run to 2100
instead of 2050" without editing source; no dt-halving convergence check from
the CLI; blocks sweep tooling (§5.2) from varying horizons. Also missing:
`SAVEPER` (output thinning — a 100k-step run currently prints/plots 100k
points; only the 1M cardinality cap bounds it).

Untracked: runtime-bound coordinate indexes (or CLI index overrides); SAVEPER
analogue (output decimation policy).

### 4.6 Exogenous time series (data variables)

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

### 4.7 Stochastics with reproducibility

Vensim: 12 truncated distributions (`RANDOM UNIFORM/NORMAL/POISSON/BETA/
BINOMIAL/GAMMA/WEIBULL/...`, `RANDOM LOOKUP` for arbitrary densities,
`RANDOM PINK NOISE` for autocorrelated noise) drawn every TIME STEP. Each call
site names a *stream ID*; stream 0 is seeded by the `NOISE SEED` **model
constant** (so the seed already lives in the model — Vensim modelers vary it
per sensitivity run), and the generator is selectable (`NOISE RNG`, default
SIMD Mersenne Twister). Graphcal is deterministic **by design** (plugin ABI
bans entropy imports; pinned tzdb; bit-identical goal). The parity path that
preserves the philosophy: *seeded, pure* PRNG — e.g. a splittable
counter-based generator (Philox-style) where the seed is an explicit `param`
and each draw site/step derives its stream deterministically (site path ×
coordinate → counter). That keeps runs reproducible, diffable, and
platform-identical — strictly cleaner than Vensim's stream-ID convention,
which it structurally resembles. Distribution sampling then enables §5.2
Monte Carlo.

Untracked entirely.

### 4.8 Subscript conveniences

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
#890 (linear algebra). Untracked: subranges/mappings, multi-axis aggregation
(has an error-message placeholder only).

## 5. Tier 1–2 gaps — the analysis workflow

Everything here is CLI/tooling layered on Tier 0; none of it needs language
changes except where noted.

### 5.1 Runs as artifacts; comparison

Vensim's every-run-is-a-dataset model (8 loaded at once, every tool overlays
them, `Runs Compare` diffs assumptions) has no counterpart: `graphcal eval` is
single-shot, plots see exactly one evaluation, and comparing scenarios means
`include`-ing a dag twice in source. Needed: a run artifact (JSON is fine —
text, diffable), `eval --save-run name` / `--compare a b`, plot overlay of
runs, and an assumptions-diff (params are typed, so this beats Vensim's).
Untracked (adjacent: #45 parallel eval, #897 sweeps).

### 5.2 Sweeps, sensitivity, Monte Carlo

Vensim: Latin Hypercube / MC over per-parameter distributions, seeds,
confidence-fan graphs, `Sensitivity2All` screening. Graphcal: domain
constraints already declare ranges "for sweeping/sampling" (verified in
`types.rs` docs) but nothing consumes them. #897 (case tables) + #45
(parallel eval) are the deterministic half; distribution-driven MC additionally
needs §4.7. Confidence-fan plots need multi-series percentile bands (#896
covers band/heatmap plot kinds). A `Sensitivity2All`-equivalent (±10% every
param, rank by effect) is cheap and high-value once sweeps exist.

### 5.3 Calibration & optimization

Vensim Pro's Powell/payoff/MCMC/Kalman stack is its deepest analytical moat.
Graphcal's stated direction — Python interop (#174, README vision; also #265
optimization-friendliness, #281 autodiff) — is the right call: expose
fast batch eval to scipy/PyMC rather than reimplementing an optimizer.
Prerequisites: stable machine-readable run I/O (§5.1), parameter metadata
(domain constraints exposed — done in eval API), cheap re-eval (#45, #360).
A native likelihood/payoff DSL and Kalman filtering should be explicit
non-goals for now.

### 5.4 Reality-Check-style conformance testing

Graphcal `assert` already covers the *checking* half — typed, indexed
(`Bool[T]` asserts check every time step — arguably cleaner than Vensim's
implication syntax), tolerance forms, `#[expected_fail]`. Missing is the
*forcing* half: Reality Check overrides model-generated values to make a
condition hold, running extra simulations per constraint. The Graphcal-native
shape is scenario-scoped assertion suites: named test scenarios (param/input
overrides + asserts evaluated under them) runnable in batch — which is exactly
the `graphcal test` idea (#656). Vensim's equilibrium test maps to an assert
pattern (`@stock[t1] ~= @stock[t0] +/- tol`) once §4 lands.

Tracked: #656.

### 5.5 Output/export

Vensim exports any run to tab/CSV; Graphcal has text/JSON stdout only. CSV
export of indexed nodes (long or wide) is table stakes for the Excel-adjacent
audience; ties into #353.

## 6. Tier 3 gaps — interactive & structural UX

### 6.1 SyntheSim-equivalent

The recompute loop already exists (LSP: 300 ms debounced whole-graph re-eval,
inlay values — "live worksheet"). Missing: interactive controls (sliders on
params, using domain-constraint bounds as slider ranges — the data is already
there), and in-editor/live charts (plots are CLI-only today). #352 (live view)
+ #100 (`graphcal watch`) + #43 (WASM playground) point the same direction: a
browser live view with sliders + auto-updating Vega-Lite plots would be a
credible SyntheSim answer, and the WASM plugin/eval architecture makes an
in-browser engine plausible.

Tracked: #352, #100, #43, #512.

### 6.2 Causal tracing & loop visibility

Causes/Uses trees ≈ dependency graph walks — `graphcal graph` (DOT) + LSP
references cover the static half crudely; #512 (DAG visualizer) and #24
(`graphcal tree`) are the tracked shape. Two SD-specific wrinkles: (1)
Vensim's *behavioral* tracing (strip graphs of a variable's causes over time)
needs per-node time-series capture, cheap once runs are artifacts (§5.1); (2)
**feedback loops are invisible in Graphcal's DAG** — the DAG is acyclic by
construction, and all SD feedback lives *inside* closure bodies. A loops tool
would have to analyze data flow within `unfold`/`integrate` bodies (or a
future per-`(node,t)` graph, §4.1). Untracked, and worth noting in any §4.1
design: whichever named-flow mechanism lands should keep flow→stock edges
*reconstructable* for tooling.

### 6.3 Gaming, apps, publishing — reframe, don't copy

Vensim: gaming mode, Venapp, `.vpmx` + free Reader, Publish Web Files
(Emscripten/WASM with SyntheSim UI). Graphcal equivalents should be:
git repo = the package; `graphcal eval` = the reader (FOSS, no license split);
WASM playground (#43) / live view (#352) = published interactive model.
Gaming (step-run-pause-decide) is a genuinely distinct interaction model;
defer unless a concrete user appears. Molecules-style library = a `graphcal`
package once §4.3 exists (#99, #154).

## 7. What NOT to copy (hold the line)

- **Implicit dimensionless & unit synonyms** — Vensim needs `Dmnl = Fraction`
  lists because units are strings. Keep dimensions structural.
- **Silent ZIDZ/lookup-clamp semantics** — offer explicit policies instead.
- **Ambient RNG state** — seeds must be explicit params (§4.7).
- **Binary run/dataset formats** (`.vdf`) — runs should be text (JSON/CSV),
  diffable like everything else.
- **Sketch-position data interleaved with equations** in the model file — any
  future diagram layer must keep layout out of the semantic source (or in a
  separate file), preserving mergeability.
- **Equation-attached-to-diagram as the only entry path** — text-first with
  rendered views, not diagram-first with generated text.

## 8. Suggested sequencing

**M1 — "port a textbook SD model"** (SIR, Bass diffusion, inventory
oscillation, World3-style stock webs):
1. §4.1 named flows / local bindings / coupled-state decision (#349, #889 + new
   design) — *first, everything composes with it*
2. `integrate` + Euler/RK4 (#351)
3. Lookups (#891)
4. SD function set: delays/smooths/test inputs (§4.3, new)
5. Runtime time-axis bounds + SAVEPER-style output thinning (§4.5, new)

**M2 — "believe the model"**: run artifacts + compare (§5.1, new) → CSV/series
data in & out (#353/#892/#355) → sweeps (#897/#45) → seeded RNG + MC + fan
charts (§4.7/#896) → `graphcal test` scenarios (#656).

**M3 — "estimate & optimize"**: Python bridge (#174) with batch eval, exposed
domain constraints, autodiff later (#281/#265).

**M4 — "show it live"**: live view with sliders (#352) + WASM playground (#43)
+ DAG/loop visualization (#512/#24/§6.2).

Gaps with no tracking issue yet (candidates to file): local `let` bindings /
flow naming inside recurrences; per-`(node,t)` granularity decision; SD
function library; stock clamping semantics; runtime index bounds; SAVEPER;
data-variable interpolation modes; run artifacts & compare; seeded PRNG +
distributions; CSV export; loop-aware tooling over closure bodies; `.mdl`/XMILE
importer (worth a look purely as an adoption funnel — PySD's parser is prior
art).

## Appendix A — Vensim function catalog → Graphcal disposition

Status: ✅ exists · 🟡 expressible as a pattern today · ❌ missing.
Path: `builtin` = add to the closed builtin set · `stdlib` = SD library (§4.3,
blocked on §4.1) · `CLI` = tooling boundary (§4.6/§5) · `plugin` = viable WASM
plugin today · `non-goal` / `divergent` = deliberate difference.

| Vensim | Semantics | Graphcal today | Path |
|---|---|---|---|
| **Stocks & init** | | | |
| `INTEG(rate, init)` | stock | 🟡 hand-Euler `unfold` | `integrate` #351 |
| `SINTEG` | non-conservative INTEG variant | ❌ | defer |
| `INITIAL(expr)` | freeze expr at t₀ | 🟡 separate node feeding init | — (works) |
| `ACTIVE INITIAL(a, i)` | break simultaneous init loops | ❌ | #888 fixpoint |
| `GAME(expr)` | gaming override hold | ❌ | non-goal (§6.3) |
| `A FUNCTION OF(...)` | placeholder RHS from sketch | ❌ | #505 `todo` marker |
| `==` unchangeable constant | non-overridable constant | ✅ `const node` | — |
| `[min,max,incr]` bounds | runtime warnings + slider ranges | ✅ domain constraints | divergent: error vs warn — needs a policy look |
| **Delays** | | | |
| `DELAY1(I)/3(I)/N` | 1st/3rd/nth-order material delay | ❌ | stdlib (hidden stocks) |
| `DELAY FIXED` | pipeline delay | ❌ | stdlib (ring-buffer state) |
| `DELAY MATERIAL/INFORMATION` | conserving delay under varying delay time | ❌ | stdlib, later |
| `DELAY CONVEYOR/BATCH/PROFILE`, `DELAYP` | conveyor/batch/profile transit | ❌ | defer |
| **Smoothing/trend** | | | |
| `SMOOTH(I)/3(I)/N` | exponential information smoothing | ❌ | stdlib |
| `TREND(x, at, it)` | fractional growth rate | ❌ | stdlib |
| `FORECAST(x, at, h)` | trend extrapolation | ❌ | stdlib |
| **Test inputs** | | | |
| `STEP`, `RAMP`, `PULSE`, `PULSE TRAIN` | canonical forcing functions | 🟡 `if`/`else` over `t` | stdlib sugar |
| **Sample/discrete** | | | |
| `SAMPLE IF TRUE(c, x, i)` | hold unless condition | 🟡 `unfold` state | stdlib |
| `SHIFT IF TRUE` | aging-chain shift | ❌ | stdlib after #889 |
| `INTEGER(x)` | truncate | ✅ `trunc` (dimensionless-only) | divergent (by design, #1039) |
| `QUANTUM(a, b)` | round down to multiple | 🟡 `b*trunc(a/b)` | maybe builtin |
| `MODULO(x, m)` | remainder | ✅ `%` operator | — |
| **Lookups** | | | |
| lookup def + `name(x)` | piecewise-linear, clamp+warn | ❌ | #891 |
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
| `RANDOM UNIFORM/NORMAL/POISSON/EXPONENTIAL/BETA/BINOMIAL/GAMMA/NEG BINOMIAL/TRIANGULAR/WEIBULL` | truncated distributions | ❌ | §4.7 seeded-pure design |
| `RANDOM LOOKUP`, `RANDOM PINK NOISE` | arbitrary density; autocorrelated noise | ❌ | later |
| `NOISE SEED`, `NOISE RNG` | in-model seed; RNG selection | ❌ | seed as `param` (§4.7) |
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
| `:NA:` | missing-value sentinel | ❌ (by design: no silent NaN) | divergent: model missing data as ADT (`Option`-style) at the data boundary |
| `GET TIME VALUE`, `TIME BASE` | calendar access; alternate axes | 🟡 `Datetime` builtins (`year`…`day_of_year`, scale conversions) | calendar time axes = open design |
| **External readers** | | | |
| `GET XLS/DIRECT/123/VDF DATA/CONSTANTS/LOOKUPS/SUBSCRIPT` | spreadsheet/dataset ingestion in-language | ❌ | CLI boundary (#353, #892); keep the core pure |
| `TABBED ARRAY` | inline array literal | ✅ `table` (typed, total) | — (stronger) |
| **Reality Check** | | | |
| `:TEST INPUT:`, `:THE CONDITION:`/`:IMPLIES:`, `RC *` | forced conformance tests | 🟡 `assert` covers checking, not forcing | #656 scenario suites |
| **Misc** | | | |
| `MESSAGE` | runtime warnings/prompts | 🟡 asserts + per-node errors | — |
| `:MACRO:` | user components w/ internal stocks | 🟡 `dag` blocks (no stateful-over-axis story yet) | §4.1 + #99 |
| String type | file names/labels | ❌ | #149 |
| External function DLLs | native extension (DSS) | ✅ WASM plugins | — (stronger: sandboxed, pinned, pure) |

## Appendix B — Sources

Vensim facts: Ventana's official documentation (`vensim.com/documentation/` —
reference guide pages for functions `fn_*.html`, equation format, subscripts,
units, macros, data, Reality Check, integration, sensitivity, optimization,
MCMC/Kalman, SyntheSim, gaming, DLL/compiled-sim, publishing, `.mdl` format,
release notes for 8.0–10.4), cross-verified against machine-readable
implementations: PySD (SDXorg — Vensim translation tables and PEG grammars),
SDEverywhere (Climate Interactive — supported-functions list, ANTLR grammar,
mdl preprocessor), simlin's mdl builtin table, and community keyword
inventories. Edition/tooling facts: vensim.com product pages and comparison
chart, Ventana forum threads (mdl-merge limitations, noise-stream semantics),
System Dynamics Society resources (SDM-Doc), and SD tool-comparison surveys.
Direct fetches of vensim.com were blocked from this environment, so official
pages were captured via search-engine retrieval; unverifiable details are
marked *(unverified)* inline.

Graphcal facts: this repository at `bdb597a` — `grammar.ebnf`,
`docs/language/*`, `crates/graphcal-compiler/src/builtin.rs`,
`registry/builtins.rs`, `tir/dim_check/infer/hir.rs`,
`crates/graphcal-eval/src/{exec_plan.rs,eval_expr/hir_eval.rs,host_fns.rs}`,
`crates/graphcal-cli/src/{main.rs,overrides.rs,json_input.rs,plot.rs}`,
`crates/graphcal-plugin-host/src/host.rs`, `tests/fixtures/`, the issue
tracker, and the removed design doc `design/11-system-dynamics.md`
(`git show d9236b35^:design/11-system-dynamics.md`).
