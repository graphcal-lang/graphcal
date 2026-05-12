# Graphcal Codebase Reading Guide

This document provides the conceptual background and structural overview needed to
navigate and understand the Graphcal source code.

## 1. Evaluation Pipeline Overview

Understanding the pipeline is the single most important prerequisite for reading the
code. Every `.gcl` source file passes through these stages in order:

```text
Source Code (.gcl)
  |
  |  [Lexer]  crates/graphcal-compiler/src/syntax/lexer.rs
  v
Token Stream  (Token + Span pairs)
  |
  |  [Parser]  crates/graphcal-compiler/src/syntax/parser/
  v
File<Raw>  (surface AST: still carries multi-decl / table-literal sugar)
  |
  |  [Desugaring]  crates/graphcal-compiler/src/desugar/   +
  |                crates/graphcal-compiler/src/syntax/desugar.rs
  v
File<Desugared>  (canonical AST: Sugar slots become Infallible)
  |
  |  [Name Resolution]  crates/graphcal-compiler/src/syntax/name_resolve.rs
  v
File<Desugared>  (NameRef / QualifiedNameRef rewritten to concrete refs)
  |
  |  [IR Lowering]  crates/graphcal-compiler/src/ir/
  v
IR  (resolved names, dependency graphs, Registry of types/units/dims)
  |
  |  [Type Resolution + Dimension Checking]  crates/graphcal-compiler/src/tir/
  v
TIR  (Registry + DagRegistry: one DagTIR per file root and per inline `dag`)
  |
  |  [Execution Plan Compilation]  crates/graphcal-eval/src/exec_plan.rs
  v
ExecPlan  (topologically sorted decls, pre-evaluated consts)
  |
  |  [Runtime Evaluation]  crates/graphcal-eval/src/eval/runtime.rs
  v
EvalResult  (HashMap<DeclName, Value> with display units attached)
```

Each stage is **strictly forward**: no backtracking to a previous stage.
Errors at any stage halt the pipeline and produce rich diagnostics via `miette`.

The AST is parameterized over a `Phase` marker (see
`crates/graphcal-compiler/src/syntax/phase.rs`):

- `File<Raw>` is what the parser produces and what the formatter consumes.
  Surface sugar (`RawDeclSugar::Multi`, `RawExprSugar::TableLiteral`) is
  representable here.
- `File<Desugared>` is what every downstream stage (name resolution, IR, TIR,
  evaluation) consumes. The `Sugar` payload is `core::convert::Infallible`,
  so the sugar variants vanish from the type system entirely.

### 1.1 Lexer -> Tokens

- Uses the `logos` crate for regex-driven tokenization.
- The `Lexer` wrapper provides **2-token lookahead** (`peek` / `peek2`) and put-back.
- Comments (`//`) are skipped during lexing; extracted separately for the formatter.
- Every token carries a byte-accurate `Span`.

### 1.2 Parser -> AST

- Recursive-descent parser with Pratt-style operator precedence for expressions.
- Precedence (lowest to highest): `->` conversion, `if/else`, `||`, `&&`,
  comparisons, `+/-`, `*/div`, unary, `^` (right-assoc), atoms.
- Declarations are parsed in `parser/decl/`, expressions in `parser/expr.rs`,
  type annotations in `parser/type_expr.rs`.
- Output is `File { declarations: Vec<Declaration> }` where each `Declaration`
  carries attributes, an `is_pub` visibility flag, a `DeclKind`, and a `Span`.

### 1.3 Desugaring -> `File<Desugared>`

Handled in `desugar/` (generic walker + `DesugarSugar` trait) plus
`syntax/desugar.rs` (the multi-decl expander):

- **Multi-declarations** — `param a: T[I], const node b: U[I, J] = table[…]{…};` —
  are expanded into N parallel ordinary declarations sharing one synthesized
  `table[…]{…}` initializer. Source spans on each synthesized declaration point
  back at the slot header, name, type annotation, and table body so diagnostics
  still land on the surface form.
- **Table literals** desugar to `ExprKind::MapLiteral`; the `table` keyword and
  axis metadata exist only in `Raw`.

The walker is generic: each surface sugar implements `DesugarSugar` (see
`desugar/mod.rs`), and the `File<Raw> -> File<Desugared>` `From` impl in
`desugar/convert.rs` dispatches `Sugar(_)` arms to the appropriate transform
while rebuilding every other node phase-by-phase.

### 1.4 Name Resolution -> resolved `File<Desugared>`

Handled in `syntax/name_resolve.rs`. After parsing + desugaring, the AST still
contains `NameRef` / `QualifiedNameRef` nodes for bare and dotted identifiers.
This pass rewrites them into concrete expression kinds, resolving against:

- Builtin constants (PI, E, TAU, SQRT2, LN2, LN10).
- Time scale names (UTC, TAI, TT, …) for `Datetime` literals.
- Local scope (for/scan/unfold/match bindings).
- Struct and union type names declared in the file.
- Index names and their variants.
- Module aliases from `import` declarations.

After this pass, no `NameRef` / `QualifiedNameRef` nodes remain.

### 1.5 IR Lowering -> IR

Handled in `ir/resolve/` and `ir/lower.rs`:

1. **Collect local names** from AST declarations.
2. **Duplicate / casing checks**: params, nodes, and const nodes must be
   `lower_snake_case`; `UPPER_SNAKE_CASE` is reserved for built-in constants
   (PI, E, TAU).
3. **Visibility validation**: required params and indexes must be `pub(bind)`;
   private items cannot be imported across files.
4. **Scope validation**: const node bodies cannot contain `@` references to
   runtime params/nodes; assert bodies cannot reference other asserts.
5. **Dependency extraction**: build `runtime_deps` (params/nodes) and `const_deps`
   (const nodes) graphs; detect cycles via `petgraph::algo::toposort`.
6. **Registry building**: register dimensions, units, indexes, struct types, and
   functions (builtins + user-defined).

The result is the `IR` struct, which bundles resolved entries, a frozen `Registry`,
and dependency maps. One `IR` corresponds to one DAG body (file root or inline
`dag` block); the project pipeline lowers each DAG body separately. Inline
`dag` bodies are compiled via `lower_dag_body_to_ir` (compiler) +
`graphcal-eval/src/inline_dag.rs` (project glue).

### 1.6 Type Resolution + Dimension Checking -> TIR

Handled in `tir/typed.rs` and `tir/dim_check/`:

1. **Resolve type annotations**: convert AST type names into `ResolvedTypeExpr`
   (concrete `Dimension` values, struct types, generics).
2. **Dimension inference**: Hindley-Milner-like constraint generation +
   unification for each expression (`tir/dim_check/infer/`).
3. **Type matching**: verify inferred type against declared type for every
   param (default), node, const node, and assert.

Dimensions are represented as `BTreeMap<BaseDimId, Rational>` (exponent map over
7 SI base dimensions). Dimension algebra (mul, div, pow) is closed and exact.

A TIR carries a `DagRegistry` (`HashMap<DagId, DagTIR>`): one `DagTIR` per file
root plus one per inline `dag { ... }` block, plus merged entries for DAGs
included from dependency files. The file's own root is reachable via
`TIR::root()` / `TIR::root_mut()`. `TIR::module_aliases` maps each `import`
alias to its target file's canonical `DagId`, so user-typed
`@alias.dag(args)` calls resolve through the same registry as same-file inline
calls.

### 1.7 ExecPlan Compilation

`exec_plan::compile()` does two topological sorts:

1. **Const sort**: order const nodes by `const_deps`, evaluate each in sequence via
   `eval_expr`, store results in `const_values` (keyed by `ScopedName`).
2. **Runtime sort**: order params + nodes by `runtime_deps`, producing
   `topo_order` (also `ScopedName`-keyed).

It also resolves domain constraints (min/max bounds from type annotations)
using the now-known const values. Domain checks themselves live in
`graphcal-eval/src/domain_check.rs` and run both at compile time
(`exec_plan`) and at runtime (`eval/runtime.rs`).

### 1.8 Runtime Evaluation

`eval::runtime::run_eval_loop()` iterates `topo_order`:

- For each declaration, check whether any dependency failed; if so, mark
  `DependencyFailed` and continue (independent nodes still evaluate).
- Evaluate the expression via `eval_expr`.
- Check domain constraints.
- Convert `RuntimeValue` -> `Value` (user-facing type with dimension + display unit).
- Attach display units by walking the expression tree for `UnitLiteral` / `Convert`
  nodes.

---

## 2. Crate Map

The workspace contains 6 crates. Dependencies flow downward in this diagram:

```text
graphcal-cli          (binary: CLI entry point)
graphcal-lsp          (binary: Language Server)
    |
    +---> graphcal-eval       (evaluation engine)
    |         |
    |         +---> graphcal-compiler   (lexer, parser, IR, TIR, registry)
    |         |         |
    |         |         +---> graphcal-io   (filesystem abstraction)
    |         |
    +---> graphcal-fmt        (source code formatter)
              |
              +---> graphcal-compiler
```

### 2.1 graphcal-compiler

The core language crate. Contains everything up to and including TIR.

| Module                       | Purpose                                                                            |
| ---------------------------- | ---------------------------------------------------------------------------------- |
| `syntax/lexer.rs`            | Tokenizer (logos-based, 2-token lookahead)                                         |
| `syntax/token.rs`            | `Token` enum definition                                                            |
| `syntax/parser/`             | Recursive-descent parser producing `File<Raw>`; `decl/` per declaration form       |
| `syntax/ast.rs`              | AST node types (`File<P>`, `Declaration<P>`, `Expr<P>`, `TypeExpr<P>`)             |
| `syntax/phase.rs`            | AST phase parameter: `Raw` vs `Desugared`; `RawDeclSugar`, `RawExprSugar`          |
| `syntax/desugar.rs`          | Multi-decl expansion (the per-sugar producer of `File<Desugared>`)                 |
| `syntax/name_resolve.rs`     | `NameRef` / `QualifiedNameRef` rewrite pass                                        |
| `syntax/names.rs`            | Newtype wrappers: `DeclName`, `DimName`, `UnitName`, `ScopedName`, …               |
| `syntax/dag_id.rs`           | `DagId` — filesystem-independent module/DAG identifier                             |
| `syntax/dimension.rs`        | `BaseDimId`, `Dimension`, `Rational` (dimension algebra)                           |
| `syntax/span.rs`             | Byte-offset source locations                                                       |
| `syntax/visitor.rs`          | Visitor pattern for AST traversal (`ExprVisitor`, `ExprVisitorMut`)                |
| `syntax/comments.rs`         | Comment extraction for the formatter                                               |
| `desugar/mod.rs`             | Generic walker + `DesugarSugar` trait                                              |
| `desugar/convert.rs`         | `From<File<Raw>> for File<Desugared>` impl that drives the walker                  |
| `desugar/desugared_ast.rs`   | Re-exports / type aliases for the post-desugar AST                                 |
| `ir/lower.rs`                | `IR` struct, `lower()` function, plus `lower_dag_body_to_ir` for inline DAGs       |
| `ir/resolve/`                | Name resolution (`names`, `scope`, `deps`), visibility, dependency extraction      |
| `tir/typed.rs`               | `TIR`, `DagTIR`, `DagRegistry`, `ResolvedTypeExpr`, `type_resolve`                 |
| `tir/dim_check/`             | Dimension inference + type matching (`infer/scalar`, `control`, `collections`, …)  |
| `registry/types.rs`          | `Registry` struct: dimensions, units, indexes, struct types, functions             |
| `registry/builtins.rs`       | Builtin constants and functions                                                    |
| `registry/declared_type.rs`  | `DeclaredType` — concrete user-facing type carried alongside values                |
| `registry/runtime_value.rs`  | `RuntimeValue` enum (Scalar/Bool/Int/Label/Struct/Indexed/Datetime)                |
| `registry/resolve_types.rs`  | Resolve `TypeExpr` against the registry                                            |
| `registry/format.rs`         | Number / unit-expression formatting helpers (re-exported via `eval`)               |
| `registry/manifest.rs`       | `graphcal.toml` parsing                                                            |
| `registry/prelude.rs`        | Prelude of built-in dimensions, units, and constants                               |
| `registry/time_scale.rs`     | `TimeScale` (UTC, TAI, TT, …) for `Datetime`                                       |
| `registry/error.rs`          | `GraphcalError` variants + miette annotations                                      |

### 2.2 graphcal-eval

Evaluation engine. Depends on `graphcal-compiler` and re-exports most of its types.

| Module                     | Purpose                                                                    |
| -------------------------- | -------------------------------------------------------------------------- |
| `eval/mod.rs`              | Public API: `compile_and_eval`, `compile_to_tir_from_project`, …           |
| `eval/runtime.rs`          | Core eval loop, `RuntimeValue` -> `Value` conversion                       |
| `eval/display.rs`          | Display unit extraction and attachment                                     |
| `eval/types.rs`            | `Value`, `EvalResult`, `DisplayUnit`, `AssertResult`, `NodeError` types    |
| `eval/project/mod.rs`      | Multi-file orchestration: alias rewrites, `pub_names` extraction, glue     |
| `eval/project/imports.rs`  | `import` / `include` processing (selective, module, parameterized)         |
| `eval/project/lowering.rs` | IR lowering + registry merging across dep files                            |
| `eval/project/pipeline.rs` | Top-level per-file evaluation orchestration                                |
| `eval_expr/mod.rs`         | Dispatch + `EvalContext`; re-exports `RuntimeValue`                        |
| `eval_expr/arithmetic.rs`  | Numeric / scalar / dimension arithmetic                                    |
| `eval_expr/collections.rs` | `Indexed`, `Struct`, table / map literals, projections                     |
| `eval_expr/control.rs`     | `if`/`else`, `match`, `for`, `scan`, `unfold`                              |
| `eval_expr/functions.rs`   | Function calls (builtin + user-defined)                                    |
| `exec_plan.rs`             | `ExecPlan` struct, topological sorts, const evaluation                     |
| `inline_dag.rs`            | Compile inline `dag { ... }` bodies into per-DAG `DagTIR`s                 |
| `domain_check.rs`          | `DomainViolation` checks for resolved domain constraints                   |
| `loader.rs`                | `LoadedProject` / `LoadedFile` / `LoadedDag` + circular-import detection   |

### 2.3 graphcal-fmt

Source code formatter.

- `format_source(&str) -> Option<String>`: parses the source, pretty-prints it
  using the `pretty` crate's `DocAllocator`.
- Declaration formatting is in `format/decl.rs`.

### 2.4 graphcal-io

Filesystem abstraction layer.

- `FileSystemReader` trait with methods for reading files and listing directories.
- Implementations: `RealFileSystem` (std::fs), `InMemoryFileSystem` (testing),
  `OverlayFileSystem` (testing).
- Enables WASM compatibility and deterministic testing.

### 2.5 graphcal-cli

Binary crate. CLI entry point with subcommands:

| Subcommand  | Purpose                                                      |
| ----------- | ------------------------------------------------------------ |
| `eval`      | Compile and evaluate a `.gcl` file; display as table or JSON |
| `format`    | Format `.gcl` files (check or write mode)                    |
| `check`     | Parse + type-check without evaluation                        |
| `lsp`       | Start Language Server Protocol server                        |

Key options: `--set name=expr` (override params), `--input file.json`,
`--format text|json`, `--plot browser|json|file.html`.

### 2.6 graphcal-lsp

Language Server Protocol implementation (async, tower-lsp).

| Module                | Feature                                                 |
| --------------------- | ------------------------------------------------------- |
| `server.rs`           | Main server loop, `run_analysis()` with 4-case dispatch |
| `hover.rs`            | Hover information                                       |
| `completion.rs`       | Autocomplete                                            |
| `goto_definition.rs`  | Jump to definition                                      |
| `references.rs`       | Find all references                                     |
| `diagnostics.rs`      | Error highlighting                                      |
| `inlay_hints.rs`      | Computed value hints                                    |
| `formatting.rs`       | Format on save                                          |
| `document_symbols.rs` | Outline view                                            |
| `rename.rs`           | Rename refactoring                                      |
| `signature_help.rs`   | Function signature popup                                |
| `document_links.rs`   | Import link navigation                                  |
| `code_actions.rs`     | Code actions (e.g., add `pub` keyword)                  |
| `cursor_context.rs`   | Context resolution at cursor position                   |
| `resolve.rs`          | Symbol resolution helpers                               |
| `convert.rs`          | Type conversions for LSP protocol types                 |
| `symbol_table.rs`     | Symbol table management for analysis                    |

`run_analysis()` skips the evaluation pipeline for library files
(`TIR::is_library()` — any required `param` or required `index`), so the editor
doesn't surface `RequiredIndexNotBound` / `RequiredParamNotProvided` diagnostics
on files meant to be consumed via a parameterized `include`. Inlay hints will
not appear for such files; this asymmetry is intentional.

---

## 3. Key Data Structures

Understanding these types is essential before diving into any crate.

### 3.1 AST Layer

```text
File<P: Phase = Raw>
  declarations: Vec<Declaration<P>>

Declaration<P>
  attributes: Vec<Attribute>    // #[assumes(...)], #[expected_fail(...)], etc.
  visibility: Visibility        // Private | Public | PublicBind
  kind: DeclKind<P>             // Param | Node | ConstNode | ... | Sugar(P::DeclSugar)
  span: Span

Expr<P>
  kind: ExprKind<P>             // Number | BinOp | FnCall | GraphRef(@name) | ... | Sugar(P::ExprSugar)
  span: Span

TypeExpr<P>                     // Dimensionless | Bool | Scalar(DimExpr) | Indexed(...) | ...
```

`P = Raw` is the parser/formatter view (sugar variants are reachable).
`P = Desugared` is what every semantic stage consumes — `Sugar(_)` payloads
are `core::convert::Infallible`, so `match` arms over them are statically
unreachable (use `crate::syntax::phase::never(x)` to discharge them).

### 3.2 IR Layer

```text
IR                              // One IR ⇔ one DAG body (file root OR inline `dag`)
  registry: Registry            // Frozen type system
  consts: Vec<ConstEntry>       // Const declarations
  params: Vec<ParamEntry>       // Param declarations
  nodes: Vec<NodeEntry>         // Node declarations
  asserts: Vec<AssertEntry>
  plots, figures, layers: Vec<...Entry>
  runtime_deps: HashMap<ScopedName, BTreeSet<ScopedName>>   // @ references
  const_deps: HashMap<ScopedName, BTreeSet<ScopedName>>
  source_order: Vec<(ScopedName, DeclCategory)>
  assumes_map, expected_fail: HashMap<ScopedName, ...>
  imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>
  imported_decl_types: HashMap<ScopedName, DeclaredType>     // values supplied later
  imported_value_sources: HashMap<ScopedName, ImportedValueSource>
  pub_names: HashSet<DeclName>
```

### 3.3 TIR Layer

The TIR is **not** flat — one file produces one `TIR` containing many
`DagTIR`s (file root + each inline `dag` + merged dep DAGs):

```text
TIR
  registry: Registry                                       // Shared by every DAG in the file
  root_dag_id: DagId                                       // Key for the file's own body
  dags: DagRegistry = HashMap<DagId, DagTIR>
  module_aliases: HashMap<String, DagId>                   // import alias → dep DagId

DagTIR
  dag_id: DagId
  consts, params, nodes, asserts, plots, figures, layers   // Same shape as IR
  runtime_deps, const_deps, source_order
  assert_names, assumes_map, expected_fail
  resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>
  domain_constraints: HashMap<ScopedName, ResolvedDomainConstraint>
  imported_values, imported_decl_types, imported_value_sources
  pub_nodes: HashSet<String>                               // Cross-file visibility proxy
```

`TIR::is_library()` returns true when any required `param` or required `index`
is present; the LSP uses this to suppress unbound-param diagnostics on files
intended to be consumed via parameterized include.

### 3.4 ExecPlan

```text
ExecPlan
  const_values: HashMap<ScopedName, RuntimeValue>   // Pre-evaluated at compile time
  imported_values: HashMap<ScopedName, RuntimeValue>
  topo_order: Vec<ScopedName>                       // Runtime evaluation order
  expressions: HashMap<ScopedName, Expr>            // Unevaluated runtime exprs
  assert_bodies, plot_bodies, figure_bodies, layer_bodies
  assumes_map: HashMap<ScopedName, Vec<ScopedName>>
  domain_constraints: HashMap<ScopedName, ResolvedDomainConstraint>
```

### 3.5 Runtime Values

Two levels of value representation:

- **`RuntimeValue`** (internal): `Scalar(f64)`, `Bool(bool)`, `Int(i64)`,
  `Label { index, variant }`, `Struct { ... }`, `Indexed { ... }`, `Datetime(Epoch)`.
  No dimension or display-unit metadata.

- **`Value`** (user-facing): Same variants, but `Scalar` carries `dimension` and
  `display_unit`. This is what appears in `EvalResult`.

### 3.6 Registry

```text
Registry
  dimensions: DimensionRegistry     // Base dims + derived dims
  units: UnitRegistry               // Named units with SI scale factors
  indexes: IndexRegistry            // Named indexes (variants or ranges)
  types: StructTypeRegistry         // Record/union type definitions
  functions: FunctionRegistry       // Builtin + user-defined functions
  prelude: Prelude                  // PI, E, TAU, etc.
```

### 3.7 Dimension

```text
Dimension = BTreeMap<BaseDimId, Rational>
```

7 base dimension IDs (Length, Mass, Time, Current, Temperature, Amount, Intensity).
`Rational { num: i32, den: i32 }` for exact exponent arithmetic.
Example: velocity = `{ Length: 1, Time: -1 }`.

---

## 4. Declaration Categories

Graphcal distinguishes declarations by category, which determines when and how
they are evaluated:

All declarations are **private by default**. The two visibility modifiers
are `pub` (visible at the include / import boundary) and `pub(bind)`
(visible AND overridable via include / import bindings). Required `index`,
`type`, and `dim` declarations must carry `pub(bind)`. The `param` keyword
never takes a visibility modifier — required `param` is implicitly bindable.

| Category          | Keyword                                     | Evaluated                                               | Can Reference                                        | Visibility                                            | Naming Convention                                |
| ----------------- | ------------------------------------------- | ------------------------------------------------------- | ---------------------------------------------------- | ----------------------------------------------------- | ------------------------------------------------ |
| Type system       | `base dim`, `dim`, `unit`, `type`, `index`  | At registry build                                       | Other type-system decls                              | `pub` / `pub(bind)`; required forms need `pub(bind)`  | PascalCase (types/indexes) or snake_case (units) |
| DAG               | `dag`                                       | At include-time (body compiled/merged on instantiation) | Own params/nodes, own imports, own includes          | `pub` optional                                        | `lower_snake_case`                               |
| Const node        | `const node`                                | Compile-time (ExecPlan)                                 | Other const nodes via `@`, built-in constants        | `pub` optional                                        | `lower_snake_case`                               |
| Param             | `param`                                     | Runtime (input)                                         | Const nodes, other params/nodes                      | none (implicitly bindable; A5)                        | `lower_snake_case`                               |
| Node              | `node`                                      | Runtime (computed)                                      | Const nodes, params, other nodes                     | `pub` optional                                        | `lower_snake_case`                               |
| Assert            | `assert`                                    | After all nodes                                         | Const nodes, params, nodes                           | `pub` optional                                        | `lower_snake_case`                               |
| Plot/Figure/Layer | `plot`, `figure`, `layer`                   | After all nodes                                         | Const nodes, params, nodes                           | `pub` optional (`pub` plot ⇒ standalone output)       | `lower_snake_case`                               |
| Multi-decl sugar  | `param,node,…  =  table[...] { … }`         | Desugared at parse                                      | Same as the slot kinds                               | none (forbidden on multi-decl in v1)                  | `lower_snake_case` per slot                      |

The `@name` syntax references params, nodes, and const nodes. Built-in
constants (PI, E, TAU, SQRT2, LN2, LN10) use bare `UPPER_SNAKE_CASE` names
without `@`.

---

## 5. Dependency Tracking and Topological Evaluation

Dependency tracking is central to Graphcal's reactive computation model:

1. **Const dependencies** (`const_deps`): extracted during name resolution.
   Const nodes form a DAG; cycles are detected and reported.
2. **Runtime dependencies** (`runtime_deps`): extracted from `@` references in
   params and nodes. Also forms a DAG.
3. **Assert assumptions** (`assumes_map`): which nodes/params an assert checks,
   declared via `#[assumes(...)]`.

Both const and runtime DAGs are topologically sorted using `petgraph`. The
sorted order determines evaluation sequence in `ExecPlan`.

---

## 6. Error Handling

All errors use the `miette` crate for rich terminal diagnostics with source
snippets, labeled spans, and error codes.

```text
CompileError
  Parse(ParseError)         // P001-P014
  Eval(GraphcalError)       // N001+, G001, D001+, S002+, E001, X001, V001+, M020
  Load(GraphcalError)

GraphcalError               // 50+ variants, each with miette diagnostic annotations
  DuplicateName (N001)
  CyclicDependency (G001)
  DimensionMismatch (D001)
  TypeAnnotationMismatch (D002)
  UnknownUnit (D003)
  ImportRuntimeItem (M020)    // import used for runtime items (use include instead)
  ImportPrivateItem (V001)      // importing a non-pub item
  RequiredItemMustBePub (V002)  // required param/index missing pub
  PrivateInPublic (V003)      // pub item exposes private type
  PubIndexVariantLiteral (V004)
  ...
```

Every error carries a `NamedSource<Arc<String>>` for inline snippet display.
Error codes are searchable (e.g., `D001` for dimension mismatch).

---

## 7. Import and Module System

Graphcal splits cross-file and DAG reuse into two keywords:

- **`import`** -- compile-time names only: `const node`, `dim`, `unit`, `type`,
  `index`, `dag`, and evaluated `assert` declarations. Using `import` for
  runtime items (`param`, `node`) is an error (M020). `import` does not accept
  parameter bindings and cannot target cross-file DAG instantiation.
- **`include`** -- selective/module inclusion with optional param/index bindings.
  It is used for runtime items (cross-file `param`s) and for DAG instantiation
  (inline, cross-file, or via a fully-qualified module path).

All paths are **dot-separated** identifiers, absolute from a package root.
The first segment is the package name (a virtual single-file package's name
is the file stem; a real package's name comes from `graphcal.toml`).
There are no quoted file strings, no `..`, and no `/` inside Graphcal source
(see `docs/language/multi-file.md`).

| Style                   | Syntax                                              | Effect                                                  |
| ----------------------- | --------------------------------------------------- | ------------------------------------------------------- |
| Bare import             | `import nasa.rocket;`                               | Bring module `rocket` into scope under its leaf name    |
| Aliased import          | `import nasa.rocket as nr;`                         | Bring the module under alias `nr`                       |
| Selective import        | `import nasa.rocket.{Orbit, compute_thrust as ct};` | Bring only the listed compile-time names                |
| Re-export (whole)       | `pub import nasa.rocket;`                           | Re-export every `pub` item from the leaf module         |
| Re-export (selective)   | `import nasa.rocket.{ pub Orbit };`                 | Re-export only the marked items                         |
| Bare include            | `include nasa.rocket.compute_thrust(args);`         | Sugar for `... as compute_thrust`                       |
| Aliased include         | `include nasa.rocket.compute_thrust(args) as ct;`   | Outputs reached as `@ct.<output>`                       |
| Selective include       | `include nasa.rocket.compute_thrust(args).{ y };`   | `y` becomes a node in the current DAG                   |
| Parameterized include   | `include lib.rocket(dry_mass: 800.0 kg).{ delta_v };` | Bind params/indexes, then take selected outputs       |
| Inline DAG include      | `include orbital_velocity(gm: @gm, r: @r).{ v };`   | Instantiate a same-file `dag` block                     |
| Inline DAG call expr    | `@compute_thrust(dry_mass: @m).thrust`              | Inline DAG invocation in expression position            |

Visibility is a two-axis split (see `docs/language/multi-file.md`):

- bare = private (V001 if imported)
- `pub` = visible at boundary, not bindable
- `pub(bind)` = visible AND bindable; required `dim` / `type` / `index` must be
  `pub(bind)` (V002)
- `param` is annotation-free and implicitly bindable (axiom A5)

`pub(bind)` is illegal on `import` (use-sites are not bindable).

Bindings on an `include` are optional for any param or index that has a
default; unbound ones keep their declared defaults. Required params and
indexes (declared without a default) must always be bound.

Inline DAG bodies see only their own declarations, their own imports, and
the outputs of their own includes -- there is no lexical inheritance from
the enclosing file's top-level scope. Top-level types (`type`, `dim`, etc.)
referenced inside a `dag` body must be brought in by an explicit `import`.

Circular imports are detected during project loading via depth-first traversal
with cycle tracking. Files are loaded in topological order (dependencies first).
Imported values are injected as pre-evaluated `RuntimeValue`s.

---

## 8. Testing Patterns

### Unit Tests

Inline `#[test]` functions in each module. Modules use
`#![allow(clippy::unwrap_used, reason = "test code")]`.

### Snapshot Tests

The `insta` crate is used extensively:

- **Error output**: `crates/graphcal-eval/tests/snapshots/`
- **Formatter**: `crates/graphcal-fmt/tests/snapshots/`

Fixture files live in `tests/fixtures/`, organized into four top-level
categories by their expected outcome:

- `tests/fixtures/valid/` -- passes `graphcal check` and evaluates cleanly.
- `tests/fixtures/valid_library/` -- passes `graphcal check` but is not
  designed to be evaluated standalone (e.g. declares `pub(bind)` required
  indexes that need binding via parameterized include). `eval` may fail.
- `tests/fixtures/runtime_error/` -- passes `graphcal check` but fails at
  evaluation (e.g. assertion failures, division by zero, domain violations).
- `tests/fixtures/invalid/` -- fails `graphcal check` (parse, type, or
  dimension errors detected statically).

Each category preserves a `multi/` subdirectory for multi-file fixture
projects.

The categorization is enforced by the `fixtures_match_their_category` test
in `crates/graphcal-cli/tests/cli.rs`.

### Integration Tests

- `crates/graphcal-eval/tests/edge_case_bugs.rs` -- regression tests
- `crates/graphcal-eval/tests/declaration_order.rs` -- property-based tests (proptest)
- `crates/graphcal-cli/tests/cli.rs` -- end-to-end CLI tests

### Running Tests

```bash
cargo test --workspace           # all tests
cargo test -p graphcal-compiler  # single crate
cargo insta review               # review snapshot changes
```

---

## 9. Design Patterns and Conventions

| Pattern                                | Where               | Why                                                     |
| -------------------------------------- | ------------------- | ------------------------------------------------------- |
| Trait-based I/O                        | `graphcal-io`       | Pluggable filesystem for WASM and testing               |
| Visitor pattern                        | `syntax/visitor.rs` | Safe recursive AST traversal                            |
| `IndexMap` ordering                    | Throughout eval     | Deterministic output preserving declaration order       |
| `Arc<String>` source sharing           | Diagnostics         | Minimize clones across error reports                    |
| Separated const/runtime phases         | `exec_plan.rs`      | Consts evaluated at compile time, runtime values lazily |
| Display units separate from dimensions | `eval/display.rs`   | Compute in SI, display in user-chosen units             |
| `Result<T, GraphcalError>` everywhere  | All crates          | Composable error handling with `?`                      |
| Per-node error containment             | `runtime.rs`        | Independent nodes evaluate even if siblings fail        |

---

## 10. Suggested Reading Order

For a first pass through the codebase, this order follows the pipeline and
builds understanding incrementally:

1. **`syntax/token.rs`** and **`syntax/ast.rs`** -- understand the token and AST
   vocabularies. These are the "alphabet" of everything downstream.
2. **`syntax/phase.rs`** -- the `Raw` / `Desugared` phase split and the
   `Sugar` slots; the type-level mechanism that keeps surface sugar out of the
   semantic core.
3. **`syntax/lexer.rs`** -- short file; see how tokens are produced.
4. **`syntax/parser/expr.rs`** -- expression parsing shows the precedence
   hierarchy and how AST nodes are constructed.
5. **`syntax/parser/decl/value.rs`** -- how param/node/const declarations are
   parsed.
6. **`desugar/mod.rs`** and **`syntax/desugar.rs`** -- how multi-decl /
   table-literal sugar is expanded into the canonical AST.
7. **`syntax/name_resolve.rs`** -- the bare-identifier rewrite pass.
8. **`ir/resolve/mod.rs`** and **`ir/resolve/deps.rs`** -- name resolution and
   dependency extraction.
9. **`ir/lower.rs`** -- the `lower()` function ties parsing and resolution together.
10. **`registry/types.rs`** -- how dimensions, units, and types are registered.
11. **`syntax/dag_id.rs`** -- the abstract DAG identity used to address file
    roots and inline DAGs uniformly.
12. **`tir/typed.rs`** -- type resolution; pay attention to the `TIR` /
    `DagTIR` / `DagRegistry` split.
13. **`tir/dim_check/infer/mod.rs`** -- dimension inference (the most
    intellectually interesting part of the compiler).
14. **`exec_plan.rs`** -- how TIR becomes an executable plan.
15. **`eval/runtime.rs`** -- the evaluation loop.
16. **`eval_expr/mod.rs`** -- expression evaluation dispatch.
17. **`eval/project/pipeline.rs`** -- per-file evaluation orchestration in
    multi-file projects.
18. **`eval/types.rs`** -- `Value` and `EvalResult` definitions.
19. **`graphcal-cli/src/main.rs`** -- see how the full pipeline is invoked.

After this pass, the LSP and formatter can be read independently
as they are self-contained consumers of the compiler/eval APIs.
