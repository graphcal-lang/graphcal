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

Labels conventionally use `PascalCase` and are namespaced by the index: `Maneuver.Departure`.

!!! note "No empty indexes"
    A finite index must declare **at least one variant** — `index Empty = {};` is rejected by the parser. The same goes for `linspace` ranges: `start > end` and `step <= 0` are invalid. This is a deliberate design choice (issue #580): with no empty case ever reachable, aggregation builtins never face the "what is `mean` of nothing?" question, indexed values always have at least one element, and there are no NaN traps to remember. Model the absence at the boundary (e.g., guard with a separate `Bool` flag or split the dag) rather than collapsing the index to zero variants.

## Indexed Values

Annotate a type with `[IndexName]` to create an indexed value:

```
dim Velocity = Length / Time;

node delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.12 km/s,
    Maneuver.Insertion: 1.83 km/s,
};
```

Both `param` and `node` can be indexed.

## Element Access

Access a specific element with `[Index.Label]`:

```
node departure_dv: Velocity = @delta_v[Maneuver.Departure];
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

node spacecraft_mass: Mass[Phase, Maneuver] = {
    (Phase.Launch, Maneuver.Departure): 5000.0 kg,
    (Phase.Launch, Maneuver.Correction): 0.0 kg,
    (Phase.Launch, Maneuver.Insertion): 0.0 kg,
    (Phase.Cruise, Maneuver.Departure): 0.0 kg,
    (Phase.Cruise, Maneuver.Correction): 4500.0 kg,
    (Phase.Cruise, Maneuver.Insertion): 0.0 kg,
    (Phase.Arrival, Maneuver.Departure): 0.0 kg,
    (Phase.Arrival, Maneuver.Correction): 0.0 kg,
    (Phase.Arrival, Maneuver.Insertion): 4000.0 kg,
};
```

Access elements with multiple index arguments:

```
node launch_dep: Mass = @spacecraft_mass[Phase.Launch, Maneuver.Departure];
```

## Mixed Label and Range Indexes

Values can be indexed by a combination of label indexes and range indexes. This is useful when you have categorical axes (e.g., maneuver types) combined with continuous axes (e.g., time steps).

### Construction via `for` Comprehension

The most common way to create a mixed-index value is with a multi-binding `for` comprehension:

```
index Maneuver = { Departure, Correction, Insertion };
index TimeStep = linspace(0.0 s, 1.0 s, step: 0.5 s);

node accel: Acceleration[Maneuver] = {
    Maneuver.Departure: 10.0 m/s^2,
    Maneuver.Correction: 5.0 m/s^2,
    Maneuver.Insertion: -3.0 m/s^2,
};

node v: Velocity[Maneuver, TimeStep] = for m: Maneuver, t: TimeStep {
    @accel[m] * t
};
```

### Construction via Map Literal with `for` Values

You can also use a map literal where the keys are label index variants and each value is a `for` comprehension over a range index:

```
node v: Velocity[Maneuver, TimeStep] = {
    Maneuver.Departure: for t: TimeStep { @accel[Maneuver.Departure] * t },
    Maneuver.Correction: for t: TimeStep { @accel[Maneuver.Correction] * t },
    Maneuver.Insertion: for t: TimeStep { @accel[Maneuver.Insertion] * t },
};
```

### Mixed-Index Element Access

Access elements by providing both a label and a range variable:

```
node departure_v: Velocity[TimeStep] = for t: TimeStep {
    @v[Maneuver.Departure, t]
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

Labels in the table body are unqualified (`Departure` instead of `Maneuver.Departure`) since the index is declared in `table[...]`. Rows are terminated with `;`.

### 2D Table

```
param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
    : Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;
};
```

The last index becomes columns, the second-to-last becomes rows. The header row starts with `:` and lists the column labels, followed by data rows with `RowLabel: value, value, ...;`.

### 3D+ Table

For three or more indexes, use slice sections with qualified labels:

```
param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
    [Time.T1]
    : Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;

    [Time.T2]
    : Departure, Correction, Insertion;
    Launch:  4800.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4300.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    3800.0 kg;
};
```

Each `[SliceLabel]` section contains its own header row and data rows. Named slice labels use `Index.Variant` syntax (or `module.Index.Variant` when the index is imported); Nat range slice labels use `#N`.

### Nat Range Tables

`table[...]` also accepts integer literals to produce positional matrix literals backed by Nat range indexes. When an axis is a Nat range, its labels are implicit `#0, #1, ...` and are omitted in the body:

```
// 1D Nat range
param v: Dimensionless[3] = table[3] {
    1.0;
    2.0;
    3.0;
};

// 2D, both axes Nat range
param m: Dimensionless[2, 3] = table[2, 3] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};

// 2D, mixed: named columns, Nat range rows
param mixed: Dimensionless[2, Maneuver] = table[2, Maneuver] {
    : Departure, Correction;
    1.0, 2.0;
    3.0, 4.0;
};

// 3D with a Nat range slice axis
param m3d: Dimensionless[2, Phase, Maneuver] = table[2, Phase, Maneuver] {
    [#0]
    : Departure, Correction;
    Launch: 1.0, 2.0;
    Cruise: 3.0, 4.0;

    [#1]
    : Departure, Correction;
    Launch: 5.0, 6.0;
    Cruise: 7.0, 8.0;
};
```

Slice labels (all but the last two axes) always require an explicit marker -- `[Index.Variant]` for named axes or `[#N]` for Nat range axes.

The `table` expression is pure syntax sugar -- it desugars to a map literal at parse time.

## Multi-declarations

A **multi-declaration** is a single surface form that introduces N parallel `param` / `node` / `const node` declarations sharing the same row axis. It aligns values that belong together on the same row:

```
pub index Component = { ComponentA, ComponentB };

param      power_consumption: Power[Component],
param      duty_cycle:        Dimensionless[Component],
const node mass_per_unit:     Mass[Component]
  = table[Component, (_, _, _)] {
      :           _,       _,    _;
      ComponentA: 10.0 W,  0.5,  2.5 kg;
      ComponentB: 12.0 W,  1.0,  3.1 kg;
  };
```

- Each slot on the left-hand side is a full declaration: kind (`param` / `node` / `const node`), name, and type annotation.
- The `table[SharedAxis, (…)]` bracket declares the row axis followed by a parenthesized slot tuple. Each tuple entry is either `_` (1-D slot typed `T[SharedAxis]`) or a named axis, including module-qualified axes (2-D slot typed `T[SharedAxis, ExtraAxis]`).
- The header row `: …;` has exactly one cell per column. For 1-D slots the cell must be `_`; for 2-D slots, list the extra-axis variants in order (bare, e.g., `Safe, Nominal`, or qualified `OpMode.Safe`). Qualification is never required but is accepted for readability.

Mixed 1-D / 2-D slots:

```
pub index Component = { ComponentA, ComponentB };
pub index OperationMode = { Safe, Nominal };

param      power_consumption:  Power[Component],
param      n_installed:        Int[Component],
const node mass_per_unit:      Mass[Component],
param      power_mode_active:  Bool[Component, OperationMode]
  = table[Component, (_, _, _, OperationMode)] {
      :            _,       _, _,      Safe,  Nominal;
      ComponentA:  10.0 W,  1, 2.5 kg, true,  true;
      ComponentB:  12.0 W,  2, 3.1 kg, false, true;
  };
```

In v2, at most one slot may carry an extra axis; multiple adjacent extra-axis slots are planned for a later extension.

### N-D with slice sections

When the shared-axis prefix has more than one axis, the body uses slice sections. Each slice section begins with a `[Axis.Variant, …]` label covering every shared axis **except the last** (which becomes the row axis), followed by a header row and data rows as usual.

```
pub index Phase = { Launch, Cruise };
pub index Component = { ComponentA, ComponentB };
pub index OperationMode = { Safe, Nominal };

param      power_consumption: Power[Phase, Component],
param      power_mode_active: Bool[Phase, Component, OperationMode]
  = table[Phase, Component, (_, OperationMode)] {
      [Phase.Launch]
      :            _,       Safe,  Nominal;
      ComponentA:  5.0 W,   true,  false;
      ComponentB:  6.0 W,   false, false;

      [Phase.Cruise]
      :            _,       Safe,  Nominal;
      ComponentA:  10.0 W,  true,  true;
      ComponentB:  12.0 W,  false, true;
  };
```

Slice labels must qualify each shared axis in the declared order (`Phase.Launch`, not bare `Launch`), matching the convention used for single-decl 3D+ tables.

### Editor integration

Each slot in a multi-declaration is its own declaration for the purposes of navigation: `gotoDefinition`, `findReferences`, `rename`, and `hover` all land on the slot header, and each slot receives its own inlay hint at its name. The formatter preserves the multi-decl surface form on round-trip — it emits the original source slice verbatim rather than the N desugared single-decls. Cell-level inlay hints (projecting slot names into the header row of the source) and canonicalization of the multi-decl body remain future work.

- Multi-declarations are **pure syntactic sugar**: each slot desugars to an ordinary declaration with its own `table[SharedAxis] { … }` initializer. Cross-slot references work exactly as for any other declarations (`@other_slot[Variant]`).
- Attributes (`#[…]`) and visibility annotations (`pub` / `pub(bind)`) are not allowed on a multi-declaration or its slots in v1.

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

### Nat Parameters in DAG Blocks

DAG blocks can work with nat range indexed values. Two nat ranges are equal if and only if their sizes are equal -- `range(3)` and `range(4)` are different indexes.

### Nat Arithmetic (Addition)

`Nat` expressions support addition, enabling size relationships:

```
param v4: Dimensionless[4] = for i: range(4) { 1.0 };
node v3: Dimensionless[3] = for i: range(3) { @v4[i] };
```

`Nat` expressions are normalized to a canonical form and compared structurally. Subtraction is not supported -- instead, express the larger side with addition (e.g., `D[N + 1]` instead of `D[N - 1]`).

### Nat Arithmetic (Multiplication)

`Nat` expressions also support multiplication. Multiplication binds tighter than addition, so `M + N * P` is parsed as `M + (N * P)`. Mixed expressions are normalized to canonical polynomial form.

### Expression-Based Indexing

Index arguments can be arbitrary integer expressions, not just loop variables. This enables patterns like finite differences where you need to access adjacent elements:

```
// Finite differences: values[i + 1] - values[i]
param values: Velocity[4] = for i: range(4) { 1.0 m/s };
node diffs: Velocity[3] = for i: range(3) { @values[i + 1] - @values[i] };
```

The compiler statically verifies bounds when possible.

Expression-based indexing supports:

- **Addition with literals**: `v[i + 1]`, `v[i + 2]` -- bounds are checked at compile time
- **Arbitrary integer expressions**: `v[some_expr]` -- evaluated at runtime

### Composing Nat Ranges with Named Indexes

Nat range indexes compose freely with named indexes:

```
index Phase = { Launch, Cruise };

node data: Dimensionless[3, Phase] = for i: range(3), p: Phase { 1.0 };
```

## Required Indexes

An index can be declared **without** specifying its variants or range values. These are **required indexes** — they must be bound via a [parameterized import](multi-file.md#index-bindings) when the file is used as a library.

### Required Named Index

```
pub(bind) index Phase;
```

This declares a named index `Phase` with no variants. A file importing this library must bind it to a concrete named index.

Required indexes form the library's bindable interface and must carry
`pub(bind)` (see [Visibility and Bindability](multi-file.md#visibility-and-bindability)).
Omitting the annotation — or writing plain `pub` — is error `V002`.

### Required Range Index

```
pub(bind) index Step: Time;
```

This declares a range index `Step` constrained to have dimension `Time`. The importer must bind it to a concrete range index with the same dimension.

### Using Required Indexes

Required indexes are used exactly like concrete indexes — in type annotations, `for` comprehensions, index access, `match`, and map/table literals:

```
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

The file cannot be evaluated standalone. It must be imported with a binding that supplies a concrete index (see [Index Bindings](multi-file.md#index-bindings)).

## `unfold` (Recurrence Relations)

`unfold` computes values over a range index where each value depends on the previous:

```
node x: Dimensionless[TimeStep] = unfold(@x0, |prev_t, t| @x[prev_t] * (1.0 + @rate * (t - prev_t)));
```

This is useful for time-stepping simulations and discrete dynamic systems.

## Aggregation Over Any Index

Use aggregation functions directly on indexed values:

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
```

Built-in aggregation functions (`sum`, `min`, `max`, `mean`, `count`) work with any index type.
