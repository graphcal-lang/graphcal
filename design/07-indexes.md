# Type System -- Indexes

> Layer 6: Finite label sets used as table axes.

## Status

**Decision level:** Mostly settled in concept. Details around operations and composition need work.

## Summary

Indexes are finite label sets used as table axes (dimensions in the xarray sense, not physical dimensions). They are distinct from dimensions (which describe physical quantities), spaces (which tag semantic contexts), and tagged unions (which are algebraic data types).

## Syntax

```rust
index Region { LEO, GTO, Lunar, Mars }
index Fuel { LH2, Methane, Hydrazine }
index Quarter { Q1, Q2, Q3, Q4 }
index MissionPhase { PhaseA, PhaseB, PhaseC }
```

## Usage

Indexes are primarily used as table axes:

```rust
table mass_budget [region: Region, fuel: Fuel] {
    isp: SpecificImpulse,
    cost_per_kg: Money / Mass,
}
```

And for aggregation:

```rust
node cost_by_region [region] = mass_budget.cost_per_kg.sum(over: fuel);
```

## Distinction from Other Constructs

| Construct | Purpose | Size | Values |
| --- | --- | --- | --- |
| `index` | Table axis labels | Finite, fixed at compile time | Label identifiers |
| `type` (tagged union) | Algebraic data type | Finite variants | Each variant can carry data |
| `space` | Semantic context tag | Finite variants | Phantom type (no data) |
| `dimension` | Physical quantity | Continuous | Numeric with units |

The key difference from tagged unions: index variants carry no data. They are C-style enums, used purely for labeling table rows/columns.

## Open Questions

- **Dynamic indexes:** Can indexes be computed at runtime (e.g., loaded from a file)? Or are they always fixed at compile time? Dynamic indexes would be needed for data-driven tables.
- **Index arithmetic:** Can indexes be combined? E.g., `index Combined = Region * Fuel` for a Cartesian product?
- **Ordering:** Are index variants ordered? Can you iterate over them? Is there a `next` / `prev` operation?
- **Membership testing:** Can you test `region == Region.LEO` in expressions?
- **Subsets:** Can you define a subset of an existing index? E.g., `index InnerPlanets = Region.{ LEO, GTO }`.
- **String mapping:** How do index labels map to strings for display and I/O? Is it just the identifier name?
- **Range indexes:** The time axis uses `range(0, 200, step: 0.1)` which generates a numeric sequence rather than named labels. Is this a special kind of index, or something different?

## Dependencies on Other Aspects

- **Tables** ([10](./10-tables-and-autofill.md)): Indexes define table axes.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Indexes are simpler than tagged unions.
- **Syntax** ([02](./02-syntax-design.md)): Index declaration syntax.
- **Spreadsheet Compatibility** ([14](./14-spreadsheet-compatibility.md)): Indexes map to row/column headers.
