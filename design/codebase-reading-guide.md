# Graphcal Codebase Reading Guide

A comprehensive guide for understanding, maintaining, and extending the graphcal codebase.

## Table of Contents

1. [What is Graphcal?](#1-what-is-graphcal)
2. [Theoretical Foundations](#2-theoretical-foundations)
3. [High-Level Architecture](#3-high-level-architecture)
4. [The Compilation and Evaluation Pipeline](#4-the-compilation-and-evaluation-pipeline)
5. [Incremental Reading Order](#5-incremental-reading-order)
6. [Crate: graphcal-syntax](#6-crate-graphcal-syntax)
7. [Crate: graphcal-registry](#7-crate-graphcal-registry)
8. [Crate: graphcal-ir](#8-crate-graphcal-ir)
9. [Crate: graphcal-tir](#9-crate-graphcal-tir)
10. [Crate: graphcal-eval](#10-crate-graphcal-eval)
11. [Crate: graphcal-io](#11-crate-graphcal-io)
12. [Crate: graphcal-fmt](#12-crate-graphcal-fmt)
13. [Crate: graphcal-dag](#13-crate-graphcal-dag)
14. [Crate: graphcal-cli](#14-crate-graphcal-cli)
15. [Crate: graphcal-lsp](#15-crate-graphcal-lsp)
16. [Testing Infrastructure](#16-testing-infrastructure)
17. [CI/CD and Developer Tooling](#17-cicd-and-developer-tooling)
18. [Design Documents and Phased Development](#18-design-documents-and-phased-development)
19. [Extending Graphcal](#19-extending-graphcal)

---

## 1. What is Graphcal?

Graphcal is a **type-safe, unit-aware, Git-friendly reactive programming language** for
engineering calculations. It aims to replace spreadsheets and simulation tools (like Excel and
Vensim) with a typed, version-controlled computation model.

A `.gcl` file describes a **directed acyclic graph (DAG)** of computations where:

- **`param`** declarations are tunable inputs (can be overridden from the CLI or JSON input).
- **`node`** declarations are computed values derived from params and other nodes.
- **`const`** declarations are compile-time constants.
- **`assert`** declarations are self-checking boolean expressions evaluated after the graph.
- **`plot`** / **`figure`** / **`layer`** declarations produce Vega-Lite visualizations.
- All values carry **physical dimensions** (Length, Time, Mass, ...) that are checked at compile
  time, so you cannot accidentally add a length to a time.

Here is a minimal example (`rocket.gcl`):

```gcl
dimension Velocity = Length / Time;
dimension Acceleration = Length / Time^2;

param dry_mass: Mass = 1200 kg;
param fuel_mass: Mass = 2800 kg;
param isp: Time = 320 s;
const G0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

Key syntax features to note:

- **`@` sigil** makes graph-level dependencies explicit: `@dry_mass` references the `param`
  named `dry_mass`. Local variables (e.g., `let` bindings inside blocks) are accessed without `@`.
- **Type annotations are required** on all declarations: `param dry_mass: Mass`.
- **Units are values**: `1200 kg` is `1200` multiplied by the scale factor of `kg`.

---

## 2. Theoretical Foundations

Understanding these concepts will help you reason about the code.

### 2.1. DAG-Based Reactive Computation

Graphcal's core model is a **Directed Acyclic Graph** where:

- Each `param` and `node` is a graph vertex.
- An edge from A to B means "B depends on A".
- Evaluation proceeds in **topological order**: every node is evaluated after all its dependencies.
- `const` values live outside the DAG -- they are evaluated at compile time in their own
  topological order.

The library `petgraph` is used for graph construction and topological sorting. Cycles are
detected and reported as errors.

### 2.2. Dimensional Analysis

Physical dimensions are represented as products of powers of **base dimensions**. Unlike the old
fixed 8-element array, dimensions now use a `BTreeMap<BaseDimId, Rational>` to support
user-defined base dimensions alongside the 8 prelude base dimensions:

```
Prelude base dimensions: Length, Time, Mass, Temperature, ElectricCurrent, Amount, LuminousIntensity, Angle
User-defined: dimension Currency;  // creates a new base dimension
```

Each base dimension has a `BaseDimId` (either `Prelude(name)` or `UserDefined(id)`), and
dimension exponents are exact `Rational` values (to support `sqrt`, which halves exponents).

Dimension arithmetic:

| Operation | Dimension Rule |
| ----------- | --------------- |
| `a + b`, `a - b` | `dim(a) == dim(b)` (must match) |
| `a * b` | `dim(a) * dim(b)` (exponents add) |
| `a / b` | `dim(a) / dim(b)` (exponents subtract) |
| `a ^ n` | `dim(a) ^ n` (exponents multiply by n) |
| `sqrt(a)` | `dim(a) ^ (1/2)` |
| `sin(a)` | `dim(a)` must be Angle; result is Dimensionless |

This is implemented in `dimension.rs` using `Rational` values for exact arithmetic.

### 2.3. Units vs. Dimensions

A critical distinction:

- **Dimensions** are *types* -- they describe what kind of quantity something is (e.g., Velocity
  = Length / Time).
- **Units** are *values* -- they are scale factors within a dimension (e.g., `km` = 1000 in the
  Length dimension, `hour` = 3600 in the Time dimension).

When you write `1200 kg`, the parser sees a number literal `1200` followed by a unit expression
`kg`. At evaluation time, the value is `1200 * scale_of(kg)`, and the dimension is `Mass`.

Unit conversion (`@transfer.tof -> hour`) divides by the target unit's scale factor for display
purposes.

### 2.4. The `@` Sigil and Purity

The `@` sigil is the central mechanism for:

1. **Making dependencies explicit**: every graph reference is visually marked.
2. **Enforcing function purity**: `fn` bodies must not contain `@`, so they cannot depend on
   graph state. This makes functions referentially transparent.
3. **Separating compile-time from runtime**: `const` bodies must not contain `@`, ensuring they
   can be evaluated before the runtime DAG exists.

### 2.5. Indexed Values

Indexed values are like typed, finite-dimensional vectors. A `cat` or `range` declaration defines a finite label
set, and `T[I]` is a value of type `T` for each label in `I`. For example:

```gcl
cat Maneuver { Departure, Correction, Insertion }
param delta_v: Velocity[Maneuver] = { ... };
```

Operations on indexed values include `for` comprehensions (map), aggregations (`sum`, `max`,
`min`, `mean`, `count`), `scan` (cumulative fold), and `unfold` (generate from seed).

Range indexes provide numeric iteration: `range TimeStep(0.0 s, 100.0 s, step: 0.1 s);`

### 2.6. DateTime Support

Graphcal supports date/time values with multiple time scales (UTC, TAI, TT, etc.) via the
`hifitime` and `jiff` crates. DateTime values are constructed with `datetime("...")` and
`epoch("...")` built-in functions, and support timezone display via `-> "timezone"` syntax.

---

## 3. High-Level Architecture

The project is a **Cargo workspace** with ten crates:

```
graphcal/
├── crates/
│   ├── graphcal-syntax/     # Lexer, parser, AST, dimension algebra, name types
│   ├── graphcal-registry/   # Type registry, builtins, prelude, error types, manifest
│   ├── graphcal-ir/         # Name resolution, dependency analysis, IR lowering
│   ├── graphcal-tir/        # Typed IR, dimension checking, type inference
│   ├── graphcal-eval/       # Execution planning, evaluation, multi-file orchestration
│   ├── graphcal-io/         # Filesystem abstraction (real + overlay for LSP)
│   ├── graphcal-fmt/        # AST-based code formatter
│   ├── graphcal-dag/        # ASCII DAG visualization
│   ├── graphcal-lsp/        # Language Server Protocol implementation
│   └── graphcal-cli/        # CLI binary (clap) with REPL
├── tree-sitter-graphcal/    # Tree-sitter grammar for syntax highlighting
├── editors/
│   ├── zed/                 # Zed editor extension
│   └── vscode/              # VS Code extension (TextMate grammar + LSP client)
├── tests/fixtures/          # .gcl example files and error test cases
├── design/                  # Language design documents
│   └── phases/              # Phased implementation plans
├── docs/                    # User-facing documentation (Zensical site)
└── .local/                  # Development notes (gitignored)
```

### Dependency Graph

```
graphcal-cli
  ├── graphcal-eval
  │     ├── graphcal-tir
  │     │     ├── graphcal-ir
  │     │     │     ├── graphcal-registry
  │     │     │     │     └── graphcal-syntax
  │     │     │     └── graphcal-syntax
  │     │     ├── graphcal-registry
  │     │     └── graphcal-syntax
  │     ├── graphcal-ir
  │     ├── graphcal-registry
  │     ├── graphcal-io
  │     └── graphcal-syntax
  ├── graphcal-fmt
  │     └── graphcal-syntax
  ├── graphcal-dag
  ├── graphcal-io
  └── graphcal-syntax

graphcal-lsp
  ├── graphcal-eval
  ├── graphcal-fmt
  ├── graphcal-io
  └── graphcal-syntax
```

- **`graphcal-syntax`** has no in-workspace dependencies. It depends on `logos` (lexing),
  `miette` (diagnostics), and `thiserror`.
- **`graphcal-registry`** depends on `graphcal-syntax` and adds `hifitime`, `jiff` (datetime),
  `toml-spanner` (manifest parsing), `indexmap`, and `serde`.
- **`graphcal-ir`** depends on `graphcal-registry` and `graphcal-syntax`, and adds `petgraph`
  (DAG, dependency graphs).
- **`graphcal-tir`** depends on `graphcal-ir`, `graphcal-registry`, and `graphcal-syntax`.
- **`graphcal-eval`** depends on `graphcal-tir`, `graphcal-ir`, `graphcal-registry`,
  `graphcal-io`, and `graphcal-syntax`. It re-exports public modules from `graphcal-registry`,
  `graphcal-ir`, and `graphcal-tir` for convenience.
- **`graphcal-io`** provides filesystem abstraction (`RealFileSystem`, `OverlayFileSystem`).
- **`graphcal-fmt`** depends on `graphcal-syntax` and adds `pretty` (pretty-printing).
- **`graphcal-dag`** depends on `petgraph` for DAG layout and ASCII rendering.
- **`graphcal-lsp`** depends on `graphcal-eval`, `graphcal-fmt`, `graphcal-io`, and
  `graphcal-syntax`, and adds `tower-lsp` (LSP protocol) and `tokio` (async runtime).
- **`graphcal-cli`** depends on `graphcal-eval`, `graphcal-fmt`, `graphcal-dag`, `graphcal-io`,
  and `graphcal-syntax`, and adds `clap` (argument parsing), `serde_json` (JSON output),
  `rustyline` (REPL), and `open` (browser).

### Layered Design

```
┌──────────────────────────────────┐
│     graphcal-cli / graphcal-lsp  │  User-facing: CLI (eval, shell, check),
│                                  │  editor integration
├──────────────────────────────────┤
│   graphcal-eval                  │  Orchestration: execution planning,
│                                  │  evaluation, multi-file loading
├──────────────────────────────────┤
│   graphcal-tir                   │  Type checking: dimension inference,
│                                  │  generic unification
├──────────────────────────────────┤
│   graphcal-ir                    │  Name resolution: reference validation,
│                                  │  dependency extraction, IR lowering
├──────────────────────────────────┤
│   graphcal-registry              │  Type system foundation: registry,
│                                  │  builtins, prelude, errors, manifest
├──────────────────────────────────┤
│   graphcal-syntax                │  Syntactic analysis: lexing, parsing,
│                                  │  AST, dimension algebra, name types
└──────────────────────────────────┘
  Utilities: graphcal-io, graphcal-fmt, graphcal-dag
```

---

## 4. The Compilation and Evaluation Pipeline

This is the core flow from source text to computed results. Understanding this pipeline is
essential for working on the codebase.

The pipeline is structured as a series of **intermediate representations** (IRs), each adding
more semantic information. The crate boundaries correspond to pipeline stages:

```
Source text (.gcl files)
        │
        ▼
   ┌─────────┐
   │  Parse   │  graphcal-syntax: text → AST
   └────┬─────┘
        │  File (list of Declarations)
        ▼
   ┌──────────┐
   │  Lower   │  graphcal-ir: name resolution (resolve/) +
   └────┬─────┘  IR construction (ir.rs) using Registry
        │  IR (resolved names + frozen Registry)
        ▼
   ┌────────────┐
   │ Type       │  graphcal-tir: resolve type annotations to
   │ Resolve    │  semantic types, resolve generics (tir.rs)
   └────┬───────┘
        │  TIR (resolved types + function signatures)
        ▼
   ┌───────────┐
   │ Fn Check  │  graphcal-ir: detect recursive function calls
   └────┬──────┘  (fn_check.rs)
        │
        ▼
   ┌───────────┐
   │ Dim Check │  graphcal-tir: infer and validate dimensions
   └────┬──────┘  for all expressions (dim_check/)
        │
        ▼                      ← compile_to_tir_*() stops here (for LSP)
   ┌───────────┐
   │  Compile  │  graphcal-eval: topological sort of consts +
   └────┬──────┘  eval, build runtime DAG (exec_plan.rs)
        │  ExecPlan (const values + topo order)
        ▼
   ┌──────────┐
   │ Evaluate │  graphcal-eval: walk DAG in topo order,
   └────┬─────┘  evaluate expressions (eval_expr/)
        │  EvalResult (consts, params, nodes with display units)
        ▼
   ┌──────────┐
   │  Output  │  graphcal-cli: format as text, JSON, or plots
   └──────────┘
```

The pipeline has two important **stopping points**:

- **`compile_to_tir_*()`** stops after dimension checking and returns the TIR. This is used by
  the LSP server, which needs type information but does not need to evaluate.
- **`compile_and_eval_*()`** runs the full pipeline through evaluation.

For **multi-file projects**, the pipeline begins with `loader.rs` (in `graphcal-eval`) which
performs a DFS over `import` declarations to load and parse all files in dependency order. Then
the pipeline runs for each file, with imported declarations propagated.

The pipeline is also **decoupled from I/O**: `compile_and_eval_project()` reads from disk using
`graphcal-io`'s `FileSystemReader` trait, while `compile_and_eval_named()` accepts an in-memory
source string. The `OverlayFileSystem` in `graphcal-io` combines disk reads with in-memory
overrides, which is critical for the LSP server working with unsaved editor buffers.

---

## 5. Incremental Reading Order

This section provides a step-by-step reading order. Each step builds on the previous one.

### Step 1: Read a `.gcl` file to understand the language surface

Start with these fixture files to get an intuitive sense of the language:

1. **`tests/fixtures/rocket.gcl`** -- The simplest complete example. Shows `dimension`, `param`,
   `node`, `const`, unit literals, the `@` sigil, and built-in functions (`ln`).
2. **`tests/fixtures/hohmann.gcl`** -- Adds struct types (`type`), block bodies with `let`
   bindings, field access (`.`), and unit conversion (`->`).
3. **`tests/fixtures/functions.gcl`** -- Adds pure functions (`fn`), dimension generics
   (`<D: Dim>`), block-body functions, and function-calling-function.
4. **`tests/fixtures/indexed.gcl`** -- Adds `cat`, indexed params (map literals), `for`
   comprehensions, aggregations (`sum`, `max`, `min`, `mean`, `count`), `scan`, and index
   generics (`<I: Index>`).
5. **`tests/fixtures/tagged_union.gcl`** -- Adds tagged union types and `match` expressions.
6. **`tests/fixtures/assertions.gcl`** -- Shows `assert` declarations for self-checking
   calculations.
7. **`tests/fixtures/range_index.gcl`** -- Shows range-based indexes and `unfold`.
8. **`tests/fixtures/datetime_basic.gcl`** -- Shows DateTime support with time scales.
9. **`tests/fixtures/plot_basic.gcl`** -- Shows Vega-Lite plot declarations.
10. **`tests/fixtures/multi/rocket_split/`** -- Shows multi-file imports with `import`.

### Step 2: Understand the AST (what the parser produces)

Read `crates/graphcal-syntax/src/ast.rs`. This defines every node in the abstract syntax tree.
The key types are:

- `File` -> `Vec<Declaration>` -> `DeclKind` (the top-level structure)
  - 13 declaration kinds: `Param`, `Node`, `Const`, `Dimension`, `Unit`, `Type`, `Fn`, `Index`,
    `Import`, `Assert`, `Plot`, `Figure`, `Layer`
- `Expr` -> `ExprKind` (the recursive expression tree)
  - 30+ expression kinds including: `Number`, `Integer`, `Bool`, `StringLiteral`, `GraphRef`,
    `ConstRef`, `BinOp`, `UnaryOp`, `FnCall`, `If`, `UnitLiteral`, `Convert`,
    `DisplayTimezone`, `AsCast`, `LocalRef`, `Block`, `FieldAccess`, `StructConstruction`,
    `MapLiteral`, `TableLiteral`, `ForComp`, `IndexAccess`, `Scan`, `Unfold`, `Match`,
    `TupleMatch`, `VariantLiteral`, `QualifiedGraphRef`
- `TypeExpr`, `DimExpr`, `UnitExpr` (type-level syntax)

Every node carries a `Span` for error reporting.

### Step 3: Understand tokens and lexing

Read these files in order:

1. **`crates/graphcal-syntax/src/span.rs`** -- Trivial but important: byte-offset spans.
2. **`crates/graphcal-syntax/src/token.rs`** -- All tokens defined via `logos` derive macro.
   Keywords, operators, delimiters, literals.
3. **`crates/graphcal-syntax/src/lexer.rs`** -- A thin wrapper around `logos::Lexer` that adds
   peek capability. The parser calls `peek()` and `next_token()` on this.

### Step 4: Understand the parser

Read `crates/graphcal-syntax/src/parser/`. The parser is split into modules:

- **`mod.rs`** -- Parser core: error types, helper methods, entry points (`parse_file()`,
  `parse_single_expr()`).
- **`expr.rs`** -- Expression parsing with precedence climbing.
- **`type_expr.rs`** -- Type expression parsing.
- **`fn_decl.rs`** -- Function declaration parsing.
- **`compound.rs`** -- Compound structures (blocks, if, match, for comprehensions).
- **`table.rs`** -- Table literal parsing.
- **`decl/`** -- Declaration parsing, split by kind:
  - `mod.rs` -- Declaration dispatcher and attribute parsing.
  - `value.rs` -- `param`/`node`/`const` declarations.
  - `type_decl.rs` -- `type` declarations (structs and tagged unions).
  - `dim_unit.rs` -- `dimension` and `unit` declarations.
  - `import.rs` -- `import` declarations (file paths, bare module paths, instantiated imports).
  - `index.rs` -- `cat`/`range` declarations (named and range).
  - `plot.rs` -- `plot` declarations.
  - `figure.rs` -- `figure` declarations.
  - `layer.rs` -- `layer` declarations.
  - `tests.rs` -- Parser tests.

The parser enforces **naming conventions** at parse time:

| Declaration | Required Casing |
| ------------- | ---------------- |
| `const` | `UPPER_SNAKE_CASE` |
| `param`, `node`, `fn` | `lower_snake_case` |
| `type`, `cat`, `range` | `PascalCase` |

Expression precedence (lowest to highest):

```
convert (->)
  ↓
if/else
  ↓
|| (or)
  ↓
&& (and)
  ↓
== != < > <= >= (comparison, non-chaining)
  ↓
+ - (additive)
  ↓
* / (multiplicative)
  ↓
- ! (unary)
  ↓
^ (power, right-associative)
  ↓
. [] (postfix: field access, index access)
  ↓
atoms (literals, identifiers, function calls, etc.)
```

### Step 5: Understand dimension algebra

Read **`crates/graphcal-syntax/src/dimension.rs`**. This is a standalone math module:

- `Rational` -- reduced rational numbers for dimension exponents (supports `sqrt` via `1/2`).
- `BaseDimId` -- identifies a base dimension: either `Prelude(name)` for the 8 built-in base
  dimensions, or `UserDefined(id)` for user-declared base dimensions.
- `Dimension` -- a `BTreeMap<BaseDimId, Rational>` mapping base dimensions to their exponents.
  Multiplication adds exponents, division subtracts them. Zero exponents are pruned.

This module is independent of all other syntax code and is used across all crates.

### Step 5b: Understand typed name wrappers

Read **`crates/graphcal-syntax/src/names.rs`**. This module defines newtype wrappers for
identifiers to prevent mixing up different kinds of names:

- `DeclName`, `DimName`, `UnitName`, `StructTypeName`, `IndexName`, `FnName`, `FieldName`,
  `VariantName`, `GenericParamName`.
- `Spanned<T>` wraps any name type with a source `Span`.

These types are used throughout all crates for type-safe name lookups.

### Step 6: Understand the type system registry

Read these files in `crates/graphcal-registry/src/`:

1. **`registry.rs`** -- The type system's lookup tables, split into **domain-specific
   registries** with a **builder/frozen pattern**:
   - `RegistryBuilder` -- mutable during construction (registers dimensions, units, etc.)
   - `Registry` -- frozen, read-only aggregate after `builder.build()`:
     - `DimensionRegistry` -- dimension names -> `Dimension`, base dim names/symbols
     - `UnitRegistry` -- unit names -> `UnitInfo` (dimension + scale)
     - `TypeRegistry` -- struct/union type definitions + variant reverse lookup
     - `FunctionRegistry` -- function definitions
     - `IndexRegistry` -- index definitions
2. **`builtins.rs`** -- Defines 30 built-in functions (`sqrt`, `cbrt`, `exp`, `expm1`, `ln`,
   `log10`, `log2`, `log`, `log1p`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`,
   `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `abs`, `floor`, `ceil`, `round`,
   `trunc`, `sign`, `min`, `max`, `hypot`, `clamp`) and 6 constants (`PI`, `E`, `TAU`,
   `SQRT2`, `LN2`, `LN10`).
   Each built-in has a `DimSignature` (a composable struct of per-parameter constraints
   and result rule) that describes its dimensional behavior.
3. **`prelude.rs`** -- Registers the standard library: 8 base dimensions, 9 derived dimensions
   (Velocity, Acceleration, Force, Energy, Power, Frequency, Pressure, Area, Volume), and 25+
   units with their scale factors.
4. **`error.rs`** -- All semantic errors are variants of `GraphcalError`, which implements
   `miette::Diagnostic`. Error codes follow the pattern `graphcal::{PREFIX}{NUMBER}`.
5. **`manifest.rs`** -- Parses `graphcal.toml` project manifests (source directories, etc.).
6. **`time_scale.rs`** -- Time scale definitions (UTC, TAI, TT, etc.) for DateTime support.
7. **`runtime_value.rs`** -- `RuntimeValue` enum used during evaluation.
8. **`declared_type.rs`** -- `DeclaredType` enum for resolved type annotations.
9. **`resolve_types.rs`** -- `ResolvedFile` and scoped name structures.

### Step 7: Understand name resolution

Read `crates/graphcal-ir/src/resolve/`. This is the first semantic pass after parsing:

- **`mod.rs`** -- Main resolver: `resolve()` and `resolve_with_imports()`.
  - Separates `const`, `param`, `node`, `assert`, `plot`, `figure`, `layer` declarations.
  - Validates all references: `@name` must refer to a known param/node; bare `NAME` in const
    context must refer to a known const.
  - Enforces `@`-prohibition rules: consts and function bodies must not use `@`.
- **`deps.rs`** -- Dependency extraction from expressions: builds dependency maps recording
  which other declarations each declaration references.
- **`names.rs`** -- Name validation utilities.
- **`scope.rs`** -- Scope validation (prohibit `@` in certain contexts).

### Step 8: Understand the IR lowering pass

Read **`crates/graphcal-ir/src/ir.rs`**. This module combines name resolution and registry
construction into a single lowering step:

- `lower()` takes an AST `File` and produces an `IR` with entry types for each declaration kind
  (`ConstEntry`, `ParamEntry`, `NodeEntry`, `AssertEntry`, `PlotEntry`, etc.).
- The IR holds resolved declarations, dependency maps, and source order.

### Step 8b: Understand function checking

Read **`crates/graphcal-ir/src/fn_check.rs`**. Validates function declarations:

- Detects recursive function calls via call graph analysis.
- Validates function arity at call sites.

### Step 9: Understand the TIR (Typed IR)

Read **`crates/graphcal-tir/src/tir.rs`**. The TIR resolves all type annotations from their
AST form to semantic types:

- `type_resolve()` takes an `IR` and produces a `TIR`.
- `ResolvedTypeExpr` variants: `Dimensionless`, `Bool`, `Int`, `Scalar(Dimension)`, `Struct`,
  `GenericStruct`, `GenericDimParam`, `GenericDimExpr`, `Indexed`, `DateTime`.
- `ResolvedFnSig` -- fully-resolved function signature with generic params, param types, and
  return type.
- Handles generic dimension parameters, index parameters, and type parameters.

### Step 10: Understand dimension checking

Read `crates/graphcal-tir/src/dim_check/`. This is the type checker, split into modules:

- **`mod.rs`** -- Main entry: `check_dimensions_tir()` and `check_override_dimension()`.
- **`infer/mod.rs`** -- Main inference dispatcher: `infer_type()`.
- **`infer/scalar.rs`** -- Scalar operations, unit conversions, casts.
- **`infer/collections.rs`** -- Indexed values, struct construction, maps, tables.
- **`infer/control.rs`** -- If/block/match type inference.
- **`infer/functions.rs`** -- Function call type inference with generic unification.
- **`builtins.rs`** -- Built-in function type signatures for dimension checking.
- **`helpers.rs`** -- Type matching, formatting, conversions.

For each declaration, the checker **infers** the dimension of the right-hand side expression
and **compares** it against the declared type annotation. Handles generic functions via
**unification**: when you call `lerp(@a, @b, 0.5)` and `lerp` has signature
`<D: Dim>(D, D, Dimensionless) -> D`, it infers `D` from the actual arguments.

### Step 11: Understand compilation and evaluation

Read these files in `crates/graphcal-eval/src/`:

1. **`exec_plan.rs`** -- Compiles a TIR into an execution plan: topological sort of const
   declarations + compile-time evaluation, then topological sort of runtime declarations
   (params + nodes) into evaluation order. Handles multi-file projects.
2. **`eval_expr/`** -- The recursive expression evaluator, split into modules:
   - `mod.rs` -- Main dispatcher: `eval_expr()` with `EvalContext`.
   - `arithmetic.rs` -- Binary/unary arithmetic operations.
   - `collections.rs` -- Maps, tables, for-comprehensions, index access, scan, unfold.
   - `control.rs` -- If/block/match evaluation.
   - `functions.rs` -- Built-in and user function calls.
3. **`eval/`** -- The **orchestrator**:
   - `mod.rs` -- Public API functions:
     - `compile_and_eval()` -- simple single-file.
     - `compile_and_eval_named()` -- with custom source name.
     - `compile_and_eval_with_overrides()` -- with parameter overrides.
   - `project.rs` -- Multi-file orchestration:
     - `compile_and_eval_project()` -- disk-based, multi-file, full pipeline.
     - `compile_and_eval_from_project()` -- from pre-loaded project.
     - `compile_to_tir_project()` -- disk-based, stops at TIR (for LSP).
     - `compile_to_tir_from_project()` -- from pre-loaded project, stops at TIR.
     - `compile_to_tir()` -- single-file, stops at TIR.
   - `runtime.rs` -- Runtime evaluation: topological sort, const eval, runtime eval.
   - `types.rs` -- Public types: `EvalResult`, `CompileError`, `Value`, `PlotSpec`,
     `FigureSpec`, `LayerSpec`, `AxisMeta`, etc.
   - `display.rs` -- Value display formatting with units.
4. **`loader.rs`** -- Multi-file DFS loader with cycle detection. `LoadedProject` can mix
   disk files and in-memory overrides (for the LSP). Uses `graphcal-io` filesystem abstraction.

### Step 12: Understand error handling

Read **`crates/graphcal-registry/src/error.rs`**. All semantic errors are variants of
`GraphcalError`, which implements `miette::Diagnostic`. Error codes follow the pattern
`graphcal::{PREFIX}{NUMBER}`:

| Prefix | Domain | Examples |
| -------- | -------- | --------- |
| N | Name resolution | N001 duplicate, N002 unknown graph ref, N003 unknown const, N004 unknown fn, N005 @-in-const, N006 casing |
| F | Functions | F001 @-in-fn, F002 recursive fn |
| G | Graph structure | G001 cycle |
| E | Evaluation | E001 runtime error |
| X | Expected failures | X001 expected failure |
| D | Dimensions | D001 mismatch, D002-D007 various dimension errors |
| S | Structs | S001-S008 struct-related errors |
| I | Indexes | I001-I007 index-related errors |
| M | Multi-file | M000-M015 import/module errors |
| A | Assertions | A002-A012 assertion errors |
| O | Overrides | O001-O004 override errors |
| C | Constraints | C001-C004 domain constraint errors |

---

## 6. Crate: graphcal-syntax

**Location**: `crates/graphcal-syntax/src/`

### File-by-File Reference

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations. All modules are `pub`. |
| `span.rs` | `Span` struct (offset + length). Converts to `miette::SourceSpan`. |
| `token.rs` | `Token` enum with `logos` derive. Keywords, operators, delimiters, literals. |
| `lexer.rs` | `Lexer` wrapper: peekable token stream. Yields `(Token, Span)` pairs. |
| `ast.rs` | Full AST definition. 13 declaration kinds, 30+ expression kinds, type/unit/dimension expressions. |
| `names.rs` | Type-safe newtype wrappers for identifiers (`DeclName`, `DimName`, `UnitName`, etc.) and `Spanned<T>`. |
| `comments.rs` | Comment extraction and source metadata for the formatter. `SourceMetadata`, `Comment`, `CommentKind`. |
| `dimension.rs` | `Rational`, `BaseDimId`, `Dimension`. Pure math, no dependencies on other syntax modules. |
| `visitor.rs` | AST visitor pattern for traversal and mutation. |
| `parser/mod.rs` | Parser core: error types, helpers, `parse_file()`, `parse_single_expr()`. |
| `parser/expr.rs` | Expression parsing with precedence climbing. |
| `parser/type_expr.rs` | Type expression parsing. |
| `parser/fn_decl.rs` | Function declaration parsing. |
| `parser/compound.rs` | Compound structures: blocks, if, match, for comprehensions. |
| `parser/table.rs` | Table literal parsing. |
| `parser/decl/mod.rs` | Declaration dispatcher and attribute parsing. |
| `parser/decl/value.rs` | `param`/`node`/`const` declarations. |
| `parser/decl/type_decl.rs` | `type` declarations (structs and tagged unions). |
| `parser/decl/dim_unit.rs` | `dimension` and `unit` declarations. |
| `parser/decl/import.rs` | `import` declarations. |
| `parser/decl/index.rs` | `cat`/`range` declarations. |
| `parser/decl/plot.rs` | `plot` declarations. |
| `parser/decl/figure.rs` | `figure` declarations. |
| `parser/decl/layer.rs` | `layer` declarations. |
| `parser/decl/tests.rs` | Parser tests. |

### Key Design Decisions

1. **Logos for lexing**: The `logos` crate generates a fast, zero-allocation lexer from regex
   patterns on the `Token` enum. Whitespace and comments are automatically skipped.

2. **Lexer as crate-internal**: Users of the crate interact through the parser, not the lexer
   directly. The `Lexer` type is `pub(crate)`.

3. **Parser split into modules**: The parser is organized by concern -- declarations, expressions,
   types, compound structures, and tables -- rather than being a single large file.

4. **Casing enforced at parse time**: The parser validates naming conventions immediately,
   producing clear errors with source spans.

5. **Rational exponents**: Using exact rationals instead of floats for dimension exponents
   avoids floating-point comparison issues. `sqrt(Length)` produces `Length^(1/2)` exactly.

6. **Dynamic dimension model**: Dimensions use `BTreeMap<BaseDimId, Rational>` rather than a
   fixed-size array. This supports user-defined base dimensions (`dimension Currency;`)
   alongside the 8 prelude base dimensions.

7. **Span on every node**: Every AST node carries a `Span`, enabling precise error reporting
   throughout the pipeline.

8. **Type-safe names**: The `names.rs` module uses newtype wrappers to prevent accidentally
   passing a `DimName` where a `UnitName` is expected.

9. **AST visitor**: The `visitor.rs` module provides a visitor pattern for traversing and
   mutating the AST without manual recursion.

---

## 7. Crate: graphcal-registry

**Location**: `crates/graphcal-registry/src/`

### graphcal-registry Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations. |
| `registry.rs` | Central type registry with builder/frozen pattern. Domain-specific sub-registries: `DimensionRegistry`, `UnitRegistry`, `TypeRegistry`, `FunctionRegistry`, `IndexRegistry`. |
| `builtins.rs` | 30 built-in functions and 6 constants. `DimSignature` for dimensional behavior. |
| `prelude.rs` | Standard library: 8 base dimensions, 9 derived dimensions, 25+ units. |
| `error.rs` | `GraphcalError` enum with miette diagnostic support. 60+ error variants across 10+ code families. |
| `manifest.rs` | `graphcal.toml` project manifest parsing (source directories, etc.). |
| `time_scale.rs` | Time scale definitions (UTC, TAI, TT, etc.) for DateTime. |
| `runtime_value.rs` | `RuntimeValue` enum: Scalar, Struct, Indexed, VariantLabel, DateTime, etc. |
| `declared_type.rs` | `DeclaredType` enum for resolved type annotations. |
| `resolve_types.rs` | `ResolvedFile` and scoped name structures. |
| `format.rs` | Formatting utilities for types and values. |

### graphcal-registry Design Decisions

1. **Separate crate for shared types**: The registry, error types, and builtins were extracted
   from `graphcal-eval` into their own crate so that `graphcal-ir` and `graphcal-tir` can
   depend on them without depending on the full eval crate. This prevents circular dependencies.

2. **Builder/frozen pattern**: `RegistryBuilder` is mutable during construction, then frozen
   into an immutable `Registry`. This ensures the type system is read-only during later phases.

3. **Composable dimension signatures**: Built-in functions describe their dimensional behavior
   via `DimSignature` with `ParamDim` (Fixed, Bind, Ref) and `ResultDim` (Fixed, Var, VarPow)
   constructors. This allows the dimension checker to handle any built-in generically.

4. **Centralized error types**: All `GraphcalError` variants live in one place with miette
   annotations, making it easy to maintain consistent diagnostics.

---

## 8. Crate: graphcal-ir

**Location**: `crates/graphcal-ir/src/`

### graphcal-ir Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations. |
| `ir.rs` | IR data structures: `IR`, entry types (`ConstEntry`, `ParamEntry`, `NodeEntry`, `AssertEntry`, `PlotEntry`, etc.). `lower()` function. |
| `fn_check.rs` | Function signature validation, arity checking, recursion detection via call graph. |
| `resolve/mod.rs` | Main resolver: `resolve()`, `resolve_with_imports()`. Validates references, enforces casing. |
| `resolve/deps.rs` | Dependency extraction from expressions. |
| `resolve/names.rs` | Name validation utilities. |
| `resolve/scope.rs` | Scope validation (prohibit `@` in certain contexts). |
| `resolve/tests.rs` | Resolution tests. |

### graphcal-ir Design Decisions

1. **Separate crate from eval**: Name resolution and IR lowering are independent of evaluation,
   so they live in their own crate. This enables `graphcal-tir` to depend on the IR without
   pulling in the evaluator.

2. **Entry types per declaration kind**: Each declaration kind (`const`, `param`, `node`, etc.)
   has its own entry type in the IR, carrying the resolved information specific to that kind.

---

## 9. Crate: graphcal-tir

**Location**: `crates/graphcal-tir/src/`

### graphcal-tir Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations. |
| `tir.rs` | TIR data structures: `TIR`, `ResolvedTypeExpr`, `ResolvedFnSig`. `type_resolve()` function. |
| `dim_check/mod.rs` | Main dimension checker: `check_dimensions_tir()`, `check_override_dimension()`. |
| `dim_check/helpers.rs` | Type matching, formatting, conversions. |
| `dim_check/builtins.rs` | Built-in function type signatures for dimension checking. |
| `dim_check/infer/mod.rs` | Main inference dispatcher: `infer_type()`, `infer_type_with_owner()`. |
| `dim_check/infer/scalar.rs` | Scalar operations, unit conversions, casts. |
| `dim_check/infer/collections.rs` | Indexed values, struct construction, maps, tables. |
| `dim_check/infer/control.rs` | If/block/match type inference. |
| `dim_check/infer/functions.rs` | Function call type inference with generic unification. |
| `dim_check/tests.rs` | Dimension checking tests. |

### graphcal-tir Design Decisions

1. **Separate crate from eval**: Type checking is independent of evaluation and can stop
   at the TIR level (used by the LSP).

2. **Modular inference**: The dimension inference code is split by expression category
   (scalar, collections, control flow, functions) rather than being a single large function.

---

## 10. Crate: graphcal-eval

**Location**: `crates/graphcal-eval/src/`

### graphcal-eval Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Re-exports from `graphcal-registry`, `graphcal-ir`, `graphcal-tir`. Own modules: `eval`, `eval_expr`, `exec_plan`, `io`, `loader`. |
| `exec_plan.rs` | Execution planning for multi-file projects: topological sort, const eval scheduling. |
| `loader.rs` | Multi-file DFS loader with cycle detection. `LoadedProject` with filesystem abstraction. |
| `io.rs` | Filesystem abstraction traits (deprecated in favor of `graphcal-io`). |
| `eval/mod.rs` | Public API: `compile_and_eval()`, `compile_and_eval_named()`, `compile_and_eval_with_overrides()`. |
| `eval/project.rs` | Multi-file orchestrator: `compile_and_eval_project()`, `compile_to_tir_project()`, etc. |
| `eval/runtime.rs` | Runtime evaluation engine: topological sort, const evaluation, runtime evaluation. |
| `eval/types.rs` | Public types: `EvalResult`, `CompileError`, `Value`, `PlotSpec`, `FigureSpec`, `LayerSpec`, `AxisMeta`, etc. |
| `eval/display.rs` | Value display formatting with units. |
| `eval/format.rs` | Number formatting utilities. |
| `eval/tests.rs` | Evaluation tests. |
| `eval_expr/mod.rs` | Main expression evaluator: `eval_expr()` dispatcher with `EvalContext`. |
| `eval_expr/arithmetic.rs` | Binary/unary arithmetic operations. |
| `eval_expr/collections.rs` | Maps, tables, for-comprehensions, index access, scan, unfold. |
| `eval_expr/control.rs` | If/block/match evaluation. |
| `eval_expr/functions.rs` | Built-in and user function calls. |

### graphcal-eval Design Decisions

1. **Re-export facade**: `graphcal-eval` re-exports all public modules from `graphcal-registry`,
   `graphcal-ir`, and `graphcal-tir`. Downstream crates (CLI, LSP) can depend on just
   `graphcal-eval` to get the full API.

2. **Pipeline decoupled from I/O**: `compile_and_eval_project()` reads from disk via
   `FileSystemReader`, while `compile_and_eval_named()` works with in-memory strings.
   `LoadedProject` can mix disk files with in-memory overrides.

3. **Two evaluation phases**: Constants are evaluated at "compile time" (in `exec_plan.rs`), and
   params/nodes are evaluated at "runtime" (in DAG topological order). This separation is clean
   because `@` is prohibited in constants.

4. **Modular expression evaluation**: The evaluator is split by expression category, mirroring
   the dimension checker's structure.

5. **Display unit extraction**: When a node uses unit conversion (`->`) or a unit literal, the
   display unit is extracted for pretty-printing. The internal value is always in SI base units.

### Important Public Types

| Type | Location | Purpose |
| ------ | ---------- | --------- |
| `EvalResult` | eval/types.rs | Final output: consts, params, nodes, plots, figures with display metadata |
| `CompileError` | eval/types.rs | Error wrapper for all pipeline stages |
| `Value` | eval/types.rs | Display-friendly value with SI value, display value, and unit label |
| `PlotSpec` | eval/types.rs | Vega-Lite plot specification |
| `FigureSpec` | eval/types.rs | Multi-plot figure specification |
| `LayerSpec` | eval/types.rs | Overlaid plot specification |
| `LoadedProject` | loader.rs | Collection of loaded/parsed files (disk or in-memory) |

---

## 11. Crate: graphcal-io

**Location**: `crates/graphcal-io/src/`

### graphcal-io Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations and `FileSystemReader` trait. |
| `real_fs.rs` | `RealFileSystem` implementation using `std::fs`. |
| `overlay_fs.rs` | `OverlayFileSystem`: combines in-memory overrides with disk reads. Used by the LSP for unsaved buffers. |

---

## 12. Crate: graphcal-fmt

**Location**: `crates/graphcal-fmt/src/`

### graphcal-fmt Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Public API: `format_source(source: &str) -> Option<String>`. |
| `format/mod.rs` | `Formatter` struct and top-level format functions. |
| `format/decl.rs` | Declaration formatting for all declaration types. |
| `format/expr.rs` | Expression formatting. |
| `format/type_expr.rs` | Type expression formatting. |

### graphcal-fmt Architecture

The formatter is **AST-based**: it parses the source, formats the AST using the `pretty` crate
for layout, and produces formatted output. It returns `None` if parsing fails (unparsable files
are not formatted).

Key features:

- **Comment preservation**: Uses `SourceMetadata` from `graphcal-syntax/src/comments.rs` to
  extract and reattach comments to their corresponding AST nodes.
- **Blank line preservation**: Blank lines between declarations are preserved.
- **Pretty-printing**: Uses the `pretty` crate's optimal layout algorithm with a default line
  width of 100 characters.
- **Number formatting**: Preserves original number formatting from the source text.

---

## 13. Crate: graphcal-dag

**Location**: `crates/graphcal-dag/src/`

### graphcal-dag Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Public `render()` function and algorithm overview. |
| `layout.rs` | Sugiyama layered layout (layer assignment, crossing reduction). |
| `canvas.rs` | 2D character canvas for drawing. |
| `render.rs` | Node/edge rendering with Unicode box-drawing characters. |

### graphcal-dag Architecture

Renders a `petgraph` DAG as ASCII art using the Sugiyama algorithm for layered graph layout.
Used for dependency visualization in the CLI's `shell` subcommand (`:graph` REPL command).

---

## 14. Crate: graphcal-cli

**Location**: `crates/graphcal-cli/src/`

### graphcal-cli Files

| File | Purpose |
| ------ | --------- |
| `main.rs` | CLI entry point: clap args, subcommands, output formatting. |
| `json_input.rs` | Converts JSON input files to parameter overrides. Supports scalars, structs, tagged unions, indexed params. |
| `plot.rs` | Vega-Lite plot rendering: generates HTML, opens in browser or writes to file. |
| `shell/mod.rs` | Interactive REPL implementation with `rustyline`. |
| `shell/commands.rs` | REPL commands (`:help`, `:graph`, `:set`, etc.). |
| `shell/highlight.rs` | Syntax highlighting for REPL input. |
| `shell/format.rs` | Output formatting for REPL. |
| `shell/graph.rs` | Dependency graph visualization in REPL (uses `graphcal-dag`). |

### CLI Structure

```sh
graphcal eval <FILE> [--format text|json] [--set 'name=expr'] [--input input.json] [--no-assert] [--plot browser|json|path] [--root DIR] [--allow-defaults]
graphcal format [--check] <PATHS>...
graphcal typecheck <PATHS>... [--root DIR]
graphcal lsp
graphcal shell [FILE] [--set 'name=expr'] [--input input.json] [--root DIR]
```

Five subcommands:

1. **`eval`** -- Evaluate a `.gcl` file. Supports `--set` for inline overrides, `--input` for
   JSON parameter files, `--format` for text/JSON output, `--plot` for Vega-Lite plot output,
   `--no-assert` to skip assertions, `--root` to specify project root, and `--allow-defaults`
   to allow params with defaults when using overrides.
2. **`format`** -- Format `.gcl` files using `graphcal-fmt`. With `--check`, reports unformatted
   files without modifying them.
3. **`typecheck`** -- Check files for errors without evaluation (stops at TIR).
4. **`lsp`** -- Start the Language Server Protocol server.
5. **`shell`** -- Start an interactive REPL with syntax highlighting and graph visualization.

### Output Formats

**Text output** (default): Aligned columns with unit display.

```text
dry_mass    = 1200 kg
fuel_mass   = 2800 kg
G0          = 9.80665 m/s^2
v_exhaust   = 3138.128 m/s
mass_ratio  = 3.333333
delta_v     = 3778.691 m/s
```

**JSON output**: Structured with `si_value`, optional `display_value` and `unit`, and nested
struct/indexed values.

### JSON Input

The `--input` flag accepts a JSON file to set parameter values. The `json_input.rs` module
converts JSON values to GCL expressions:

- Strings are parsed as GCL expressions (e.g., `"1200 kg"`)
- Numbers become numeric literals
- Booleans become boolean literals
- Objects with `"variant"` key become tagged union values
- Objects with named keys become indexed param values

---

## 15. Crate: graphcal-lsp

**Location**: `crates/graphcal-lsp/src/`

### graphcal-lsp Files

| File | Purpose |
| ------ | --------- |
| `lib.rs` | Module declarations. Public API: `async fn run()`. |
| `server.rs` | Main LSP server: file tracking, incremental analysis, request dispatch. |
| `symbol_table.rs` | Symbol resolution and scope analysis from TIR. |
| `cursor_context.rs` | Determines what entity is under the cursor (for hover, go-to-def, etc.). |
| `diagnostics.rs` | Converts `GraphcalError` to LSP diagnostics with related information. |
| `rename.rs` | Rename refactoring across declarations and references. |
| `completion.rs` | Code completion for declarations, fields, variants, built-ins. |
| `convert.rs` | Conversions between internal types and LSP protocol types. |
| `hover.rs` | Hover information: type, dimension, documentation. |
| `document_symbols.rs` | Document outline (symbols for breadcrumbs/outline view). |
| `references.rs` | Find all references to a symbol. |
| `goto_definition.rs` | Jump to definition of a symbol. |
| `inlay_hints.rs` | Inline type hints showing computed values. |
| `formatting.rs` | Document formatting using `graphcal-fmt`. |
| `signature_help.rs` | Function signature help while typing arguments. |
| `document_links.rs` | Clickable links for `import` declaration paths. |
| `resolve.rs` | Symbol resolution helpers. |

### graphcal-lsp Architecture

The LSP server uses `tower-lsp` on a `tokio` async runtime:

1. **File tracking**: On `did_open` / `did_change`, the server stores the current buffer content
   in memory.
2. **Incremental analysis**: `run_analysis()` in `server.rs` compiles the in-memory source using
   `compile_and_eval_named()` or `compile_to_tir_project()`. It handles four cases based on
   whether TIR compilation and in-memory parsing succeed or fail.
3. **Symbol table**: Built from the TIR, mapping source spans to declarations, references, and
   types.
4. **Pull-based features**: Inlay hints and document symbols are pull-based (the editor requests
   them). After updating analysis, the server sends `client.inlay_hint_refresh()` to trigger
   re-fetching.
5. **Push-based features**: Diagnostics are push-based via `publish_diagnostics`.

### LSP Features

| Feature | Status |
| --------- | -------- |
| Document Symbols | Implemented |
| Go to Definition | Implemented |
| Hover | Implemented |
| Find References | Implemented |
| Rename | Implemented |
| Diagnostics | Implemented (with related information) |
| Inlay Hints | Implemented (with computed values) |
| Completion | Implemented |
| Signature Help | Implemented |
| Formatting | Implemented (via graphcal-fmt) |
| Document Links | Implemented |
| Semantic Tokens | Not implemented |

---

## 16. Testing Infrastructure

### Test Categories

#### 1. Unit Tests (inline)

Located in `#[cfg(test)]` modules within source files:

- **`dimension.rs`**: Property-based tests (proptest) verifying algebraic laws of `Rational` and
  `Dimension`.
- **`parser/decl/tests.rs`**: Tests for parsing all declaration types, expression precedence,
  casing validation, and error cases.
- **`builtins.rs`**: Tests for built-in function evaluation.
- **`prelude.rs`**: Tests for prelude dimension/unit correctness.
- **`resolve/tests.rs`**: Name resolution tests.
- **`dim_check/tests.rs`**: Dimension checking tests.
- **`eval/tests.rs`**: Evaluation tests.

#### 2. Snapshot Tests (insta)

Located across multiple crates with snapshots in `snapshots/` subdirectories:

- **Error snapshots** (`graphcal-eval/tests/error_snapshots.rs`): 60 snapshot tests that compile
  `.gcl` files from `tests/fixtures/errors/`, render the error using
  `miette::NarratableReportHandler`, and compare against stored `.snap` files.
- **Formatter snapshots** (`graphcal-fmt/tests/format_tests.rs`): 68 snapshot tests verifying
  formatter output.
- **DAG snapshots** (`graphcal-dag/src/snapshots/`): 6 snapshot tests for ASCII DAG rendering.
- **REPL snapshots** (`graphcal-cli/src/shell/snapshots/`): 4 snapshot tests for shell output.

**To update snapshots** after intentional changes: `cargo insta review`.

#### 3. Integration Tests (CLI)

Located in `crates/graphcal-cli/tests/cli.rs`. These run the compiled `graphcal` binary as
a subprocess and check:

- Correct text and JSON output for various fixtures.
- Error handling (missing files, syntax errors, semantic errors).
- `--set` flag functionality (single, multiple, error cases).
- Multi-file import success and error cases.

#### 4. Property-Based Tests (proptest)

Used in `dimension.rs` to verify algebraic properties:

- `Rational` reduction, arithmetic identities, associativity.
- `Dimension` multiplication/division laws, dimensionless identity.

#### 5. Regression Tests

Located in `crates/graphcal-eval/tests/edge_case_bugs.rs`. These tests cover specific bugs
that have been found and fixed, ensuring they do not regress.

#### 6. Test Fixtures

Located in `tests/fixtures/`:

```
tests/fixtures/
├── rocket.gcl              # Tsiolkovsky rocket equation
├── hohmann.gcl             # Hohmann transfer with structs
├── functions.gcl           # Pure functions with generics
├── generics.gcl            # Generic functions and types
├── indexed.gcl             # Indexed values and aggregation
├── range_index.gcl         # Range-based indexes
├── tagged_union.gcl        # Tagged union types
├── tagged_union_param.gcl  # Tagged union as parameters
├── variant_match.gcl       # Pattern matching on variants
├── variant_comparison.gcl  # Variant comparisons
├── assertions.gcl          # Assert declarations
├── assertions_assumes.gcl  # Assert with assumes
├── assertions_indexed.gcl  # Indexed assertions
├── assertions_fail.gcl     # Expected assertion failures
├── assertions_tolerance_fail.gcl  # Tolerance assertion failures
├── integers.gcl            # Integer type support
├── time_scan.gcl           # Time-based scan (system dynamics)
├── user_dimensions.gcl     # User-defined dimensions
├── orbital.gcl             # Simple orbital velocity
├── constants.gcl           # Constants-only file
├── table_literal.gcl       # Table literal syntax
├── power_budget.gcl        # Power budget example
├── thermal_analysis.gcl    # Thermal analysis example
├── datetime_*.gcl          # DateTime support (6 files)
├── domain_*.gcl            # Domain constraints (2 files)
├── plot_*.gcl              # Vega-Lite plot examples (5 files)
├── figure_*.gcl            # Figure declarations (2 files)
├── layer_basic.gcl         # Layer declarations
├── expected_fail_*.gcl     # Expected failure tests (7 files)
├── comments_in_expressions.gcl  # Comment preservation
├── format_edge_cases.gcl   # Formatter edge cases
├── parenthesized_exprs.gcl # Parenthesized expressions
├── input_*.json            # JSON input files (4 files)
├── multi/                  # Multi-file test projects (30+ directories)
│   ├── rocket_split/       # Basic multi-file import
│   ├── alias/              # Import aliases
│   ├── alias_conflict/     # Import alias conflicts
│   ├── assertions/         # Multi-file assertions
│   ├── auto_assert*/       # Auto-assert features
│   ├── bare_import_*/      # Bare module path imports (7 dirs)
│   ├── diamond_assert/     # Diamond import assertions
│   ├── explicit_index/     # Explicit index imports
│   ├── imported_deps/      # Imported dependencies
│   ├── instantiated_import*/ # Instantiated imports (7 dirs)
│   ├── mission_plan/       # Mission planning example
│   ├── module_import*/     # Module imports (5 dirs)
│   ├── parent_import*/     # Parent directory imports (2 dirs)
│   ├── required_param_import/ # Required parameter imports
│   └── ...
└── errors/                 # 60 files designed to trigger specific errors
    ├── duplicate.gcl
    ├── unknown_ref.gcl
    ├── dim_mismatch_add.gcl
    ├── cycle.gcl
    ├── non_exhaustive_match.gcl
    ├── assert_not_bool.gcl
    ├── datetime_*.gcl      # DateTime error cases
    ├── domain_*.gcl        # Domain constraint errors
    └── ...
```

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run with snapshot review
cargo insta test --workspace
cargo insta review         # Interactive review of changed snapshots

# Run specific test
cargo test -p graphcal-eval error_duplicate_name

# Run CLI integration tests only
cargo test -p graphcal-cli
```

---

## 17. CI/CD and Developer Tooling

### CI Pipeline (`.github/workflows/ci.yaml`)

Five parallel jobs:

| Job | What it does |
| ----- | ------------- |
| `checks` | Clippy (all features + no default features), rustfmt, doc generation, cargo check |
| `test` | `cargo test --workspace` |
| `coverage` | `cargo-llvm-cov` -> lcov -> Codecov upload |
| `msrv` | Minimum Supported Rust Version check via `cargo-hack` |
| `typos` | Spell checking with `crate-ci/typos` |

A `collect` job aggregates results to provide a single status check.

There is also a `docs.yaml` workflow for building the Zensical documentation site.

### Justfile

```bash
just lint       # Run all lints (clippy with all features + no default features, fmt, doc, check)
just test       # Run all tests
just coverage   # Generate HTML coverage report
```

### Pre-commit Hooks (`.pre-commit-config.yaml`)

- Trailing whitespace, end-of-file fixer, TOML/YAML/JSON validation
- Large file check, merge conflict check, mixed line endings
- Symlink check, shebang check, illegal Windows names check
- Markdown linting (`markdownlint`)
- Spell checking (`typos`)
- No direct commits to `main`

### Clippy Configuration

The workspace uses **strict clippy settings** (defined in root `Cargo.toml`):

- All lint groups at `warn`: pedantic, nursery, perf, complexity, style, suspicious
- `correctness` at `deny`
- `unsafe_code` at `warn`
- Additional warns: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`,
  `unreachable`, `dbg_macro`, `print_stdout`, `print_stderr`
- Pragmatic allows: `indexing_slicing`, `missing_errors_doc`, `missing_panics_doc`,
  `multiple_crate_versions`, `derive_partial_eq_without_eq`

### Rust Toolchain

- Stable channel (`rust-toolchain.toml`)
- MSRV: 1.91+ (edition 2024)

---

## 18. Design Documents and Phased Development

### Design Document Structure

All design documents are in `design/`. The `design/README.md` is the index.

**22 aspect documents** cover orthogonal design dimensions:

| # | Document | Topic |
| --- | ---------- | ------- |
| 01 | computation-model | DAG of param/node/const, reactive evaluation |
| 02 | syntax-design | Keywords, expressions, statements |
| 03 | primitives | f64, i64, bool, Str, DateTime |
| 04 | dimensions-and-units | Dimensions as types, units as values |
| 05 | algebraic-data-types | Structs and tagged unions |
| 06 | spaces | Semantic context tags (e.g., coordinate frames) |
| 07 | indexes | Finite label sets for table axes |
| 08 | scoping | `@` sigil for graph scope vs. local scope |
| 09 | namespace | Modules, imports, visibility, prelude |
| 10 | tables-and-autofill | N-dimensional labeled tables |
| 11 | system-dynamics | Temporal simulation via scan pattern |
| 12 | pure-functions | `fn` keyword, purity via `@` prohibition |
| 13 | live-view | Auto-rendered visualization |
| 14 | spreadsheet-compat | Excel import/export |
| 15 | python-interop | PyO3 bindings |
| 16 | git-workflow | Scenarios and version control |
| 17 | error-messages | Error codes and diagnostics |
| 18 | non-si-dimensions | Non-SI unit systems |
| 19 | assertions-and-testing | Assert/assumes/test features |
| 20 | type-system-stratification | Type system layers (proposal) |
| 21 | separate-label-indexes-from-tagged-unions | Index/union separation |
| 22 | vega-lite-plotting | Vega-Lite integration for plots/figures/layers |

Additional design documents:

- **`graphcal-format-plan.md`** -- Design and implementation plan for the code formatter.
- **`primitive-types-analysis.md`** -- Analysis of primitive type choices.
- **`grammar.ebnf`** -- Formal grammar specification.
- **`IDEAS.md`** -- Feature ideas and brainstorming.

### Phased Implementation

The project follows incremental phases documented in `design/phases/`. Each phase produces a
working artifact with locked design decisions.

| Phase | Feature | Status |
| ------- | --------- | -------- |
| 0 | Scalar graph (param/node/const, f64 only) | Complete |
| 1 | Dimensions and units | Complete |
| 2 | Structs and multi-line nodes | Complete |
| 3 | Pure functions | Complete |
| 4 | Multi-file and namespaces | Complete |
| 5 | Indexed values | Complete |
| 6 | Scenarios and CLI (MVP) | Complete |
| 7+ | System dynamics, spaces, tagged unions, TUI, ... | In progress |

Post-MVP features that have been implemented beyond the original phase plan:

- Tagged unions and match expressions
- Range indexes and unfold
- DateTime support with time scales
- Domain constraints on parameters
- Vega-Lite plotting (plot, figure, layer declarations)
- Interactive REPL (shell subcommand)
- User-defined base dimensions
- Bare module path imports
- Instantiated imports with param bindings
- Project manifests (`graphcal.toml`)
- Integer type support
- Tuple match expressions

### Key Design Inspirations

| Inspiration | What was borrowed |
| ------------- | ------------------ |
| Numbat | Dimensions as types, units as values |
| Sguaba | Phantom-typed coordinate frames (spaces) |
| Gleam | Unified `type` keyword for structs/enums |
| marimo | Reactive DAG, pure text files |
| Salsa | Incremental computation, dirty tracking |
| Typst/comemo | Memoization with stable identity |

---

## 19. Extending Graphcal

### Adding a New Built-in Function

1. Add the function entry to `BUILTIN_FUNCTIONS` in `crates/graphcal-registry/src/builtins.rs`.
   Specify: name, eval closure (operating on `f64` values), and `DimSignature` (built using
   convenience constructors like `DimSignature::all_dimensionless`, `DimSignature::free_to_pow`,
   `DimSignature::same_dim`, etc.).
   The arity is derived from the number of parameters in the `DimSignature`.
2. No code changes are needed in `dim_check/` -- the generic `infer_fn_dim` interpreter
   handles any `DimSignature` automatically.
3. Add test fixtures in `tests/fixtures/` and snapshot tests in
   `crates/graphcal-eval/tests/error_snapshots.rs` for error cases.
4. Update LSP completion in `crates/graphcal-lsp/src/completion.rs` if the function needs
   special handling.

### Adding a New Declaration Kind

1. Add the new variant to `DeclKind` in `crates/graphcal-syntax/src/ast.rs`.
2. Add any new tokens to `crates/graphcal-syntax/src/token.rs`.
3. Add parsing logic in `crates/graphcal-syntax/src/parser/decl/` (create a new file or extend
   an existing one, and handle the new keyword in `decl/mod.rs`).
4. Handle the new declaration in `crates/graphcal-ir/src/resolve/` (name resolution).
5. Handle IR lowering in `crates/graphcal-ir/src/ir.rs`.
6. Handle type resolution in `crates/graphcal-tir/src/tir.rs` if it has type annotations.
7. Handle dimension checking in `crates/graphcal-tir/src/dim_check/` if applicable.
8. Handle compilation in `crates/graphcal-eval/src/exec_plan.rs` if it participates in the DAG.
9. Handle evaluation in `crates/graphcal-eval/src/eval/` and `eval_expr/`.
10. Update the CLI output formatting in `crates/graphcal-cli/src/main.rs` if the declaration
    produces output.
11. Update the formatter in `crates/graphcal-fmt/src/format/`.
12. Update LSP features (symbol table, document symbols, completion, etc.).
13. Update the tree-sitter grammar in `tree-sitter-graphcal/grammar.js`.
14. Update editor extensions (VS Code TextMate grammar, Zed highlights).
15. Add tests at every level.

### Adding a New Expression Kind

1. Add the variant to `ExprKind` in `ast.rs`.
2. Add parsing in `parser/expr.rs` or `parser/compound.rs` at the appropriate precedence level.
3. Add dimension inference in `dim_check/infer/` (in the appropriate sub-module).
4. Add evaluation in `eval_expr/` (in the appropriate sub-module).
5. Update the formatter in `format/expr.rs`.
6. Update the AST visitor in `visitor.rs`.
7. Add test fixtures and snapshot tests.

### Adding a New Unit or Dimension to the Prelude

1. Edit `crates/graphcal-registry/src/prelude.rs`.
2. Use `builder.register_dimension()` for new derived dimensions.
3. Use `builder.register_unit()` for new units, specifying the dimension and SI scale factor.
4. Use `builder.register_base_dimension_with_symbol()` for new base dimensions (rare).

### Adding a New Error

1. Add a variant to `GraphcalError` in `crates/graphcal-registry/src/error.rs`.
2. Add the `#[error(...)]`, `#[diagnostic(code(...))]`, and `#[label(...)]` attributes.
3. Emit the error at the appropriate point in the pipeline.
4. Create a `.gcl` fixture file in `tests/fixtures/errors/`.
5. Add a snapshot test in `crates/graphcal-eval/tests/error_snapshots.rs`.
6. Run `cargo insta review` to approve the new snapshot.
7. Ensure the LSP diagnostics module handles the new error properly.

### General Tips for Contributors

- **Read the design doc first**: Before implementing a feature, check if there's a design
  document in `design/` or a phase document in `design/phases/`.
- **Follow the pipeline order**: Changes typically flow from syntax -> IR -> TIR -> eval.
- **Update all layers**: New features need updates across the syntax crate, registry, IR, TIR,
  eval, formatter, LSP, tree-sitter grammar, and editor extensions.
- **Every error needs a test**: Add both the error fixture (`.gcl`) and a snapshot test.
- **Use `insta` for regression**: Snapshot tests catch unintended changes to error messages.
- **Run `just lint` and `just test`** before committing.
- **Check `.local/` for prior research**: Development notes may contain useful context on design
  decisions.
- **Crate boundaries matter**: The pipeline crates (`graphcal-syntax` -> `graphcal-registry` ->
  `graphcal-ir` -> `graphcal-tir` -> `graphcal-eval`) have strict dependency ordering.
  `graphcal-eval` re-exports everything, but internal crates cannot depend on crates above
  them in the stack.
