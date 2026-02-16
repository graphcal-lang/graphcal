# Phase 0: Scalar Graph

> `param` + `node` + `const` with `f64` only. Single file. CLI evaluation.

## Goal

Prove the core computation model works end-to-end: parse a `.gcl` file,
build a DAG, topologically sort, evaluate, print results. Everything is
`f64` -- no dimensions, no units, no types.

## Design Decisions to Lock

These open questions from the aspect docs must be resolved before implementing Phase 0.

### From [01-computation-model](../01-computation-model.md)

- [x] **Evaluation strategy for Phase 0:** Eager (evaluate all nodes in topological order).
      Incremental recomputation (dirty tracking with early cutoff) is deferred to later phases
      but the architecture is designed for it. See the evaluation strategy section in
      [01-computation-model.md](../01-computation-model.md).
- [x] **Error propagation:** Fail-fast for Phase 0 (entire evaluation fails on first error).
      The accumulator-based error collection pattern (inspired by Salsa) will be adopted in
      later phases to enable partial evaluation and multiple error reporting.
      The fail-fast approach is incrementally migratable to the accumulator pattern.
      All errors use miette's `Diagnostic` trait. See [17-error-messages.md](../17-error-messages.md).
- [x] **Cycle detection:** Cycles produce a clear compile error with the cycle path printed.
      Uses miette's `#[related]` for cycle chain rendering.

### From [02-syntax-design](../02-syntax-design.md) (subset)

- [x] **Semicolon rule:** Every top-level declaration (`param`, `node`, `const`)
      ends with `;`. Block bodies are deferred to Phase 2.
- [x] **Numeric literals:** Support `_` separators (`200_000`) and scientific
      notation (`3.98e5`).
- [x] **Operator precedence:** `^` binds tighter than unary `-`.
      `-x^2` is parsed as `-(x^2)`, matching Python and mathematical convention.
      Full precedence (lowest to highest): conditional (`if`/`else`), `||`, `&&`,
      comparison (`<`, `>`, `<=`, `>=`, `==`, `!=`), `+`/`-`, `*`/`/`, unary `-`/`!`,
      `^` (right-associative).
- [x] **Comments:** `//` line comments only. `///` doc comments are not supported
      in Phase 0.
- [x] **Built-in functions:** `sqrt`, `exp`, `ln`, `abs`, `sin`, `cos`, `tan`,
      `asin`, `acos`, `atan2`, `min`, `max`, `floor`, `ceil`.
      Built-in constants: `PI`, `E` (UPPER_SNAKE_CASE, consistent with `const` casing).

### From [08-scoping](../08-scoping.md)

- [x] **`@` for `param`/`node` references only:** `@name` references graph scope
      (`param` and `node` declarations). `const` values are referenced by bare
      `UPPER_SNAKE_CASE` names without `@`, because `const` is evaluated at compile
      time and is not part of the runtime DAG. In Phase 0, bare `lower_snake_case`
      names in expressions are always errors (no `let` bindings yet).
- [x] **Duplicate name detection:** `param x = 1; node x = 2;` is a compile error.
      Names must be unique across all `param`, `node`, and `const` declarations.

### From [17-error-messages](../17-error-messages.md)

- [x] **Error format:** Use miette's `GraphicalReportHandler` for terminal output (Unicode
      box-drawing, ANSI colors, labeled spans). See [17-error-messages.md](../17-error-messages.md).
- [x] **Error codes:** Assign from the start using the `graphcal::{PREFIX}{NUMBER}` scheme.
      Phase 0 needs `P0xx` (parse), `G0xx` (graph), and `N0xx` (namespace) error codes.

### Naming and file conventions

- [x] **CLI binary name:** `graphcal`.
- [x] **File extension:** `.gcl`.
- [x] **CLI command:** `graphcal eval <file>`.

### Casing rules (compiler-enforced)

- [x] **`const`:** `UPPER_SNAKE_CASE` (e.g., `const G0 = 9.80665;`). Compile error
      if not UPPER_SNAKE_CASE.
- [x] **`param`:** `lower_snake_case` (e.g., `param dry_mass = 1200.0;`). Compile error
      if not lower_snake_case.
- [x] **`node`:** `lower_snake_case` (e.g., `node delta_v = ...;`). Compile error
      if not lower_snake_case.
- [x] **Built-in constants:** `UPPER_SNAKE_CASE` (`PI`, `E`). No exceptions.
- [x] **Built-in functions:** `lower_snake_case` (`sqrt`, `ln`, `atan2`). Already consistent.

These casing rules enable unambiguous bare-name resolution at every reference site:

| Syntax | Meaning | Example |
| --- | --- | --- |
| `@name` | Graph reference (`param` or `node`) | `@dry_mass`, `@v_exhaust` |
| `NAME` | Compile-time `const` or built-in constant | `G0`, `PI` |
| `name(...)` | Function call (built-in for Phase 0) | `sqrt(2.0)`, `ln(@mass_ratio)` |

### `const` semantics

- [x] **Compile-time evaluation:** `const` declarations are evaluated before the
      runtime DAG. A `const` **cannot** reference `@` (graph scope). It can only
      reference:
  - Literal values (`42.0`, `9.80665`)
  - Other `const` names (bare UPPER_SNAKE_CASE)
  - Built-in constants (`PI`, `E`)
  - Built-in function calls applied to the above (`sqrt(2.0)`)
  - In later phases: user-defined pure functions (`fn`)
- [x] **`const` dependency resolution:** `const` declarations are topologically
      sorted among themselves. Cycles among `const`s are compile errors.

## Two-Phase Evaluation Model

Phase 0 uses a two-phase evaluation:

1. **Compile-time phase:** Topologically sort and evaluate all `const` declarations.
   `const` expressions may only reference other `const`s, built-in constants, and
   built-in functions. The result is a set of resolved constant values.

2. **Runtime phase:** Build the DAG from `param` and `node` declarations, with
   resolved `const` values inlined. Topologically sort and evaluate in order.

This separation is invisible to the user in Phase 0 (everything runs at once),
but establishes the correct semantics for later phases where `const` values can
be inlined by the compiler and are not affected by scenario overrides.

## Syntax Supported in Phase 0

```ebnf
File          = Declaration*
Declaration   = ParamDecl | NodeDecl | ConstDecl | Comment
ParamDecl     = "param" LOWER_IDENT "=" Expr ";"
NodeDecl      = "node"  LOWER_IDENT "=" Expr ";"
ConstDecl     = "const" UPPER_IDENT "=" ConstExpr ";"
Comment       = "//" <anything until newline>

(* Runtime expressions — used in param/node *)
Expr          = Conditional | OrExpr
Conditional   = "if" Expr "{" Expr "}" "else" "{" Expr "}"
OrExpr        = AndExpr ("||" AndExpr)*
AndExpr       = CompExpr ("&&" CompExpr)*
CompExpr      = AddExpr (("<" | ">" | "<=" | ">=" | "==" | "!=") AddExpr)?
AddExpr       = MulExpr (("+" | "-") MulExpr)*
MulExpr       = UnaryExpr (("*" | "/") UnaryExpr)*
UnaryExpr     = ("-" | "!") UnaryExpr | PowerExpr
PowerExpr     = Atom ("^" UnaryExpr)?          (* right-associative *)
Atom          = NUMBER | GRAPH_REF | CONST_REF | FnCall | "(" Expr ")" | BOOL

(* Const expressions — used in const declarations, no @ allowed *)
ConstExpr     = ConstConditional | ConstOrExpr
ConstConditional = "if" ConstExpr "{" ConstExpr "}" "else" "{" ConstExpr "}"
ConstOrExpr   = ConstAndExpr ("||" ConstAndExpr)*
ConstAndExpr  = ConstCompExpr ("&&" ConstCompExpr)*
ConstCompExpr = ConstAddExpr (("<" | ">" | "<=" | ">=" | "==" | "!=") ConstAddExpr)?
ConstAddExpr  = ConstMulExpr (("+" | "-") ConstMulExpr)*
ConstMulExpr  = ConstUnaryExpr (("*" | "/") ConstUnaryExpr)*
ConstUnaryExpr = ("-" | "!") ConstUnaryExpr | ConstPowerExpr
ConstPowerExpr = ConstAtom ("^" ConstUnaryExpr)?
ConstAtom     = NUMBER | CONST_REF | ConstFnCall | "(" ConstExpr ")" | BOOL

GRAPH_REF     = "@" LOWER_IDENT
CONST_REF     = UPPER_IDENT
FnCall        = LOWER_IDENT "(" (Expr ("," Expr)*)? ")"
ConstFnCall   = LOWER_IDENT "(" (ConstExpr ("," ConstExpr)*)? ")"
NUMBER        = [0-9]+ ("_"? [0-9]+)* ("." [0-9]+ ("_"? [0-9]+)*)? ([eE] [+-]? [0-9]+)?
BOOL          = "true" | "false"
LOWER_IDENT   = [a-z][a-z0-9_]*
UPPER_IDENT   = [A-Z][A-Z0-9_]*
```

### Notes on the grammar

- `Expr` and `ConstExpr` share the same structure but `ConstExpr` does not allow
  `GRAPH_REF` (`@`). In the implementation, this can be a single parser with a
  semantic check rather than duplicated grammar productions.
- `LOWER_IDENT` starts with a lowercase letter and contains only lowercase letters,
  digits, and underscores (`lower_snake_case`).
- `UPPER_IDENT` starts with an uppercase letter and contains only uppercase letters,
  digits, and underscores (`UPPER_SNAKE_CASE`).
- `NUMBER` supports `_` separators and scientific notation.

### What is NOT in the grammar yet

- Type annotations (`: Type`)
- Units after literals (`400 km`)
- Block bodies (`{ let ...; expr }`)
- `type`, `dimension`, `unit`, `space`, `index`, `table`, `fn`
- `use`, `private`
- Multi-file
- Doc comments (`///`)

## Implementation Scope

| Component | Description |
| --- | --- |
| **Parser** | `.gcl` file -> AST (single file) |
| **Name resolver** | Resolve `@` references and bare const references, detect duplicates, detect cycles, enforce casing rules |
| **Const evaluator** | Topologically sort `const` declarations, evaluate compile-time expressions |
| **DAG builder** | AST -> `petgraph` DAG (params and nodes only, consts inlined) |
| **Evaluator** | Topological sort, evaluate `f64` arithmetic |
| **CLI** | `graphcal eval <file>` prints all values; `--format json` for machine-readable output |
| **Error reporter** | Parse errors, unknown references, cycles, casing violations (via miette) |

## Out of Scope

Everything not listed above. Specifically:

- Dimensions, units, type annotations
- Structs, tagged unions
- Multi-line node bodies, `let` bindings
- Functions (`fn`)
- Multi-file, imports, prelude
- Tables
- Scenarios, `--set param=value` CLI overrides
- TUI, web UI
- Python interop
- Spreadsheet import/export

## Milestone Test

```gcl
// rocket.gcl
param dry_mass = 1200.0;
param fuel_mass = 2800.0;
param isp = 320.0;
const G0 = 9.80665;

node v_exhaust = @isp * G0;
node mass_ratio = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v = @v_exhaust * ln(@mass_ratio);
```

```sh
$ graphcal eval rocket.gcl
dry_mass     = 1200
fuel_mass    = 2800
isp          = 320
G0           = 9.80665
v_exhaust    = 3138.128
mass_ratio   = 3.333333
delta_v      = 3783.277
```

### Const cross-reference test

```gcl
// constants.gcl
const G0 = 9.80665;
const TWO_G0 = 2.0 * G0;
const HALF_PI = PI / 2.0;
const SQRT2 = sqrt(2.0);

param radius = 100.0;
node circumference = 2.0 * PI * @radius;
node area = PI * @radius ^ 2.0;
```

### JSON output test

```sh
$ graphcal eval rocket.gcl --format json
{
  "const": {
    "G0": 9.80665
  },
  "param": {
    "dry_mass": 1200.0,
    "fuel_mass": 2800.0,
    "isp": 320.0
  },
  "node": {
    "v_exhaust": 3138.128,
    "mass_ratio": 3.333333333333333,
    "delta_v": 3783.2773918685007
  }
}
```

### Error cases that must work

```gcl
// error[graphcal::N002]: unknown graph reference
node x = @nonexistent + 1.0;

// error[graphcal::G001]: cyclic dependency
node a = @b + 1.0;
node b = @a + 1.0;

// error[graphcal::N001]: duplicate name
param x = 1.0;
node x = 2.0;

// error[graphcal::P0xx]: @ not allowed in const expression
const BAD = @some_param * 2.0;

// error[graphcal::P0xx]: const name must be UPPER_SNAKE_CASE
const bad_name = 42.0;

// error[graphcal::P0xx]: param name must be lower_snake_case
param BadParam = 42.0;

// error[graphcal::G001]: cyclic dependency among consts
const A = B + 1.0;
const B = A + 1.0;
```

## Open Questions

All previously open questions have been resolved. See the "Design Decisions to Lock"
section above for the full list of decisions.
