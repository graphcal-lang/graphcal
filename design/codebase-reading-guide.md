# Graphcal Codebase Reading Guide

A comprehensive guide for understanding, maintaining, and extending the graphcal codebase.

## Table of Contents

1. [What is Graphcal?](#1-what-is-graphcal)
2. [Theoretical Foundations](#2-theoretical-foundations)
3. [High-Level Architecture](#3-high-level-architecture)
4. [The Evaluation Pipeline](#4-the-evaluation-pipeline)
5. [Incremental Reading Order](#5-incremental-reading-order)
6. [Crate: graphcal-syntax](#6-crate-graphcal-syntax)
7. [Crate: graphcal-eval](#7-crate-graphcal-eval)
8. [Crate: graphcal-cli](#8-crate-graphcal-cli)
9. [Testing Infrastructure](#9-testing-infrastructure)
10. [CI/CD and Developer Tooling](#10-cicd-and-developer-tooling)
11. [Design Documents and Phased Development](#11-design-documents-and-phased-development)
12. [Extending Graphcal](#12-extending-graphcal)

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

The project is a **Cargo workspace** with three crates:

```
graphcal/
├── crates/
│   ├── graphcal-syntax/   # Lexer, parser, AST, dimension algebra
│   ├── graphcal-eval/     # Name resolution, type checking, evaluation
│   └── graphcal-cli/      # CLI binary (clap)
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
  └── graphcal-syntax
```

- **`graphcal-syntax`** has no in-workspace dependencies. It depends on `logos` (lexing),
  `miette` (diagnostics), and `thiserror`.
- **`graphcal-eval`** depends on `graphcal-syntax` and adds `petgraph` (DAG), `indexmap`
  (ordered maps), and `serde` (serialization).
- **`graphcal-cli`** depends on both and adds `clap` (argument parsing) and `serde_json`
  (JSON output).

### Layered Design

```
┌─────────────────────────────────┐
│        graphcal-cli             │  User-facing: argument parsing, output formatting
├─────────────────────────────────┤
│        graphcal-eval            │  Semantic analysis: resolution, type checking,
│                                 │  DAG construction, evaluation
├─────────────────────────────────┤
│       graphcal-syntax           │  Syntactic analysis: lexing, parsing, AST,
│                                 │  dimension algebra
└─────────────────────────────────┘
```

---

## 4. The Evaluation Pipeline

This is the core flow from source text to computed results. Understanding this pipeline is
essential for working on the codebase. The pipeline is orchestrated in
`graphcal-eval/src/eval.rs`.

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
   │ Resolve  │  resolve.rs: extract declarations, validate names,
   └────┬─────┘  build dependency maps, check for @-in-const violations
        │  ResolvedFile (separated consts/params/nodes, dependency maps)
        ▼
   ┌──────────┐
   │ Register │  registry.rs + prelude.rs: populate Registry with
   └────┬─────┘  dimensions, units, struct types, index types, functions
        │  Registry (lookup tables for the type system)
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
        ▼
   ┌────────────┐
   │ Const Eval │  const_eval.rs: topological sort of consts,
   └────┬───────┘  evaluate at compile time
        │  HashMap<String, RuntimeValue> (const values)
        ▼
   ┌───────────┐
   │ Build DAG │  dag.rs: build petgraph from param/node dependencies,
   └────┬──────┘  topological sort (detect cycles)
        │  RuntimeGraph (petgraph + topo order + expressions)
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

For **multi-file projects**, the pipeline begins with `loader.rs` which performs a DFS over
`use` declarations to load and parse all files in dependency order. Then the pipeline runs for
each file, with imported declarations propagated via `ImportedNames`.

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
5. **`tests/fixtures/multi/rocket_split/`** -- Shows multi-file imports with `use`.

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
(~2900 lines). It is a **recursive descent parser** with precedence climbing for expressions.

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

### Step 6: Understand name resolution

Read **`crates/graphcal-eval/src/resolve.rs`**. This is the first semantic pass after parsing:

- Separates `const`, `param`, and `node` declarations.
- Validates all references: `@name` must refer to a known param/node; bare `NAME` in const
  context must refer to a known const.
- Builds **dependency maps**: for each declaration, which other declarations does it reference?
- Enforces `@`-prohibition rules: consts and function bodies must not use `@`.
- The output is `ResolvedFile` with separated declaration lists and dependency maps.

### Step 7: Understand the type system registry

Read these files in order:

1. **`crates/graphcal-eval/src/builtins.rs`** -- Defines built-in functions (`sqrt`, `ln`,
   `sin`, ...) and constants (`PI`, `E`). Each built-in has a `DimSignature` that describes its
   dimensional behavior (e.g., `sqrt` takes `D` and returns `D^(1/2)`).
2. **`crates/graphcal-eval/src/prelude.rs`** -- Registers the standard library: 8 base
   dimensions, 9 derived dimensions, and ~25 units with their scale factors.
3. **`crates/graphcal-eval/src/registry.rs`** -- The `Registry` struct is the central lookup
   table for the type system. It stores dimensions, units, struct definitions, function
   definitions, and index definitions. Methods like `resolve_dim_expr()` convert AST type
   annotations into concrete `Dimension` values.

### Step 8: Understand dimension checking

Read **`crates/graphcal-eval/src/dim_check.rs`**. This is the type checker:

- For each declaration, **infers** the dimension of the right-hand side expression.
- **Compares** the inferred dimension against the declared type annotation.
- Handles generic functions via **unification**: when you call `lerp(@a, @b, 0.5)` and `lerp`
  has signature `<D: Dim>(D, D, Dimensionless) -> D`, it infers `D` from the actual arguments.
- Reports mismatches with precise source spans.

Key types: `DeclaredType` (Scalar, Struct, Indexed) and `InferredType` (same + LoopVar).

### Step 9: Understand evaluation

Read these files:

1. **`crates/graphcal-eval/src/const_eval.rs`** -- Topologically sorts constants and evaluates
   them. Simple: build a mini-DAG of const dependencies, sort, evaluate in order.
2. **`crates/graphcal-eval/src/dag.rs`** -- Builds the runtime DAG from param/node dependencies
   using petgraph. Returns a `RuntimeGraph` with topological order.
3. **`crates/graphcal-eval/src/eval_expr.rs`** -- The recursive expression evaluator. Defines
   `RuntimeValue` (Scalar, Struct, Indexed, VariantLabel) and `eval_expr()` which pattern-matches
   on `ExprKind` to compute values. This is where the actual math happens.
4. **`crates/graphcal-eval/src/eval.rs`** -- The **orchestrator**. Contains `compile_and_eval()`
   and `compile_and_eval_project()` which wire together all the phases listed in Section 4. Also
   handles parameter overrides (`--set`) and display unit extraction.

### Step 10: Understand multi-file support

Read **`crates/graphcal-eval/src/loader.rs`**. It performs a DFS from the root `.gcl` file:

- For each `use` declaration, resolves the file path, loads and parses it.
- Detects circular imports via a loading stack.
- Returns files in topological (post-order) so dependencies are processed before dependents.

### Step 11: Understand error handling

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
| M | Multi-file | M001 file not found, M002 circular import |
| O | Overrides | O001 override non-param |

### Step 12: Understand the CLI

Read **`crates/graphcal-cli/src/main.rs`**. The CLI is the user-facing entry point:

- Uses `clap` for argument parsing.
- Currently has one subcommand: `eval`.
- Parses `--set` overrides by using `graphcal_syntax::parser::Parser::parse_single_expr()`.
- Calls `compile_and_eval_project()` from the eval crate.
- Formats output as text (aligned columns with units) or JSON.
- Uses `miette`'s fancy handler for colorful error reporting in the terminal.

---

## 6. Crate: graphcal-syntax

**Location**: `crates/graphcal-syntax/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | 12 | Module declarations. The `lexer` module is `pub(crate)`, all others are `pub`. |
| `span.rs` | 58 | `Span` struct (offset + length). Converts to `miette::SourceSpan`. |
| `token.rs` | 652 | `Token` enum with `logos` derive. 40+ token variants. Display impl for error messages. |
| `lexer.rs` | 164 | `Lexer` wrapper: peekable token stream. Yields `(Token, Span)` pairs. |
| `ast.rs` | 548 | Full AST definition. 9 declaration kinds, 19 expression kinds, type/unit/dimension expressions. |
| `dimension.rs` | 617 | `Rational`, `BaseDim`, `Dimension`. Pure math, no dependencies on other syntax modules. |
| `parser.rs` | 2934 | Recursive descent parser. Precedence climbing for expressions. 80+ unit tests. |

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

---

## 7. Crate: graphcal-eval

**Location**: `crates/graphcal-eval/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `lib.rs` | 22 | Module declarations. Only `eval`, `error`, and `loader` are fully `pub`. |
| `resolve.rs` | 1578 | Name resolution: separate declarations, validate refs, build dep maps. |
| `dim_check.rs` | 2541 | Dimension type checking with inference and generic unification. |
| `eval.rs` | 1623 | Pipeline orchestrator: `compile_and_eval()`, `compile_and_eval_project()`. |
| `eval_expr.rs` | 769 | Recursive expression evaluator: `eval_expr()` and `RuntimeValue`. |
| `fn_check.rs` | 378 | Function recursion detection via call graph + topological sort. |
| `dag.rs` | 146 | DAG construction from dependencies using petgraph. |
| `const_eval.rs` | 173 | Compile-time constant evaluation in topological order. |
| `registry.rs` | 392 | Type system lookup tables: dimensions, units, types, functions, indexes. |
| `builtins.rs` | 228 | Built-in functions (sqrt, sin, ...) and constants (PI, E). |
| `prelude.rs` | 194 | Standard library: base/derived dimensions and units. |
| `loader.rs` | 245 | Multi-file DFS loader with cycle detection. |
| `error.rs` | 431 | `GraphcalError` enum with miette diagnostic support. |

### Key Design Decisions

1. **Multi-pass architecture**: The pipeline has distinct, well-separated passes (resolve →
   register → dim_check → const_eval → dag → eval). Each pass has a clear input and output.
   This makes it easy to reason about and test each phase independently.

2. **Registry as central state**: The `Registry` is built incrementally and passed to later
   phases. It acts as the "symbol table" for the type system.

3. **Two evaluation phases**: Constants are evaluated at "compile time" (before the DAG is
   built), and params/nodes are evaluated at "runtime" (in DAG topological order). This
   separation is clean because `@` is prohibited in constants.

4. **IndexMap for ordered results**: The `EvalResult` uses `IndexMap` to preserve source-code
   order in output, which is important for human-readable display.

5. **Display unit extraction**: When a node uses unit conversion (`->`) or a unit literal, the
   display unit is extracted for pretty-printing. The internal value is always in SI base units.

### Important Internal Types

| Type | Location | Purpose |
| ------ | ---------- | --------- |
| `ResolvedFile` | resolve.rs | Output of name resolution: separated declarations + dependency maps |
| `Registry` | registry.rs | Lookup table for dimensions, units, types, functions, indexes |
| `DeclaredType` | dim_check.rs | Resolved type annotation: Scalar(Dimension), Struct, or Indexed |
| `RuntimeValue` | eval_expr.rs | Runtime value: Scalar(f64), Struct(IndexMap), Indexed(IndexMap), VariantLabel |
| `RuntimeGraph` | dag.rs | petgraph + topological order + expression map |
| `EvalResult` | eval.rs | Final output: consts, params, nodes with display metadata |
| `Value` | eval.rs | Display-friendly value with SI value, display value, and unit label |

---

## 8. Crate: graphcal-cli

**Location**: `crates/graphcal-cli/src/`

### File-by-File Reference

| File | Lines | Purpose |
| ------ | ------- | --------- |
| `src/main.rs` | ~180 | CLI entry point: clap args, `--set` parsing, output formatting. |
| `tests/cli.rs` | ~350 | End-to-end integration tests using the compiled binary. |

### CLI Structure

```
graphcal eval <FILE> [--format text|json] [--set 'name=expr'] ...
```

The CLI is deliberately thin:

1. Parse arguments with `clap`.
2. Parse `--set` override expressions using `graphcal_syntax::parser::Parser::parse_single_expr()`.
3. Call `graphcal_eval::eval::compile_and_eval_project()`.
4. Format and print the result.
5. On error, use `miette`'s fancy handler for colorful diagnostics.

### Output Formats

**Text output** (default): Aligned columns with unit display.

```
dry_mass    = 1200 kg
fuel_mass   = 2800 kg
G0          = 9.80665 m/s^2
v_exhaust   = 3138.128 m/s
mass_ratio  = 3.333333
delta_v     = 3778.691 m/s
```

**JSON output**: Structured with `si_value`, optional `display_value` and `unit`, and nested
struct/indexed values.

---

## 9. Testing Infrastructure

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

This ensures error messages don't regress. There are **34+ snapshot tests** covering:

- Duplicate names, unknown references, casing violations
- Dimension mismatches, unknown units/dimensions
- Cycles in constants and runtime graph
- Struct errors (missing/extra fields, unknown types)
- Index errors (unknown index, missing/extra variants)
- Function errors (wrong arity, recursion, @-in-fn)
- Runtime errors (division by zero, sqrt of negative)

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

#### 5. Test Fixtures

Located in `tests/fixtures/`:

```
tests/fixtures/
├── rocket.gcl              # Tsiolkovsky rocket equation
├── hohmann.gcl             # Hohmann transfer with structs
├── functions.gcl           # Pure functions with generics
├── indexed.gcl             # Indexed values and aggregation
├── orbital.gcl             # Simple orbital velocity
├── constants.gcl           # Constants-only file
├── multi/
│   └── rocket_split/       # Multi-file import example
│       ├── main.gcl
│       ├── constants.gcl
│       └── params.gcl
└── errors/                 # 34+ files designed to trigger specific errors
    ├── duplicate.gcl
    ├── unknown_ref.gcl
    ├── dim_mismatch_add.gcl
    ├── cycle.gcl
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

## 10. CI/CD and Developer Tooling

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

## 11. Design Documents and Phased Development

### Design Document Structure

All design documents are in `design/`. The `design/README.md` is the index.

**17 aspect documents** cover orthogonal design dimensions:

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
| 6 | Scenarios and CLI (MVP) | In progress |
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

## 12. Extending Graphcal

### Adding a New Built-in Function

1. Add the function entry to `BUILTIN_FUNCTIONS` in `crates/graphcal-eval/src/builtins.rs`.
   Specify: name, arity, eval closure (operating on `f64` values), and `DimSignature`.
2. If needed, add a new `DimSignature` variant and handle it in `dim_check.rs` at the
   `BuiltinFunction` match arm in `infer_type()`.
3. Add test fixtures in `tests/fixtures/` and snapshot tests in
   `crates/graphcal-eval/tests/error_snapshots.rs` for error cases.

### Adding a New Declaration Kind

1. Add the new variant to `DeclKind` in `crates/graphcal-syntax/src/ast.rs`.
2. Add any new tokens to `crates/graphcal-syntax/src/token.rs`.
3. Add parsing logic in `crates/graphcal-syntax/src/parser.rs` (handle the new keyword in
   `parse_declaration()`).
4. Handle the new declaration in `crates/graphcal-eval/src/resolve.rs` (name resolution).
5. Handle dimension checking in `crates/graphcal-eval/src/dim_check.rs` if applicable.
6. Handle evaluation in `crates/graphcal-eval/src/eval.rs` and `eval_expr.rs`.
7. Update the CLI output formatting in `crates/graphcal-cli/src/main.rs` if the declaration
   produces output.
8. Add tests at every level.

### Adding a New Expression Kind

1. Add the variant to `ExprKind` in `ast.rs`.
2. Add parsing in `parser.rs` at the appropriate precedence level.
3. Add dimension inference in `dim_check.rs` (in the `infer_type()` match).
4. Add evaluation in `eval_expr.rs` (in the `eval_expr()` match).
5. Add test fixtures and snapshot tests.

### Adding a New Unit or Dimension to the Prelude

1. Edit `crates/graphcal-eval/src/prelude.rs`.
2. Use `registry.register_dimension()` for new dimensions.
3. Use `registry.register_unit()` for new units, specifying the dimension and SI scale factor.

### Adding a New Error

1. Add a variant to `GraphcalError` in `crates/graphcal-eval/src/error.rs`.
2. Add the `#[error(...)]`, `#[diagnostic(code(...))]`, and `#[label(...)]` attributes.
3. Emit the error at the appropriate point in the pipeline.
4. Create a `.gcl` fixture file in `tests/fixtures/errors/`.
5. Add a snapshot test in `crates/graphcal-eval/tests/error_snapshots.rs`.
6. Run `cargo insta review` to approve the new snapshot.

### General Tips for Contributors

- **Read the design doc first**: Before implementing a feature, check if there's a design
  document in `design/` or a phase document in `design/phases/`.
- **Follow the pipeline order**: Changes typically flow from syntax → resolve → dim_check → eval.
- **Every error needs a test**: Add both the error fixture (`.gcl`) and a snapshot test.
- **Use `insta` for regression**: Snapshot tests catch unintended changes to error messages.
- **Run `just lint` and `just test`** before committing.
- **Check `.local/` for prior research**: Development notes may contain useful context on design
  decisions.
