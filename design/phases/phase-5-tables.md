# Phase 5: 1D Tables

> Single-axis tables with map, reduce, scan.

## Goal

Prove that spreadsheet-like tabular data works: declare a table schema,
populate it with rows, compute derived columns (map), aggregate (reduce),
and accumulate (scan). This is the feature that makes Cellgraph feel
like a spreadsheet, not just a calculator.

## Prerequisites

Phases 0-4 must be complete. Tables use dimensions (Phase 1), structs
(Phase 2), functions (Phase 3), and can span files (Phase 4).

## Design Decisions to Lock

### From [10-tables-and-autofill](../10-tables-and-autofill.md) (1D subset)

- [ ] **Table declaration syntax:** `table name { col: Type, ... }` declares a schema.
      Is this a type declaration (like `type`) or a separate construct?
- [ ] **Table population:** `param name: schema = [ ... ];` to populate with literal data.
      Tuple syntax for rows: `("Departure", 2.46 km/s, 300 s)`.
      Or struct syntax: `{ name: "Departure", delta_v: 2.46 km/s, duration: 300 s }`?
- [ ] **Table as param vs node:** Can a table be a computed node (all rows generated),
      or only a param (user-supplied data)?
- [ ] **Row identity:** Are rows identified by position (index 0, 1, 2, ...) or by
      a key column?
- [ ] **Dynamic row count:** Is the number of rows fixed at parse time, or can rows
      be added at runtime?

### Column Expressions (Map)

- [ ] **Syntax:** `node table.new_col: Type = <expr using row>;`
      The `row` variable gives access to the current row's fields.
- [ ] **`row` binding:** Is `row` a keyword, a special variable, or an implicit parameter?
      Does it need `@`? (Recommendation: no `@` -- `row` is local to the expression.)
- [ ] **Column ordering:** Can column B reference column A defined in the same table?
      E.g., `node t.fuel = ...;` then `node t.cost = row.fuel * @price;`.
      This creates dependencies between column expressions -- must be topologically ordered.
- [ ] **Graph dependency:** A column expression like `node t.fuel = row.dv / @v_exhaust * @mass`
      creates a graph edge from `v_exhaust` and `mass` to the column. Is the column itself
      a single node in the DAG, or one node per row?

### Aggregation (Reduce)

- [ ] **Syntax:** `node total = table.column.sum();`
      What aggregation functions are built-in? `sum`, `min`, `max`, `mean`, `count`, `last`, `first`?
- [ ] **Return type:** `sum()` on a `Mass` column returns `Mass`. `count()` returns `i64`.
      `mean()` returns the same dimension. Confirm dimensional semantics for each.
- [ ] **Empty table:** What does `sum()` return for an empty table? `0`? Error?

### Running Totals (Scan)

- [ ] **Syntax:** `node table.col = scan(init, |acc, row| expr);`
      `acc` is the accumulator, `row` is the current row. `init` is the initial value.
- [ ] **Lambda syntax:** `|acc, row| expr` for single expression,
      `|acc, row| { let ...; expr }` for multi-line. Confirm.
- [ ] **`@` in scan lambdas:** `@` references graph nodes inside the lambda. Confirm.
- [ ] **Type of `acc`:** Must match the return type. `scan(0.0 m/s, |acc, row| acc + row.dv)`
      has type `Velocity` because `acc` starts as `Velocity`.

### Primitives extension

- [ ] **`Str` type:** Tables need string columns for names/labels. Add `Str` to
      the primitive set. String literals: `"double-quoted"`.
- [ ] **`i64` type:** May be needed for counts. Implicit `i64` -> `f64` conversion?
- [ ] **`bool` type:** May be needed for filter-like columns. Already in Phase 0 grammar.

## Syntax Supported in Phase 5

Everything from Phase 4, plus:

```ebnf
// Table schema declaration (no trailing ;)
TableDecl    = "table" IDENT "{" FieldList "}"

// Table population (as param)
ParamDecl    = "param" IDENT ":" IDENT "=" "[" RowList "]" ";"
RowList      = (RowLiteral ",")* RowLiteral ","?
RowLiteral   = "(" ExprList ")"
ExprList     = (Expr ",")* Expr ","?

// Column expression (map)
ColNodeDecl  = "node" IDENT "." IDENT (":" TypeExpr)? "=" Expr ";"

// Aggregation
AggExpr      = IDENT "." IDENT "." AGG_FN "()"
AGG_FN       = "sum" | "min" | "max" | "mean" | "count" | "first" | "last"

// Scan
ScanExpr     = "scan" "(" Expr "," Lambda ")"
Lambda       = "|" IDENT "," IDENT "|" (Expr | Block)

// Row field access
RowAccess    = "row" "." IDENT

// String literal
STRING       = '"' <characters> '"'
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Table schema registry** | Store table definitions (column names + types) |
| **Table literal parser** | Parse row tuples, type-check against schema |
| **Column expression evaluator** | Apply expression to each row (map) |
| **Aggregation functions** | sum, min, max, mean, count, first, last |
| **Scan evaluator** | Sequential accumulation with lambda |
| **Row scope** | `row.field` resolution within column expressions |
| **Table-aware DAG** | Column expressions as nodes that depend on table data + graph nodes |
| **`Str` primitive** | String type, string literals, basic operations |

## Out of Scope

- Multi-dimensional tables (N-dim with indexes) -- Phase 7
- Table joins
- Filtering / where clauses
- Dynamic row insertion/removal
- Table rendering (TUI) -- Phase 11

## Milestone Test

```rust
table maneuvers {
    name: Str,
    delta_v: Velocity,
    duration: Time,
}

param maneuvers: maneuvers = [
    ("Departure",  2.46 km/s, 300 s),
    ("Correction", 0.05 km/s,  60 s),
    ("Insertion",  1.48 km/s, 240 s),
];

param dry_mass: Mass = 1200 kg;
param isp: SpecificImpulse = 320 s;
node v_exhaust: Velocity = @isp * @G0;

// Map: compute a new column
node maneuvers.fuel: Mass = @dry_mass * (exp(row.delta_v / @v_exhaust) - 1.0);

// Scan: cumulative delta-v
node maneuvers.cum_dv: Velocity = scan(0.0 m/s, |acc, row| acc + row.delta_v);

// Reduce: total fuel
node total_fuel: Mass = maneuvers.fuel.sum();
node max_dv: Velocity = maneuvers.delta_v.max();
```

```
$ cellgraph eval mission/
maneuvers:
  | name       | delta_v   | duration | fuel      | cum_dv    |
  | Departure  | 2.46 km/s | 300 s    | 1523.7 kg | 2.46 km/s |
  | Correction | 0.05 km/s |  60 s    |   19.3 kg | 2.51 km/s |
  | Insertion  | 1.48 km/s | 240 s    |  735.1 kg | 3.99 km/s |

total_fuel = 2278.1 kg
max_dv     = 2.46 km/s
```

## Open Questions

- [ ] How should tables be displayed in CLI output? The above shows a formatted table,
      but is this the default or opt-in?
- [ ] Can a column expression reference another computed column of the same table?
      E.g., `node t.cost = row.fuel * @price;` where `fuel` is also computed.
      This requires ordering column expressions in the DAG.
- [ ] Should there be a way to reference a specific row? E.g., `maneuvers[0].delta_v`
      or `maneuvers["Departure"].delta_v`?
