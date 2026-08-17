# Extensibility Mechanisms — An Options Catalog

> [!WARNING]
> This content was written by an AI agent and must be verified by a human
> developer. After human verification, this alert may be removed.

This document is a deliberately divergent brainstorm for the discussion around
[#1401](https://github.com/graphcal-lang/graphcal/issues/1401) (candidate
features from capability research). The question it explores: **how can
Graphcal be made extensible enough that most of those features can be built
*outside* the core — by users, as packages and extensions — rather than
implemented inside the language?**

The catalog is filed for discussion as
[#1411](https://github.com/graphcal-lang/graphcal/issues/1411); comments
belong there.

It is an options catalog, not a design proposal. Options overlap, some are
mutually exclusive, and none are ranked by feasibility or preference — the
goal is breadth. Each mechanism has an ID (`A1`, `C3`, …) so later discussion
can reference options precisely. Feasibility, cost, and sequencing are
explicitly out of scope here.

Companion context: `internals/linear-algebra-survey.md` (what indexed values
can express today), `docs/language/extern-functions.md` (the current plugin
seam), `docs/tenax-integration.md` (the one existing external-shell protocol).

## 1. Framing

### 1.1 The Pi lesson, translated

Pi Coding Agent is a useful reference point for extensibility done well: the
core stays a minimal loop, and everything else — tools, commands, UI — is
implemented through a small, stable extension API that the maintainers
themselves use for built-in features. The transferable lessons are not about
agents:

1. **Minimal core ≠ few features; it means few, aggressively enforced
   invariants.** Pi's kernel is "the loop"; Graphcal's kernel is *the trust
   chain*: parse → resolve → check dimensions/types/shapes → evaluate purely →
   pin everything reviewable. Everything that merely *computes, imports, or
   renders* is a candidate for extension space.
2. **Extensions are written in something users already have.** For Pi that is
   TypeScript. For Graphcal the ladder is: `.gcl` itself first (packages),
   sandboxed WASM second (any toolchain), stable protocols third (any
   process).
3. **The core team dogfoods the extension API.** Built-ins that cannot be
   expressed through public mechanisms are treated as extension-API bugs.
4. **Distribution is trivial.** A plugin is a file; a package is a git
   dependency; trust is a lockfile diff. Graphcal already has all three
   primitives — they just serve only one extension kind today.

The Graphcal-specific twist: extensibility must never weaken the safety
story. So the recurring shape below is **"untrusted extensions propose,
trusted core checks"** — the same architecture as proof assistants with small
kernels (Lean/Coq: tactics are arbitrary programs, but the kernel re-checks
every proof they emit).

### 1.2 Invariants any mechanism must preserve

Every option in this catalog is judged (later, not here) against four
invariants, which are the actual "minimal core":

- **I1 — Checked boundary.** Every extension surface has a declared contract
  in Graphcal vocabulary (dimensions, types, shapes, domains), verified by the
  core at load/check time. The existing manifest-vs-declaration verification
  is the template.
- **I2 — Staged effects.** Evaluation stays pure and deterministic. Effects
  (I/O, nondeterminism) may exist only in shells *before* evaluation
  (snapshotted, content-addressed inputs) or *after* it (emitters that cannot
  influence values).
- **I3 — Reviewable trust.** Extension artifacts enter a project only through
  plain-text diffs (declarations, schemas) plus lockfile pins (bytes,
  snapshots). No prompt-at-load, no ambient discovery.
- **I4 — Propose-and-verify where possible.** Prefer designs where the core
  can cheaply re-check an extension's claim (a residual, a shape law, a
  round-trip) over designs where the extension must simply be believed.

### 1.3 Why #1401 does not fit through the current seam

The plugin system today opens exactly one seam: **pure kernel functions over
`f64` SI quantities, `Bool`/`Int`, dense arrays, and record-shaped results.**
Reading #1401 against that seam, the 39 items fail to pass through it for six
distinct reasons — and each reason names a family of mechanisms:

| Gap | Symptom in #1401 | Mechanism family |
|-----|------------------|------------------|
| Value vocabulary too narrow | complex numbers, text, datetimes, ragged/sparse data, keys | **A** — widen the kernel seam |
| No compile-time participation | FFT bin counts, schema checks, prepared shapes | **B** — extensions in the checking phase |
| No higher-order calls or protocols | root finding, ODE, `iterate_until` need a *function* argument | **C** — higher-order and protocol seams |
| No effectful boundary story | CSV/XLSX import, reports, lineage | **D** — staged effect boundaries |
| Vocabulary closed to packages | tables, graphs, calendars, temperature points want new *types* | **E** — in-language extensibility |
| Syntax and presentation closed | LP modeling blocks, stencil sugar, new plot encodings | **F** — syntax participation |

Group **G** collects the ecosystem mechanics (tooling, distribution, trust)
that make any of the above adoptable.

## 2. Mechanism catalog

### Group A — Widen the kernel seam (evolve the extern-function ABI)

- **A1. Vocabulary widening (ABI v5+).** Add value kinds incrementally:
  `Complex` quantities, `Bool`/`Int` array elements, `Key<I>` (opaque index
  labels as ordinals), datetime instants on a declared scale, struct
  *parameters*, arrays of structs, dimension-variable struct fields. The
  boring, direct continuation of v3→v4. Unlocks: complex linear algebra, FFT,
  masks/ranking, atomic solver results.
- **A2. Arrow as the boundary encoding.** Replace (or augment) the bespoke
  pointer+extents ABI with Arrow buffers: validity bitmaps give missing-data
  representation, dictionary arrays give categoricals/keys, list arrays give
  ragged results, struct arrays give records, extension types give sparse.
  One encoding shared by the plugin ABI, the Tenax protocol, and future
  importers/emitters. Manifests still declare Graphcal-typed signatures that
  *lower* to Arrow schemas (I1 intact). Prior art: Arrow C data interface,
  ADBC, polars expression plugins, DataFusion UDFs.
- **A3. WASM Component Model / WIT interfaces.** Express plugin contracts in
  WIT instead of hand-rolled `f64` slots; the canonical ABI handles strings,
  records, lists, variants; `resource` types give handles (see A4) for free.
  Trades bespoke-ABI simplicity for ecosystem tooling (bindings generators in
  many languages). Prior art: wasmtime components, Extism, Zed extensions.
- **A4. Opaque resource handles.** A plugin declares `resource SparseMatrix`
  or `resource CholeskyFactor`; functions produce/consume handles that
  Graphcal treats as unforgeable typed values — they flow through nodes, but
  only functions of the owning plugin can look inside. Enables
  factor-once/solve-many, sparse assembly, PRNG streams, table handles,
  compiled meshes. Determinism story: a handle is content-addressed by the
  pure call graph that produced it, so re-evaluation replays identically.
  Prior art: Postgres extension types, SQLite virtual tables, WIT resources.
- **A5. Richer result records.** Extend record-shaped results with
  dimension-variable fields, array-valued fields, and anonymous multi-value
  returns — so `solve` can atomically return `{root, residual, iterations,
  converged}` without repeated calls or convention-based pairing.
- **A6. Structured failure vocabulary.** Manifests declare typed failure
  variants (`NoBracket`, `MaxIterations{n}`, `OutOfValidityDomain{field}`)
  instead of only string messages. Failures become data that `match`,
  assertions, and diagnostics can consume — the beginning of an honest
  "runtime validity" story.
- **A7. Cost contracts.** Manifests declare a cost class or fuel estimate as
  a function of input extents. Hosts use it to budget; the LSP uses it to
  warn before keystroke-frequency re-evaluation runs an `O(N³)` kernel.
- **A8. Trusted-native extension tier.** Formalize the existing
  `HostFunctionRegistry` as a user-facing tier for embedders/organizations:
  native libraries (LAPACK, SuiteSparse) wrapped behind the same declared
  signatures, explicitly configured, with **visible provenance** — values
  computed by native code are marked "not bit-reproducible / trusted-native"
  and the mark propagates to outputs. Prior art: Postgres trusted vs
  untrusted languages, Deno `--allow-ffi`.
- **A9. Import-site vocabulary binding.** Today only the eight prelude base
  dimensions cross the manifest boundary. Let manifests declare *abstract*
  dimension (or index/type) slots that the `.gcl` import block binds to
  project-defined vocabulary: `import plugin "fin.wasm" as fin with dim
  Currency = money.Currency { ... }`. Vocabulary stays in `.gcl` (the current
  rule), kernels stay generic over it.

### Group B — Compile-time participation (extensions in the checking phase)

- **B1. Shape-law manifests.** Allow a result extent to be a checked-Nat
  expression over parameter extents: `fft(xs: D[N]) -> Complex[N/2 + 1]`,
  `window(xs: D[N], w: 3) -> D[N - 2]`, `chol(A: D[N, N]) -> D[N, N]`. The
  plugin never invents an extent (current rule preserved); it *declares a
  law* that the core's `nat.rs` engine checks and propagates. Directly the
  issue's "derived axes and richer checked Nat arithmetic". Prior art: shape
  functions in Dex/Futhark, ONNX shape inference.
- **B2. Compile-time (const-eval) plugin functions.** The existing sandbox is
  already deterministic, fueled, and I/O-free — exactly what safe const-eval
  requires. Let declared plugin functions run during *checking*: derive an
  index's cardinality from a schema, validate a mesh file's shape promises,
  precompute quadrature tables into consts, verify a task network is acyclic
  before evaluation exists. Prior art: Zig `comptime`, Rust const fn +
  proc-macro split.
- **B3. Refinement/precondition manifests.** Extern signatures declare domain
  constraints in the same language as param domains (`requires t:
  Temperature(min: 0 K, exclusive)`). The core discharges them statically
  against declared domains where provable and installs runtime guards
  *outside* the plugin otherwise — contract violations surface as Graphcal
  diagnostics, not kernel-internal `fail!`s. Prior art: Ada 2012 contracts,
  refinement types.
- **B4. Property/law manifests with mechanical checking.** Plugins declare
  algebraic laws in a small vocabulary — `lerp(a, a, t) == a`, monotonicity
  in an argument, permutation equivariance, `matmul(inv(A), A) ≈ I` — and
  `graphcal plugin test` (and project CI) property-fuzzes the binary against
  its own claims. The manifest stops being documentation and becomes a
  testable spec (I4). Prior art: QuickCheck/proptest law testing.
- **B5. Checker/lint packs.** A second extension kind that consumes a stable,
  read-only "checked-project facts" schema (declarations, types, dimensions,
  dependency edges, spans — see G2) and emits extra diagnostics: org style
  rules, numerical-hygiene rules ("flag subtraction of near-equal
  quantities"), process rules ("every `param` carries `#[source(...)]`").
  Same WASM sandbox, run at check time. Prior art: Clippy/ESLint plugin
  models, Semgrep.
- **B6. Extension-provided editor intelligence.** Manifests carry doc
  comments, examples, units-of-use notes; the LSP surfaces hover/signature
  help for plugin functions, inlay hints derived from B1 shape laws, and
  quick fixes ("insert `import plugin` block from module manifest").

### Group C — Higher-order and protocol seams (solvers without solver built-ins)

- **C1. Graph-valued arguments (callbacks).** Let an extern signature take a
  dag blueprint: `fn brent<D: Dim, E: Dim>(f: dag(x: D) -> E, lo: D, hi: D,
  tol: E) -> D`. Execution is a coroutine: the plugin yields "evaluate `f`
  at x", the host evaluates the pure blueprint, resumes the plugin. Both
  sides stay pure; fuel is shared. Root finding, quadrature, ODE
  right-hand-sides, fitting objectives all become *library* concerns. Prior
  art: every solver-callback C API; wasm side via a step-function protocol or
  stack switching.
- **C2. Solver-protocol interface (inversion of control).** The dual of C1:
  the core owns one generic bounded-iteration construct, and a plugin
  implements a declared protocol — `init(args) -> State`, `step(State) ->
  State`, `converged(State) -> Bool`, `extract(State) -> Result` (State as
  A4 handle or declared record). The core owns the loop, the work budget,
  convergence bookkeeping, and can expose iteration traces to plots and
  diagnostics. One language construct, unbounded algorithm ecosystem. Prior
  art: SUNDIALS stepper interfaces, Rust `Iterator`.
- **C3. Implicit/relational declarations with pluggable backends.** A single
  core form for "this node is defined by a condition, not a formula":
  `node x: Length = solve fluids.residual(x, @p) == 0 N in [0 m, 10 m] with
  {tol: 1e-9, max_iter: 100};` — with the strategy dispatched to any
  extension implementing the C2 protocol. Same surface later hosts
  `minimize`/`fit` (least squares) and LP/MILP objective blocks. Prior art:
  Modelica equation sections, MiniZinc solver backends, CVXPY.
- **C4. Chunked/streaming kernel protocol.** Kernels may implement
  `begin(shape) -> State`, `absorb(State, chunk) -> State`, `finish(State) ->
  Result`; the host feeds slices under explicit memory bounds. Out-of-core
  and lazy pipelines become possible while each call stays pure. Directly
  aimed at "chunked, tiled, lazy, or streaming indexed evaluation". Prior
  art: streaming UDAFs (DataFusion), online aggregation.
- **C5. The in-language route: grow pure `.gcl` power.** Add bounded
  `iterate_until` (mandatory fuel + tolerance), early-exit/first-passing
  search, and dag blueprints as *bindable values* — then Newton, RK4,
  binning, and stencils are pure `.gcl` packages, and WASM is reserved for
  performance kernels. The language stays total (fuel makes iteration
  bounded), and iterative methods become reviewable source instead of binary.
  Prior art: Futhark loops, total-FP languages.
- **C6. Deterministic generator protocol.** Splittable seeded streams as
  explicit values: `resource Rng; fn split(r: Rng) -> {a: Rng, b: Rng}; fn
  normal(r: Rng, mu: D, sigma: D) -> {value: D, next: Rng}`. Sampling is
  explicit-state, replayable, and diffable — no ambient randomness anywhere.
  Prior art: JAX PRNG keys.

### Group D — Staged effect boundaries (data in, artifacts out; purity intact)

- **D1. Reader/importer extensions.** A new declaration:
  `import data "data/loads.csv" via csv as loads: LoadTable;` where the
  *core* performs the I/O, hashes the bytes, and pins them in
  `graphcal.lock` (extending the existing plugin-pin machinery), and the
  reader extension is a **pure function bytes → typed value + diagnostics**
  checked against a declared schema. A malformed file is a load error with a
  span, never a NaN. This one mechanism carries schema-checked tabular
  import, missing-data policy declaration, and data-lineage pinning at once.
  Prior art: Nix fixed-output derivations (hash-pinned impurity), Bazel
  repository rules, Pandoc readers.
- **D2. Offline data compilers.** The zero-core-change variant of D1: a
  convention for external tools that compile foreign data into reviewable
  canonical `.gcl` (or a frozen snapshot format) — `graphcal-import-xlsx
  workbook.xlsx > tables.gcl`. Diffs of regenerated files *are* the
  provenance trail. Could be wired via a build hook in `graphcal.toml`.
  Prior art: protobuf codegen, sqlc.
- **D3. Dataset packages.** Versioned property tables, catalogs, and
  calibrations as ordinary git-dependency packages (the package manager
  already exists) with validity-domain declarations and provenance metadata
  in the manifest. Material libraries, holiday calendars, and standards
  tables become dependencies with the same trust story as code. Prior art: R
  data packages.
- **D4. Emitter/report extensions.** A post-evaluation stage: the typed
  `EvalResult` plus presentation facts, serialized to a stable schema, handed
  to write-only extensions that render calculation certificates, schedules,
  Gantt/histogram/control-chart encodings, SMT-LIB exports of assertions, or
  interchange formats. Emitters run after evaluation and cannot influence
  values (I2). A companion move: make the plot spec an open *typed data*
  vocabulary (Vega-Lite-style) so new encodings are renderer packages, not
  grammar changes. Prior art: Pandoc writers, nbconvert exporters.
- **D5. Stabilize the "model server" seam.** The Tenax integration is
  already the right shape: compile once, stream Arrow batches through a pure
  model. Documenting and versioning that protocol as *the* embedding
  interface lets any orchestrator — Python notebooks, Excel adapters, grid
  runners, optimization loops — drive a checked model without touching the
  core. The issue's "functional-core integration architecture" item is
  largely "write this ADR and commit to the protocol". Prior art: LSP/DAP
  (protocol beats plugin), Jupyter kernel protocol.
- **D6. Capability grants in the manifest.** `graphcal.toml` grants each
  extension exactly the effects it may use: `[extensions.csv] may =
  ["read:data/*.csv"]`. Ungranted capability requests fail at load. Extends
  the lockfile's "which bytes" trust with "which effects" — all reviewable in
  one diff. Prior art: Deno permissions, browser extension manifests.
- **D7. Input adapters / resolved params.** Extensions that resolve `param`
  values in the shell *before* evaluation (sensor feeds, database lookups),
  snapshotting value + timestamp + source hash into the run record. The
  evaluator sees plain values; the audit trail sees provenance. Prior art:
  Terraform data sources.

### Group E — In-language and vocabulary extensibility (packages, not binaries)

- **E1. Targeted language growth for library authors.** Possibly the
  highest-leverage "extension mechanism" is making `.gcl` itself capable of
  hosting libraries: parametric record/row generics, dag blueprints as
  values, richer index algebra (concatenation, products, checked subsets).
  Then typed row datasets, join/group-by, graph/topology operations, and
  heterogeneous block vectors are *packages* — reviewable source, no ABI.
  Prior art: the "Growing a Language" strategy; Haskell/Rust pushing features
  into libraries via typeclasses/traits.
- **E2. Unit-system extension points.** The core adds *mechanisms* — affine
  unit scales (temperature points vs differences), logarithmic units (dB),
  calendar/day-count scales — and packages declare the actual vocabularies
  and conversion laws. "No implicit conversion" stays; the set of expressible
  unit families opens. (Temperature-interval semantics and day-count
  conventions from #1401 land here as vocabulary, once the mechanism
  exists.)
- **E3. Numeric-carrier policy types.** If decimal/fixed-point must be core
  (it touches the value model), keep it minimal: an exact-decimal carrier
  with *mandatory explicit rounding-mode arguments*, and let packages bundle
  policies (`accounting.round_half_even_2dp`). Core provides the carrier;
  policy is vocabulary.
- **E4. Index providers.** Let finite indexes be *derived* at checked
  boundaries — from a dataset import (D1 + B2: "the labels of this index are
  the part numbers in this pinned file"), from checked Nat arithmetic, from
  set algebra over existing indexes — with cardinality frozen before
  evaluation. This is the issue's "prepared-input shape schemas" and half of
  "irregular coordinate indexes".
- **E5. Prelude self-hosting (the dogfood test).** Re-express slices of
  today's built-ins — aggregations, the demo plugin, some plot marks — as
  bundled packages/extensions using only public mechanisms. Every place that
  fails is an extension-API gap found before users hit it. Adopt the policy
  version too: **new capability lands as an extension first; promotion into
  core requires an argument, not the reverse.** Prior art: Pi's built-ins as
  extensions; VS Code shipping built-in features as extensions.
- **E6. Capability levels, not version lockstep.** Extensions declare
  `requires capability: shape-laws >= 1` rather than pinning compiler
  versions; the host advertises its level per extension kind. Keeps a young,
  churning ABI honest about what works where. Prior art: LSP capability
  negotiation.

### Group F — Syntax participation (the most radical band)

- **F1. Declarative macros (pattern → template).** Hygienic,
  non-computational rewrite rules defined in `.gcl` packages: enough for
  LP-block sugar, stencil-neighborhood sugar, and boilerplate derives.
  Expansion is deterministic and dumpable (`graphcal dump expanded`), so
  review remains possible. Prior art: `macro_rules!`, `syntax-rules`.
- **F2. Procedural syntax extensions (compile-time WASM expanders).**
  `use syntax lp from "ext/lp.wasm"; ... lp! { maximize profit subject to
  ... }` — the expander receives a token tree and returns AST-as-data, which
  the core then validates *exactly like parsed source* (all checks still
  apply; the expander is pinned, fueled, pure). The sandbox designed for
  kernels is, unchanged, a safe macro processor. Prior art: proc-macros,
  Racket readers, Julia macros.
- **F3. Attribute/derive hooks.** Extensions react to attributes on
  declarations — `#[uncertainty(normal, 2 %)]`, `#[source("datasheet X
  rev3")]` — either as checked metadata consumed by B5 lint packs and D4
  emitters, or as expansion inputs. Cheap, contained, and useful for the
  reports/lineage items without opening general syntax.
- **F4. Typed literal extensions.** A narrow hook: packages define literal
  readers for their nominal types (`wkt"POINT(1 2)"`, `iso"2026-08-15Z"`,
  `mat[[1, 2], [3, 4]]`), validated at compile time via B2. Most of the
  ergonomic win of macros at a fraction of the surface.
- **F5. The closed-syntax stance (anti-option).** Deliberately refuse
  F1–F4 forever: one syntax, all extension through functions, protocols, and
  data. For a safety-first language this is a coherent position — uniform
  review, uniform tooling, no dialect drift — and it deserves to compete as
  an explicit choice rather than a default. Prior art: Go's philosophy, Elm.
- **F6. Dialect files (generated-source convention).** The middle path:
  foreign syntaxes live in *separate files* compiled to `.gcl` by external
  tools (D2), never inline. Syntax diversity exists at file granularity;
  review always sees plain `.gcl` artifacts.

### Group G — Tooling, distribution, and ecosystem mechanics

- **G1. CLI subcommand extensions.** `graphcal x <name>` discovers
  `graphcal-<name>` binaries on PATH (cargo/git-style), with a documented way
  to reuse the pipeline. Community tooling ships without core releases.
- **G2. A stable "checked-project facts" export.** One versioned, documented,
  read-only schema of post-check facts — declarations, resolved types,
  dimensions, dependency edges, spans, presentation facts. This is the
  keystone that B5 linters, D4 emitters, docs generators, and CI dashboards
  all stand on; ecosystems that retrofit it late (rustc) envy those that had
  it early (clang tooling, `go/analysis`). `graphcal dump` today is
  explicitly *not* this; the mechanism is deciding to have a real one.
- **G3. Per-kind conformance kits.** Grow `graphcal plugin test` into
  conformance suites per extension kind (kernel, reader, emitter, expander,
  solver protocol): golden files, B4 law fuzzing, and a differential mode
  that compares two implementations of one contract.
- **G4. Scaffolds per kind.** `graphcal plugin new --kind
  {kernel,reader,emitter,solver,lint,syntax}` — templates encode each
  contract so authors start correct. (Pi's lesson about trivial
  bootstrapping.)
- **G5. Registry and curation tiers.** A package index whose badges are
  *machine-derived*: pure-wasm / capability-using / native-tier; law-manifest
  coverage; conformance results. Possibly an official `graphcal-contrib`
  org as the extended standard library (the SciPy-to-NumPy relationship) —
  a social mechanism that keeps the core small by giving ambitious features a
  respectable home outside it.
- **G6. Manifest-claim verification in CI.** `graphcal verify-extensions`
  re-fuzzes every pinned extension against its declared shape laws (B1) and
  property laws (B4) in project CI — trust, but mechanically re-verify (I4).
- **G7. Embedder trait surface.** Officially support Rust-level host
  extension: evaluator drivers (scheduling/materialization policies for the
  chunked/lazy item), cache policies, value-store backends, custom
  front-ends over `graphcal-eval`. Documented traits instead of "read the
  source".
- **G8. Compile-to-artifact (extensibility in reverse).** Export a checked
  dag as a standalone WASM/C kernel *with its own manifest* — a Graphcal
  model becomes a plugin for other hosts (web dashboards, PLCs, other
  Graphcal projects). Sealed compiled subgraphs double as a performance
  story, and the manifest format gets a second producer, which keeps it
  honest.

## 3. Cross-cutting themes

- **Untrusted propose, trusted verify.** Wherever a claim is cheaper to check
  than to compute, have the core check it: a root-finder returns `x` and the
  core evaluates `|f(x)| < tol` with existing assertion machinery; a
  factorization returns factors and the core spot-checks reconstruction on
  sampled entries; an expander returns AST that goes through every normal
  check. This converts "audit the binary" into "check the claim" — the
  proof-assistant architecture, and the theme most aligned with the project's
  ethos.
- **One boundary encoding.** A2/C4/D1/D4/D5 all move typed data across a
  boundary. Choosing one self-describing encoding (Arrow is already in the
  building via Tenax) makes every seam cheaper than designing four bespoke
  ones.
- **One trust ledger.** `graphcal.lock` pins plugin bytes today. The same
  ledger can pin data snapshots (D1), expanders (F2), emitters (D4), dataset
  packages (D3), and capability grants (D6): one reviewable diff answers
  "what can affect my numbers?"
- **Effects only at the rim.** The composed picture of Group D: importers
  snapshot the world before evaluation; evaluation is pure; emitters render
  after it. The reactive core never blocks on, or observes, the world.
- **Extension-first as policy, not architecture.** Pi stays minimal because
  its maintainers *choose* to build features as extensions and treat friction
  as API bugs (E5). No mechanism substitutes for that policy.
- **Capability negotiation over version lockstep** (E6) keeps a young
  extension surface evolvable without ecosystem breakage.

## 4. Coverage map for issue #1401

Candidate mechanisms per item; **core?** marks items with an irreducible core
component (value model, type system, or evaluation semantics) that no
extension mechanism plausibly absorbs in full.

### Numerical algorithms and solver contracts

| Item | Candidate mechanisms | Core? |
|------|---------------------|-------|
| `iterate_until` | C2, C5, C3 | the one bounded-loop construct |
| Root finding / goal seek | C1, C2, C3, A5, A6 | — |
| Nonlinear systems | C1, C2, C3, A4 (Jacobian/state handles), B1 | — |
| ODE/DAE + events | C1, C2, A4, A6 (event variants) | — |
| PDE / stencils | C5 + windows, F1 sugar, C4 chunking, B1 | — |
| Complex reductions / linear algebra | A1 (Complex), A2, B1 | Complex as a value kind |
| Sparse matrices / solvers | A4 handles, A2 sparse encoding, C4 | — |
| Matrix decompositions | A1/A2, A4 factors, B1 shape laws, I4 certificates | — |
| Least squares / fitting | C1/C3 (model as blueprint), A5 diagnostics, B4 | — |
| Quadrature / special functions | plain kernels today, C1 integrands, B2 tables | — |
| FFT / signal kernels | A1 Complex, B1 (`N/2+1`), A7 cost | — |
| Unit-aware LP/MILP | C3 backend protocol, F1/F2 surface, A4 model handles, D4 export | — |
| Deterministic sampling / stats | C6 PRNG protocol, A1 Int arrays, kernels | — |

### Types, shapes, indexes, and execution

| Item | Candidate mechanisms | Core? |
|------|---------------------|-------|
| Atomic multi-field results | A5 (partially exists), A1 | — |
| Richer plugin ABI value types | A1, A2, A3, A9 | — |
| Heterogeneous typed block vectors | E1 row generics, A2 struct arrays | likely type system |
| Typed topology / graph ops | E1 packages, A4 handles, B2 acyclicity checks | — |
| Derived axes / checked Nat arithmetic | B1, E4 | `nat.rs` engine growth |
| Irregular coordinate / datetime indexes | E4, E2 scales | index model |
| Ragged / runtime-validity results | A2 list arrays, A6 typed absence | value model |
| Chunked / tiled / lazy / streaming | C4, G7 drivers, D5 external orchestration | evaluation model |
| Prepared-input shape schemas | B2, E4, D1 schemas | — |
| Neighborhood / lag / window / search | C5, B1-lawed kernels, F1 sugar | — |
| Masks, reductions, sort/rank/bin | A1 (Bool/Int arrays) kernels, C5, B1 | — |
| Temperature point/interval semantics | E2 affine mechanism + vocabulary packages | affine-scale mechanism |
| Decimal / fixed-point / rounding | E3 carrier + policy packages | the carrier itself |
| Calendar / work-period semantics | D3 data packages, E2 scales, kernels | — |

### Data and workbook migration boundaries

| Item | Candidate mechanisms | Core? |
|------|---------------------|-------|
| Schema-checked tabular import | D1, D2, B2 | — |
| Runtime text / opaque identifiers | A1/A2 text, E4 keys | an opaque-atom value kind |
| Typed row datasets with keys | E1, D1, A4 | likely type system |
| Join / group-by / pivot / sort | E1 packages or A4+kernels, C4 at scale | — |
| Missing-data schemas and policies | A2 validity bitmaps, D1 policies, A6 | possibly value model |
| Versioned engineering datasets | D3, D6, one-ledger theme | — |
| Time-series alignment / resampling | kernels + E4 indexes + D1 | — |
| Task-network / BOM analysis | E1 packages (needs graphs), B2 checks | — |
| Declarative calculation reports | D4, F3 attributes, G2 facts | — |
| Additional plot encodings | D4 + plot-spec-as-open-data | — |
| Data lineage / snapshot metadata | D1 hashing, D7 run records, one-ledger theme | — |
| Functional-core integration architecture | D5, G7, G2 | an ADR, mostly |

Read horizontally, roughly **32 of 39 items** are reachable primarily through
extension mechanisms; the residue that keeps pointing back at the core is
small and coherent: *one bounded-iteration construct, a couple of value kinds
(Complex, decimal carrier, opaque atoms, typed absence), affine unit scales,
Nat-engine growth, and the index/value-model questions around
ragged/lazy data.* That residue is a candidate definition of "what actually
belongs in the language" — everything else in #1401 could, under some option
above, be somebody's package.

## 5. Open questions for the design discussion

1. **Syntax: open or closed?** F1/F2 versus F5 is the most identity-defining
   choice in this catalog; F6/F4 are the hedges. Everything else composes
   with either answer.
2. **One encoding or several?** Adopt Arrow at the ABI itself (A2 — weight
   inside wasm kernels?) or keep the bespoke ABI and use Arrow only at the
   shells (D1/D4/D5)?
3. **Handles versus values.** Do opaque resources (A4) break "everything is
   a transparent value"? Is content-addressed replay a sufficient determinism
   story for factors, tables, RNG streams?
4. **Is a non-bit-reproducible tier acceptable at all** (A8), even clearly
   marked, in a language whose pitch is determinism?
5. **How much language growth before the language stops being small?** E1
   (generics, blueprints-as-values) competes directly with A4/C-protocols for
   the same use cases — source-reviewable libraries versus binary kernels.
6. **Where does law verification run** (B4/G6): load time (latency), lock
   time, CI-only?
7. **Protocol churn.** Freeze the solver protocol (C2) early, or incubate it
   in a `graphcal-contrib` space (G5) until it earns stability?
8. **Compile-time execution versus LSP latency.** B2/F2 put plugin execution
   on the keystroke-frequency check path — budget model?
9. **Where do capability grants live** — `graphcal.toml`, the lockfile, or
   split declaration/pin like plugins today (D6)?
10. **Declaration surface sprawl.** `import plugin`, `import data`,
    `use syntax`, emitters, lint packs… one unified `extension` declaration
    family, or one keyword per kind?
