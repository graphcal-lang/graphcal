---
icon: material/format-list-numbered
---

# Indexes

Indexes are finite label sets used for collections of values. They enable typed, dimension-safe operations over multiple related values.

## Finite Label Indexes

Declare a finite index with named labels:

```
index Maneuver = { Departure, Correction, Insertion }
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

## Range Indexes

Range indexes generate labels from numeric stepping:

```
index TimeStep = range(0.0 s, 1.0 s, step: 0.5 s);
```

This creates an index with elements at `0.0 s`, `0.5 s`, and `1.0 s`.

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
