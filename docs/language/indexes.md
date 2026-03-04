---
icon: material/format-list-numbered
---

# Indexes

Indexes are finite label sets used for collections of values. They enable typed, dimension-safe operations over multiple related values.

## Finite Label Indexes

Declare a finite index with named labels:

```
cat Maneuver { Departure, Correction, Insertion }
```

Labels follow `PascalCase` convention and are namespaced by the index: `Maneuver::Departure`.

## Indexed Values

Annotate a type with `[IndexName]` to create an indexed value:

```
dimension Velocity = Length / Time;

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};
```

Both `param` and `node` can be indexed.

## Element Access

Access a specific element with `[Index::Label]`:

```
node departure_dv: Velocity = @delta_v[Maneuver::Departure];
```

Or with a loop variable:

```
node doubled: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

## `for` Comprehensions

Transform each element of an indexed value:

```
node doubled: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

The result is a new indexed value with the same index.

## Aggregation Functions

Reduce an indexed comprehension to a single scalar:

| Function | Description | Result Type |
|----------|-------------|-------------|
| `sum(...)` | Sum of all elements | Same dimension as elements |
| `max(...)` | Maximum element | Same dimension as elements |
| `min(...)` | Minimum element | Same dimension as elements |
| `mean(...)` | Arithmetic mean | Same dimension as elements |
| `count(...)` | Number of elements | `Dimensionless` |

```
node total: Velocity = sum(for m: Maneuver { @delta_v[m] });
node largest: Velocity = max(for m: Maneuver { @delta_v[m] });
node n: Dimensionless = count(for m: Maneuver { @delta_v[m] });
```

## `scan` (Cumulative Fold)

`scan` computes a running accumulation across the index order:

```
node cumulative: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, val| acc + val);
```

Arguments:

1. The indexed value to scan over
2. The initial accumulator value
3. A closure `|acc, val| expr` that combines the accumulator with each element

The result is an indexed value where each element is the accumulated result up to and including that element.

## Multi-Indexed Values

Values can be indexed by multiple indexes using tuple keys:

```
cat Phase { Launch, Cruise, Arrival }

param spacecraft_mass: Mass[Phase, Maneuver] = {
    (Phase::Launch, Maneuver::Departure): 5000.0 kg,
    (Phase::Launch, Maneuver::Correction): 0.0 kg,
    (Phase::Launch, Maneuver::Insertion): 0.0 kg,
    (Phase::Cruise, Maneuver::Departure): 0.0 kg,
    (Phase::Cruise, Maneuver::Correction): 4500.0 kg,
    (Phase::Cruise, Maneuver::Insertion): 0.0 kg,
    (Phase::Arrival, Maneuver::Departure): 0.0 kg,
    (Phase::Arrival, Maneuver::Correction): 0.0 kg,
    (Phase::Arrival, Maneuver::Insertion): 4000.0 kg,
};
```

Access elements with multiple index arguments:

```
node launch_dep: Mass = @spacecraft_mass[Phase::Launch, Maneuver::Departure];
```

## Table Literals

For multi-indexed values, the `table` expression provides a spreadsheet-like layout that is easier to read:

### 1D Table

```
param delta_v: Velocity[Maneuver] = table[Maneuver] {
    Departure:  2.46 km/s;
    Correction: 0.12 km/s;
    Insertion:  1.83 km/s;
};
```

Labels in the table body are unqualified (`Departure` instead of `Maneuver::Departure`) since the index is declared in `table[...]`. Rows are terminated with `;`.

### 2D Table

```
param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
    Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;
};
```

The last index becomes columns, the second-to-last becomes rows. The first row lists column headers, followed by data rows with `RowLabel: value, value, ...;`.

### 3D+ Table

For three or more indexes, use slice sections with qualified labels:

```
param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
    [Time::T1]
    Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;

    [Time::T2]
    Departure, Correction, Insertion;
    Launch:  4800.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4300.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    3800.0 kg;
};
```

Each `[SliceLabel]` section contains its own column header row and data rows. Slice labels use `Index::Variant` syntax.

The `table` expression is pure syntax sugar -- it desugars to a map literal at parse time.

## Range Indexes

Range indexes generate labels from numeric stepping:

```
range TimeStep(0.0 s, 1.0 s, step: 0.5 s);
```

This creates an index with elements at `0.0 s`, `0.5 s`, and `1.0 s`.

## Required Indexes

A `cat` or `range` index can be declared **without** specifying its variants or range values. These are **required indexes** — they must be bound via a [parameterized import](multi-file.md#index-bindings) when the file is used as a library.

### Required Named Index

```
cat Phase;
```

This declares a named index `Phase` with no variants. A file importing this library must bind it to a concrete named index.

### Required Range Index

```
range Step: Time;
```

This declares a range index `Step` constrained to have dimension `Time`. The importer must bind it to a concrete range index with the same dimension.

### Using Required Indexes

Required indexes are used exactly like concrete indexes — in type annotations, `for` comprehensions, index access, `match`, and map/table literals:

```
cat Phase;

param cost: Dimensionless[Phase];
node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

The file cannot be evaluated standalone. It must be imported with a binding that supplies a concrete index (see [Index Bindings](multi-file.md#index-bindings)).

## `unfold` (Recurrence Relations)

`unfold` computes values over a range index where each value depends on the previous:

```
node x: Dimensionless[TimeStep] = unfold(@x0, |prev_t, t| {
    let dt = t - prev_t;
    @x[prev_t] * (1.0 + @rate * dt)
});
```

This is useful for time-stepping simulations and discrete dynamic systems.

## Index Generics

Functions can be generic over indexes:

```
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);

node total_dv: Velocity = total(@delta_v);
```

`<I: Index>` declares an index type parameter, allowing the function to work with any index.
