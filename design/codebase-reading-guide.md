# Graphcal Codebase Reading Guide

A comprehensive guide for understanding, maintaining, and extending the graphcal codebase.

## Table of Contents

1. [What is Graphcal?](#1-what-is-graphcal)
2. [Theoretical Foundations](#2-theoretical-foundations)
3. [High-Level Architecture](#3-high-level-architecture)
4. [The Compilation and Evaluation Pipeline](#4-the-compilation-and-evaluation-pipeline)
5. [Incremental Reading Order](#5-incremental-reading-order)
6. [Crate: graphcal-syntax](#6-crate-graphcal-syntax)
7. [Crate: graphcal-eval](#7-crate-graphcal-eval)
8. [Crate: graphcal-cli](#8-crate-graphcal-cli)
9. [Crate: graphcal-fmt](#9-crate-graphcal-fmt)
10. [Crate: graphcal-lsp](#10-crate-graphcal-lsp)
11. [Testing Infrastructure](#11-testing-infrastructure)
12. [CI/CD and Developer Tooling](#12-cicd-and-developer-tooling)
13. [Design Documents and Phased Development](#13-design-documents-and-phased-development)
14. [Extending Graphcal](#14-extending-graphcal)

---

## 1. What is Graphcal?

Graphcal is a **type-safe, unit-aware, Git-friendly reactive programming language** for
engineering calculations. It aims to replace spreadsheets and simulation tools (like Excel and
Vensim) with a typed, version-controlled computation model.

A `.gcl` file describes a **directed acyclic graph (DAG)** of computations where:

- **`param`** declarations are tunable inputs (can be overridden from the CLI).
- **`node`** declarations are computed values derived from params and other nodes.
- **`const`** declarations are compile-time constants.
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

Physical dimensions form a **vector space** over rational numbers. Each physical quantity has a
dimension that is a product of powers of 8 base dimensions:

```
Dimension = Length^a * Time^b * Mass^c * Temperature^d * ElectricCurrent^e * Amount^f * LuminousIntensity^g * Angle^h
```

where `a, b, c, ...` are rational numbers (to support `sqrt`, which halves exponents).

Dimension arithmetic:

| Operation | Dimension Rule |
| ----------- | --------------- |
| `a + b`, `a - b` | `dim(a) == dim(b)` (must match) |
| `a * b` | `dim(a) * dim(b)` (exponents add) |
| `a / b` | `dim(a) / dim(b)` (exponents subtract) |
| `a ^ n` | `dim(a) ^ n` (exponents multiply by n) |
| `sqrt(a)` | `dim(a) ^ (1/2)` |
| `sin(a)` | `dim(a)` must be Angle; result is Dimensionless |

This is implemented as an 8-element array of `Rational` values in `dimension.rs`.

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

Indexed values are like typed, finite-dimensional vectors. An `index` defines a finite label
set, and `T[I]` is a value of type `T` for each label in `I`. For example:

```gcl
index Maneuver = { Departure, Correction, Insertion }
param delta_v: Velocity[Maneuver] = { ... };
```

Operations on indexed values include `for` comprehensions (map), aggregations (`sum`, `max`,
`min`, `mean`, `count`), and `scan` (cumulative fold).

---

## 3. High-Level Architecture

The project is a **Cargo workspace** with five crates:

```
graphcal/
├── crates/
│   ├── graphcal-syntax/   # Lexer, parser, AST, dimension algebra, name types
│   ├── graphcal-eval/     # Name resolution, type checking, compilation, evaluation
│   ├── graphcal-fmt/      # AST-based code formatter
│   ├── graphcal-lsp/      # Language Server Protocol implementation
│   └── graphcal-cli/      # CLI binary (clap)
├── tree-sitter-graphcal/  # Tree-sitter grammar for syntax highlighting
├── editors/
│   ├── zed/               # Zed editor extension
│   └── vscode/            # VS Code extension
├── tests/fixtures/        # .gcl example files and error test cases
├── design/                # Language design documents
│   └── phases/            # Phased implementation plans
└── .local/                # Development notes (gitignored)
```

### Dependency Graph

```
graphcal-cli
  ├── graphcal-eval
  │     └── graphcal-syntax
  ├── graphcal-fmt
  │     └── graphcal-syntax
  └── graphcal-syntax

graphcal-lsp
  ├── graphcal-eval
  │     └── graphcal-syntax
  ├── graphcal-fmt
  │     └── graphcal-syntax
  └── graphcal-syntax
```

- **`graphcal-syntax`** has no in-workspace dependencies. It depends on `logos` (lexing),
  `miette` (diagnostics), and `thiserror`.
- **`graphcal-eval`** depends on `graphcal-syntax` and adds `petgraph` (DAG), `indexmap`
  (ordered maps), and `serde` (serialization).
- **`graphcal-fmt`** depends on `graphcal-syntax` and adds `pretty` (pretty-printing).
- **`graphcal-lsp`** depends on `graphcal-eval`, `graphcal-fmt`, and `graphcal-syntax`, and adds
  `tower-lsp` (LSP protocol) and `tokio` (async runtime).
- **`graphcal-cli`** depends on `graphcal-eval`, `graphcal-fmt`, and `graphcal-syntax`, and adds
  `clap` (argument parsing) and `serde_json` (JSON output).

### Layered Design

```
┌─────────────────────────────────┐
│     graphcal-cli / graphcal-lsp │  User-facing: CLI, editor integration
├─────────────────────────────────┤
│   graphcal-eval / graphcal-fmt  │  Semantic analysis & formatting:
│                                 │  IR, TIR, type checking, evaluation, formatting
├─────────────────────────────────┤
│       graphcal-syntax           │  Syntactic analysis: lexing, parsing, AST,
│                                 │  dimension algebra, name types
└─────────────────────────────────┘
```

---

## 4. The Compilation and Evaluation Pipeline

This is the core flow from source text to computed results. Understanding this pipeline is
essential for working on the codebase. The pipeline is orchestrated in
`graphcal-eval/src/eval.rs`.

The pipeline is structured as a series of **intermediate representations** (IRs), each adding
more semantic information:

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
   │  Lower   │  ir.rs: name resolution (resolve.rs) + registry
   └────┬─────┘  construction (registry.rs + prelude.rs)
        │  IR (resolved names + frozen Registry)
        ▼
   ┌────────────┐
   │ Type       │  tir.rs: resolve type annotations to semantic
   │ Resolve    │  types, resolve generics
   └────┬───────┘
        │  TIR (resolved types + function signatures)
        ▼
   ┌───────────┐
   │ Fn Check  │  fn_check.rs: detect recursive function calls
   └────┬──────┘
        │
        ▼
   ┌───────────┐
   │ Dim Check │  dim_check.rs: infer and validate dimensions for
   └────┬──────┘  all expressions; check type annotations match
        │
        ▼                      ← compile_to_tir_*() stops here (for LSP)
   ┌───────────┐
   │  Compile  │  exec_plan.rs: topological sort of consts + eval,
   └────┬──────┘  build runtime DAG (detect cycles)
        │  ExecPlan (const values + topo order)
        ▼
   ┌──────────┐
   │ Evaluate │  eval.rs + eval_expr.rs: walk DAG in topo order,
   └────┬─────┘  evaluate each expression recursively
        │  EvalResult (consts, params, nodes with display units)
        ▼
   ┌──────────┐
   │  Output  │  graphcal-cli: format as text or JSON
   └──────────┘
```

The pipeline has two important **stopping points**:

- **`compile_to_tir_*()`** stops after dimension checking and returns the TIR. This is used by
  the LSP server, which needs type information but does not need to evaluate.
- **`compile_and_eval_*()`** runs the full pipeline through evaluation.

For **multi-file projects**, the pipeline begins with `loader.rs` which performs a DFS over
`use` declarations to load and parse all files in dependency order. Then the pipeline runs for
each file, with imported declarations propagated via `ImportedNames`.

The pipeline is also **decoupled from I/O**: `compile_and_eval_project()` reads from disk, while
`compile_and_eval_named()` accepts an in-memory source string. This separation is critical for
the LSP server, which works with unsaved editor buffers.

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
4. **`tests/fixtures/indexed.gcl`** -- Adds `index`, indexed params (map literals), `for`
   comprehensions, aggregations (`sum`, `max`, `min`, `mean`, `count`), `scan`, and index
   generics (`<I: Index>`).
5. **`tests/fixtures/tagged_union.gcl`** -- Adds tagged union types and `match` expressions.
6. **`tests/fixtures/assertions.gcl`** -- Shows `assert` declarations for self-checking
   calculations.
7. **`tests/fixtures/multi/rocket_split/`** -- Shows multi-file imports with `use`.

### Step 2: Understand the AST (what the parser produces)

Read `crates/graphcal-syntax/src/ast.rs`. This defines every node in the abstract syntax tree.
The key types are:

- `File` → `Vec<Declaration>` → `DeclKind` (the top-level structure)
- `Expr` → `ExprKind` (the recursive expression tree)
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

Read **`crates/graphcal-syntax/src/parser.rs`**. This is the largest file in the syntax crate
(~4400 lines). It is a **recursive descent parser** with precedence climbing for expressions.

Key entry points:

- `parse_file()` -- parses a complete `.gcl` file into a `File` AST.
- `parse_single_expr()` -- parses a standalone expression (used by `--set` flag).

The parser enforces **naming conventions** at parse time:

| Declaration | Required Casing |
| ------------- | ---------------- |
| `const` | `UPPER_SNAKE_CASE` |
| `param`, `node`, `fn` | `lower_snake_case` |
| `type`, `index` | `PascalCase` |

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
- `BaseDim` -- the 8 base dimensions (Length, Time, Mass, ...).
- `Dimension` -- an 8-element array of `Rational` values representing a physical dimension.
  Multiplication adds exponents, division subtracts them.

This module is independent of all other syntax code and is used by the eval crate.

### Step 5b: Understand typed name wrappers

Read **`crates/graphcal-syntax/src/names.rs`**. This module defines newtype wrappers for
identifiers to prevent mixing up different kinds of names:

- `DeclName`, `DimName`, `UnitName`, `StructTypeName`, `IndexName`, `FnName`, `FieldName`,
  `VariantName`, `GenericParamName`.
- `Spanned<T>` wraps any name type with a source `Span`.

These types are used throughout the eval crate for type-safe name lookups.

### Step 6: Understand name resolution

Read **`crates/graphcal-eval/src/resolve.rs`**. This is the first semantic pass after parsing:

- Separates `const`, `param`, `node`, and `assert` declarations.
- Validates all references: `@name` must refer to a known param/node; bare `NAME` in const
  context must refer to a known const.
- Builds **dependency maps**: for each declaration, which other declarations does it reference?
- Enforces `@`-prohibition rules: consts and function bodies must not use `@`.
- The output is `ResolvedFile` with separated declaration lists and dependency maps.

### Step 7: Understand the IR lowering pass

Read **`crates/graphcal-eval/src/ir.rs`**. This module combines name resolution and registry
construction into a single lowering step:

- `lower_to_builder()` takes an AST `File` and returns a `(RegistryBuilder, UnfrozenIR)`.
- The `UnfrozenIR` holds resolved declarations, dependency maps, and source order.
- After imports are registered, call `UnfrozenIR::freeze(builder.build())` to produce the
  final `IR` with a frozen, immutable `Registry`.

### Step 8: Understand the type system registry

Read these files in order:

1. **`crates/graphcal-eval/src/builtins.rs`** -- Defines 14 built-in functions (`sqrt`, `ln`,
   `sin`, `cos`, `abs`, `floor`, `ceil`, `atan2`, `min`, `max`, ...) and constants (`PI`, `E`).
   Each built-in has a `DimSignature` that describes its dimensional behavior.
2. **`crates/graphcal-eval/src/prelude.rs`** -- Registers the standard library: 8 base
   dimensions, 9 derived dimensions, and 20+ units with their scale factors.
3. **`crates/graphcal-eval/src/registry.rs`** -- The type system's lookup tables, split into
   **domain-specific registries** with a **builder/frozen pattern**:
   - `RegistryBuilder` -- mutable during construction (registers dimensions, units, etc.)
   - `Registry` -- frozen, read-only aggregate after `builder.build()`:
     - `DimensionRegistry` -- dimension names → `Dimension`
     - `UnitRegistry` -- unit names → `UnitInfo` (dimension + scale)
     - `TypeRegistry` -- struct/union type definitions + variant reverse lookup
     - `FunctionRegistry` -- function definitions
     - `IndexRegistry` -- index definitions

### Step 9: Understand the TIR (Typed IR)

Read **`crates/graphcal-eval/src/tir.rs`**. The TIR resolves all type annotations from their
AST form to semantic types:

- `type_resolve()` takes an `IR` and produces a `TIR`.
- `ResolvedTypeExpr` variants: `Dimensionless`, `Bool`, `Int`, `Scalar(Dimension)`, `Struct`,
  `GenericStruct`, `GenericDimParam`, `GenericDimExpr`, `Indexed`.
- `ResolvedFnSig` -- fully-resolved function signature with generic params, param types, and
  return type.
- Handles generic dimension parameters and index parameters.

### Step 10: Understand dimension checking

Read **`crates/graphcal-eval/src/dim_check.rs`**. This is the type checker:

- For each declaration, **infers** the dimension of the right-hand side expression.
- **Compares** the inferred dimension against the declared type annotation.
- Handles generic functions via **unification**: when you call `lerp(@a, @b, 0.5)` and `lerp`
  has signature `<D: Dim>(D, D, Dimensionless) -> D`, it infers `D` from the actual arguments.
- Reports mismatches with precise source spans.

Key types: `DeclaredType` (Scalar, Struct, Indexed) and `InferredType` (same + LoopVar).

### Step 11: Understand compilation and evaluation

Read these files:

1. **`crates/graphcal-eval/src/exec_plan.rs`** -- Compiles a TIR into an `ExecPlan`: topological
   sort of const declarations + compile-time evaluation, then topological sort of runtime
   declarations (params + nodes) into evaluation order.
2. **`crates/graphcal-eval/src/eval_expr.rs`** -- The recursive expression evaluator. Defines
   `RuntimeValue` (Scalar, Struct, Indexed, VariantLabel) and `eval_expr()` which pattern-matches
   on `ExprKind` to compute values. This is where the actual math happens.
3. **`crates/graphcal-eval/src/eval.rs`** -- The **orchestrator**. Key public functions:
   - `compile_and_eval_project()` -- disk-based, multi-file, full pipeline.
   - `compile_and_eval_named()` -- in-memory source, full pipeline.
   - `compile_to_tir_project()` -- disk-based, stops at TIR (for tooling/LSP).
   - `compile_and_eval_from_project()` -- full pipeline from a `LoadedProject`.

### Step 12: Understand multi-file support

Read **`crates/graphcal-eval/src/loader.rs`**. It performs a DFS from the root `.gcl` file:

- For each `use` declaration, resolves the file path, loads and parses it.
- Detects circular imports via a loading stack.
- Returns files in topological (post-order) so dependencies are processed before dependents.
- `LoadedProject` can mix disk files and in-memory overrides (for the LSP).

### Step 13: Understand error handling

Read **`crates/graphcal-eval/src/error.rs`**. All semantic errors are variants of
`GraphcalError`, which implements `miette::Diagnostic`. Error codes follow the pattern
`graphcal::{PREFIX}{NUMBER}`:

| Prefix | Domain | Examples |
| -------- | -------- | --------- |
| N | Name resolution | N001 duplicate, N002 unknown graph ref |
| F | Functions | F001 @-in-fn, F002 recursive fn |
| G | Graph structure | G001 cycle |
| E | Evaluation | E001 runtime error |
| D | Dimensions | D001 mismatch, D004 unknown unit |
| S | Structs | S001 duplicate let, S003 unknown field |
| I | Indexes | I001 unknown index, I004 index mismatch |
| M | Multi-file | M000 file not found, M002 circular import |
| A | Assertions | A002 assumes on const, A003 @-in-assert |
| O | Overrides | O001 override non-param |

### Step 14: Understand the CLI

Read **`crates/graphcal-cli/src/main.rs`**. The CLI is the user-facing entry point:

- Uses `clap` for argument parsing.
- Has four subcommands: `eval`, `format`, `typecheck`, and `lsp`.
- Parses `--set` overrides by using `graphcal_syntax::parser::Parser::parse_single_expr()`.
- Supports JSON parameter input files via `json_input.rs`.
- Calls `compile_and_eval_project()` from the eval crate.
- Formats output as text (aligned columns with units) or JSON.
- Uses `miette`'s fancy handler for colorful error reporting in the terminal.

---

## 6. Crate: graphcal-syntax

**Location**: `crates/graphcal-syntax/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | ~14 | Module declarations. All modules are `pub`. |
| `span.rs` | ~66 | `Span` struct (offset + length). Converts to `miette::SourceSpan`. |
| `token.rs` | ~739 | `Token` enum with `logos` derive. 40+ token variants. Display impl for error messages. |
| `lexer.rs` | ~166 | `Lexer` wrapper: peekable token stream. Yields `(Token, Span)` pairs. |
| `ast.rs` | ~813 | Full AST definition. 10 declaration kinds, 23 expression kinds, type/unit/dimension expressions. |
| `names.rs` | ~250 | Type-safe newtype wrappers for identifiers (`DeclName`, `DimName`, `UnitName`, etc.) and `Spanned<T>`. |
| `comments.rs` | ~175 | Comment extraction and source metadata for the formatter. `SourceMetadata`, `Comment`, `CommentKind`. |
| `dimension.rs` | ~750 | `Rational`, `BaseDim`, `Dimension`. Pure math, no dependencies on other syntax modules. |
| `parser.rs` | ~4447 | Recursive descent parser. Precedence climbing for expressions. 100+ unit tests. |

### Key Design Decisions

1. **Logos for lexing**: The `logos` crate generates a fast, zero-allocation lexer from regex
   patterns on the `Token` enum. Whitespace and comments are automatically skipped.

2. **Lexer as crate-internal**: Users of the crate interact through the parser, not the lexer
   directly. The `Lexer` type is `pub(crate)`.

3. **Casing enforced at parse time**: The parser validates naming conventions immediately,
   producing clear errors with source spans.

4. **Rational exponents**: Using exact rationals instead of floats for dimension exponents
   avoids floating-point comparison issues. `sqrt(Length)` produces `Length^(1/2)` exactly.

5. **Span on every node**: Every AST node carries a `Span`, enabling precise error reporting
   throughout the pipeline.

6. **Type-safe names**: The `names.rs` module uses newtype wrappers to prevent accidentally
   passing a `DimName` where a `UnitName` is expected. This catches name-category bugs at
   compile time.

---

## 7. Crate: graphcal-eval

**Location**: `crates/graphcal-eval/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | ~23 | Module declarations. `eval`, `error`, `loader`, `registry`, `tir`, and `builtins` are `pub`. |
| `resolve.rs` | ~2022 | Name resolution: separate declarations, validate refs, build dep maps. |
| `ir.rs` | ~674 | Intermediate Representation: combines resolution + registry building. `lower_to_builder()`, `UnfrozenIR`, `IR`. |
| `tir.rs` | ~1530 | Typed IR: resolves type annotations to semantic types. `type_resolve()`, `TIR`, `ResolvedTypeExpr`. |
| `dim_check.rs` | ~3420 | Dimension type checking with inference and generic unification. |
| `eval.rs` | ~2434 | Pipeline orchestrator: `compile_and_eval_*()`, `compile_to_tir_*()`. |
| `eval_expr.rs` | ~1517 | Recursive expression evaluator: `eval_expr()` and `RuntimeValue`. |
| `exec_plan.rs` | ~266 | Compilation to execution plan: const eval + DAG topological sort. Replaces former `dag.rs` + `const_eval.rs`. |
| `fn_check.rs` | ~421 | Function recursion detection via call graph + topological sort. |
| `registry.rs` | ~919 | Domain-specific registries with builder/frozen pattern. `RegistryBuilder` → `Registry`. |
| `builtins.rs` | ~236 | 14 built-in functions (sqrt, sin, abs, floor, ceil, atan2, min, max, ...) and constants (PI, E). |
| `prelude.rs` | ~260 | Standard library: 8 base dimensions, 9 derived dimensions, and 20+ units. |
| `loader.rs` | ~415 | Multi-file DFS loader with cycle detection. `LoadedProject`. |
| `error.rs` | ~541 | `GraphcalError` enum with miette diagnostic support. 10 error code families (N/F/G/E/D/S/I/M/A/O). |

### Key Design Decisions

1. **IR-based multi-pass architecture**: The pipeline transforms through explicit IRs
   (AST → IR → TIR → ExecPlan → EvalResult). Each IR adds semantic information while
   maintaining a clear boundary.

2. **Builder/frozen pattern for Registry**: The `RegistryBuilder` is mutable during construction,
   then frozen into an immutable `Registry` with domain-specific sub-registries
   (`DimensionRegistry`, `UnitRegistry`, `TypeRegistry`, `FunctionRegistry`, `IndexRegistry`).
   This ensures the type system is read-only during later phases.

3. **Pipeline decoupled from I/O**: `compile_and_eval_project()` reads from disk, while
   `compile_and_eval_named()` works with in-memory strings. `LoadedProject` can mix disk files
   with in-memory overrides. This enables the LSP to analyze unsaved buffers.

4. **Two evaluation phases**: Constants are evaluated at "compile time" (in `ExecPlan`), and
   params/nodes are evaluated at "runtime" (in DAG topological order). This separation is clean
   because `@` is prohibited in constants.

5. **IndexMap for ordered results**: The `EvalResult` uses `IndexMap` to preserve source-code
   order in output, which is important for human-readable display.

6. **Display unit extraction**: When a node uses unit conversion (`->`) or a unit literal, the
   display unit is extracted for pretty-printing. The internal value is always in SI base units.

### Important Internal Types

| Type | Location | Purpose |
| ------ | ---------- | --------- |
| `ResolvedFile` | resolve.rs | Output of name resolution: separated declarations + dependency maps |
| `UnfrozenIR` / `IR` | ir.rs | Lowered IR with resolved names and registry |
| `TIR` | tir.rs | Typed IR with resolved type expressions and function signatures |
| `ResolvedTypeExpr` | tir.rs | Semantic type: Scalar, Struct, GenericStruct, Indexed, Bool, Int, etc. |
| `RegistryBuilder` / `Registry` | registry.rs | Mutable builder → frozen lookup tables for the type system |
| `DeclaredType` | dim_check.rs | Resolved type annotation: Scalar(Dimension), Struct, or Indexed |
| `RuntimeValue` | eval_expr.rs | Runtime value: Scalar(f64), Struct(IndexMap), Indexed(IndexMap), VariantLabel |
| `ExecPlan` | exec_plan.rs | Compiled execution plan: const values + topo order + expressions |
| `EvalResult` | eval.rs | Final output: consts, params, nodes with display metadata |
| `Value` | eval.rs | Display-friendly value with SI value, display value, and unit label |
| `LoadedProject` | loader.rs | Collection of loaded/parsed files (disk or in-memory) |

---

## 8. Crate: graphcal-cli

**Location**: `crates/graphcal-cli/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `main.rs` | ~670 | CLI entry point: clap args, subcommands, output formatting. |
| `json_input.rs` | ~658 | Converts JSON input files to parameter overrides. Supports scalars, structs, tagged unions, indexed params. |
| `tests/cli.rs` | ~350+ | End-to-end integration tests using the compiled binary. |

### CLI Structure

```sh
graphcal eval <FILE> [--format text|json] [--set 'name=expr'] [--input input.json] [--no-assert]
graphcal format [--check] <PATHS>...
graphcal typecheck <PATHS>...
graphcal lsp
```

Four subcommands:

1. **`eval`** -- Evaluate a `.gcl` file. Supports `--set` for inline overrides, `--input` for
   JSON parameter files, `--format` for text/JSON output, and `--no-assert` to skip assertions.
2. **`format`** -- Format `.gcl` files using `graphcal-fmt`. With `--check`, reports unformatted
   files without modifying them.
3. **`typecheck`** -- Check files for errors without evaluation (stops at TIR).
4. **`lsp`** -- Start the Language Server Protocol server.

### Output Formats

**Text output** (default): Aligned columns with unit display.

```sh
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

## 9. Crate: graphcal-fmt

**Location**: `crates/graphcal-fmt/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | ~28 | Public API: `format_source(source: &str) -> Option<String>`. |
| `format.rs` | ~1002 | `Formatter` struct and format functions for every AST node type. |
| `tests/format_tests.rs` | | Snapshot tests for formatter output. |

### Architecture

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

## 10. Crate: graphcal-lsp

**Location**: `crates/graphcal-lsp/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | ~22 | Module declarations. Public API: `async fn run()`. |
| `server.rs` | ~941 | Main LSP server: file tracking, incremental analysis, request dispatch. |
| `symbol_table.rs` | ~1062 | Symbol resolution and scope analysis from TIR. |
| `cursor_context.rs` | ~350 | Determines what entity is under the cursor (for hover, go-to-def, etc.). |
| `diagnostics.rs` | ~291 | Converts `GraphcalError` to LSP diagnostics with related information. |
| `rename.rs` | ~265 | Rename refactoring across declarations and references. |
| `completion.rs` | ~214 | Code completion for declarations, fields, variants, built-ins. |
| `convert.rs` | ~133 | Conversions between internal types and LSP protocol types. |
| `hover.rs` | ~124 | Hover information: type, dimension, documentation. |
| `document_symbols.rs` | ~94 | Document outline (symbols for breadcrumbs/outline view). |
| `references.rs` | ~79 | Find all references to a symbol. |
| `goto_definition.rs` | ~77 | Jump to definition of a symbol. |
| `inlay_hints.rs` | ~67 | Inline type hints showing computed values. |
| `formatting.rs` | ~64 | Document formatting using `graphcal-fmt`. |
| `signature_help.rs` | ~43 | Function signature help while typing arguments. |
| `document_links.rs` | ~39 | Clickable links for `use` import paths. |

### Architecture

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
| Semantic Tokens | Deferred |

---

## 11. Testing Infrastructure

### Test Categories

#### 1. Unit Tests (inline)

Located in `#[cfg(test)]` modules within source files:

- **`token.rs`**: Tests for individual token lexing.
- **`dimension.rs`**: Property-based tests (proptest) verifying algebraic laws of `Rational` and
  `Dimension`.
- **`parser.rs`**: 80+ tests for parsing all declaration types, expression precedence,
  casing validation, and error cases.
- **`main.rs`** (cli): Tests for `format_number()`.

#### 2. Snapshot Tests (insta)

Located in `crates/graphcal-eval/tests/error_snapshots.rs` with snapshots in
`crates/graphcal-eval/tests/snapshots/`.

These tests:

1. Load a `.gcl` file from `tests/fixtures/errors/`.
2. Compile it (expecting an error).
3. Render the error using `miette::NarratableReportHandler` (deterministic, non-colorful).
4. Compare against a stored `.snap` file using the `insta` crate.

This ensures error messages don't regress. There are **48+ snapshot tests** covering:

- Duplicate names, unknown references, casing violations
- Dimension mismatches, unknown units/dimensions
- Cycles in constants and runtime graph
- Struct errors (missing/extra fields, unknown types)
- Index errors (unknown index, missing/extra variants)
- Function errors (wrong arity, recursion, @-in-fn)
- Runtime errors (division by zero, sqrt of negative)
- Assertion errors (non-bool assert, assumes violations)
- Tagged union errors (non-exhaustive match, duplicate arms)
- Range index errors

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

Located in `crates/graphcal-eval/tests/edge_case_bugs.rs` (~850 lines). These tests cover
specific bugs that have been found and fixed, ensuring they do not regress.

#### 6. Formatter Snapshot Tests

Located in `crates/graphcal-fmt/tests/format_tests.rs`. These verify that the formatter
produces expected output for various `.gcl` files.

#### 7. Test Fixtures

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
├── integers.gcl            # Integer type support
├── time_scan.gcl           # Time-based scan (system dynamics)
├── user_dimensions.gcl     # User-defined dimensions
├── orbital.gcl             # Simple orbital velocity
├── constants.gcl           # Constants-only file
├── multi/                  # Multi-file test projects
│   ├── rocket_split/       # Basic multi-file import
│   ├── alias/              # Import aliases
│   ├── alias_conflict/     # Import alias conflicts
│   ├── assertions/         # Multi-file assertions
│   ├── explicit_index/     # Explicit index imports
│   └── imported_deps/      # Imported dependencies
└── errors/                 # 50+ files designed to trigger specific errors
    ├── duplicate.gcl
    ├── unknown_ref.gcl
    ├── dim_mismatch_add.gcl
    ├── cycle.gcl
    ├── non_exhaustive_match.gcl
    ├── assert_not_bool.gcl
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

## 12. CI/CD and Developer Tooling

### CI Pipeline (`.github/workflows/ci.yaml`)

Five parallel jobs:

| Job | What it does |
| ----- | ------------- |
| `checks` | Clippy (all + no features), rustfmt, doc generation, cargo check, no uncommitted changes |
| `test` | `cargo test --workspace` |
| `coverage` | `cargo-llvm-cov` → lcov → Codecov upload |
| `msrv` | Minimum Supported Rust Version check via `cargo-hack` |
| `typos` | Spell checking with `crate-ci/typos` |

A `collect` job aggregates results to provide a single status check.

### Justfile

```bash
just lint       # Run all lints (clippy, fmt, doc, check)
just test       # Run all tests
just coverage   # Generate HTML coverage report
```

### Pre-commit Hooks (`.pre-commit-config.yaml`)

- Trailing whitespace, end-of-file fixer, TOML/YAML/JSON validation
- Large file check, merge conflict check, mixed line endings
- Markdown linting (`markdownlint`)
- Spell checking (`typos`)

### Clippy Configuration

The workspace uses **strict clippy settings** (defined in root `Cargo.toml`):

- All lint groups at `warn`: pedantic, nursery, perf, complexity, style, suspicious
- `correctness` at `deny`
- `unsafe_code` at `warn`
- Some pragmatic allows: `unwrap_used`, `expect_used`, `panic` (TODO to tighten)

### Rust Toolchain

- Stable channel (`rust-toolchain.toml`)
- MSRV: 1.91+ (edition 2024)

---

## 13. Design Documents and Phased Development

### Design Document Structure

All design documents are in `design/`. The `design/README.md` is the index.

**20 aspect documents** cover orthogonal design dimensions:

| # | Document | Topic |
| --- | ---------- | ------- |
| 01 | computation-model | DAG of param/node/const, reactive evaluation |
| 02 | syntax-design | Keywords, expressions, statements |
| 03 | primitives | f64, i64, bool, Str, Datetime, Option |
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

Additional design documents:

- **`graphcal-format-plan.md`** -- Design and implementation plan for the code formatter.
- **`primitive-types-analysis.md`** -- Analysis of primitive type choices.

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
| 7+ | System dynamics, spaces, tagged unions, TUI, ... | Future |

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

## 14. Extending Graphcal

### Adding a New Built-in Function

1. Add the function entry to `BUILTIN_FUNCTIONS` in `crates/graphcal-eval/src/builtins.rs`.
   Specify: name, arity, eval closure (operating on `f64` values), and `DimSignature`.
2. If needed, add a new `DimSignature` variant and handle it in `dim_check.rs` at the
   `BuiltinFunction` match arm in `infer_type()`.
3. Add test fixtures in `tests/fixtures/` and snapshot tests in
   `crates/graphcal-eval/tests/error_snapshots.rs` for error cases.
4. Update LSP completion in `crates/graphcal-lsp/src/completion.rs` if the function needs
   special handling.

### Adding a New Declaration Kind

1. Add the new variant to `DeclKind` in `crates/graphcal-syntax/src/ast.rs`.
2. Add any new tokens to `crates/graphcal-syntax/src/token.rs`.
3. Add parsing logic in `crates/graphcal-syntax/src/parser.rs` (handle the new keyword in
   `parse_declaration()`).
4. Handle the new declaration in `crates/graphcal-eval/src/resolve.rs` (name resolution).
5. Handle IR lowering in `crates/graphcal-eval/src/ir.rs`.
6. Handle type resolution in `crates/graphcal-eval/src/tir.rs` if it has type annotations.
7. Handle dimension checking in `crates/graphcal-eval/src/dim_check.rs` if applicable.
8. Handle compilation in `crates/graphcal-eval/src/exec_plan.rs` if it participates in the DAG.
9. Handle evaluation in `crates/graphcal-eval/src/eval.rs` and `eval_expr.rs`.
10. Update the CLI output formatting in `crates/graphcal-cli/src/main.rs` if the declaration
    produces output.
11. Update the formatter in `crates/graphcal-fmt/src/format.rs`.
12. Update LSP features (symbol table, document symbols, completion, etc.).
13. Update the tree-sitter grammar in `tree-sitter-graphcal/grammar.js`.
14. Add tests at every level.

### Adding a New Expression Kind

1. Add the variant to `ExprKind` in `ast.rs`.
2. Add parsing in `parser.rs` at the appropriate precedence level.
3. Add dimension inference in `dim_check.rs` (in the `infer_type()` match).
4. Add evaluation in `eval_expr.rs` (in the `eval_expr()` match).
5. Update the formatter in `format.rs`.
6. Add test fixtures and snapshot tests.

### Adding a New Unit or Dimension to the Prelude

1. Edit `crates/graphcal-eval/src/prelude.rs`.
2. Use `builder.register_dimension()` for new dimensions.
3. Use `builder.register_unit()` for new units, specifying the dimension and SI scale factor.

### Adding a New Error

1. Add a variant to `GraphcalError` in `crates/graphcal-eval/src/error.rs`.
2. Add the `#[error(...)]`, `#[diagnostic(code(...))]`, and `#[label(...)]` attributes.
3. Emit the error at the appropriate point in the pipeline.
4. Create a `.gcl` fixture file in `tests/fixtures/errors/`.
5. Add a snapshot test in `crates/graphcal-eval/tests/error_snapshots.rs`.
6. Run `cargo insta review` to approve the new snapshot.
7. Ensure the LSP diagnostics module handles the new error properly.

### General Tips for Contributors

- **Read the design doc first**: Before implementing a feature, check if there's a design
  document in `design/` or a phase document in `design/phases/`.
- **Follow the pipeline order**: Changes typically flow from syntax → IR → TIR → dim_check →
  exec_plan → eval.
- **Update all layers**: New features need updates across the syntax crate, eval crate,
  formatter, LSP, tree-sitter grammar, and editor extensions.
- **Every error needs a test**: Add both the error fixture (`.gcl`) and a snapshot test.
- **Use `insta` for regression**: Snapshot tests catch unintended changes to error messages.
- **Run `just lint` and `just test`** before committing.
- **Check `.local/` for prior research**: Development notes may contain useful context on design
  decisions.
