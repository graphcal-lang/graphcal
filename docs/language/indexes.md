---
icon: material/format-list-numbered
---

# Indexes

Indexes are finite label sets used for collections of values. They enable typed, dimension-safe operations over multiple related values.

## Finite Label Indexes

Declare a finite index with named labels:

```
index Maneuver = { Departure, Correction, Insertion };
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

For nat range indexes, you can also use integer expressions as index arguments:

```
node shifted: Dimensionless[3] = for i: range(3) { @v[i + 1] };
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

Values can be indexed by multiple label indexes using tuple keys:

```
index Phase = { Launch, Cruise, Arrival };

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

## Mixed Label and Range Indexes

Values can be indexed by a combination of label indexes and range indexes. This is useful when you have categorical axes (e.g., maneuver types) combined with continuous axes (e.g., time steps).

### Construction via `for` Comprehension

The most common way to create a mixed-index value is with a multi-binding `for` comprehension:

```
index Maneuver = { Departure, Correction, Insertion };
index TimeStep = linspace(0.0 s, 1.0 s, step: 0.5 s);

param accel: Acceleration[Maneuver] = {
    Maneuver::Departure: 10.0 m/s^2,
    Maneuver::Correction: 5.0 m/s^2,
    Maneuver::Insertion: -3.0 m/s^2,
};

node v: Velocity[Maneuver, TimeStep] = for m: Maneuver, t: TimeStep {
    @accel[m] * t
};
```

### Construction via Map Literal with `for` Values

You can also use a map literal where the keys are label index variants and each value is a `for` comprehension over a range index:

```
param v: Velocity[Maneuver, TimeStep] = {
    Maneuver::Departure: for t: TimeStep { @accel[Maneuver::Departure] * t },
    Maneuver::Correction: for t: TimeStep { @accel[Maneuver::Correction] * t },
    Maneuver::Insertion: for t: TimeStep { @accel[Maneuver::Insertion] * t },
};
```

### Mixed-Index Element Access

Access elements by providing both a label and a range variable:

```
node departure_v: Velocity[TimeStep] = for t: TimeStep {
    @v[Maneuver::Departure, t]
};
```

### Aggregation

Aggregate over either axis independently:

```
// Sum over the label axis for each time step
node total_v: Velocity[TimeStep] = for t: TimeStep {
    sum(for m: Maneuver { @v[m, t] })
};

// Max over the time axis for each maneuver
node max_v: Velocity[Maneuver] = for m: Maneuver {
    max(for t: TimeStep { @v[m, t] })
};
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
index TimeStep = linspace(0.0 s, 1.0 s, step: 0.5 s);
```

This creates an index with elements at `0.0 s`, `0.5 s`, and `1.0 s`.

## Nat Range Indexes

Integer literals in index position create anonymous **nat range** indexes. These are useful for vectors, matrices, and other fixed-size numeric arrays:

```
// A 3-element vector
param v: Dimensionless[3] = for i: range(3) { 1.0 };

// A 2x3 matrix
param m: Dimensionless[2, 3] = for i: range(2), j: range(3) { 1.0 };
```

The integer `3` in `Dimensionless[3]` internally creates an anonymous index `range(3)` with elements `{0, 1, 2}`.

### Iterating over Nat Ranges

Use `for i: range(N)` to iterate over a nat range index:

```
node doubled: Dimensionless[3] = for i: range(3) { @v[i] * 2.0 };
```

The loop variable `i` has type `Int` and can be used to index into nat-range-indexed values.

### Generic Functions with Nat Parameters

Functions can be generic over nat range sizes with `N: Nat`:

```
fn transpose<M: Nat, N: Nat, D: Dim>(a: D[M, N]) -> D[N, M] =
    for j: range(N), i: range(M) { a[i, j] };

fn dot<N: Nat, D1: Dim, D2: Dim>(a: D1[N], b: D2[N]) -> D1 * D2 =
    sum(for i: range(N) { a[i] * b[i] });
```

When calling a generic function, `Nat` parameters are inferred from the argument shapes. Two nat ranges are equal if and only if their sizes are equal — `range(3)` and `range(4)` are different indexes.

### Nat Arithmetic (Addition)

`Nat` expressions support addition, enabling functions that relate input and output sizes:

```
// drop_last: takes a vector of size N + 1, returns a vector of size N
fn drop_last<N: Nat, D: Dim>(v: D[N + 1]) -> D[N] =
    for i: range(N) { v[i] };

param v4: Dimensionless[4] = for i: range(4) { 1.0 };
node v3: Dimensionless[3] = drop_last(@v4);
// The compiler solves N + 1 = 4 to deduce N = 3
```

Addition works in both type position (`D[N + 1]`) and for-range bindings (`range(N + 1)`):

```
fn pad_zero<N: Nat>(v: Dimensionless[N]) -> Dimensionless[N + 1] =
    for i: range(N + 1) { if i < N { v[i] } else { 0.0 } };
```

`Nat` expressions are normalized to a canonical form and compared structurally. Subtraction is not supported — instead, express the larger side with addition (e.g., `D[N + 1]` instead of `D[N - 1]`).

### Nat Arithmetic (Multiplication)

`Nat` expressions also support multiplication, enabling functions that relate sizes through products — useful for reshape, flatten, and Kronecker product patterns:

```
// Flatten a matrix into a vector
fn flatten<M: Nat, N: Nat, D: Dim>(a: D[M, N]) -> D[M * N] =
    for k: range(M * N) { a[k / N, k % N] };

param mat: Dimensionless[2, 3] = for i: range(2), j: range(3) { 1.0 };
node flat: Dimensionless[6] = flatten(@mat);
// The compiler solves M = 2, N = 3 from the argument, then M * N = 6
```

Multiplication binds tighter than addition, so `M + N * P` is parsed as `M + (N * P)`. Mixed expressions are normalized to canonical polynomial form:

```
// Flatten and pad with a trailing zero
fn flatten_and_pad<M: Nat, N: Nat>(a: Dimensionless[M, N]) -> Dimensionless[M * N + 1] =
    for k: range(M * N + 1) {
        if k < M * N { a[k / N, k % N] } else { 0.0 }
    };
```

### Expression-Based Indexing

Index arguments can be arbitrary integer expressions, not just loop variables. This enables patterns like finite differences where you need to access adjacent elements:

```
// Finite differences: values[i + 1] - values[i]
fn diff<N: Nat, D: Dim>(values: D[N + 1], dt: Time) -> (D / Time)[N] =
    for i: range(N) { (values[i + 1] - values[i]) / dt };
```

The compiler statically verifies bounds when possible. In the example above, `i` has type `Fin(N)` (values in `[0, N)`), so `i + 1` is guaranteed to be less than `N + 1` — matching the size of `values`.

Expression-based indexing supports:

- **Addition with literals**: `v[i + 1]`, `v[i + 2]` — bounds are checked at compile time
- **Arbitrary integer expressions**: `v[some_expr]` — evaluated at runtime

```
// Shift left by k positions
fn shift_left<N: Nat, D: Dim>(v: D[N + 1]) -> D[N] =
    for i: range(N) { v[i + 1] };

// Shift left by 2 positions
fn shift_left_by2<N: Nat, D: Dim>(v: D[N + 2]) -> D[N] =
    for i: range(N) { v[i + 2] };
```

### Composing Nat Ranges with Named Indexes

Nat range indexes compose freely with named indexes:

```
index Phase = { Launch, Cruise };

param data: Dimensionless[3, Phase] = for i: range(3), p: Phase { 1.0 };
```

## Required Indexes

An index can be declared **without** specifying its variants or range values. These are **required indexes** — they must be bound via a [parameterized import](multi-file.md#index-bindings) when the file is used as a library.

### Required Named Index

```
index Phase;
```

This declares a named index `Phase` with no variants. A file importing this library must bind it to a concrete named index.

### Required Range Index

```
index Step: Time;
```

This declares a range index `Step` constrained to have dimension `Time`. The importer must bind it to a concrete range index with the same dimension.

### Using Required Indexes

Required indexes are used exactly like concrete indexes — in type annotations, `for` comprehensions, index access, `match`, and map/table literals:

```
index Phase;

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
