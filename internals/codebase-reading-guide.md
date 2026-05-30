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
- **IR** means **Intermediate Representation**. In general compiler terminology,
  an IR is any representation between syntax and execution. In Graphcal,
  `IR` specifically means the declaration-level representation for one DAG body:
  checked declaration lists, dependency edges, visibility/import metadata, and a
  registry built from declarations.
- **HIR** means **High-level Intermediate Representation**. It is still close to
  source expression/type structure, but reference positions are semantic rather
  than syntactic. HIR replaces source paths with canonical owner-qualified names,
  lexical IDs, and typed built-in variants.
- **TIR** means **Typed Intermediate Representation**. It is the type- and
  dimension-checked representation used by the evaluator. TIR combines IR's
  declaration/DAG structure with HIR-derived resolved references, resolved type
  expressions, dimension facts, and per-DAG compilation state.

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
3. **`File<Resolved>`: what can be resolved using only this file?**
   Local expression names are rewritten into more specific AST forms, but this
   is still not project-wide resolution. Type paths, index paths, and module
   qualified names may still need import/include context.
4. **Loader and `ModuleResolver`: which file or DAG does a module path mean?**
   The loader performs filesystem work and turns import/include paths into
   canonical `DagId`s. The compiler-side `ModuleResolver` then answers semantic
   lookup questions without doing I/O. This keeps disk paths and import aliases
   out of the core compiler data.
5. **IR: what declarations does this DAG body contain?**
   IR groups the body into consts, params, nodes, asserts, registry entries, and
   dependency maps. This is where checks such as duplicate names, visibility,
   bindability, declaration categories, and dependency edges are easiest because
   they require seeing the whole DAG body.
6. **HIR: what does each reference point to?**
   HIR lowers expression and type references from source paths into typed
   identities: declarations, dimensions, indexes, constructors, locals, generic
   params, and built-ins. After this boundary, code should not need to ask
   whether the string `"sum"`, `"PI"`, or `"helpers.physics.mass"` has a special
   meaning; it should pattern-match on typed values instead.
7. **TIR: is the program type- and dimension-correct?**
   TIR combines the declaration structure from IR with HIR references, resolved
   type expressions, dimension facts, domain constraints, inline DAG bodies, and
   dependency DAGs. This is the checked program representation used to prepare
   execution.
8. **ExecPlan: what exact work should runtime evaluation do?**
   `ExecPlan` evaluates compile-time constants, sorts runtime declarations, and
   stores runtime-ready maps. The evaluator should not need to rebuild registries,
   re-resolve names, or recompute dependency order.

Some names in this pipeline can sound misleading if read too literally:

- `File<Resolved>` means local unresolved expression nodes are gone; it does
  **not** mean every path has a canonical module owner.
- IR is not another copy of the AST; it is the declaration and registry view of
  one DAG body.
- HIR is "high-level" because it keeps expression/type tree shapes, not because
  it must be built before IR.
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
File<Desugared>
  |
  |  crates/graphcal-compiler/src/syntax/name_resolve.rs
  v
File<Resolved>  (locally normalized syntax AST)
  |
  |  crates/graphcal-compiler/src/ir/
  |  crates/graphcal-compiler/src/syntax/module_resolve.rs
  v
IR + ModuleResolver
  |
  |  crates/graphcal-compiler/src/hir/
  |  crates/graphcal-compiler/src/tir/
  v
TIR  (DagTIR + DagSemanticBody)
  |
  |  crates/graphcal-eval/src/exec_plan.rs
  v
ExecPlan
  |
  |  crates/graphcal-eval/src/eval/runtime.rs
  v
EvalResult
```

The pipeline is forward-only: parser sugar is removed before IR, module paths
are resolved to canonical owners before module-aware TIR/eval, and runtime maps
use owner-qualified declaration identities. Source `ScopedName`s and spans are
kept at diagnostics and formatting boundaries; semantic compile/eval decisions
use HIR and `ResolvedName`-based data.

### 1.1 AST Phases

The AST is parameterized by a `Phase` marker in
`crates/graphcal-compiler/src/syntax/phase.rs`.

```text
File<Raw> -> File<Desugared> -> File<Resolved>
```

The marker controls only the slots that actually vary by phase:

| Slot        | Raw             | Desugared       | Resolved     |
| ----------- | --------------- | --------------- | ------------ |
| `DeclSugar` | `RawDeclSugar`  | `Infallible`    | `Infallible` |
| `ExprSugar` | `RawExprSugar`  | `Infallible`    | `Infallible` |
| `RefSugar`  | `UnresolvedRef` | `UnresolvedRef` | `Infallible` |

Type-level syntactic references such as type applications, dimension terms, and
index expressions are `NamePath` in every phase, so they are represented
directly as `NamePath` fields rather than as `Phase` associated types.

`File<Raw>` is produced by the parser and consumed by surface-aware tooling such
as the formatter. `File<Desugared>` has no multi-decl or table-literal sugar,
but still carries unresolved expression references. `File<Resolved>` has no
sugar and no `ExprKind::UnresolvedRef`, but it is still a syntax-layer AST:
type-level paths, index paths, module-qualified references, and some match
patterns are intentionally preserved as structured paths until the module
resolver/HIR boundary can attach canonical owners.

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

### 1.4 Local Name Resolution

`syntax/name_resolve.rs` consumes `File<Desugared>` and returns
`File<Resolved>`. It removes `ExprKind::UnresolvedRef` by rewriting local syntax
into concrete AST expression forms such as const references, local references,
variant literals, type-system references, or struct construction.

This pass resolves against built-in constants, time scale names, local
expression bindings, declarations visible in the same file, and syntactic module
aliases. It is deliberately not the final module-aware resolver: paths that need
canonical file/DAG ownership are resolved later by `syntax/module_resolve.rs`
and lowered into HIR. After this pass, `RefSugar = Infallible`, but `NamePath`
and `IdentPath` values can still appear in type/index positions and in
module-sensitive expression surfaces.

### 1.5 Module Resolver and HIR Boundary

`syntax/module_resolve.rs` builds project-wide, owner-qualified symbol tables.
It stores one `ModuleSymbols` table per canonical `DagId` and one `ModuleScope`
per module for import/include aliases and selective imports. It resolves
syntactic `NamePath` / `IdentPath` values into canonical
`ResolvedName<namespace::...>` values or `ResolvedIndexVariant` values. Source
qualifier text is used only to find a module alias; successful lookups carry the
canonical `DagId` owner.

`graphcal-eval/src/loader.rs` remains the only layer that resolves import paths
to files/DAGs. `LoadedProject::build_module_resolver()` hands those already
resolved edges to the pure compiler resolver, including inline DAGs and
instantiated include owners.

`crates/graphcal-compiler/src/hir/` is the semantic boundary after syntax:

- `hir/types.rs` models type expressions using `BuiltinType`,
  `ResolvedName`, and lexical `GenericParamId`s.
- `hir/expr.rs` models value expressions using canonical declaration refs,
  constructor refs, `ResolvedIndexVariant`s, `LocalId`s, typed built-ins, and
  semantic `MatchPattern` variants.
- `hir/lower.rs` lowers syntax AST type references into HIR with a
  `ModuleResolver`, a `GenericScope`, and an optional prelude scope.

TIR stores HIR expressions and HIR-derived semantic metadata in
`DagSemanticBody`. Syntax AST data remains attached to declarations for
source-facing features such as diagnostics, formatting, assertions, plots, and
LSP presentation.

### 1.6 IR Lowering

`ir/lower.rs` and `ir/resolve/` lower a resolved AST into `IR`.

The IR stage:

- Checks duplicate names and declaration naming rules.
- Validates visibility and bindability.
- Validates scope rules, such as no runtime `@` references in `const node`
  bodies.
- Extracts `const_deps` and `runtime_deps` as structured `ScopedName` maps.
- Builds the leaf-keyed `Registry` for dimensions, units, indexes,
  struct/union types, and functions.
- Carries import metadata and pre-evaluated imported values across file/DAG
  boundaries.

One `IR` represents one DAG body: either a file root or an inline `dag` block.
IR is still the compatibility boundary for source-shaped declaration keys;
owner-qualified declaration dependencies are collected from HIR during TIR
construction.

### 1.7 TIR and Dimension Checking

`tir/typed.rs` resolves type annotations into semantic type expressions.
`tir/dim_check/` infers and checks dimensions and concrete value types.

In the module-aware project path, TIR resolution receives both a
`ModuleResolver` and a `ModuleTypeRegistry`. Syntax type expressions are first
lowered to HIR, then resolved against owner-qualified definitions. Checked DAG
body construction uses `type_resolve_with_modules()` for file roots and
`type_resolve_single_with_modules()` for inline DAG bodies.

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

`DagTIR` carries one semantic body:

- `semantic.expressions`: HIR expressions for const/default/node bodies.
- `semantic.dependencies`: owner-qualified declaration dependency maps and
  source-span graph/const-reference targets.
- `semantic.collection_refs`: canonical index and variant references for
  map/table/index/match inference.
- `semantic.constructor_refs`: canonical constructor-call and constructor-match
  metadata.
- `semantic.inline_dag_refs`: canonical inline-DAG call routing metadata.
- `semantic.type_defs`: owner-qualified struct/type definitions used by eval.
- `semantic.decl_bindings`: visible `ScopedName` keys mapped to
  `ResolvedName<Decl>` identities at the source boundary.

Dimensions are exact exponent maps:

```text
Dimension = BTreeMap<BaseDimId, Rational>
```

Dimension inference is split by expression families under
`tir/dim_check/infer/`. When an inference helper still receives a syntax AST
expression for span-oriented diagnostics, it consults the semantic body for
canonical index/constructor/inline-DAG ownership instead of re-resolving dotted
source paths.

### 1.8 Execution and Runtime Evaluation

`exec_plan::compile()` performs two topological passes:

1. Sort and evaluate `const node` declarations into `const_values`.
2. Sort runtime `param` and `node` declarations into `topo_order`.

Runtime execution is keyed by `RuntimeDeclKey`, which wraps canonical
`ResolvedName<Decl>` identities so same-leaf declarations from different DAGs do
not collide. Const and runtime declaration evaluation require
`DagSemanticBody::expressions` and use `eval_expr/hir_eval.rs`; missing semantic
expressions are internal consistency errors.

The execution plan also resolves domain constraints from type annotations and
from struct/union member fields. Domain checks run both when compile-time values
are known and at runtime.

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
graphcal-compiler  syntax, HIR, registry, IR, TIR
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

| Path                     | Purpose                                                    |
| ------------------------ | ---------------------------------------------------------- |
| `syntax/ast.rs`          | Phase-parameterized AST definitions                        |
| `syntax/phase.rs`        | `Raw`, `Desugared`, `Resolved`, sugar/path slots, `never`  |
| `syntax/names.rs`        | `NameAtom`, typed name newtypes, paths, resolved names     |
| `dag_id.rs`              | Filesystem-independent DAG identity                        |
| `syntax/parser/`         | Parser for declarations, expressions, types, tables        |
| `syntax/name_resolve.rs` | Local unresolved-expression rewrite to `File<Resolved>`    |
| `syntax/module_resolve.rs` | Owner-qualified module symbol tables and path resolution |
| `desugar/`               | Phase walker and AST alias modules                         |
| `hir/`                   | Resolved type/value expression boundary                    |
| `ir/lower.rs`            | IR entries and lowering entry points                       |
| `ir/resolve/`            | Name, scope, dependency, visibility resolution             |
| `registry/`              | Dimensions, units, indexes, types, values, built-ins       |
| `tir/typed.rs`           | `TIR`, `DagTIR`, `DagSemanticBody`, resolved type expressions |
| `tir/dim_check/`         | Dimension/type inference and checking                      |

### 2.2 `graphcal-eval`

The evaluator owns project loading, cross-file orchestration, execution-plan
compilation, and expression evaluation.

| Path                 | Purpose                                                        |
| -------------------- | -------------------------------------------------------------- |
| `loader.rs`          | `LoadedProject`, `LoadedFile`, `LoadedDag`, import resolution  |
| `eval/project/`      | Multi-file compile/eval orchestration                          |
| `inline_dag.rs`      | Inline DAG compilation helpers                                 |
| `decl_key.rs`        | Runtime declaration keys backed by `ResolvedName<Decl>`        |
| `exec_plan.rs`       | Const evaluation, runtime topological order, domain prep       |
| `domain_check.rs`    | Runtime and compile-time domain validation                     |
| `eval/runtime.rs`    | Evaluation loop                                                |
| `eval/display.rs`    | Display-unit extraction and attachment                         |
| `eval/types.rs`      | Public `EvalResult`, `Value`, plot/assert result types         |
| `eval_expr/`         | Syntax-AST expression evaluator by expression family           |
| `eval_expr/hir_eval.rs` | HIR expression evaluator with canonical references          |

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
| `symbol_table.rs`           | Typed `SymbolKey` collection for editor features  |
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
inline DAGs. It is a non-empty sequence of segments, not a path string.

Examples:

- `helpers/math.gcl` becomes `DagId(["helpers", "math"])`.
- `dag burn { ... }` inside that file becomes
  `DagId(["helpers", "math", "burn"])`.

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
joined strings as map keys inside the loader. `LoadedProject::build_module_resolver()`
then turns the loaded files, inline DAGs, and pre-resolved edges into the
compiler's `ModuleResolver`.

### 3.5 IR

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
construction is deterministic. Owner-qualified dependency maps are collected
later from HIR and stored on `DagTIR::semantic.dependencies`.

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

`tir/typed.rs` also defines `ModuleTypeRegistry`, an owner-qualified view over
registries used during module-aware TIR resolution. It keys dimensions, indexes,
struct types, and constructors by `ResolvedName`; project lowering inserts the
synthetic Graphcal prelude owner alongside current/dependency registries.

### 3.7 TIR and `DagTIR`

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
  semantic: DagSemanticBody
  source_order
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
children or `module_aliases`. Module-aware callers should prefer
`DagTIR::semantic.inline_dag_refs` when evaluating a specific expression because
it already carries canonical call routing.

### 3.8 ExecPlan

`ExecPlan` is the runtime-ready form of a root `DagTIR`:

```text
ExecPlan
  const_values: RuntimeValueMap
  imported_values: RuntimeValueMap
  topo_order: Vec<RuntimeDeclKey>
  expressions: HashMap<RuntimeDeclKey, Expr>
  assert_bodies
  plot_bodies
  figure_bodies
  layer_bodies
  assumes_map
  expected_fail
  domain_constraints: HashMap<RuntimeDeclKey, ResolvedDomainConstraint>
  struct_field_constraints: HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>
```

It contains no parser or IR registry-building work; it is ready for evaluation.
`RuntimeDeclKey` lives in `graphcal-eval/src/decl_key.rs` and keeps runtime
maps keyed by canonical declaration identity.

### 3.9 Runtime Values

There are two value layers:

- `RuntimeValue` is internal and unit-normalized. It carries no display-unit
  metadata, but label/struct/indexed values carry type identity through
  `IndexTypeRef` / `StructTypeRef` so owner-qualified type/index identity is
  preserved during evaluation.
- `Value` is user-facing and appears in `EvalResult`. Scalar values carry a
  dimension and optional display-unit information; labels, structs, and indexed
  values keep public identity carriers for diagnostics/output.

`TypeNameRef<Ns>` in `registry/declared_type.rs` is the shared identity carrier
for type-level runtime/public values. It stores both a display leaf and a
canonical `ResolvedName<Ns>`.

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
2. Parse, desugar, and locally name-resolve each file.
3. Resolve import/include paths to `DagId`s.
4. Lift inline `dag` blocks into `LoadedDag`s.
5. Build a dependency-first `load_order`.
6. Detect circular imports during traversal.

Project lowering then builds the `ModuleResolver` from the loaded graph, builds a
`ModuleTypeRegistry` from the current file, prelude, and evaluated dependencies,
lowers dependencies before dependents, merges registries/DAG TIRs, and evaluates
the requested root.

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

| Convention                         | Where                    | Why                                                                 |
| ---------------------------------- | ------------------------ | ------------------------------------------------------------------- |
| AST phases                         | `syntax/phase.rs`        | Parser-only constructs are statically excluded downstream           |
| `NameAtom` / `NameDef<Ns>`         | `syntax/names.rs`        | Definition leaves cannot accidentally contain dotted paths           |
| `NamePath` / `IdentPath`           | syntax AST               | Preserve source qualification until a resolver has module context    |
| `ResolvedName` / variants          | HIR/TIR/eval             | Carry canonical owner identity instead of source alias strings       |
| `ModuleResolver`                   | `syntax/module_resolve.rs` | Keep module lookup pure and owner-qualified                        |
| HIR                                | `hir/`                   | Consume syntax paths and expose semantic references                  |
| `DagId`                            | `dag_id.rs`              | Keep filesystem paths at loader boundaries                           |
| `ModulePathKey`                    | `loader.rs`              | Keep module paths structured instead of separator-joined             |
| `RuntimeDeclKey`                   | `crates/graphcal-eval/src/decl_key.rs` | Prevent runtime value collisions across DAG owners      |
| `TypeNameRef` identity carriers    | `registry/declared_type.rs` | Preserve index/struct owners through runtime/public values       |
| Trait-based I/O                    | `graphcal-io`            | Deterministic tests and editor integration                           |
| Visitor pattern                    | `syntax/visitor.rs`      | Centralized AST traversal                                            |
| `BTreeSet` in dep values           | IR/TIR deps              | Deterministic graph construction                                     |
| `IndexMap` in output-facing maps   | eval/display output      | Stable user-facing order                                             |
| Separate const/runtime phases      | `exec_plan.rs`           | Compile-time values and runtime values have different failure modes  |
| Display units outside dimensions   | `eval/display.rs`        | Compute in SI, display in requested units                            |

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
7. `crates/graphcal-compiler/src/syntax/parser/type_expr.rs`
8. `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
9. `crates/graphcal-compiler/src/desugar/convert.rs`
10. `crates/graphcal-compiler/src/syntax/desugar.rs`
11. `crates/graphcal-compiler/src/syntax/name_resolve.rs`
12. `crates/graphcal-compiler/src/syntax/module_resolve.rs`
13. `crates/graphcal-compiler/src/hir/types.rs`
14. `crates/graphcal-compiler/src/hir/lower.rs`
15. `crates/graphcal-compiler/src/hir/expr.rs`
16. `crates/graphcal-compiler/src/ir/resolve/mod.rs`
17. `crates/graphcal-compiler/src/ir/resolve/deps.rs`
18. `crates/graphcal-compiler/src/ir/lower.rs`
19. `crates/graphcal-compiler/src/registry/types.rs`
20. `crates/graphcal-compiler/src/dag_id.rs`
21. `crates/graphcal-eval/src/loader.rs`
22. `crates/graphcal-eval/src/eval/project/pipeline.rs`
23. `crates/graphcal-eval/src/eval/project/lowering.rs`
24. `crates/graphcal-compiler/src/tir/typed.rs`
25. `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
26. `crates/graphcal-eval/src/inline_dag.rs`
27. `crates/graphcal-eval/src/exec_plan.rs`
28. `crates/graphcal-eval/src/decl_key.rs`
29. `crates/graphcal-eval/src/eval/runtime.rs`
30. `crates/graphcal-eval/src/eval_expr/hir_eval.rs`
31. `crates/graphcal-eval/src/eval_expr/mod.rs`
32. `crates/graphcal-eval/src/eval/types.rs`
33. `crates/graphcal-cli/src/main.rs`

After that, read `graphcal-lsp` and `graphcal-fmt` as consumers of the
compiler/evaluator APIs.
