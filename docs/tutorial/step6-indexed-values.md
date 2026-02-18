---
icon: material/numeric-6-circle
---

# Step 6: Indexed Values

In this step, you'll work with indexed collections to handle multiple related values, like a delta-v budget with several maneuvers.

## Defining an Index

An `index` declares a finite set of labels:

```
index Maneuver = { Departure, Correction, Insertion }
```

## Indexed Parameters

Use `[IndexName]` to declare an indexed parameter:

```
dimension Velocity = Length / Time;

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};
```

Each label in the index gets its own value.

## Direct Element Access

Access a specific element with `[Index::Label]`:

```
node departure_dv: Velocity = @delta_v[Maneuver::Departure];
```

## `for` Comprehensions

Transform each element of an indexed value with `for`:

```
node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

This produces a new indexed value with each element doubled.

## Aggregations

Reduce an indexed value to a single scalar:

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = max(for m: Maneuver { @delta_v[m] });
node min_dv: Velocity = min(for m: Maneuver { @delta_v[m] });
node mean_dv: Velocity = mean(for m: Maneuver { @delta_v[m] });
node n_maneuvers: Dimensionless = count(for m: Maneuver { @delta_v[m] });
```

Available aggregation functions: `sum`, `max`, `min`, `mean`, `count`.

## Scan (Cumulative Fold)

`scan` computes a running accumulation across the index:

```
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, val| acc + val);
```

This produces:

- `Departure`: 2.46 km/s
- `Correction`: 2.58 km/s (2.46 + 0.12)
- `Insertion`: 4.41 km/s (2.58 + 1.83)

## Generic Functions with Index Constraints

Functions can be generic over indexes:

```
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);

node total_check: Velocity = total(@delta_v);
```

`<I: Index>` declares an index type parameter, similar to `<D: Dim>` for dimensions.

## Complete Example

```
dimension Velocity = Length / Time;

index Maneuver = { Departure, Correction, Insertion }

param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.12 km/s,
    Maneuver::Insertion: 1.83 km/s,
};

node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};

node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = max(for m: Maneuver { @delta_v[m] });
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, val| acc + val);
node departure_dv: Velocity = @delta_v[Maneuver::Departure];
```

## What You Learned

- **`index`** declarations for finite label sets
- **Indexed values** with `Type[Index]` syntax
- **`for` comprehensions** to transform each element
- **Aggregations**: `sum`, `max`, `min`, `mean`, `count`
- **`scan`** for cumulative folds
- **Index generics** with `<I: Index>` in functions

## What's Next?

Congratulations! You've completed the tutorial. You now know the core features of Graphcal.

For deeper understanding, explore the [Language Reference](../language/index.md) for formal documentation of all features, or check the [CLI Reference](../cli-reference.md) for all command-line options.
