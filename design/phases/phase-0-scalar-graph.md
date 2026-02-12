# Phase 0: Scalar Graph

> `param` + `node` + `const` with `f64` only. Single file. CLI evaluation.

## Goal

Prove the core computation model works end-to-end: parse a `.graph` file,
build a DAG, topologically sort, evaluate, print results. Everything is
`f64` -- no dimensions, no units, no types.

## Design Decisions to Lock

These open questions from the aspect docs must be resolved before implementing Phase 0.

### From [01-computation-model](../01-computation-model.md)

- [ ] **Evaluation strategy for Phase 0:** Eager (evaluate all nodes in topological order).
      Lazy and incremental recomputation are optimizations for later.
- [ ] **Error propagation:** If a node fails (e.g., division by zero), does the
      entire evaluation fail, or do downstream nodes get an error value?
      Recommendation: fail-fast for Phase 0, revisit when `Option`/`Result` types exist.
- [ ] **Cycle detection:** Confirm that cycles produce a clear compile error with
      the cycle path printed.

### From [02-syntax-design](../02-syntax-design.md) (subset)

- [ ] **Semicolon rule:** Every top-level declaration (`param`, `node`, `const`)
      ends with `;`. Block bodies are deferred to Phase 2.
- [ ] **Numeric literals:** Support `_` separators (`200_000`) and scientific
      notation (`3.98e5`)?
- [ ] **Operator precedence:** Define precedence for `+`, `-`, `*`, `/`, `^`, unary `-`.
      Does `^` bind tighter than unary `-`? (i.e., is `-x^2` parsed as `-(x^2)` or `(-x)^2`?)
- [ ] **Comments:** `//` line comments. `///` doc comments deferred?
- [ ] **Built-in functions:** Which math functions are built-in? At minimum:
      `sqrt`, `exp`, `ln`, `abs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan2`,
      `min`, `max`, `floor`, `ceil`. Also `pi` and `e` as built-in constants.

### From [08-scoping](../08-scoping.md)

- [ ] **`@` in single-expression nodes:** In `node x = @a + @b;`, every reference
      is `@`-prefixed. There are no local variables in Phase 0 (no `let`), so bare
      names are always errors. Confirm this.
- [ ] **Duplicate name detection:** `param x = 1; node x = 2;` should be a compile error.

### From [17-error-messages](../17-error-messages.md) (minimal)

- [ ] **Error format:** Use Rust-style diagnostics with file, line, column, and caret?
- [ ] **Error codes:** Assign codes now, or defer to later?

## Syntax Supported in Phase 0

```ebnf
File         = Declaration*
Declaration  = ParamDecl | NodeDecl | ConstDecl | Comment
ParamDecl    = "param" IDENT "=" Expr ";"
NodeDecl     = "node"  IDENT "=" Expr ";"
ConstDecl    = "const" IDENT "=" Expr ";"
Comment      = "//" <anything until newline>

Expr         = Conditional | OrExpr
Conditional  = "if" Expr "{" Expr "}" "else" "{" Expr "}"
OrExpr       = AndExpr ("||" AndExpr)*
AndExpr      = CompExpr ("&&" CompExpr)*
CompExpr     = AddExpr (("<" | ">" | "<=" | ">=" | "==" | "!=") AddExpr)?
AddExpr      = MulExpr (("+" | "-") MulExpr)*
MulExpr      = UnaryExpr (("*" | "/") UnaryExpr)*
UnaryExpr    = ("-" | "!") UnaryExpr | PowerExpr
PowerExpr    = Atom ("^" UnaryExpr)?     // right-associative
Atom         = NUMBER | GRAPH_REF | FnCall | "(" Expr ")" | BOOL
GRAPH_REF    = "@" IDENT
FnCall       = IDENT "(" (Expr ("," Expr)*)? ")"
NUMBER       = [0-9]+ ("." [0-9]+)? ([eE] [+-]? [0-9]+)?
BOOL         = "true" | "false"
IDENT        = [a-zA-Z_][a-zA-Z0-9_]*
```

### What is NOT in the grammar yet

- Type annotations (`: Type`)
- Units after literals (`400 km`)
- Block bodies (`{ let ...; expr }`)
- `type`, `dimension`, `unit`, `space`, `index`, `table`, `fn`
- `use`, `private`
- Multi-file

## Implementation Scope

| Component | Description |
| --- | --- |
| **Parser** | `.graph` file -> AST (single file) |
| **Name resolver** | Resolve `@` references, detect duplicates, detect cycles |
| **DAG builder** | AST -> `petgraph` DAG |
| **Evaluator** | Topological sort, evaluate `f64` arithmetic |
| **CLI** | `cellgraph eval <file>` prints all node values |
| **Error reporter** | Parse errors, unknown references, cycles |

## Out of Scope

Everything not listed above. Specifically:

- Dimensions, units, type annotations
- Structs, tagged unions
- Multi-line node bodies, `let` bindings
- Functions (`fn`)
- Multi-file, imports, prelude
- Tables
- Scenarios
- TUI, web UI
- Python interop
- Spreadsheet import/export

## Milestone Test

```rust
// rocket.graph
param dry_mass = 1200.0;
param fuel_mass = 2800.0;
param isp = 320.0;
const G0 = 9.80665;

node v_exhaust = @isp * @G0;
node mass_ratio = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v = @v_exhaust * ln(@mass_ratio);
```

```
$ cellgraph eval rocket.graph
dry_mass     = 1200
fuel_mass    = 2800
isp          = 320
G0           = 9.80665
v_exhaust    = 3138.128
mass_ratio   = 3.333333
delta_v      = 3783.277
```

### Error cases that must work

```rust
// error: unknown reference
node x = @nonexistent + 1;

// error: cycle
node a = @b + 1;
node b = @a + 1;

// error: duplicate name
param x = 1;
node x = 2;
```

## Open Questions

- [ ] Should `const` be evaluated at "compile time" (before the DAG), or is it
      just a node that can't be overridden? For Phase 0 the distinction doesn't
      matter, but it affects later phases (e.g., can a `const` reference a `param`?).
- [ ] Should the output format be customizable (JSON, table, etc.), or is plain text
      sufficient for Phase 0?
- [ ] Should `cellgraph eval` accept `--set param=value` CLI overrides even in Phase 0,
      or defer that to Phase 6 (scenarios)?
- [ ] Naming: is the CLI command `cellgraph` or `kasuri`?
