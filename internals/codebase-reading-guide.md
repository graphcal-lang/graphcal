# Graphcal Codebase Reading Guide

This guide is a map for reading the Graphcal source code as it exists today.
It focuses on the compiler/evaluator pipeline, the crate boundaries, and the
typed data structures that carry language semantics through the system.

## 1. Pipeline

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
File<Desugared>
  |
  |  crates/graphcal-compiler/src/syntax/name_resolve.rs
  v
File<Resolved>
  |
  |  crates/graphcal-compiler/src/ir/
  v
IR
  |
  |  crates/graphcal-compiler/src/tir/
  v
TIR
  |
  |  crates/graphcal-eval/src/exec_plan.rs
  v
ExecPlan
  |
  |  crates/graphcal-eval/src/eval/runtime.rs
  v
EvalResult
```

The pipeline is strictly forward. Earlier surface forms disappear as the type
of the data changes, so later stages do not need to remember parser-only cases.

### 1.1 AST Phases

The AST is parameterized by a `Phase` marker in
`crates/graphcal-compiler/src/syntax/phase.rs`.

```text
File<Raw> -> File<Desugared> -> File<Resolved>
```

The marker controls three phase-specific slots:

| Slot        | Raw             | Desugared       | Resolved     |
| ----------- | --------------- | --------------- | ------------ |
| `DeclSugar` | `RawDeclSugar`  | `Infallible`    | `Infallible` |
| `ExprSugar` | `RawExprSugar`  | `Infallible`    | `Infallible` |
| `RefSugar`  | `UnresolvedRef` | `UnresolvedRef` | `Infallible` |

`File<Raw>` is produced by the parser and consumed by surface-aware tooling such
as the formatter. `File<Desugared>` has no multi-decl or table-literal sugar,
but still carries unresolved bare and dotted identifiers. `File<Resolved>` is
the semantic AST used by IR, TIR, and evaluation; no sugar and no unresolved
references are representable.

The alias modules keep signatures readable:

- `desugar/desugared_ast.rs` pins AST aliases to `Desugared`.
- `desugar/resolved_ast.rs` pins AST aliases to `Resolved`.

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

The parser emits `File<Raw>`. Declarations carry typed names where possible
(`DeclName`, `DimName`, `UnitName`, etc.), a `Visibility`, attributes, a
`DeclKind<Raw>`, and a source span.

### 1.3 Desugaring

Desugaring converts parser-only constructs into canonical AST forms:

- Multi-declarations expand into ordinary `param`, `node`, or `const node`
  declarations.
- Table literals become `ExprKind::MapLiteral`.
- Tuple-match helper desugaring is run by loader entry points before name
  resolution.

The generic phase walker lives in `desugar/convert.rs` and `desugar/mod.rs`.
The multi-declaration expander lives in `syntax/desugar.rs`.

### 1.4 Name Resolution

`syntax/name_resolve.rs` consumes `File<Desugared>` and returns
`File<Resolved>`. It rewrites `ExprKind::UnresolvedRef` into concrete AST
forms such as const references, local references, variant literals, or struct
construction.

This pass resolves against built-in constants, time scale names, local
expression bindings, type/index declarations, and module aliases. After it
runs, `RefSugar = Infallible`, so unresolved identifier paths are impossible
downstream.

### 1.5 IR Lowering

`ir/lower.rs` and `ir/resolve/` lower a resolved AST into `IR`.

The IR stage:

- Checks duplicate names and declaration naming rules.
- Validates visibility and bindability.
- Validates scope rules, such as no runtime `@` references in `const node`
  bodies.
- Extracts `const_deps` and `runtime_deps`.
- Builds the `Registry` for dimensions, units, indexes, struct/union types,
  and functions.
- Carries import metadata and pre-evaluated imported values across file/DAG
  boundaries.

One `IR` represents one DAG body: either a file root or an inline `dag` block.

### 1.6 TIR and Dimension Checking

`tir/typed.rs` resolves type annotations into semantic type expressions.
`tir/dim_check/` infers and checks dimensions and concrete value types.

The TIR is not flat:

```text
TIR
  registry
  root_dag_id
  dags: HashMap<DagId, DagTIR>
  module_aliases
```

Each file root and inline `dag` body is represented by a `DagTIR`. Dependency
files and dependency DAGs are merged into the same `DagRegistry` using their
canonical `DagId`s.

Dimensions are exact exponent maps:

```text
Dimension = BTreeMap<BaseDimId, Rational>
```

Dimension inference is split by expression families under
`tir/dim_check/infer/`.

### 1.7 Execution and Runtime Evaluation

`exec_plan::compile()` performs two topological passes:

1. Sort and evaluate `const node` declarations into `const_values`.
2. Sort runtime `param` and `node` declarations into `topo_order`.

It also resolves domain constraints from type annotations and from struct/union
member fields. Domain checks run both when compile-time values are known and at
runtime.

`eval/runtime.rs` evaluates declarations in topological order. A failed node is
contained as a `NodeError`; independent nodes can still evaluate. Internal
`RuntimeValue`s are converted to user-facing `Value`s with dimensions and
display units before they appear in `EvalResult`.

## 2. Workspace Map

The workspace contains six Rust crates:

```text
graphcal-cli       binary: CLI shell
graphcal-lsp       binary/library: Language Server Protocol
graphcal-eval      evaluation, project orchestration, loader
graphcal-compiler  syntax, registry, IR, TIR
graphcal-fmt       formatter
graphcal-io        filesystem abstraction
```

The important dependency direction is:

```text
graphcal-cli
  -> graphcal-eval
  -> graphcal-compiler
  -> graphcal-io

graphcal-lsp
  -> graphcal-eval
  -> graphcal-compiler

graphcal-fmt
  -> graphcal-compiler
```

### 2.1 `graphcal-compiler`

The compiler crate owns the functional core through TIR.

| Path                     | Purpose                                               |
| ------------------------ | ----------------------------------------------------- |
| `syntax/ast.rs`          | Phase-parameterized AST definitions                   |
| `syntax/phase.rs`        | `Raw`, `Desugared`, `Resolved`, sugar slots, `never`  |
| `syntax/names.rs`        | Typed name newtypes and `ScopedName`                  |
| `dag_id.rs`              | Filesystem-independent DAG identity                   |
| `syntax/parser/`         | Parser for declarations, expressions, types, tables   |
| `syntax/name_resolve.rs` | Bare/dotted identifier rewrite to `File<Resolved>`    |
| `desugar/`               | Phase walker and AST alias modules                    |
| `ir/lower.rs`            | IR entries and lowering entry points                  |
| `ir/resolve/`            | Name, scope, dependency, visibility resolution        |
| `registry/`              | Dimensions, units, indexes, types, values, built-ins  |
| `tir/typed.rs`           | `TIR`, `DagTIR`, resolved type expressions, Nat forms |
| `tir/dim_check/`         | Dimension/type inference and checking                 |

### 2.2 `graphcal-eval`

The evaluator owns project loading, cross-file orchestration, execution-plan
compilation, and expression evaluation.

| Path              | Purpose                                                       |
| ----------------- | ------------------------------------------------------------- |
| `loader.rs`       | `LoadedProject`, `LoadedFile`, `LoadedDag`, import resolution |
| `eval/project/`   | Multi-file compile/eval orchestration                         |
| `inline_dag.rs`   | Inline DAG compilation helpers                                |
| `exec_plan.rs`    | Const evaluation, runtime topological order, domain prep      |
| `domain_check.rs` | Runtime and compile-time domain validation                    |
| `eval/runtime.rs` | Evaluation loop                                               |
| `eval/display.rs` | Display-unit extraction and attachment                        |
| `eval/types.rs`   | Public `EvalResult`, `Value`, plot/assert result types        |
| `eval_expr/`      | Expression evaluator by expression family                     |

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

### 2.5 `graphcal-cli`

The CLI is the imperative shell around the library pipeline.

Subcommands:

| Command  | Purpose                                 |
| -------- | --------------------------------------- |
| `eval`   | Compile and evaluate a `.gcl` file      |
| `check`  | Parse and type-check without evaluation |
| `format` | Format files or check formatting        |
| `lsp`    | Start the language server               |

Key files:

- `main.rs` owns command dispatch and exit-code behavior.
- `overrides.rs` parses `--set` and `--input` parameter overrides.
- `display.rs` renders text output.
- `plot.rs` renders plot/figure/layer output.

### 2.6 `graphcal-lsp`

The LSP consumes compiler/evaluator APIs and adds editor-facing analysis:

| Path                        | Feature                                           |
| --------------------------- | ------------------------------------------------- |
| `server.rs`                 | Server lifecycle and `run_analysis()`             |
| `diagnostics.rs`            | Compiler/evaluator diagnostics to LSP diagnostics |
| `symbol_table.rs`           | Symbol collection for editor features             |
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

### 2.7 Editors and Grammars

Syntax/editor surfaces live outside the Rust workspace:

- `grammar.ebnf` is the formal grammar source of truth.
- `tree-sitter-graphcal/` contains the tree-sitter grammar and highlight
  queries.
- `editors/vscode/` contains the VS Code extension and TextMate grammar.
- `editors/zed/` contains the Zed extension and bundled grammar artifact.

When syntax changes, update these together with the compiler/parser and docs.

## 3. Core Data Structures

### 3.1 Typed Names

Identifier strings are wrapped as semantic newtypes in `syntax/names.rs`:
`DeclName`, `DimName`, `UnitName`, `StructTypeName`, `IndexName`, `FnName`,
`FieldName`, `VariantName`, `ConstructorName`, `GenericParamName`,
`LocalName`, and `ModuleAliasName`.

Use these newtypes in the core instead of naked `String`s. `ScopedName` carries
local versus module-qualified declaration names structurally:

```text
ScopedName::Local(name)
ScopedName::Qualified { module, member }
```

The `Display` implementation may render `module::member`, but that string is a
boundary representation. Core code should pattern-match the variant.

### 3.2 DAG Identity

`dag_id.rs` defines `DagId`, the canonical identity for file roots and
inline DAGs. It is a non-empty sequence of segments, not a path string.

Examples:

- `helpers/math.gcl` becomes `DagId(["helpers", "math"])`.
- `dag burn { ... }` inside that file becomes
  `DagId(["helpers", "math", "burn"])`.

Filesystem paths are converted to `DagId` at loader boundaries. Compiler and
evaluator internals should use `DagId` rather than `PathBuf` when referring to
compiled modules or DAG bodies.

### 3.3 Loader Types

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
  ast: File<Resolved>
  named_source: NamedSource<Arc<String>>
  resolved_imports: HashMap<ModulePathKey, DagId>
  inline_dags: Vec<LoadedDag>

LoadedDag
  dag_id: DagId
  parent_dag_id: DagId
  name: String
  body: Vec<Declaration<Resolved>>
  resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>
```

`ModulePathKey` stores import/include path segments as a vector. It avoids using
joined strings as map keys inside the loader.

### 3.4 IR

`IR` contains the semantic declaration lists and dependency metadata for one DAG
body:

```text
IR
  registry: Registry
  consts, params, nodes, asserts, plots, figures, layers
  runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>
  const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>
  source_order: Vec<(ScopedName, DeclCategory)>
  assert_names
  assumes_map
  expected_fail
  imported_values
  imported_decl_types
  imported_value_sources
  pub_names
```

The dependency maps are keyed by `ScopedName`. Value sets use `BTreeSet` so DAG
construction is deterministic.

### 3.5 Registry

`registry/types.rs` defines the frozen `Registry`:

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

### 3.6 TIR and `DagTIR`

`TIR` wraps one file-scoped registry and all DAGs reachable from that file.

```text
TIR
  registry: Registry
  root_dag_id: DagId
  dags: HashMap<DagId, DagTIR>
  module_aliases: HashMap<String, DagId>

DagTIR
  dag_id: DagId
  consts, params, nodes, asserts, plots, figures, layers
  runtime_deps, const_deps, source_order
  assert_names, assumes_map, expected_fail
  resolved_decl_types
  domain_constraints
  imported_values
  imported_decl_types
  imported_value_sources
  pub_nodes
```

`TIR::root()` and `TIR::root_mut()` access the file root. `TIR::lookup_call_target`
and `TIR::resolve_call_path` resolve inline DAG call paths through same-file
children or `module_aliases`.

### 3.7 ExecPlan

`ExecPlan` is the runtime-ready form of a root `DagTIR`:

```text
ExecPlan
  const_values
  imported_values
  topo_order
  expressions
  assert_bodies
  plot_bodies
  figure_bodies
  layer_bodies
  assumes_map
  expected_fail
  domain_constraints
  struct_field_constraints
```

It contains no parser or IR registry-building work; it is ready for evaluation.

### 3.8 Runtime Values

There are two value layers:

- `RuntimeValue` is internal and unit-normalized. It carries no display-unit
  metadata.
- `Value` is user-facing and appears in `EvalResult`. Scalar values carry a
  dimension and optional display-unit information.

## 4. Declarations, Visibility, and Evaluation

All declarations are private unless made visible. `pub` means visible at a
module/include boundary. `pub(bind)` means visible and bindable. Required
`dim`, `type`, and `index` declarations must be `pub(bind)`. `param`
declarations do not take a visibility modifier; required params are implicitly
bindable.

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

Graphcal has separate mechanisms for compile-time names and DAG/value reuse:

- `import` brings compile-time declarations into scope: const nodes,
  dimensions, units, types, indexes, DAG names, and evaluated asserts.
- `include` instantiates or merges runtime outputs, with optional param/index
  bindings.

Import/include paths are dot-separated module paths in source. Loader internals
drop spans and store path segments in `ModulePathKey`; compiled DAG identity is
stored in `DagId`.

Project loading:

1. Determine the project root from `graphcal.toml`, an explicit root, or loose
   single-file mode.
2. Parse, desugar, tuple-desugar, and name-resolve each file.
3. Resolve import/include paths to `DagId`s.
4. Lift inline `dag` blocks into `LoadedDag`s.
5. Build a dependency-first `load_order`.
6. Detect circular imports during traversal.

The project pipeline in `eval/project/` then lowers dependencies before
dependents, merges registries/DAG TIRs, and evaluates the requested root.

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
```

Each diagnostic carries a `NamedSource<Arc<String>>` for rich output. Error
codes such as `D001`, `V001`, and `M020` are searchable in the source.

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

- `crates/graphcal-eval/tests/error_snapshots.rs`
- `crates/graphcal-eval/tests/edge_case_bugs.rs`
- `crates/graphcal-eval/tests/declaration_order.rs`
- `crates/graphcal-fmt/tests/format_tests.rs`
- `crates/graphcal-cli/tests/cli.rs`

Useful commands:

```bash
cargo test --workspace
cargo test -p graphcal-compiler
cargo insta review
just lint
```

## 8. Conventions Worth Keeping in Mind

| Convention                       | Where               | Why                                                                 |
| -------------------------------- | ------------------- | ------------------------------------------------------------------- |
| AST phases                       | `syntax/phase.rs`   | Parser-only constructs are statically excluded downstream           |
| Typed names                      | `syntax/names.rs`   | Avoid mixing semantic identifier categories                         |
| `DagId`                          | `dag_id.rs`         | Keep filesystem paths at loader boundaries                          |
| `ModulePathKey`                  | `loader.rs`         | Keep module paths structured instead of separator-joined            |
| Trait-based I/O                  | `graphcal-io`       | Deterministic tests and editor integration                          |
| Visitor pattern                  | `syntax/visitor.rs` | Centralized AST traversal                                           |
| `BTreeSet` in dep values         | IR/TIR deps         | Deterministic graph construction                                    |
| `IndexMap` in output-facing maps | eval/display output | Stable user-facing order                                            |
| Separate const/runtime phases    | `exec_plan.rs`      | Compile-time values and runtime values have different failure modes |
| Display units outside dimensions | `eval/display.rs`   | Compute in SI, display in requested units                           |

When adding a feature, update the grammar, parser, compiler stages, evaluator,
LSP/editor surfaces, docs, and fixtures together. The compiler core should carry
semantic distinctions as types, not string conventions.

## 9. Suggested Reading Order

For a first pass, read in pipeline order:

1. `crates/graphcal-compiler/src/syntax/token.rs`
2. `crates/graphcal-compiler/src/syntax/names.rs`
3. `crates/graphcal-compiler/src/syntax/ast.rs`
4. `crates/graphcal-compiler/src/syntax/phase.rs`
5. `crates/graphcal-compiler/src/syntax/lexer.rs`
6. `crates/graphcal-compiler/src/syntax/parser/expr.rs`
7. `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
8. `crates/graphcal-compiler/src/desugar/convert.rs`
9. `crates/graphcal-compiler/src/syntax/desugar.rs`
10. `crates/graphcal-compiler/src/syntax/name_resolve.rs`
11. `crates/graphcal-compiler/src/ir/resolve/mod.rs`
12. `crates/graphcal-compiler/src/ir/resolve/deps.rs`
13. `crates/graphcal-compiler/src/ir/lower.rs`
14. `crates/graphcal-compiler/src/registry/types.rs`
15. `crates/graphcal-compiler/src/dag_id.rs`
16. `crates/graphcal-eval/src/loader.rs`
17. `crates/graphcal-compiler/src/tir/typed.rs`
18. `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
19. `crates/graphcal-eval/src/eval/project/pipeline.rs`
20. `crates/graphcal-eval/src/inline_dag.rs`
21. `crates/graphcal-eval/src/exec_plan.rs`
22. `crates/graphcal-eval/src/eval/runtime.rs`
23. `crates/graphcal-eval/src/eval_expr/mod.rs`
24. `crates/graphcal-eval/src/eval/types.rs`
25. `crates/graphcal-cli/src/main.rs`

After that, read `graphcal-lsp` and `graphcal-fmt` as consumers of the
compiler/evaluator APIs.
