# Graphcal Codebase Reading Guide

This guide is a map for reading the Graphcal source code as it exists today.
It focuses on the compiler/evaluator pipeline, the crate boundaries, and the
typed data structures that carry language semantics through the system.

## 1. Pipeline

### Representation Names

Compiler code commonly names data shapes by how far they are from source text
and how many semantic invariants they carry:

- **AST** means **Abstract Syntax Tree**. It is a tree-shaped representation of
  source syntax after parsing. An AST is still source-oriented: it preserves
  spans, syntactic forms, and source paths so tools can point back to what the
  user wrote.
- **IR** means **Intermediate Representation**. It is the general compiler term
  for any representation between syntax and execution; Graphcal does not use a
  bare `IR` phase type.
- **HIR** means **High-level Intermediate Representation**. `HirDag` is one
  canonically identified DAG body, and `HirProject` is the complete pre-check
  project. HIR remains close to source expression/type structure, but replaces
  source paths with canonical owner-qualified names, lexical IDs, and typed
  built-in variants.
- **TIR** means **Typed Intermediate Representation**. It is the type- and
  dimension-checked representation used by the evaluator. TIR combines HIR's
  declaration/DAG structure with resolved type expressions, dimension facts,
  and per-DAG compilation state.

These names are relative, not universal. Another compiler may use "HIR" or
"TIR" differently; in this codebase, read them as the boundaries above.

### Pipeline Rationale

A `.gcl` file cannot safely go straight from text to evaluation. Early stages
need to remember exactly what the user wrote, while later stages need to know
what those words mean in the whole project.

For example, this source reference:

```gcl
import helpers.physics
node force = helpers.physics.mass * acceleration
```

starts life as text with spans, then as a dotted path, and eventually as a
reference to one specific declaration owned by one specific `DagId`. Keeping
those steps separate prevents bugs where two files define the same leaf name, an
import alias changes, or a runtime map accidentally treats `a.mass` and
`b.mass` as the same value.

Read the pipeline as a sequence of practical questions:

1. **Parser / AST: what did the user write?**
   The AST keeps source spans, punctuation-sensitive shapes, and surface syntax
   needed by diagnostics, formatting, and the LSP. At this point `foo.bar` is
   still just a path written in the file.
2. **Desugared AST: what is the simpler form of that syntax?**
   Parser conveniences such as multi-declarations and table literals are
   expanded so later passes do not each need to understand every shortcut.
3. **Loader and `ModuleResolver`: which file or DAG does a module path mean?**
   The loader performs filesystem work and turns import/include paths into
   canonical `DagId`s, but it does not merge dependency ASTs into the root
   `File`. Each loaded file keeps its own `File<Desugared>`. The compiler-side
   `ModuleResolver` then answers semantic lookup questions without doing I/O.
   This keeps disk paths and import aliases out of the core compiler data.
4. **Unfrozen IR: what declarations does this DAG body contain?**
   The IR assembly stage groups the body into consts, params, nodes, asserts,
   and registry entries, still carrying syntactic bodies. This is where checks
   such as duplicate names, visibility, bindability, and declaration categories
   are easiest because they require seeing the whole DAG body — and where
   include instantiation rewrites reference paths and overrides replace param
   defaults, because both are syntactic operations that must happen before
   resolution.
5. **HIR at the freeze boundary: what does each reference point to?**
   `UnfrozenIR::freeze` is the single resolution stage of the pipeline. It
   lowers every assembled declaration body to HIR, classifying and resolving
   each reference path in one pass: declarations, dimensions, indexes,
   constructors, locals, generic params, and built-ins. After this boundary,
   code should not need to ask whether the string `"sum"`, `"PI"`, or
   `"helpers.physics.mass"` has a special meaning; it should pattern-match on
   typed values instead. The frozen `HirDag` carries its canonical `DagId` and
   no syntax-AST expression.
6. **TIR: is the program type- and dimension-correct?**
   TIR combines the declaration structure from HIR with its bodies, resolved
   type expressions, dimension facts, domain constraints, inline DAG bodies, and
   dependency DAGs. The dependency graph is derived from the HIR bodies. This
   is the checked program representation used to prepare execution.
7. **ExecPlan: what exact work should runtime evaluation do?**
   `ExecPlan` evaluates compile-time constants, sorts runtime declarations, and
   stores runtime-ready maps. The evaluator should not need to rebuild registries,
   re-resolve names, or recompute dependency order.

Some names in this pipeline can sound misleading if read too literally:

- `UnfrozenIR` is syntactic declaration assembly, not another copy of the AST.
- HIR is "high-level" because it keeps expression/type tree shapes. Each
  `HirDag` owns one canonically identified DAG module, while `HirProject` is the
  complete project-level phase value that owns those modules before checking.
- TIR is not just HIR with types; it is the checked, per-DAG program model.

`DagTIR` keeps source-facing declarations for diagnostics and presentation, but
semantic value bodies live in `DagSemanticBody`. Checked expression,
dependency, constructor, collection, inline-DAG, type-definition, and
declaration-binding facts are required fields of that semantic body and are
keyed by canonical `ResolvedName`/`DagId` identities where ownership matters.

Every `.gcl` file moves forward through the same core stages:

```text
Source text
  |
  |  crates/graphcal-compiler/src/syntax/lexer.rs
  v
Tokens with Span
  |
  |  crates/graphcal-compiler/src/syntax/parser/
  v
File<Raw>
  |
  |  crates/graphcal-compiler/src/desugar/
  |  crates/graphcal-compiler/src/syntax/desugar.rs
  v
File<Desugared>  (the final syntax-AST phase)
  |
  |  crates/graphcal-compiler/src/ir/          (declaration-shell checks,
  |  crates/graphcal-compiler/src/syntax/module_resolve.rs   assembly)
  v
UnfrozenIR + ModuleResolver
  |
  |  UnfrozenIR::freeze — the single resolution stage
  |  crates/graphcal-compiler/src/hir/
  v
HirDag  (one canonically identified DAG module)
  |
  |  crates/graphcal-eval/src/project_compiler/pipeline.rs
  v
HirProject  (every file-root and inline `HirDag`; no checked/runtime facts)
  |
  |  crates/graphcal-eval/src/project_compiler/checking.rs
  |  crates/graphcal-compiler/src/tir/
  v
CheckedProject / TIR  (DagTIR + DagSemanticBody)
  |
  |  crates/graphcal-eval/src/exec_plan.rs
  v
ExecPlan
  |
  |  crates/graphcal-eval/src/eval/runtime.rs
  v
RuntimeEvaluation  (SI values + contained errors + root result)
  |
  |  crates/graphcal-eval/src/eval/project/prepared.rs
  v
EvalResult  (public project output assembly)
```

The pipeline is forward-only: parser sugar is removed before IR assembly,
reference paths are lowered through HIR to canonical owners at the freeze
boundary before TIR/eval, and runtime maps use owner-qualified declaration
identities. Source `ScopedName`s
and spans are kept at diagnostics and formatting boundaries; semantic
compile/eval decisions use HIR and `ResolvedName`-based data.

### 1.1 AST Phases

The AST is parameterized by a `Phase` marker in
`crates/graphcal-compiler/src/syntax/phase.rs`.

```text
File<Raw> -> File<Desugared>
```

The marker controls only the slots that actually vary by phase:

| Slot        | Raw             | Desugared       |
| ----------- | --------------- | --------------- |
| `DeclSugar` | `RawDeclSugar`  | `Infallible`    |
| `ExprSugar` | `RawExprSugar`  | `Infallible`    |
| `RefSugar`  | `UnresolvedRef` | `UnresolvedRef` |

Type-level syntactic references such as type applications, dimension terms, and
index expressions are `NamePath` in every phase, so they are represented
directly as `NamePath` fields rather than as `Phase` associated types.

`File<Raw>` is produced by the parser and consumed by surface-aware tooling such
as the formatter. `File<Desugared>` has no multi-decl or table-literal sugar
and is the final syntax-AST phase: expression references stay syntactic
(`ExprKind::UnresolvedRef` paths, plus parser-produced `GraphRef`s), and
type-level paths, index paths, and match patterns are intentionally preserved
as structured paths. HIR lowering is the single stage that classifies and
resolves all of them against the lexical scope and the module resolver.

The alias module keeps signatures readable:

- `desugar/desugared_ast.rs` pins AST aliases to `Desugared`.

When a post-desugar match sees an impossible payload, use
`syntax::phase::never(x)` rather than a runtime `unreachable!()`.

### 1.2 Lexing and Parsing

The lexer is `logos`-based and produces tokens with byte-accurate `Span`s.
The parser is recursive descent with Pratt-style expression precedence.

Key files:

- `syntax/token.rs` defines the token vocabulary.
- `syntax/lexer.rs` wraps tokenization and lookahead.
- `syntax/parser/expr.rs` parses expressions.
- `syntax/parser/type_expr.rs` parses type and dimension expressions.
- `syntax/parser/decl/` contains per-declaration parsers.
- `syntax/parser/table.rs` parses table-literal surface syntax.

The parser emits `File<Raw>`. Declarations carry typed definition leaves where
possible (`DeclName`, `DimName`, `UnitName`, etc.), a `Visibility`, attributes,
a `DeclKind<Raw>`, and a source span. Reference positions that may be qualified
are parsed as `IdentPath`/`NamePath`, not as namespace-specific leaf names.
Match arms start as `MatchPattern::Path`; semantic categorization into
constructor patterns versus index-label patterns happens only when a resolver
has enough information to prove the kind.

### 1.3 Desugaring

Desugaring converts parser-only constructs into canonical AST forms:

- Multi-declarations expand into ordinary `param`, `node`, or `const node`
  declarations.
- Table literals become `ExprKind::MapLiteral`.

The generic phase walker lives in `desugar/convert.rs` and `desugar/mod.rs`.
The multi-declaration expander lives in `syntax/desugar.rs`.

### 1.4 Module Resolver and HIR Boundary

`syntax/module_resolve.rs` builds project-wide, owner-qualified symbol tables.
It stores one `ModuleSymbols` table per canonical `DagId` and one `ModuleScope`
per module for import/include aliases and selective imports. It resolves
syntactic `NamePath` / `IdentPath` values into canonical
`ResolvedName<namespace::...>` values or `ResolvedIndexVariant` values. Source
qualifier text is used only to find a scoped module/DAG binding; successful
lookups carry the canonical `DagId` owner. For inline-call paths, an imported
alias resolves to its exact file-root or inline-DAG target and is itself
callable; additional segments descend through child DAG modules.

`graphcal-eval/src/loader.rs` remains the only layer that resolves import paths
to files/DAGs. This is physical project loading and path resolution, not
semantic AST merging: each loaded file owns its own `File<Desugared>`, and inline
`dag` bodies are lifted into `LoadedDag`s. `LoadedProject::build_module_resolver()`
hands those already resolved edges to the pure compiler resolver, including
inline DAGs and instantiated include owners.

`crates/graphcal-compiler/src/hir/` is the semantic boundary after syntax and
the single resolution stage of the compiler:

- `hir/types.rs` models type expressions using `BuiltinType`,
  `ResolvedName`, normalized Nat forms, and lexical `GenericParamId`s.
- `hir/expr.rs` models value expressions using canonical declaration refs,
  constructor refs, `ResolvedIndexVariant`s, `LocalId`s, typed built-ins, and
  semantic `MatchPattern` variants. Its lowerer consumes desugared expressions
  directly and classifies every reference path in one pass: lexical locals,
  built-in constants and time scales, constructors, type-system names, generic
  `Nat` params, and declarations — resolving each to its canonical identity at
  the same time. Power operators also retain the parser's typed
  `PowerExponent` classification, backed by `ExactRational`, so checking and
  evaluation never infer an exact exponent from binary64 or source text.
- `hir/lower.rs` lowers syntax AST type references into HIR with a
  `ModuleResolver`, a `GenericScope`, and an optional prelude scope.

Expression lowering is diagnostic-accumulating: `lower_expr_tolerant` turns an
unresolvable reference into an explicit `hir::ExprKind::Error` node and records
the diagnostic, so IDE consumers keep working on incomplete code. The strict
`lower_expr` entry point rejects any tree containing an error node, so the
batch pipeline never observes one.

TIR stores HIR expressions and HIR-derived semantic metadata in
`DagSemanticBody`. Frozen declaration shells retain source-facing names, spans,
and provenance for diagnostics and editor presentation. Value annotations,
declaration bounds, nominal generic defaults, and nominal field signatures are
all already HIR.

### 1.5 IR Assembly and the Freeze Boundary

`ir/lower.rs` and `ir/resolve/` assemble a desugared AST into an `UnfrozenIR`,
and `UnfrozenIR::freeze` lowers it into a `HirDag`.

The assembly stage (syntactic, pre-resolution):

- Checks duplicate names, declaration naming rules, visibility, bindability,
  and attribute placement on declaration shells.
- Builds the leaf-keyed `Registry` for dimensions, units, indexes,
  struct/union types, and functions.
- Carries import metadata and canonical `HirImportedBinding` targets across
  file/DAG boundaries; checked types and optional constant values do not exist
  at this phase.
- Hosts include instantiation (`merge_dependency`: prefixing and index/type
  rebinding as reference-path rewrites) and override application, which must
  happen before resolution.

The freeze boundary (`UnfrozenIR::freeze(registry, owner, resolver, src)`):

- Lowers every nominal generic default, nominal field annotation/bound,
  const/param/node type annotation, declaration bound, and
  const/param/node/assert body to HIR (strict — an unresolvable reference fails
  the compile with a spanned definition-source diagnostic).
- Strictly lowers every plot/figure/layer expression; tolerant lowering is
  reserved for editor-facing incomplete buffers.
- Converts the frontend `Registry` into `SemanticRegistry`, whose Rust type
  omits syntax-backed nominal definitions and inline-DAG AST bodies. Canonical
  `NominalTypeRegistry` and typed child-DAG identities are the only authorities
  available after this point.

One `HirDag` represents one DAG body and stores its own canonical identity:
either a file root or an inline `dag` block. `HirProject` owns exactly one such
frozen module for every loaded file root and inline DAG. It retains typed root
and load-order identities plus a narrow borrow of loader-owned plugin
verification inputs; the loaded source AST arena does not cross this boundary.
Entry names stay source-shaped `ScopedName`s for presentation, but
value-declaration signatures, nominal definitions, and bodies are HIR;
owner-qualified declaration dependencies are collected from those HIR bodies
during TIR construction. TIR resolves and validates HIR nominal signatures; it
never lowers their syntax AST. Scope policies that need resolved
references (no runtime `@` in `const node` bodies, no `@assert` references, A10
variant literal rules) run when `HirProject` is consumed by checking.

### 1.6 TIR and Dimension Checking

`tir/typed.rs` resolves type annotations into semantic type expressions.
`tir/materialized_shape.rs` defines the checked total-cardinality policy carried
by concrete indexed expressions. `tir/presentation.rs` defines owner-qualified,
structurally nested display provenance and checked plot-channel dimensions.
`tir/dim_check/` infers and installs concrete value types, eager shape facts,
and presentation facts before evaluation.

In the module-aware project path, TIR resolution receives both a
`ModuleResolver` and the project-wide `ProjectTypeStore`. The resolver maps
source aliases to canonical identities; the store is the authoritative lookup
for owner-qualified dimensions, units, indexes, nominal types, and
constructors. Value-declaration bodies, type annotations, and domain bounds
arrive already lowered to HIR. TIR resolves canonical HIR type references
against the store without re-reading source paths; declaration bounds move into
checked semantic storage without another lowering pass. Before TIR
construction, each `HirImportedBinding` is replaced by
an atomic checked `ImportedBinding` carrying the same canonical target, its
resolved declared type, and an optional dependency constant value. TIR validates
that no lexical key or canonical target changed across the boundary. Checked
DAG body construction then consumes each HIR module.

The TIR is not flat:

```text
TIR
  registry                 // root frontend/formatting artifact
  project_type_store       // canonical owner-qualified definitions
  root_dag_id
  dags: HashMap<DagId, DagTIR>
  module_aliases
```

Each file root and inline `dag` body is represented by a `DagTIR`. Dependency
files are not merged at the AST stage; dependency DAG TIRs are merged into the
same `DagRegistry` during project lowering/finalization using their canonical
`DagId`s.

Each category-specific declaration record on `DagTIR` owns its HIR body exactly
once (`ConstEntry.expr`, `ParamEntry.default_expr`, `NodeEntry.expr`,
`AssertEntry.body`, and plot/figure/layer bodies). A private declaration index
maps canonical IDs to those records; no semantic side map clones bodies.
`DagSemanticBody` contains derived facts only:

- `semantic.dependencies`: owner-qualified declaration dependency maps.
- `semantic.collection_refs`: shared handles to canonical indexes used by
  map/table/index/match inference.
- `semantic.constructor_refs`: canonical constructor-call and constructor-match
  metadata backed by shared project-store type handles.
- `semantic.type_defs`: resolved field/default semantics plus shared handles to
  canonical nominal definitions.
- `semantic.decl_bindings`: visible `ScopedName` keys mapped to
  `ResolvedName<Decl>` identities at the source boundary.

Dimensions are exact exponent maps:

```text
Dimension = BTreeMap<BaseDimId, Rational>
```

Dimension inference is split by expression families under
`tir/dim_check/infer/` and operates on HIR expressions, consulting the semantic
body for canonical index/constructor/inline-DAG ownership.

### 1.7 Execution and Runtime Evaluation

Checking evaluates `const node` declarations and resolves top-level and
struct-field domain constraints once, retaining those immutable stores on the
checked project. Runtime preparation reuses the same stores and only builds the
topological `param`/`node` schedule.

Runtime execution is keyed by `RuntimeDeclKey`, which wraps canonical
`ResolvedName<Decl>` identities so same-leaf declarations from different DAGs do
not collide. Const and runtime declaration evaluation read the authoritative
`DagTIR` declaration records through typed ID-to-record indexes and use
`eval_expr/hir_eval.rs`. Assertions and visualization declarations are also
read directly from `DagTIR`, not copied into `ExecPlan`.

`eval/runtime.rs` evaluates declarations in topological order. A failed node is
contained as a `NodeError`; independent nodes can still evaluate. One
`RuntimeEvaluation` retains the SI-normalized value map, contained-error map,
and display-aware root result together. `PreparedProject` then assembles the
normal project-level `EvalResult`.

`graphcal dump` is deliberately only a debugging shell: each stage stops at an
existing pipeline boundary and pretty-prints that boundary's Rust `Debug`
representation. It does not define a serialization schema or a parallel
inspection model. `dump hir` prints the production `HirProject` continuation;
static checking consumes that same value to produce `CheckedProject`.

## 2. Workspace Map

The workspace contains seven Rust crates:

```text
graphcal-cli       binary/library: CLI shell
graphcal-lsp       binary/library: Language Server Protocol
graphcal-eval      evaluation, project orchestration, loader
graphcal-compiler  syntax, HIR, registry, IR, TIR
graphcal-fmt       formatter
graphcal-io        filesystem abstraction
graphcal-package   pure package manifest/lockfile domain model
```

The important dependency direction is:

```text
graphcal-cli
  -> graphcal-eval
  -> graphcal-fmt
  -> graphcal-io
  -> graphcal-package

graphcal-eval
  -> graphcal-compiler
  -> graphcal-io

graphcal-lsp
  -> graphcal-eval
  -> graphcal-compiler

graphcal-fmt
  -> graphcal-compiler

graphcal-package   # no Graphcal-internal crate dependencies
```

### 2.1 `graphcal-compiler`

The compiler crate owns the functional core through TIR.

| Path                          | Purpose                                                       |
| ----------------------------- | ------------------------------------------------------------- |
| `syntax/ast.rs`               | Phase-parameterized AST aggregate and re-exports              |
| `syntax/ast/common.rs`        | Shared AST nodes and typed common fields                      |
| `syntax/ast/value.rs`         | Expression/value AST definitions                              |
| `syntax/ast/decl.rs`          | Declaration AST definitions                                   |
| `syntax/ast/plot_props.rs`    | Syntax-level plot/figure/layer property names                 |
| `syntax/ast/format_equivalent.rs` | Surface-equivalence checks used by formatting/tooling     |
| `plot_props.rs`               | Semantic plot/mark/composition property registry              |
| `syntax/phase.rs`             | `Raw`, `Desugared`, sugar/path slots, `never`                 |
| `syntax/names.rs`             | `NameAtom`, typed name newtypes, paths, resolved names        |
| `nat.rs`                      | Normalized type-level Nat polynomial forms                    |
| `dag_id.rs`                   | Filesystem-independent DAG identity                           |
| `syntax/parser/`              | Parser for declarations, expressions, types, tables           |
| `syntax/module_resolve.rs`    | Owner-qualified module symbol tables and path resolution      |
| `desugar/`                    | Phase walker and the `Desugared` AST alias module             |
| `hir/`                        | Canonical semantic type/value expressions and lowering boundary |
| `hir/source_interface.rs`     | Direct parameter/node/index provenance retained through HIR     |
| `ir/instance.rs`              | Typed template-instance edges and binding environments         |
| `ir/lower.rs`                 | IR assembly, body-source provenance, strict HIR freeze boundary |
| `ir/resolve/`                 | Declaration-shell collection and validation                   |
| `registry/`                   | Dimensions, units, indexes, types, values, built-ins          |
| `tir/materialized_shape.rs`   | Checked total cardinality for eagerly materialized indexed values |
| `tir/presentation.rs`         | Structured owner-qualified display and plot-channel facts     |
| `tir/typed.rs`                | Typed semantic bodies, including atomic dynamic-unit entries   |
| `tir/dim_check/`              | Dimension/type inference, including scalar unit-scale checks   |
| `tir/dim_check/plot.rs`       | Plot/figure/layer dimension validation                        |
| `tir/dim_check/presentation.rs` | Checked presentation derivation from canonical HIR          |

### 2.2 `graphcal-eval`

The evaluation crate contains the loader shell, a compiler-facing pure project
checking layer, and the runtime evaluator. The checked boundary keeps source
elaboration out of runtime modules even though both currently share this crate.

| Path                              | Purpose                                                        |
| --------------------------------- | -------------------------------------------------------------- |
| `loader.rs`                       | `LoadedProject`, source arena, and loader-resolved module edges |
| `project_compiler/session.rs`     | Public `ProjectCompiler` and validated `CheckedProject`         |
| `project_compiler/imports.rs`     | Compile-time imports and typed instance requests                |
| `project_compiler/recursion.rs`   | Inline-DAG instance cycle detection                             |
| `project_compiler/generic_leakage.rs` | Include-boundary generic visibility checks                  |
| `project_compiler/template.rs`    | Canonical shared pre-HIR template store                         |
| `project_compiler/entry_interface.rs` | Checked syntax-independent entry-DAG runtime interface      |
| `project_compiler/model.rs`       | Internal typed pass artifacts and instance requests             |
| `project_compiler/hir_project.rs` | Complete resolved-project phase value                            |
| `project_compiler/lowering.rs`    | Pure per-file and inline-DAG HIR elaboration                     |
| `project_compiler/checking.rs`    | HIR-to-TIR interface resolution and mandatory static checks      |
| `project_compiler/pipeline.rs`    | Dependency-ordered HIR lowering and checking continuations       |
| `project_compiler/qualified_refs.rs` | Module-aware ambiguous reference classification             |
| `project_compiler/registry_merge.rs` | Frontend-only registry composition                          |
| `eval/project/prepare.rs`         | Checked-project to runtime-plan transition                      |
| `eval/project/model_schema.rs`    | Finite typed arena for recursive model-value definitions        |
| `eval/project/prepared.rs`        | Reusable typed binding and evaluation API                       |
| `eval/project/output.rs`          | Presentation-only output projection                            |
| `inline_dag.rs`                   | Inline-DAG self-import preprocessing                            |
| `decl_key.rs`                     | Runtime declaration keys backed by `ResolvedName<Decl>`         |
| `execution_facts.rs`              | Per-DAG checked constants, constraints, schedules, and source   |
| `exec_plan.rs`          | Const evaluation, runtime topological order, domain prep      |
| `domain_check.rs`       | Runtime and compile-time domain validation                    |
| `eval/runtime.rs`       | Evaluation loop                                               |
| `eval/display.rs`       | Application of checked structured presentation facts          |
| `eval/plot_data.rs`     | Runtime plot/figure/layer data extraction                     |
| `eval/public_projection.rs` | Fallible checked runtime-to-public value projection       |
| `eval/types.rs`         | Public `EvalResult`, `Value`, plot/assert result types        |
| `eval_expr/`            | HIR expression evaluation kernels by expression family        |
| `eval_expr/numeric.rs`  | Shared checked numeric helpers for expression evaluation      |
| `eval_expr/work_budget.rs` | Declaration-scoped kernel budgets and cancellation cadence  |
| `eval_expr/unit_scale.rs` | Dynamic unit-scale resolution and finite-quantity validation |
| `eval_expr/aggregations.rs` | Aggregation built-ins such as sum/mean/min/max/count     |
| `eval_expr/conversions.rs` | Unit/type conversion helpers                              |
| `eval_expr/hir_eval.rs` | HIR expression evaluator with canonical references            |
| `graph_ir/`             | Dependency-graph export model and DOT rendering               |

The public API is re-exported from `eval/mod.rs`, including
`compile_and_eval_project`, `compile_to_tir_project`, and
`compile_and_eval_from_project`.

### 2.3 `graphcal-fmt`

The formatter parses `File<Raw>` and prints it with the `pretty` crate.
Formatting modules are split by syntax family under `src/format/`.

### 2.4 `graphcal-io`

`graphcal-io` isolates filesystem access behind `FileSystemReader`.
Implementations include real, in-memory, and overlay filesystems. The loader
uses this crate so tests and editor integrations can run deterministically
without direct disk coupling.

### 2.5 `graphcal-package`

`graphcal-package` is a pure package-management domain crate. It has no Git,
filesystem, cache, or CLI I/O; callers provide manifest text, lockfile text,
source metadata, and materialized dependency manifests.

The crate owns typed package identifiers (`PackageName`, `DependencyName`,
`PackageInstanceId`, `GitCommitHash`, `GitUrl`), manifest parsing for
`graphcal.toml`, lockfile parsing/serialization for `graphcal.lock`, and
validation of the locked package graph. Keep credentials, cache paths, and Git
commands outside this crate; it should remain the functional core for package
resolution.

### 2.6 `graphcal-cli`

The CLI is the imperative shell around the library pipeline and package
management commands. The package has both a binary target (`main.rs`) and a small
library target (`lib.rs`) so tests and the binary can share pure format-discovery
helpers.

Subcommands:

| Command  | Purpose                                      |
| -------- | -------------------------------------------- |
| `eval`   | Compile and evaluate a `.gcl` file           |
| `check`  | Parse and type-check without evaluation      |
| `format` | Format files or check formatting             |
| `dump`   | Debug-print pipeline artifacts (experimental) |
| `graph`  | Export a dependency graph                    |
| `deps`   | Manage package dependencies / lockfiles      |
| `lsp`    | Start the language server                    |

Key files:

- `main.rs` owns command dispatch and exit-code behavior.
- `lib.rs` exposes the reusable CLI library surface.
- `format.rs` discovers `.gcl` files and classifies format status without printing.
- `deps.rs` is the imperative shell for `graphcal deps lock`: root discovery,
  Git/cache materialization, tree hashing, and writing `graphcal.lock` around
  `graphcal-package`'s pure model.
- `json_input.rs` reads bounded JSON input for params.
- `overrides.rs` parses `--set` and `--input` parameter overrides.
- `display.rs` renders normal eval text output.
- `dump.rs` selects one pipeline boundary and pretty-prints its unstable Rust
  `Debug` representation.
- `plot.rs` renders plot/figure/layer output.

### 2.7 `graphcal-lsp`

The LSP consumes compiler/evaluator APIs and adds editor-facing analysis:

| Path                        | Feature                                           |
| --------------------------- | ------------------------------------------------- |
| `server.rs`                 | Server lifecycle and `run_analysis()`             |
| `diagnostics.rs`            | Compiler/evaluator diagnostics to LSP diagnostics |
| `symbol_table.rs`           | Typed `SymbolKey` index from decl shells + tolerantly lowered HIR |
| `completion.rs`             | Completion                                        |
| `hover.rs`                  | Hover                                             |
| `goto_definition.rs`        | Go to definition                                  |
| `references.rs`             | Find references                                   |
| `rename.rs`                 | Rename                                            |
| `inlay_hints.rs`            | Computed value hints                              |
| `formatting.rs`             | Formatting provider                               |
| `document_symbols.rs`       | Outline symbols                                   |
| `document_links.rs`         | Import/include links                              |
| `signature_help.rs`         | Function signatures                               |
| `code_actions.rs`           | Quick fixes                                       |
| `cursor_context.rs`         | Cursor-sensitive context                          |
| `resolve.rs` / `convert.rs` | Shared resolution and protocol conversion helpers |

`run_analysis()` treats library files specially. A file with required params or
required indexes is not evaluated standalone, so diagnostics avoid surfacing
unbound input errors for files intended to be consumed through parameterized
includes.

### 2.8 Editors and Grammars

Syntax/editor surfaces live outside the Rust workspace:

- `grammar.ebnf` is the formal grammar source of truth.
- `graphcal-lang/tree-sitter-graphcal` contains the tree-sitter grammar and
  highlight queries.
- `graphcal-lang/vscode-graphcal` contains the VS Code extension and TextMate
  grammar.
- `graphcal-lang/zed-graphcal` contains the Zed extension and bundled grammar
  artifact.

When syntax changes, update these together with the compiler/parser and docs.

## 3. Core Data Structures

### 3.1 Typed Names

Identifier leaf segments are `NameAtom`s. Definition-site names are
`NameDef<Ns>` aliases in `syntax/names.rs`: `DeclName`, `DimName`, `UnitName`,
`StructTypeName`, `IndexName`, `FnName`, `FieldName`, `IndexVariantName`,
`ConstructorName`, `GenericParamName`, `LocalName`, `ModuleAliasName`, and
`PlotPropertyName`.

Use these newtypes for actual definition leaves only. Reference positions that
may be qualified stay as `IdentPath`/`NamePath` until module-aware resolution
produces `ResolvedName<Ns>` or `ResolvedIndexVariant`. `ResolvedName<Ns>` is the
core owner-qualified identity: a canonical `DagId` owner plus a namespace-typed
leaf atom. `ResolvedIndexVariant` stores the resolved index identity plus the
variant leaf.

`ScopedName` carries legacy declaration lookup/display paths structurally as
qualifier segments plus a member. Its dotted `Display` form is a boundary
representation for diagnostics and protocols, not a string to split in the
functional core.

### 3.2 Module Resolver and HIR

`ModuleResolver` is a pure, project-wide symbol resolver:

```text
ModuleResolver
  modules: HashMap<DagId, ModuleSymbols>
  scopes: HashMap<DagId, ModuleScope>

ModuleSymbols
  owner: DagId
  decls, dimensions, units, struct_types, indexes, constructors

ModuleScope
  module_aliases
  selected_decls, selected_dimensions, selected_units
  selected_struct_types, selected_indexes, selected_constructors
```

It is built from loader-resolved import/include edges, not from filesystem I/O.
It enforces visibility at module boundaries and returns `ResolvedName` /
`ResolvedIndexVariant` values for successful lookups.

HIR is the first layer where references are intended to be truly semantic:

- Module-owned names carry `ResolvedName<Ns>`.
- Index labels carry `ResolvedIndexVariant`.
- Generic parameters carry `GenericParamId`, not module names.
- Expression locals carry `LocalId`.
- Built-ins use closed enums such as `BuiltinType`, `BuiltinConst`, and
  `BuiltinFnName`.
- Match patterns are semantic (`Constructor` or `IndexLabel`) after HIR
  lowering; syntax-only `MatchPattern::Path` does not cross the HIR boundary.

### 3.3 DAG Identity

`dag_id.rs` defines `DagId`, the canonical identity for file roots and
inline DAGs. It is an opaque package identity plus a non-empty sequence of
module segments, not a path string. Virtual single-file projects,
manifest-backed packages, locked dependency instances, and synthetic test
contexts all receive package ids at the loader/test boundary; there is no
package-less DAG, and the compiler core does not inspect the package id's
origin.

Examples:

- `helpers/math.gcl` in package `math` becomes
  `DagId(package = "math", segments = ["helpers", "math"])`.
- `dag burn { ... }` inside that file becomes
  `DagId(package = "math", segments = ["helpers", "math", "burn"])`.

Filesystem paths are converted to `DagId` at loader boundaries. Compiler and
evaluator internals should use `DagId` rather than `PathBuf` when referring to
compiled modules or DAG bodies.

### 3.4 Loader Types

`graphcal-eval/src/loader.rs` parses and resolves a project into:

```text
LoadedProject
  files: HashMap<DagId, LoadedFile>
  root: DagId
  load_order: Vec<DagId>  // dependencies before dependents

LoadedFile
  path: PathBuf
  dag_id: DagId
  source: Arc<String>
  ast: File<Desugared>
  named_source: NamedSource<Arc<String>>
  resolved_imports: HashMap<ModulePathKey, DagId>
  inline_dags: Vec<LoadedDag>

LoadedDag
  dag_id: DagId
  parent_dag_id: DagId
  name: String
  body: Vec<Declaration<Desugared>>
  resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>
```

`ModulePathKey` stores import/include path segments as a vector. It avoids using
joined strings as map keys inside the loader. `LoadedProject::build_module_resolver()`
then turns the loaded files, inline DAGs, and pre-resolved edges into the
compiler's `ModuleResolver`.

The loader's `ast: File<Desugared>` field is per file. It means "this source
file has been parsed, desugared, and connected to loader-side import/include
edges"; it does not mean imported declarations have been copied into the root
file AST, and reference resolution has not happened yet — that is the freeze
boundary's job.

### 3.5 IR

`IR` contains the semantic declaration lists for one DAG body, with bodies
already lowered to HIR at `UnfrozenIR::freeze`:

```text
UnfrozenIR             // assembly stage: syntactic bodies
  consts, params, nodes, asserts, plots, figures, layers  (desugared AST)
  + merge_dependency, add_*_alias, override_param_default

IR = UnfrozenIR::freeze(registry, owner, resolver, src)
  registry: Registry
  consts, params, nodes, asserts          (hir::Expr / hir::AssertBody bodies)
  plots, figures, layers                  (LoweredPlotBody / lowered fields)
  source_order: Vec<(ScopedName, DeclCategory)>
  assert_names
  assumes_map
  expected_fail: HashMap<ScopedName, ParsedExpectedFailMetadata>
    parsed keys + authored resolution owner + source provenance
  imported_bindings: HashMap<ScopedName, ImportedBinding>
    canonical target + declared type + optional pre-evaluated value
  external_surface: ExternalDeclSurface
    explicit_exports  // declarations carrying `pub` / `pub(bind)`
    input_ports       // bindable and projectable annotation-free params
```

There are no dependency maps on `IR`: the owner-qualified dependency graph is
collected from the HIR bodies during TIR construction and stored on
`DagTIR::semantic.dependencies` (value sets use `BTreeSet` so DAG construction
is deterministic).

### 3.6 Registry

`registry/types.rs` defines the frozen, leaf-keyed `Registry`:

```text
Registry
  dimensions
  units
  indexes
  types
  functions
  prelude
```

Associated registry modules define declared types, runtime values, built-ins,
formatting, manifest parsing, and time scales.

`tir/typed.rs` also defines `ProjectTypeStore`, the authoritative
owner-qualified semantic store used during module-aware TIR resolution. It keys
dimensions, units, indexes, nominal types, and constructors by `ResolvedName`.
The project compiler creates it once, installs the synthetic Graphcal prelude,
and adds only native definitions from each resolver module. Imported spellings
remain resolver bindings; they are not copied into the store under importer
owners. The leaf-keyed `Registry` remains confined to frontend declaration
construction, finite-index support, time zones, and formatting metadata.

### 3.7 TIR and `DagTIR`

`TIR` wraps one file-scoped registry and all DAGs reachable from that file.

```text
TIR
  registry: Registry                 // frontend/formatting boundary
  project_type_store: ProjectTypeStore
  root_dag_id: DagId
  dags: HashMap<DagId, DagTIR>
  module_aliases: HashMap<ModuleAliasName, DagId>

DagTIR
  dag_id: DagId
  consts, params, nodes, asserts, plots, figures, layers
  semantic: DagSemanticBody
  source_order
  assert_names, assumes_map
  expected_fail: HashMap<ScopedName, ResolvedExpectedFailMetadata>
    canonical keys + authored diagnostic source
  resolved_decl_types
  semantic.domain_bounds  // checked, unevaluated HIR bound expressions
  imported_bindings: HashMap<ScopedName, ImportedBinding>
  projectable_outputs  // explicit node exports + param input ports
```

`TIR::root()` and `TIR::root_mut()` access the file root. `TIR::lookup_call_target`
and `TIR::resolve_call_path` resolve inline DAG call paths through same-file
children or `module_aliases`. Module-aware callers should prefer
`DagTIR::semantic.inline_dag_refs` when evaluating a specific expression because
it already carries canonical call routing.

### 3.8 ExecPlan

`ExecPlan` is the runtime-ready form of a root `DagTIR`:

```text
ExecPlan
  const_values: Arc<RuntimeValueMap>  // retained checked fact store
  imported_values: RuntimeValueMap
  topo_order: Vec<RuntimeDeclKey>
  assumes_map
  expected_fail
  domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>
  struct_field_constraints: Arc<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>
```

It contains no cloned HIR bodies and no parser or registry-building work;
evaluation reads declaration/assertion/visualization records from the checked
`TIR`. `ResolvedDomainConstraint` lives in `graphcal-eval/src/domain_check.rs` and has
separate quantity (`f64`), integer (`i64`), and same-scale datetime instant
representations, so constraint families cannot mix after resolution.
`RuntimeDeclKey` lives in `graphcal-eval/src/decl_key.rs` and keeps runtime maps
keyed by canonical declaration identity.

### 3.9 Runtime Values

There are two value layers:

- `RuntimeValue` is internal and unit-normalized. It carries no display-unit
  metadata, but label/struct/indexed values carry type identity through
  `IndexTypeRef` / `StructTypeRef` so owner-qualified type/index identity is
  preserved during evaluation.
- `Value` is user-facing and appears in `EvalResult`. Quantity values carry a
  dimension and optional display-unit information; labels, structs, and indexed
  values keep public identity carriers for diagnostics/output.

`TypeNameRef<Ns>` in `registry/declared_type.rs` is the shared identity carrier
for declared type-level runtime/public values. It stores both a display leaf and
a canonical `ResolvedName<Ns>`.

Index type references are split by semantic kind. Declared indexes use
`TypeNameRef<namespace::Index>` and have a canonical `ResolvedName<Index>`.
Compiler-generated structural indexes use `FiniteIndexRef`: concrete `Fin(N)`
forms carry a validated non-zero `FiniteIndex`, while symbolic generic forms
carry a normalized `NatPolyForm` from `nat.rs`. Finite indexes intentionally do
not have fake resolved names or registry-key strings; callers that need declared
index ownership use `declared_resolved()`, and callers that need finite-index
semantics use `finite_index_ref()` / `finite_index()` /
`finite_index_form()`.

## 4. Declarations, Visibility, and Evaluation

Ordinary declarations are private unless explicitly exported. `pub` means
exported at a module/include boundary, and `pub(bind)` means exported and
bindable. Required `dim`, `type`, and `index` declarations must be `pub(bind)`.
A `param` is separate from that visibility matrix: the declaration kind itself
creates an annotation-free named input port. A missing default makes the input
port required. Its effective value is projectable like an exported value, but
its role remains distinct from declarations carrying an explicit `pub` marker.

| Category          | Main syntax                                | Evaluation phase                                | Reference rules                     |
| ----------------- | ------------------------------------------ | ----------------------------------------------- | ----------------------------------- |
| Type system       | `base dim`, `dim`, `unit`, `type`, `index` | Registry build                                  | Other type-system declarations      |
| DAG               | `dag`                                      | Compiled per body, instantiated by include/call | Own declarations, imports, includes |
| Const node        | `const node`                               | Compile time                                    | Const nodes and built-ins           |
| Param             | `param`                                    | Runtime input/default                           | Consts, params, nodes               |
| Node              | `node`                                     | Runtime computed                                | Consts, params, nodes               |
| Assert            | `assert`                                   | After runtime values                            | Consts, params, nodes               |
| Plot/Figure/Layer | `plot`, `figure`, `layer`                  | After runtime values                            | Consts, params, nodes               |

`@name` references graph values: params, nodes, and const nodes. Built-in
constants such as `PI`, `E`, `TAU`, `SQRT2`, `LN2`, and `LN10` are bare names.

## 5. Import, Include, and Project Loading

Graphcal has separate mechanisms for compile-time names and runtime instances:

- `import` is compile-time-only. It brings constants, static units, dimensions,
  types, indexes, constructors, and callable DAG blueprints into scope. It
  never creates a default runtime instance, evaluates assertions, or exports a
  runtime-dependent unit scale.
- `include` creates an explicit DAG instance, with optional value/index/type/
  dimension bindings. Assertions belong to that concrete instance. A module
  that declares a runtime-dependent unit cannot be included (`M026`): dynamic
  units stay encapsulated inside an entry or explicitly called DAG.

Import/include paths are dot-separated module paths in source. Loader internals
drop spans and store path segments in `ModulePathKey`; compiled DAG identity is
stored in `DagId`.

Project loading:

1. Determine the project root from `graphcal.toml`, an explicit root, or loose
   single-file mode.
2. Parse and desugar each file.
3. Resolve import/include paths to `DagId`s.
4. Index inline `dag` blocks as `LoadedDag`s. Each entry holds a validated
   locator into its file AST; the AST remains the sole owner of the body.
5. Build a dependency-first `load_order`.
6. Detect circular imports during traversal.

After project loading, the compiler performs several assembly steps. Do not read
these as repeated merges of one giant project AST. They merge different products
at the stage where each product first has the information it needs:

| Step                          | Merged Product                                                   | Stage                             | Why Here                                                                                                                       |
| ----------------------------- | ---------------------------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Import scope assembly         | canonical constant targets and module/source aliases             | before current-file HIR lowering  | HIR records canonical targets only; checked types and optional constant values are attached later.                             |
| Canonical type-store assembly | native owner-qualified dimensions, units, indexes, and types      | complete HIR -> TIR checking      | Every HIR module contributes before checking, so resolver aliases point to one canonical definition without checked dependencies feeding lowering. |
| Frontend registry seeding     | leaf/alias views needed to build local declarations               | local IR registry builder         | This is a construction boundary only; copied frontend views are filtered out of the canonical store.                            |
| Module-template elaboration   | one shared `UnfrozenIR`/frontend-registry template per canonical `DagId` | project-session template store | Repeated include/call sites reuse import processing and body lowering rather than recompiling the source template.              |
| Instantiated include assembly | instance-specialized declarations plus a typed binding environment | unfrozen IR builder             | The current monomorphizing implementation exposes declarations to the importer graph while retaining the template/instance edge. |
| Dependency DAG attachment     | already-compiled dependency `DagTIR`s keyed by canonical `DagId` | TIR finalization                  | Cross-file DAG calls need callable checked templates, but those templates remain separate owners.                              |

The invariant is that each source `File<Desugared>` AST owns its bodies once;
`LoadedDag` indexes those bodies rather than cloning them. `import` assembles
lexical bindings only. A canonical `ModuleTemplateStore` elaborates each file or
inline-DAG template once per project session and shares it through `Arc`.
Instantiated `include` or call sites clone that immutable template only at the
specialization boundary and record an `InstanceRecord`: its `InstanceId` pairs
the canonical template with a fresh concrete owner, while
`InstanceBindingEnvironment` carries value, index, type, and dimension
substitutions. The current evaluator still monomorphizes declarations into the
importer, but semantic declaration records carry the explicit concrete owner;
source-facing prefixes are lookup/presentation names, not the source of semantic
identity. Runtime-dependent units are deliberately excluded from this include
composition model rather than fabricating instance-qualified unit definitions.

`ProjectCompiler` builds the `ModuleResolver` once and lowers every loaded
module into `HirProject` using HIR-only dependency interfaces. HIR import
bindings contain canonical targets but cannot express checked types or values.
Checking then builds one `ProjectTypeStore` from the complete HIR project,
attaches checked imported interfaces, consumes each HIR body into TIR, and runs
all mandatory checks. Dependency aliases never become importer-owned semantic
definitions. Each `HirDag` also retains an ordered typed record of parameter,
node, and index declarations authored directly in that DAG, excluding merged
include declarations. Checking attaches declared types, runtime keys, defaults,
visibility, and required-index facts to that record. A successful
`CheckedProject` therefore retains the final TIR, constant pool, resolved
constraints, and complete checked entry interface; runtime preparation never
consults the desugared root AST.

Package locking is adjacent to, not part of, this compile/eval pipeline. The
`graphcal deps lock` shell materializes Git dependencies and writes
`graphcal.lock`; the pure package facts and lock graph validation live in
`graphcal-package`. The loader/project compiler should consume already-resolved
filesystem/module inputs rather than run Git or parse lockfile conventions in
the compiler core.

## 6. Errors

Errors use `miette` diagnostics with source snippets, spans, labels, and codes.

Common layers:

```text
CompileError
  Parse(ParseError)
  Eval(GraphcalError)

GraphcalError
  DuplicateName
  CyclicDependency
  DimensionMismatch
  TypeAnnotationMismatch
  UnknownUnit
  ImportRuntimeItem
  ImportPrivateItem
  RequiredItemMustBePub
  PrivateInPublic
  PubIndexVariantLiteral
  ...

ModuleResolveError
  DuplicateModule
  DuplicateSymbol
  UnknownName
  PrivateName
  AmbiguousIndexVariant
  UnexpectedDeclKind
  ...
```

`ModuleResolveError` is produced by the pure module resolver and mapped to
`CompileError`/`GraphcalError` at project boundaries. Each diagnostic carries a
`NamedSource<Arc<String>>` for rich output. Error codes such as `D001`, `V001`,
and `M020` are searchable in the source.

## 7. Tests and Fixtures

Tests use a mix of inline unit tests, snapshot tests, integration tests, and
property tests.

Fixture categories are enforced by
`crates/graphcal-cli/tests/cli.rs`:

- `tests/fixtures/valid/` checks and evaluates cleanly.
- `tests/fixtures/valid_library/` checks cleanly but is not meant to evaluate
  standalone.
- `tests/fixtures/runtime_error/` checks cleanly and fails at runtime.
- `tests/fixtures/invalid/` fails during static checking.

Important test locations:

- `crates/graphcal-package/src/lib.rs` (manifest/lockfile/package-graph unit tests)
- `crates/graphcal-eval/src/graph_ir/` (graph export unit tests)
- `crates/graphcal-eval/tests/error_snapshots.rs`
- `crates/graphcal-eval/tests/edge_case_bugs.rs`
- `crates/graphcal-eval/tests/phase0_regressions.rs`
- `crates/graphcal-eval/tests/declaration_order.rs`
- `crates/graphcal-fmt/tests/format_tests.rs`
- `crates/graphcal-cli/tests/cli.rs`
- `crates/graphcal-cli/tests/dump.rs`

Useful commands:

```bash
cargo test --workspace
cargo test -p graphcal-compiler
cargo test -p graphcal-package
cargo insta review
just lint
```

## 8. Conventions Worth Keeping in Mind

| Convention                       | Where                                  | Why                                                                 |
| -------------------------------- | -------------------------------------- | ------------------------------------------------------------------- |
| AST phases                       | `syntax/phase.rs`                      | Parser-only constructs are statically excluded downstream           |
| `NameAtom` / `NameDef<Ns>`       | `syntax/names.rs`                      | Definition leaves cannot accidentally contain dotted paths          |
| `NamePath` / `IdentPath`         | syntax AST                             | Preserve source qualification until a resolver has module context   |
| `ResolvedName` / variants        | HIR/TIR/eval                           | Carry canonical owner identity instead of source alias strings      |
| `ModuleResolver`                 | `syntax/module_resolve.rs`             | Keep module lookup pure and owner-qualified                         |
| HIR                              | `hir/`                                 | The single resolution stage: consume syntax paths once              |
| `DagId`                          | `dag_id.rs`                            | Keep filesystem paths at loader boundaries                          |
| `ModulePathKey`                  | `loader.rs`                            | Keep module paths structured instead of separator-joined            |
| `RuntimeDeclKey`                 | `crates/graphcal-eval/src/decl_key.rs` | Prevent runtime value collisions across DAG owners                  |
| `TypeNameRef` identity carriers  | `registry/declared_type.rs`            | Preserve declared index/struct owners through runtime/public values |
| Finite-index identity carriers   | `nat.rs`, `registry/declared_type.rs` | Keep concrete/symbolic `Fin` axes typed, not fake resolved names    |
| Trait-based I/O                  | `graphcal-io`                          | Deterministic tests and editor integration                          |
| Package identifier newtypes      | `graphcal-package`                     | Keep package/alias/instance/Git identities typed                    |
| Visitor pattern                  | `syntax/visitor.rs`                    | Centralized AST traversal                                           |
| `BTreeSet` in dep values         | IR/TIR deps                            | Deterministic graph construction                                    |
| `IndexMap` in output-facing maps | eval/display output                    | Stable user-facing order                                            |
| Separate const/runtime phases    | `exec_plan.rs`                         | Compile-time values and runtime values have different failure modes |
| Display units outside dimensions | `eval/display.rs`                      | Compute in SI, display in requested units                           |

When adding a feature, update the grammar, parser, compiler stages, evaluator,
LSP/editor surfaces, docs, and fixtures together. The compiler core should carry
semantic distinctions as types, not string conventions.

## 9. Suggested Reading Order

All Rust files in the workspace, in library-consumer order: every `use`d file appears before the file that imports it, so by the time you read a file you have already seen everything it imports. The order was derived from the actual `use`/`pub use` graph (re-exports resolved to the defining file) by `./internals/reading-order.py`; re-run it after refactors to regenerate the order (stage headings are curated by hand as contiguous slices of its output). Keep this order in sync with dependency-affecting refactors so `AGENTS.md`'s topological-ordering guidance remains actionable. A few groups are mutually dependent and cannot be fully ordered; they are marked with a note and ordered with the most reusable definitions first. `mod.rs`/`lib.rs` files that only declare submodules carry no imports and appear early as a table of contents for their subtree. The preceding sections provide the conceptual map.

### Stage 0 - Module maps and dependency-free leaves

1. `crates/graphcal-compiler/src/syntax/attribute.rs`
2. `crates/graphcal-compiler/src/syntax/non_empty.rs`
3. `crates/graphcal-compiler/src/syntax/phase.rs`
4. `crates/graphcal-compiler/src/syntax/mod.rs`
5. `crates/graphcal-compiler/src/desugar/mod.rs`
6. `crates/graphcal-compiler/src/registry/mod.rs`
7. `crates/graphcal-compiler/src/registry/time_scale.rs`
8. `crates/graphcal-compiler/src/registry/time_zone.rs`
9. `crates/graphcal-compiler/src/ir/mod.rs`
10. `crates/graphcal-compiler/src/tir/mod.rs`
11. `crates/graphcal-compiler/src/source_line.rs`
12. `crates/graphcal-compiler/src/text_position.rs`
13. `crates/graphcal-compiler/src/lib.rs`
14. `crates/graphcal-compiler/src/datetime_literal.rs`
15. `crates/graphcal-compiler/src/builtin.rs`
16. `crates/graphcal-compiler/src/complex_value.rs`
17. `crates/graphcal-compiler/src/dag_id.rs`

### Stage 1 - Names, spans, tokens, lexer, and syntax-domain leaves

Note: `token.rs`, `comments.rs`, and `lexer.rs` are mutually dependent.

1. `crates/graphcal-compiler/src/syntax/names.rs`
2. `crates/graphcal-compiler/src/syntax/import_category.rs`
3. `crates/graphcal-compiler/src/stack.rs`
4. `crates/graphcal-compiler/src/syntax/ast/plot_props.rs`
5. `crates/graphcal-compiler/src/syntax/decl_name.rs`
6. `crates/graphcal-compiler/src/syntax/span.rs`
7. `crates/graphcal-compiler/src/syntax/token.rs`
8. `crates/graphcal-compiler/src/syntax/comments.rs`
9. `crates/graphcal-compiler/src/syntax/lexer.rs`
10. `crates/graphcal-compiler/src/syntax/parser/token_stream.rs`
11. `crates/graphcal-compiler/src/syntax/function_name.rs`
12. `crates/graphcal-compiler/src/syntax/plugin.rs`
13. `crates/graphcal-compiler/src/syntax/index_name.rs`
14. `crates/graphcal-compiler/src/syntax/local_name.rs`
15. `crates/graphcal-compiler/src/syntax/module_name.rs`
16. `crates/graphcal-compiler/src/syntax/dimension.rs`
17. `crates/graphcal-compiler/src/syntax/type_name.rs`
18. `crates/graphcal-compiler/src/nat.rs`

### Stage 2 - Core AST, parser entry, and traversal

Note: `syntax/ast/value.rs`, `syntax/ast/decl.rs`, `syntax/ast/format_equivalent.rs`, `syntax/ast.rs`, and `syntax/parser/mod.rs` are mutually dependent; they are ordered with reusable AST pieces first.

1. `crates/graphcal-compiler/src/syntax/ast/common.rs`
2. `crates/graphcal-compiler/src/exact_rational.rs`
3. `crates/graphcal-compiler/src/dimension.rs`
4. `crates/graphcal-compiler/src/cancellation.rs`
5. `crates/graphcal-compiler/src/syntax/ast/value.rs`
6. `crates/graphcal-compiler/src/syntax/ast/decl.rs`
7. `crates/graphcal-compiler/src/syntax/ast/format_equivalent.rs`
8. `crates/graphcal-compiler/src/syntax/ast.rs`
9. `crates/graphcal-compiler/src/syntax/parser/mod.rs`
10. `crates/graphcal-compiler/src/syntax/visitor.rs`

### Stage 3 - Parser submodules and surface desugaring

1. `crates/graphcal-compiler/src/syntax/parser/compound.rs`
2. `crates/graphcal-compiler/src/syntax/parser/type_expr.rs`
3. `crates/graphcal-compiler/src/syntax/parser/expr.rs`
4. `crates/graphcal-compiler/src/syntax/parser/table.rs`
5. `crates/graphcal-compiler/src/syntax/parser/decl/dim_unit.rs`
6. `crates/graphcal-compiler/src/syntax/parser/decl/index.rs`
7. `crates/graphcal-compiler/src/syntax/parser/decl/type_decl.rs`
8. `crates/graphcal-compiler/src/syntax/parser/decl/import.rs`
9. `crates/graphcal-compiler/src/syntax/parser/decl/dag.rs`
10. `crates/graphcal-compiler/src/syntax/parser/decl/layer.rs`
11. `crates/graphcal-compiler/src/syntax/parser/decl/plot.rs`
12. `crates/graphcal-compiler/src/syntax/parser/decl/figure.rs`
13. `crates/graphcal-compiler/src/syntax/parser/decl/tests.rs`
14. `crates/graphcal-compiler/src/syntax/desugar.rs`

### Stage 4 - Desugared AST phase

1. `crates/graphcal-compiler/src/desugar/desugared_ast.rs`
2. `crates/graphcal-compiler/src/desugar/convert.rs`

### Stage 5 - Registry and module resolution

Note: `registry/types.rs` and `registry/prelude.rs` are mutually dependent. `registry/types.rs` owns only the aggregate registry and builder while re-exporting the domain registries for compatibility.

1. `crates/graphcal-compiler/src/registry/format.rs`
2. `crates/graphcal-compiler/src/function_signature.rs`
3. `crates/graphcal-compiler/src/registry/dag.rs`
4. `crates/graphcal-compiler/src/registry/dimension_registry.rs`
5. `crates/graphcal-compiler/src/registry/index.rs`
6. `crates/graphcal-compiler/src/registry/type_def.rs`
7. `crates/graphcal-compiler/src/registry/unit.rs`
8. `crates/graphcal-compiler/src/registry/types.rs`
9. `crates/graphcal-compiler/src/registry/prelude.rs`
10. `crates/graphcal-compiler/src/registry/declared_type.rs`
11. `crates/graphcal-compiler/src/registry/resolve_types.rs`
12. `crates/graphcal-compiler/src/registry/error.rs`
13. `crates/graphcal-compiler/src/registry/runtime_value.rs`
14. `crates/graphcal-compiler/src/syntax/module_resolve.rs`

### Stage 6 - Name resolution (IR)

1. `crates/graphcal-compiler/src/ir/resolve/names.rs`
2. `crates/graphcal-compiler/src/ir/resolve/deps.rs`
3. `crates/graphcal-compiler/src/ir/resolve/mod.rs`

### Stage 7 - HIR and builtin signatures

Note: `hir/types.rs`, `hir/lower.rs`, `hir/expr.rs`, and `hir/mod.rs` form a mutually dependent group. `registry/builtins.rs` is upstream of HIR and provides evaluation/dimension signatures for the built-in domain model.

1. `crates/graphcal-compiler/src/hir/source_interface.rs`
2. `crates/graphcal-compiler/src/registry/builtins.rs`
3. `crates/graphcal-compiler/src/hir/types.rs`
4. `crates/graphcal-compiler/src/hir/diagnostics.rs`
5. `crates/graphcal-compiler/src/hir/lower.rs`
6. `crates/graphcal-compiler/src/hir/expr.rs`
7. `crates/graphcal-compiler/src/hir/nominal.rs`
8. `crates/graphcal-compiler/src/hir/mod.rs`
9. `crates/graphcal-compiler/src/tir/dim_check/builtins.rs`

### Stage 8 - IR lowering, TIR, dimension checking, and late surface helpers

Note: `tir/typed/model.rs`, `tir/typed/type_expr.rs`, `tir/typed/collect.rs`, `tir/dim_check/helpers.rs`, `tir/typed/ops.rs`, `tir/typed.rs`, and `tir/dim_check/mod.rs` are mutually dependent. `decl/multi.rs` and `decl/mod.rs` are also mutually dependent and appear here because their actual imports depend on AST aggregate/re-export files that sort after the core checker files.

1. `crates/graphcal-compiler/src/ir/imported_binding.rs`
2. `crates/graphcal-compiler/src/ir/instance.rs`
3. `crates/graphcal-compiler/src/ir/override_reconciliation.rs`
4. `crates/graphcal-compiler/src/ir/lower.rs`
5. `crates/graphcal-compiler/src/tir/materialized_shape.rs`
6. `crates/graphcal-compiler/src/tir/presentation.rs`
7. `crates/graphcal-compiler/src/tir/typed/model.rs`
8. `crates/graphcal-compiler/src/tir/typed/type_expr.rs`
9. `crates/graphcal-compiler/src/tir/typed/collect.rs`
10. `crates/graphcal-compiler/src/tir/dim_check/helpers.rs`
11. `crates/graphcal-compiler/src/tir/typed/ops.rs`
12. `crates/graphcal-compiler/src/tir/typed.rs`
13. `crates/graphcal-compiler/src/tir/dim_check/mod.rs`
14. `crates/graphcal-compiler/src/tir/typed/tests.rs`
15. `crates/graphcal-compiler/src/ir/resolve/tests.rs`
16. `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
17. `crates/graphcal-compiler/src/tir/dim_check/infer/complex.rs`
18. `crates/graphcal-compiler/src/tir/dim_check/infer/builtin_call.rs`
19. `crates/graphcal-compiler/src/tir/dim_check/tests.rs`
20. `crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs`
21. `crates/graphcal-compiler/src/tir/dim_check/infer/linear_algebra.rs`
22. `crates/graphcal-compiler/src/tir/dim_check/infer/hir.rs`
23. `crates/graphcal-compiler/src/syntax/parser/decl/multi.rs`
24. `crates/graphcal-compiler/src/syntax/parser/decl/mod.rs`
25. `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
26. `crates/graphcal-compiler/src/plot_props.rs`
27. `crates/graphcal-compiler/src/plot_shape.rs`
28. `crates/graphcal-compiler/src/tir/dim_check/plot.rs`
29. `crates/graphcal-compiler/src/tir/dim_check/model_schema.rs`
30. `crates/graphcal-compiler/src/tir/dim_check/presentation.rs`

### Stage 9 - Filesystem abstraction (`graphcal-io`)

Note: all five files are mutually dependent; `lib.rs` comes last because it re-exports and ties together the implementations.

1. `crates/graphcal-io/src/in_memory_fs.rs`
2. `crates/graphcal-io/src/real_fs.rs`
3. `crates/graphcal-io/src/overlay_fs.rs`
4. `crates/graphcal-io/src/source_tree.rs`
5. `crates/graphcal-io/src/lib.rs`

### Stage 10 - Package domain model (`graphcal-package`)

1. `crates/graphcal-package/src/lib.rs`

### Stage 11 - Plugin ABI protocol (`graphcal-plugin-abi`)

Note: the three files are mutually dependent (`lib.rs` owns the protocol constants and re-exports); the codec modules come first because they carry the substance.

1. `crates/graphcal-plugin-abi/src/section.rs`
2. `crates/graphcal-plugin-abi/src/manifest.rs`
3. `crates/graphcal-plugin-abi/src/lib.rs`

### Stage 12 - Plugin authoring SDK and proc-macro (`graphcal-plugin*`)

Note: the proc-macro pipeline is parse/lower/manifest/codegen, but `codegen.rs` appears after `graphcal-plugin/src/lib.rs` because generated tokens reference the SDK runtime support re-exported by that crate.

1. `crates/graphcal-plugin-macros/src/lib.rs`
2. `crates/graphcal-plugin-macros/src/dims.rs`
3. `crates/graphcal-plugin-macros/src/parse.rs`
4. `crates/graphcal-plugin-macros/src/rational.rs`
5. `crates/graphcal-plugin-macros/src/lower.rs`
6. `crates/graphcal-plugin-macros/src/manifest.rs`
7. `crates/graphcal-plugin/src/lib.rs`
8. `crates/graphcal-plugin-macros/src/codegen.rs`

### Stage 13 - Runtime values and expression evaluator

Note: `eval_expr/work_budget.rs`, `eval_expr/linear_algebra_lu.rs`, `eval_expr/linear_algebra.rs`, `eval_expr/builtin_call.rs`, `eval_expr/arithmetic.rs`, `eval_expr/aggregations.rs`, `eval_expr/unit_scale.rs`, `eval_expr/hir_eval.rs`, and `eval_expr/mod.rs` form a mutually dependent group.

1. `crates/graphcal-eval/src/decl_key.rs`
2. `crates/graphcal-eval/src/eval_expr/numeric.rs`
3. `crates/graphcal-eval/src/eval_expr/work_budget.rs`
4. `crates/graphcal-eval/src/eval_expr/linear_algebra_lu.rs`
5. `crates/graphcal-eval/src/eval_expr/linear_algebra.rs`
6. `crates/graphcal-eval/src/eval_expr/conversions.rs`
7. `crates/graphcal-eval/src/eval_expr/datetime.rs`
8. `crates/graphcal-eval/src/lib.rs`
9. `crates/graphcal-eval/src/host_fns.rs`
10. `crates/graphcal-eval/src/domain_check.rs`
11. `crates/graphcal-eval/src/execution_facts.rs`
12. `crates/graphcal-eval/src/eval/bindings.rs`
13. `crates/graphcal-eval/src/eval_expr/complex.rs`
14. `crates/graphcal-eval/src/eval_expr/builtin_call.rs`
15. `crates/graphcal-eval/src/eval_expr/arithmetic.rs`
16. `crates/graphcal-eval/src/eval_expr/aggregations.rs`
17. `crates/graphcal-eval/src/eval_expr/unit_scale.rs`
18. `crates/graphcal-eval/src/eval_expr/hir_eval.rs`
19. `crates/graphcal-eval/src/eval_expr/mod.rs`
20. `crates/graphcal-eval/src/exec_plan.rs`
21. `crates/graphcal-eval/src/import_surface.rs`

### Stage 14 - Project loading, checking, and runtime orchestration

Note: loader I/O, pure project checking, and runtime preparation are now
separate module families. `project_compiler/entry_interface.rs` attaches
checked semantic facts to the direct HIR source interface;
`project_compiler/model.rs` supplies the remaining internal pass model;
`eval/project/prepare.rs` is the only transition that consumes `CheckedProject`
into an execution plan.

1. `crates/graphcal-eval/src/eval/types.rs`
2. `crates/graphcal-eval/src/eval/display.rs`
3. `crates/graphcal-eval/src/loader.rs`
4. `crates/graphcal-eval/src/inline_dag.rs`
5. `crates/graphcal-eval/src/project_compiler/template.rs`
6. `crates/graphcal-eval/src/project_compiler/entry_interface.rs`
7. `crates/graphcal-eval/src/project_compiler/model.rs`
8. `crates/graphcal-eval/src/project_compiler/hir_project.rs`
9. `crates/graphcal-eval/src/project_compiler/qualified_refs.rs`
10. `crates/graphcal-eval/src/project_compiler/recursion.rs`
11. `crates/graphcal-eval/src/project_compiler/generic_leakage.rs`
12. `crates/graphcal-eval/src/project_compiler/registry_merge.rs`
13. `crates/graphcal-eval/src/project_compiler/imports.rs`
14. `crates/graphcal-eval/src/project_compiler/lowering.rs`
15. `crates/graphcal-eval/src/project_compiler/checking.rs`
16. `crates/graphcal-eval/src/project_compiler/pipeline.rs`
17. `crates/graphcal-eval/src/project_compiler/session.rs`
18. `crates/graphcal-eval/src/project_compiler/mod.rs`
19. `crates/graphcal-eval/src/eval/plot_data.rs`
20. `crates/graphcal-eval/src/eval/public_projection.rs`
21. `crates/graphcal-eval/src/eval/runtime.rs`
22. `crates/graphcal-eval/src/eval/project/model_schema.rs`
23. `crates/graphcal-eval/src/eval/project/output.rs`
24. `crates/graphcal-eval/src/eval/project/prepared.rs`
25. `crates/graphcal-eval/src/eval/project/prepare.rs`
26. `crates/graphcal-eval/src/eval/project/mod.rs`
27. `crates/graphcal-eval/src/eval/mod.rs`
28. `crates/graphcal-eval/src/eval/tests.rs`
29. `crates/graphcal-eval/src/graph_ir/mod.rs`
30. `crates/graphcal-eval/src/graph_ir/dot.rs`

### Stage 15 - Tenax Arrow transport (`graphcal-tenax`)

1. `crates/graphcal-tenax/src/lib.rs`

### Stage 16 - Browser WASM adapter (`graphcal-wasm`)

Note: the order tool groups these files because `lib.rs` re-exports the browser transport types. The practical review order remains project validation, value conversion, diagnostic conversion, then the imperative adapter shell.

1. `crates/graphcal-wasm/src/project.rs`
2. `crates/graphcal-wasm/src/output.rs`
3. `crates/graphcal-wasm/src/diagnostics.rs`
4. `crates/graphcal-wasm/src/lib.rs`

### Stage 17 - WASM plugin host (`graphcal-plugin-host`)

Note: all five host files are mutually dependent. `convert.rs` is the untrusted-boundary leaf; `host.rs` and `module.rs` wrap engine/module loading; `registry.rs` bridges loaded projects to the evaluator; `lib.rs` re-exports the public surface.

1. `crates/graphcal-plugin-host/src/convert.rs`
2. `crates/graphcal-plugin-host/src/host.rs`
3. `crates/graphcal-plugin-host/src/module.rs`
4. `crates/graphcal-plugin-host/src/registry.rs`
5. `crates/graphcal-plugin-host/src/lib.rs`

### Stage 18 - Formatter (`graphcal-fmt`)

Note: `format/type_expr.rs`, `format/expr.rs`, `format/decl.rs`, and `format/mod.rs` form a mutually dependent group. `lib.rs` is the public formatting API shell and appears first in the generated order.

1. `crates/graphcal-fmt/src/lib.rs`
2. `crates/graphcal-fmt/src/format/type_expr.rs`
3. `crates/graphcal-fmt/src/format/expr.rs`
4. `crates/graphcal-fmt/src/format/decl.rs`
5. `crates/graphcal-fmt/src/format/mod.rs`

### Stage 19 - LSP prelude and CLI shell

Note: `json_input.rs`, `overrides.rs`, and `main.rs` form a mutually dependent group. `main.rs` consumes the `graphcal` library target's `format` module as well as binary-local modules, so the CLI package is ordered as one shell group here.

1. `crates/graphcal-lsp/src/lib.rs`
2. `crates/graphcal-lsp/src/convert.rs`
3. `crates/graphcal-lsp/src/cursor_context.rs`
4. `crates/graphcal-lsp/src/symbol_table.rs`
5. `crates/graphcal-lsp/src/formatting.rs`
6. `crates/graphcal-cli/src/display.rs`
7. `crates/graphcal-cli/src/plot.rs`
8. `crates/graphcal-cli/src/format.rs`
9. `crates/graphcal-cli/src/json_input.rs`
10. `crates/graphcal-cli/src/overrides.rs`
11. `crates/graphcal-cli/src/model.rs`
12. `crates/graphcal-cli/src/main.rs`
13. `crates/graphcal-cli/src/dump.rs`
14. `crates/graphcal-cli/src/deps.rs`
15. `crates/graphcal-cli/src/lib.rs`

### Stage 20 - Language server (`graphcal-lsp`)

Note: the feature modules from `resolve.rs` onward and `server.rs` are mutually dependent (each feature references `server::Backend`); the features come first because `server.rs` orchestrates them all.

1. `crates/graphcal-lsp/src/diagnostics.rs`
2. `crates/graphcal-lsp/src/resolve.rs`
3. `crates/graphcal-lsp/src/completion.rs`
4. `crates/graphcal-lsp/src/signature_help.rs`
5. `crates/graphcal-lsp/src/inlay_hints.rs`
6. `crates/graphcal-lsp/src/document_symbols.rs`
7. `crates/graphcal-lsp/src/document_links.rs`
8. `crates/graphcal-lsp/src/code_actions.rs`
9. `crates/graphcal-lsp/src/goto_definition.rs`
10. `crates/graphcal-lsp/src/references.rs`
11. `crates/graphcal-lsp/src/rename.rs`
12. `crates/graphcal-lsp/src/hover.rs`
13. `crates/graphcal-lsp/src/server.rs`

### Stage 21 - CLI plugin support and integration tests

1. `crates/graphcal-cli/build.rs`
2. `crates/graphcal-cli/src/plugin.rs`
3. `crates/graphcal-plugin/tests/expansion.rs`
4. `crates/graphcal-plugin/tests/prelude_drift.rs`
5. `crates/graphcal-eval/tests/declaration_order.rs`
6. `crates/graphcal-eval/tests/edge_case_bugs.rs`
7. `crates/graphcal-eval/tests/phase0_regressions.rs`
8. `crates/graphcal-eval/tests/error_snapshots.rs`
9. `crates/graphcal-plugin-host/tests/runtime.rs`
10. `crates/graphcal-plugin-host/tests/project_eval.rs`
11. `crates/graphcal-fmt/tests/format_tests.rs`
12. `crates/graphcal-cli/tests/cli.rs`
13. `crates/graphcal-cli/tests/plugin_cmd.rs`
14. `crates/graphcal-cli/tests/plugin_e2e.rs`
15. `crates/graphcal-cli/tests/dump.rs`
16. `crates/graphcal-wasm/tests/tutorial_examples.rs`
17. `crates/graphcal-wasm/tests/wasm_runtime.rs`
