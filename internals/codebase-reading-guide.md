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
AST  (File -> Vec<Declaration>, each containing Expr trees)
  |
  |  [Name Resolution + IR Lowering]  crates/graphcal-compiler/src/ir/
  v
IR  (resolved names, dependency graphs, Registry of types/units/dims)
  |
  |  [Type Resolution + Dimension Checking]  crates/graphcal-compiler/src/tir/
  v
TIR  (fully typed IR with concrete Dimensions on every declaration)
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

### 1.3 Name Resolution + IR Lowering -> IR

Handled in `ir/resolve/` and `ir/lower.rs`:

1. **Collect local names** from AST declarations.
2. **Duplicate / casing checks**: params, nodes, and const nodes must be
   `lower_snake_case`; `UPPER_SNAKE_CASE` is reserved for built-in constants
   (PI, E, TAU).
3. **Visibility validation**: required params and indexes must be `pub`;
   private items cannot be imported across files.
4. **Scope validation**: const node bodies cannot contain `@` references to
   runtime params/nodes; assert bodies cannot reference other asserts.
5. **Dependency extraction**: build `runtime_deps` (params/nodes) and `const_deps`
   (const nodes) graphs; detect cycles via `petgraph::algo::toposort`.
6. **Registry building**: register dimensions, units, indexes, struct types, and
   functions (builtins + user-defined).

The result is the `IR` struct, which bundles resolved entries, a frozen `Registry`,
and dependency maps.

### 1.4 Type Resolution + Dimension Checking -> TIR

Handled in `tir/typed.rs` and `tir/dim_check/`:

1. **Resolve type annotations**: convert AST type names into `ResolvedTypeExpr`
   (concrete `Dimension` values, struct types, generics).
2. **Dimension inference**: Hindley-Milner-like constraint generation + unification
   for each expression.
3. **Type matching**: verify inferred type against declared type for every
   param (default), node, const node, and assert.

Dimensions are represented as `BTreeMap<BaseDimId, Rational>` (exponent map over
7 SI base dimensions). Dimension algebra (mul, div, pow) is closed and exact.

### 1.5 ExecPlan Compilation

`exec_plan::compile()` does two topological sorts:

1. **Const sort**: order const nodes by `const_deps`, evaluate each in sequence via
   `eval_expr`, store results in `const_values`.
2. **Runtime sort**: order params + nodes by `runtime_deps`, producing `topo_order`.

It also resolves domain constraints (min/max bounds from type annotations)
using the now-known const values.

### 1.6 Runtime Evaluation

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

| Module                | Purpose                                                                       |
| --------------------- | ----------------------------------------------------------------------------- |
| `syntax/lexer.rs`     | Tokenizer (logos-based, 2-token lookahead)                                    |
| `syntax/token.rs`     | `Token` enum definition                                                       |
| `syntax/parser/`      | Recursive-descent parser producing AST                                        |
| `syntax/ast.rs`       | AST node types (`File`, `Declaration`, `Expr`, `TypeExpr`)                    |
| `syntax/names.rs`     | Newtype wrappers: `DeclName`, `DimName`, `UnitName`, etc.                     |
| `syntax/dimension.rs` | `BaseDimId`, `Dimension`, `Rational` (dimension algebra)                      |
| `syntax/span.rs`      | Byte-offset source locations                                                  |
| `syntax/visitor.rs`   | Visitor pattern for AST traversal                                             |
| `syntax/comments.rs`  | Comment extraction for the formatter                                          |
| `ir/lower.rs`         | `IR` struct, `lower()` function (AST -> IR)                                   |
| `ir/resolve/`         | Name resolution, scope checking, visibility, dependency extraction            |
| `tir/typed.rs`        | `TIR` struct, `resolve()` function (IR -> TIR)                                |
| `tir/dim_check/`      | Dimension inference and type matching                                         |
| `registry/types.rs`   | Core registry struct and type definitions                                     |
| `registry/`           | Type system: dimensions, units, indexes, structs, functions, builtins, errors |

### 2.2 graphcal-eval

Evaluation engine. Depends on `graphcal-compiler` and re-exports most of its types.

| Module                     | Purpose                                                                |
| -------------------------- | ---------------------------------------------------------------------- |
| `eval/mod.rs`              | Public API: `compile_and_eval`, `compile_to_tir_project`, etc.         |
| `eval/runtime.rs`          | Core eval loop, `RuntimeValue` -> `Value` conversion                   |
| `eval/display.rs`          | Display unit extraction and attachment                                 |
| `eval/types.rs`            | `Value`, `EvalResult`, `DisplayUnit` definitions                       |
| `eval/format.rs`           | Output formatting helpers                                              |
| `eval/project/`            | Multi-file project compilation (split into submodules)                 |
| `eval/project/imports.rs`  | Import and include processing                                          |
| `eval/project/lowering.rs` | IR lowering and registry merging                                       |
| `eval/project/pipeline.rs` | Evaluation orchestration                                               |
| `eval_expr/`               | Expression evaluator: arithmetic, control flow, functions, collections |
| `exec_plan.rs`             | `ExecPlan` struct, topological sorts, const evaluation                 |
| `loader.rs`                | Project loading with circular import detection                         |

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
File
  declarations: Vec<Declaration>

Declaration
  attributes: Vec<Attribute>    // #[assumes(...)], etc.
  is_pub: bool                  // visibility (private by default)
  kind: DeclKind                // Param | Node | ConstNode | Dimension | Unit | ...
  span: Span

Expr
  kind: ExprKind                // Number | BinOp | FnCall | GraphRef(@name) | ...
  span: Span

TypeExpr                        // Dimensionless | Bool | Scalar(DimExpr) | Indexed(...) | ...
```

### 3.2 IR Layer

```text
IR
  registry: Registry            // Frozen type system
  consts: Vec<ConstEntry>       // Const declarations
  params: Vec<ParamEntry>       // Param declarations
  nodes: Vec<NodeEntry>         // Node declarations
  asserts: Vec<AssertEntry>
  plots, figures, layers: ...
  runtime_deps: HashMap<ScopedName, HashSet<ScopedName>>   // @ references
  const_deps: HashMap<ScopedName, HashSet<ScopedName>>
  source_order: Vec<(ScopedName, DeclCategory)>
  imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>
```

### 3.3 TIR Layer

Same shape as IR but with:

- `resolved_decl_types: HashMap<ScopedName, ResolvedTypeExpr>` -- every declaration
  has a concrete type with resolved dimensions.
- `domain_constraints: HashMap<ScopedName, ResolvedDomainConstraint>` -- min/max bounds.

### 3.4 ExecPlan

```text
ExecPlan
  const_values: HashMap<DeclName, RuntimeValue>   // Pre-evaluated at compile time
  topo_order: Vec<DeclName>                        // Runtime evaluation order
  expressions: HashMap<DeclName, Expr>             // Unevaluated runtime exprs
  assert_bodies, plot_bodies, ...
  domain_constraints: HashMap<DeclName, ResolvedDomainConstraint>
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

All declarations are **private by default**. The `pub` keyword makes them
visible to other files. Required params and indexes must be marked `pub`.

| Category          | Keyword                                          | Evaluated                                               | Can Reference                                        | Visibility                                     | Naming Convention                                |
| ----------------- | ------------------------------------------------ | ------------------------------------------------------- | ---------------------------------------------------- | ---------------------------------------------- | ------------------------------------------------ |
| Type system       | `base dim`, `dim`, `unit`, `type`, `index`, `fn` | At registry build                                       | Other type-system decls                              | `pub` optional; required `index` must be `pub` | PascalCase (types/indexes) or snake_case (units) |
| DAG               | `dag`                                            | At include-time (body compiled/merged on instantiation) | Parent-scope imports, local params/nodes/const nodes | `pub` optional                                 | `lower_snake_case`                               |
| Const node        | `const node`                                     | Compile-time (ExecPlan)                                 | Other const nodes via `@`, built-in constants        | `pub` optional                                 | `lower_snake_case`                               |
| Param             | `param`                                          | Runtime (input)                                         | Const nodes, other params/nodes                      | `pub` optional; required params must be `pub`  | `lower_snake_case`                               |
| Node              | `node`                                           | Runtime (computed)                                      | Const nodes, params, other nodes                     | `pub` optional                                 | `lower_snake_case`                               |
| Assert            | `assert`                                         | After all nodes                                         | Const nodes, params, nodes                           | `pub` optional                                 | `lower_snake_case`                               |
| Plot/Figure/Layer | `plot`, `figure`, `layer`                        | After all nodes                                         | Const nodes, params, nodes                           | `pub` optional                                 | `lower_snake_case`                               |

The `@name` syntax references params, nodes, and const nodes. Built-in
constants (PI, E, TAU) use bare `UPPER_SNAKE_CASE` names without `@`.

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
  Parse(ParseError)         // P001-P005
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
  `index`, `fn`, `dag`, and evaluated `assert` declarations. Using `import` for
  runtime items (`param`, `node`) is an error (M020). `import` does not accept
  parameter bindings and cannot target cross-file DAG paths.
- **`include`** -- selective/module inclusion with optional param/index bindings.
  It is used for runtime items and for DAG instantiation (inline, cross-file, or
  bare-module DAG references).

| Style                   | Syntax                                           | Effect                                                  |
| ----------------------- | ------------------------------------------------ | ------------------------------------------------------- |
| Selective import        | `import "./lib.gcl" { fn_a, const_b as alias }`  | Import specific compile-time names                      |
| Module import           | `import "./lib.gcl" as ns`                       | Import under namespace (`ns::name`)                     |
| Parent scope            | `import .. { name1, name2 }`                     | Access enclosing DAG scope from inside a `dag`          |
| Selective include       | `include "./model.gcl" { x, y as z }`            | Bring selected exported names into scope                |
| Module include          | `include "./model.gcl" as model`                 | Bring exported names under a module prefix              |
| Parameterized include   | `include "./model.gcl"(param_x: 42.0) { y }`     | Bind params/indexes, then include selected outputs      |
| Inline DAG include      | `include orbital_velocity(gm: @gm, r: @r) { v }` | Instantiate a same-file `dag` block                     |
| Cross-file DAG include  | `include "./file.gcl"/dag_name(x: 1.0) { y }`    | Instantiate a named `dag` from another file             |
| Bare-module DAG include | `include pkg/lib/dag_name(x: 1.0) { y }`         | Instantiate a `dag` defined in the resolved module file |

Only `pub` items can be imported/included across files (V001).

When an `include` has any bindings, Graphcal enters strict binding mode:
all params and indexes with defaults must be bound explicitly unless the
declaration has `#[allow_defaults]`. Required params/indexes must always be
bound.

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

Fixture files live in `tests/fixtures/` (single-file) and
`tests/fixtures/multi/` (multi-file projects).

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
2. **`syntax/lexer.rs`** -- short file; see how tokens are produced.
3. **`syntax/parser/expr.rs`** -- expression parsing shows the precedence
   hierarchy and how AST nodes are constructed.
4. **`syntax/parser/decl/value.rs`** -- how param/node/const declarations are
   parsed.
5. **`ir/resolve/mod.rs`** and **`ir/resolve/deps.rs`** -- name resolution and
   dependency extraction.
6. **`ir/lower.rs`** -- the `lower()` function ties parsing and resolution together.
7. **`registry/types.rs`** -- how dimensions, units, and types are registered.
8. **`tir/typed.rs`** -- type resolution.
9. **`tir/dim_check/infer/mod.rs`** -- dimension inference (the most
   intellectually interesting part of the compiler).
10. **`exec_plan.rs`** -- how TIR becomes an executable plan.
11. **`eval/runtime.rs`** -- the evaluation loop.
12. **`eval_expr/mod.rs`** -- expression evaluation dispatch.
13. **`eval/types.rs`** -- `Value` and `EvalResult` definitions.
14. **`graphcal-cli/src/main.rs`** -- see how the full pipeline is invoked.

After this pass, the LSP and formatter can be read independently
as they are self-contained consumers of the compiler/eval APIs.
