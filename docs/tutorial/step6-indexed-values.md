---
icon: material/numeric-6-circle
---

# Step 6: Indexed Values

In this step, you'll work with indexed collections to handle multiple related values, like a delta-v budget with several maneuvers.

## Defining an Index

An `index` declaration defines a finite set of labels:

```
index Maneuver = { Departure, Correction, Insertion };
```

## Indexed Values

Use `[IndexName]` to declare an indexed value:

```
node delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.12 km/s,
    Maneuver.Insertion: 1.83 km/s,
};
```

Each label in the index gets its own value.

## Direct Element Access

Access a specific element with `[Index.Label]`:

```
node departure_dv: Velocity = @delta_v[Maneuver.Departure];
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

Reduce a rank-one indexed quantity to a single quantity. `count` works with any
non-indexed element type and returns `Int`:

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node min_dv: Velocity = minimum(for m: Maneuver { @delta_v[m] });
node mean_dv: Velocity = mean(for m: Maneuver { @delta_v[m] });
node n_maneuvers: Int = count(for m: Maneuver { @delta_v[m] });
node normalized: Velocity = @total_dv / to_float(@n_maneuvers);
```

Available aggregation functions: `sum`, `maximum`, `minimum`, `mean`, `count`.
Use `to_float` explicitly when a scalar quantity calculation needs an integer
count. Direct multi-axis aggregation is not yet defined.

## Scan (Cumulative Fold)

`scan` computes a running accumulation across the index:

```
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
```

This produces:

- `Departure`: 2.46 km/s
- `Correction`: 2.58 km/s (2.46 + 0.12)
- `Insertion`: 4.41 km/s (2.58 + 1.83)

The accumulation follows the order in which the labels were declared in
`index Maneuver = { Departure, Correction, Insertion };` — not the order in
which a map literal happens to list its entries. Declare labels in a
meaningful sequence when you plan to `scan` over the axis.

A `scan` source has exactly one axis, but its accumulator may itself be an
indexed vector or matrix. In that case the source axis is prepended to the
accumulator axes in the result; see
[Indexed recurrence state](../language/indexes.md#indexed-recurrence-state).

## Complete Example

```
index Maneuver = { Departure, Correction, Insertion };

node delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.12 km/s,
    Maneuver.Insertion: 1.83 km/s,
};

node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};

node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node n_maneuvers: Int = count(@delta_v);
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
node departure_dv: Velocity = @delta_v[Maneuver.Departure];
```

## Try It in Your Browser

Indexed values render as expandable tables. Change one maneuver value and watch the aggregate and cumulative outputs update.

<div class="gc-playground-host">
  <graphcal-playground example="step-6">
    <p class="gc-playground-fallback">The editable playground requires JavaScript. Read the tutorial above or <a href="../assets/playground/examples/step-6/main.gcl">open the static example source</a>.</p>
  </graphcal-playground>
</div>

Expected initial output includes `total_dv = 4.41 km/s` and three cumulative entries.

## What You Learned

- **`index`** declarations for finite label sets
- **Indexed values** with `Type[Index]` syntax
- **`for` comprehensions** to transform each element
- **Aggregations**: `sum`, `maximum`, `minimum`, `mean`, `count`
- **`scan`** for cumulative folds
- **Aggregation functions** that work with any index

## What's Next?

Congratulations! You've completed the tutorial. You now know the core features of Graphcal.

For deeper understanding, explore the [Language Reference](../language/index.md) for formal documentation of all features, or check the [CLI Reference](../cli-reference.md) for all command-line options.
