# Tables and Autofill

> First-class N-dimensional labeled tables with map/scan/reduce.

## Status

**Decision level:** Partially settled. Core concepts established, but syntax details and multi-dimensional semantics need refinement.

## Summary

Tables are typed, named collections with column schemas -- the analog of Excel tables or named ranges. Three primitives -- **map**, **reduce**, **scan** -- cover the full range of spreadsheet autofill patterns.

## Table Declaration

```rust
table maneuvers {
    name: Str,
    delta_v: Velocity,
    duration: Time,
}
```

Tables can be populated as parameters:

```rust
param maneuvers: maneuvers = [
    ("Departure burn", 1.2 km/s, 300 s),
    ("Correction 1",  0.05 km/s, 60 s),
    ("Plane change",  0.8 km/s, 180 s),
];
```

## Column Expressions (Map)

Like dragging a formula down a column in a spreadsheet:

```rust
node maneuvers.fuel_mass: Mass = row.delta_v / @v_exhaust * @dry_mass;
node maneuvers.fuel_cost: Money = row.fuel_mass * @fuel_price_per_kg;
```

## Aggregations (Reduce)

```rust
node total_fuel: Mass = maneuvers.fuel_mass.sum();
node max_delta_v: Velocity = maneuvers.delta_v.max();
```

## Running Totals (Scan)

```rust
node maneuvers.cumulative_dv: Velocity = scan(0.0 m/s, |acc, row| acc + row.delta_v);
```

## Multi-Dimensional Tables

### Explicit Dimensions via Indexes

```rust
index Region { LEO, GTO, Lunar }
index Fuel { LH2, Methane }

table mass_budget [region: Region, fuel: Fuel] {
    isp: SpecificImpulse,
    cost_per_kg: Money / Mass,
}
```

### Aggregation Across Dimensions

```rust
// Sum across fuels -> 1D table indexed by maneuver
node fuel_by_maneuver [maneuver]: Mass =
    mass_budget.fuel_mass.sum(over: fuel);

// Sum across everything -> scalar
node total_fuel: Mass = mass_budget.fuel_mass.sum();
```

### Dimensional Type System for Tables

```
Scalar           -- f64, Mass, Velocity, etc.
Table<[A]>       -- 1D, indexed by A
Table<[A, B]>    -- 2D, indexed by A x B
Table<[A, B, C]> -- 3D, etc.
```

The compiler enforces axis consistency in aggregations.

## Auto-Rendering

The live view automatically picks the best visualization based on dimensionality:

- **1D**: Simple table
- **2D**: Matrix / heatmap
- **3D**: Matrix with slice selector
- **4D+**: Multiple slice selectors

## Open Questions

- **Table as param vs node:** The design shows `param maneuvers` for data input and `node maneuvers.fuel_mass` for computed columns. Can an entire table be a computed node (e.g., generated from a function)?
- **Row identity:** How are individual rows referenced? By index position? By a key column?
- **Dynamic row count:** Can rows be added/removed at runtime? Or is the table shape fixed at compile time?
- **Cross-table references:** How does a column expression in one table reference values from another table? E.g., `@maneuvers[row.maneuver].delta_v`.
- **Table joins:** Is there a join operation for combining tables on shared indexes?
- **Missing values:** How are missing values handled in multi-dimensional tables? If not every `(region, fuel)` combination has data, is that an error or `Option`?
- **Column ordering:** Does the order of column expressions matter? Can column B reference column A defined in the same map pass?
- **Filtering:** Can you filter a table (e.g., `maneuvers.where(row.delta_v > 1 km/s)`)? Or is this out of scope?
- **`row` binding:** The `row` variable in column expressions is implicit/magical. Should it be more explicit? How does it interact with the `@` scoping rule?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Tables are collections of nodes in the DAG.
- **Indexes** ([07](./07-indexes.md)): Indexes define table axes.
- **Dimensions** ([04](./04-dimensions-and-units.md)): Table columns have dimensional types.
- **System Dynamics** ([11](./11-system-dynamics.md)): `scan` over time axis for simulation.
- **Live View** ([13](./13-live-view.md)): Tables are rendered as grids.
- **Spreadsheet Compatibility** ([14](./14-spreadsheet-compatibility.md)): Tables map to Excel tables/ranges.
