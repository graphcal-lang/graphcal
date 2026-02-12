# Phase 5: Indexed Values

> Type-level indexing with explicit `for` comprehensions, `sum`/`reduce`,
> and `scan`. No implicit broadcasting.

## Goal

Prove that labeled, multi-axis data works as a first-class part of the type
system. An **indexed value** `T[I]` is a total map from index labels to values
of type `T`. Combined with explicit `for` comprehensions, this replaces
spreadsheet-like tabular computation while being type-safe and unambiguous.

This phase subsumes the original "1D tables" and "N-dim tables" designs into
a single unified mechanism. There is no `table` keyword — "tables" are simply
co-indexed `param`/`node`/`const` declarations.

## Prerequisites

Phases 0-3 must be complete. Indexed values use dimensions (Phase 1),
structs (Phase 2), and functions (Phase 3). Phase 4 (multi-file) is
not required.

## Design Decisions — Locked

### Index Declaration

- [x] **`index` keyword:** `index Name = { Variant1, Variant2, ... }` declares a
      finite label set. Variants are PascalCase identifiers.
- [x] **Variant access:** `Index::Variant` using `::` path separator, consistent
      with module paths (Phase 4). Importable with `use Index::*` in Phase 4.
- [x] **No data payload:** Index variants carry no data. They are C-style enums
      used purely for labeling. Data-carrying variants are tagged unions (Phase 10).

### Indexed Types

- [x] **`T[I]` as a type:** An indexed type means "a value of type `T` for every
      label in index `I`." This is a total map — every label must have a value.
- [x] **Multi-axis:** `T[I, J]` is sugar for `T[I][J]` — a value of type `T` for
      every combination of `I` and `J` labels. Both notations are equivalent.
- [x] **Type annotations:** `param x: Velocity[Maneuver]`, `node y: Mass[A, B]`.
      The index is part of the type, not the declaration syntax.
- [x] **Scalars are unindexed:** A plain `Velocity` is not indexed. There is no
      implicit broadcasting — combining scalar and indexed values requires explicit
      `for` comprehension.

### Literal Syntax

- [x] **Map literal:** Indexed values are populated with `{ Index::Variant: expr, ... }`.
      All variants must be present (total map, no missing values).
- [x] **Nested literals:** Multi-axis values nest: `{ A::A1: { B::B1: ..., B::B2: ... }, ... }`.

### `for` Comprehension

- [x] **Explicit iteration:** All element-wise operations require `for`. No implicit
      broadcasting. `for i: I { expr }` produces a value of type `T[I]` where `T`
      is the type of `expr`.
- [x] **Multi-axis sugar:** `for i: I, j: J { expr }` is sugar for
      `for i: I { for j: J { expr } }`, producing `T[I, J]`.
- [x] **Loop variable:** The loop variable (e.g., `i`) is used to index into
      indexed values: `@x[i]`. It is a lowercase identifier.
- [x] **Scalar access inside `for`:** Scalar (unindexed) values like `@dry_mass`
      are accessed directly without indexing.
- [x] **Nesting:** `for` can nest arbitrarily. Inner `for` can reference outer
      loop variables.

### Indexing (Element Access)

- [x] **By label:** `@x[Index::Variant]` extracts a scalar from an indexed value.
- [x] **By loop variable:** `@x[i]` inside a `for i: I { ... }` extracts the
      element corresponding to the current label.
- [x] **Partial indexing:** `@x[Index::A1]` on a `T[A, B]` value produces `T[B]`.
      Fixing one axis yields a value indexed by the remaining axes.
- [x] **Multi-axis indexing:** `@x[Index::A1, Index::B2]` or `@x[i, j]` for
      full element access on multi-axis values.

### Reduction

- [x] **`sum()`:** `sum(indexed_expr)` collapses all axes, returning a scalar.
      Inside a `for`, `sum(for j: J { ... })` collapses only the `J` axis.
- [x] **Other aggregations:** `min()`, `max()`, `mean()`, `count()` follow the
      same pattern. `count()` returns `Dimensionless`.
- [x] **Dimensional semantics:** `sum()` on `Mass[I]` returns `Mass`.
      `mean()` on `Mass[I]` returns `Mass`. `min()`/`max()` preserve dimension.

### Scan

- [x] **Ordered accumulation:** `scan(indexed_expr, init, |acc, val| body)`
      produces an indexed value of the same shape. Processes elements in
      declaration order of the index variants.
- [x] **Type:** `scan` on `T[I]` with accumulator type `U` produces `U[I]`.
      The init value has type `U`.

### Functions with Indexed Types

- [x] **`Index` as generic constraint:** `fn total<D: Dim, I: Index>(values: D[I]) -> D`
      allows functions generic over both dimension and index.
- [x] **Concrete indexed params:** `fn f(x: Velocity[Maneuver]) -> Velocity` also works.
- [x] **`for` in function bodies:** Functions can use `for` comprehensions.

### Primitives Extension

- [x] **`Str` type:** String type for labels/names. String literals: `"double-quoted"`.
      Needed for `param name: Str[Maneuver] = { ... }`.

## Syntax Supported in Phase 5

Everything from Phase 3, plus:

```ebnf
// Index declaration
IndexDecl     = "index" UPPER_IDENT "=" "{" VariantList "}"
VariantList   = UPPER_IDENT ("," UPPER_IDENT)* ","?

// Indexed type annotation
TypeExpr      = BaseType ("[" IndexList "]")?
IndexList     = UPPER_IDENT ("," UPPER_IDENT)*

// Map literal (indexed value)
MapLiteral    = "{" MapEntry ("," MapEntry)* ","? "}"
MapEntry      = QualVariant ":" Expr
QualVariant   = UPPER_IDENT "::" UPPER_IDENT

// for comprehension
ForExpr       = "for" ForBindings "{" Expr "}"
ForBindings   = ForBinding ("," ForBinding)*
ForBinding    = LOWER_IDENT ":" UPPER_IDENT

// Indexing
IndexExpr     = Expr "[" IndexArg ("," IndexArg)* "]"
IndexArg      = QualVariant | LOWER_IDENT

// Aggregation
AggExpr       = AGG_FN "(" Expr ")"
AGG_FN        = "sum" | "min" | "max" | "mean" | "count"

// Scan
ScanExpr      = "scan" "(" Expr "," Expr "," Lambda ")"
Lambda        = "|" LOWER_IDENT "," LOWER_IDENT "|" (Expr | Block)

// String literal
STRING        = '"' <characters> '"'
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Index registry** | Store index definitions (name, ordered list of variants) |
| **Index variant resolution** | Resolve `Maneuver::Departure` to an index + variant |
| **Indexed type in type system** | `T[I]` as a type constructor; multi-axis `T[I, J]` |
| **Map literal parser** | Parse `{ Index::Variant: expr, ... }` with totality check |
| **`for` comprehension** | Parse and evaluate `for i: I { expr }` |
| **Indexing operator** | Parse and evaluate `expr[i]`, `expr[Index::Variant]` |
| **Aggregation functions** | `sum`, `min`, `max`, `mean`, `count` over indexed values |
| **Scan evaluator** | Sequential accumulation with lambda over indexed values |
| **Dimension checker updates** | Type-check indexed types, `for`, indexing, aggregations |
| **`Str` primitive** | String type, string literals |
| **`Index` generic constraint** | `<I: Index>` in function signatures |
| **CLI output** | Display indexed values as tables |

## Out of Scope

- Multi-file (Phase 4)
- Dynamic indexes (runtime-loaded label sets)
- Index arithmetic (Cartesian products, subsets)
- Table joins
- Filtering / where clauses
- Range indexes (numeric sequences for time axes — Phase 8)
- Tagged unions (Phase 10)
- Sparse indexed values / missing values / `Option<T>`
- `i64`, `bool`, `Datetime` primitives

## Milestone Test

```kasuri
index Maneuver = { Departure, Correction, Insertion }

dimension Velocity = Length / Time;
dimension SpecificImpulse = Time;

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.05 km/s,
    Maneuver::Insertion: 1.48 km/s,
}

param duration: Time[Maneuver] = {
    Maneuver::Departure: 300 s,
    Maneuver::Correction: 60 s,
    Maneuver::Insertion: 240 s,
}

param dry_mass: Mass = 1200 kg;
param isp: SpecificImpulse = 320 s;
const G0: Acceleration = 9.80665 m/s^2;
node v_exhaust: Velocity = @isp * @G0;

// for comprehension: compute fuel for each maneuver
node fuel: Mass[Maneuver] = for m: Maneuver {
    @dry_mass * (exp(@delta_v[m] / @v_exhaust) - 1.0)
}

// scan: cumulative delta-v
node cum_dv: Velocity[Maneuver] = scan(
    for m: Maneuver { @delta_v[m] },
    0.0 m/s,
    |acc, val| acc + val,
)

// reduce: total fuel and max delta-v
node total_fuel: Mass = sum(for m: Maneuver { @fuel[m] })
node max_dv: Velocity = max(for m: Maneuver { @delta_v[m] })
```

```text
$ kasuri eval mission.ksr
dry_mass  = 1200 kg
isp       = 320 s
G0        = 9.80665 m/s^2
v_exhaust = 3138.1 m/s

delta_v:
  | Maneuver   | delta_v   |
  | Departure  | 2.46 km/s |
  | Correction | 0.05 km/s |
  | Insertion  | 1.48 km/s |

duration:
  | Maneuver   | duration |
  | Departure  | 300 s    |
  | Correction | 60 s     |
  | Insertion  | 240 s    |

fuel:
  | Maneuver   | fuel      |
  | Departure  | 1523.7 kg |
  | Correction | 19.3 kg   |
  | Insertion  | 735.1 kg  |

cum_dv:
  | Maneuver   | cum_dv    |
  | Departure  | 2.46 km/s |
  | Correction | 2.51 km/s |
  | Insertion  | 3.99 km/s |

total_fuel = 2278.1 kg
max_dv     = 2.46 km/s
```

### Multi-axis example

```kasuri
index Row = { R1, R2 }
index Col = { C1, C2, C3 }

param P: Dimensionless[Row, Col] = {
    Row::R1: { Col::C1: 1.0, Col::C2: 2.0, Col::C3: 3.0 },
    Row::R2: { Col::C1: 4.0, Col::C2: 5.0, Col::C3: 6.0 },
}

// Sum over columns for each row
node row_sums: Dimensionless[Row] = for r: Row {
    sum(for c: Col { @P[r, c] })
}

// Transpose
node P_T: Dimensionless[Col, Row] = for c: Col, r: Row {
    @P[r, c]
}
```

### Matrix multiplication example

```kasuri
index I = { I1, I2 }
index J = { J1, J2, J3 }
index K = { K1, K2 }

param A: Dimensionless[I, J] = { ... }
param B: Dimensionless[J, K] = { ... }

// C[i, k] = sum_j(A[i, j] * B[j, k])
node C: Dimensionless[I, K] = for i: I, k: K {
    sum(for j: J { @A[i, j] * @B[j, k] })
}
```

### Generic function over indexed values

```kasuri
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);

fn dot<I: Index>(a: Dimensionless[I], b: Dimensionless[I]) -> Dimensionless =
    sum(for i: I { a[i] * b[i] });
```

### Error cases that must work

```kasuri
// error: missing variant in map literal
param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    // missing Correction and Insertion
}
//  error: missing variants `Maneuver::Correction`, `Maneuver::Insertion`

// error: indexing without for
node bad: Velocity[Maneuver] = @delta_v + @extra_dv;
//  error: cannot add Velocity[Maneuver] + Velocity[Maneuver];
//  use `for m: Maneuver { @delta_v[m] + @extra_dv[m] }` instead

// error: wrong index in indexing
node bad2: Velocity = @delta_v[Phase::Coast];
//  error: expected index `Maneuver`, got `Phase`

// error: unknown variant
node bad3: Velocity = @delta_v[Maneuver::Landing];
//  error: unknown variant `Landing` in index `Maneuver`
```

## Open Questions

- [ ] **CLI table grouping:** Should co-indexed values (e.g., `delta_v`, `duration`,
      `fuel` all indexed by `Maneuver`) be grouped into a single table in CLI output?
      Or displayed separately as shown in the milestone test?
- [ ] **Index ordering:** Are index variants ordered by declaration order? This matters
      for `scan` and for CLI display. (Recommendation: yes, declaration order.)
- [ ] **Struct-indexed values:** Can a struct be indexed? E.g.,
      `node result: TransferResult[Maneuver] = for m: Maneuver { ... }`.
      (Recommendation: yes, `T` in `T[I]` can be any value type.)
- [ ] **`for` in const expressions:** Can `const` use `for`? This would mean
      `const X: Dimensionless[I] = for i: I { ... }` — the index is known at
      compile time, so this is statically evaluable. (Recommendation: yes.)
- [ ] **Empty indexes:** Is `index Empty = {}` valid? If so, what does
      `sum(for e: Empty { ... })` return? (Recommendation: disallow empty indexes.)
- [ ] **Display units in indexed values:** Can elements of an indexed value have
      different display units? Or one display unit for the entire indexed value?
      (Recommendation: one display unit per indexed value.)

## Dependencies on Other Phases

- **Phase 1 (Dimensions):** Indexed values carry dimensional types.
- **Phase 2 (Structs):** Struct types can be indexed.
- **Phase 3 (Functions):** `<I: Index>` generic constraint, `for` in function bodies.
- **Phase 4 (Multi-file):** `use Index::*` to import variants.
- **Phase 7 → merged:** N-dim tables are now `T[I, J]`, not a separate phase.
- **Phase 8 (System Dynamics):** Time axis as a range index (extends this design).
- **Phase 9 (Spaces):** Orthogonal — `Velocity in ECI` and `Velocity[Maneuver] in ECI`.
- **Phase 11 (TUI):** `T[I]` → table, `T[I, J]` → matrix, `T[I, J, K]` → matrix + slicer.
