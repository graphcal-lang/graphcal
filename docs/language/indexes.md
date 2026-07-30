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

Named labels identify positions on an index axis. They are not values: use them
in index access, map/table keys, expected-fail keys, include index bindings, and
`match` patterns over named-index loop variables.

The label list is **ordered**: the sequence in which labels are declared is the
index order of the axis. Every index kind carries such an intrinsic order —
label declaration order here, coordinate order for
[coordinate indexes](#coordinate-indexes), and ascending positions for
[`Fin(N)`](#structural-finite-indexes). Order-sensitive operations, most
notably [`scan`](#scan-cumulative-fold), follow this order, and it also
determines rendering order and the element order of
[extern-function buffers](extern-functions.md).

!!! warning "Label order is semantic"
    `{ Departure, Correction, Insertion }` and
    `{ Insertion, Correction, Departure }` declare *different* indexes:
    `scan` accumulates in declared label order, so reordering the labels of an
    `index` declaration changes downstream results. Reorder labels only when
    the axis order itself should change.

!!! note "No empty indexes"
    Every index has at least one element. A named index must declare a label,
    `Fin(0)` is invalid, and coordinate constructors must produce at least one
    coordinate. Completed indexed values are therefore always nonempty. If an
    empty value reaches an aggregation internally, Graphcal reports `X001` as a
    compiler invariant violation; empty aggregation has no user-facing semantics.

!!! note "Contextual syntax names"
    `scan` and `unfold` select recurrence syntax only as bare call heads followed
    by `(`. `range`, `linspace`, `step`, and `points` are special only in
    coordinate-index declarations; `Fin` is special only in Index positions.
    Elsewhere these spellings remain ordinary identifiers.

## Indexed Values

Annotate a type with `[IndexName]` to create an indexed value:

```
node delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.12 km/s,
    Maneuver.Insertion: 1.83 km/s,
};
```

Both `param` and `node` can be indexed.

An indexed value is a **total map**: every label of the index must appear
exactly once, and the compiler rejects missing or duplicate entries. The
written order of map entries is presentation only — the constructed value is
normalized to the index order, so listing `Maneuver.Insertion` first produces
exactly the same value. Only the `index` declaration determines the order; a
map or table literal cannot override it.

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

For structural `Fin` indexes, use integer expressions as index arguments:

```
param v: Dimensionless[Fin(4)] = for i: Fin(4) { 1.0 };
node shifted: Dimensionless[Fin(3)] = for i: Fin(3) { @v[i + 1] };
```

## `for` Comprehensions

Transform each element of an indexed value:

```
node doubled: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

The result is a new indexed value with the same index.

For multi-axis comprehensions, the body may start with an explicit tuple-key
marker. The names are pure sugar and must exactly match the `for` bindings in
order; they do not introduce additional variables:

```
node v: Velocity[Maneuver, TimeStep] = for m: Maneuver, t: TimeStep {
    (m, t) => @accel[m] * t
};
```

## Aggregation Functions

Aggregation functions currently accept exactly one index axis. Numeric
aggregations require quantity elements. Most preserve the element dimension;
`product` raises it to the concrete axis cardinality. `count` accepts any
non-indexed element type and returns the exact cardinality as `Int`.

| Function | Description | Result Type |
|----------|-------------|-------------|
| `sum(values)` | Sum of all elements | Same dimension as elements |
| `product(values)` | Product of all elements | Element dimension raised to axis cardinality |
| `maximum(values)` | Maximum element | Same dimension as elements |
| `minimum(values)` | Minimum element | Same dimension as elements |
| `mean(values)` | Arithmetic mean | Same dimension as elements |
| `rss(values)` | Root sum square | Same dimension as elements |
| `count(values)` | Number of elements | `Int` |

```
node total: Velocity = sum(for m: Maneuver { @delta_v[m] });
node largest: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node combined_sigma: Velocity = rss(for m: Maneuver { @delta_v[m] });
node n: Int = count(for m: Maneuver { @delta_v[m] });
node normalized: Velocity = sum(@delta_v) / to_float(count(@delta_v));
```

`Int` and quantity arithmetic never mix implicitly. Use `to_float(count(...))`
when a scalar calculation needs the cardinality. A multi-axis value must first be
projected to a single axis with an explicit `for` comprehension (`D021`); total-
and partial-axis aggregation are not yet defined.

## `scan` (Cumulative Fold)

`scan` computes a running accumulation across the index order:

```
node cumulative: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
```

Arguments:

1. The indexed value to scan over
2. The initial accumulator value
3. A closure `|acc, item| expr` that combines the accumulator with each item

The result is an indexed value where each element is the accumulated result up to and including that element.

The accumulation order is the index order, which is intrinsic to the index —
never to how a particular value was written:

- **Named index**: label declaration order (`Departure`, then `Correction`,
  then `Insertion` for the `Maneuver` axis above).
- **Coordinate index**: coordinate order, from `start` toward `end`
  (descending when the step is negative).
- **`Fin(N)`**: ascending positions `#0` through `#(N-1)`.

Because indexed values are normalized to index order, scanning a map literal
that lists `Maneuver.Insertion` first still accumulates `Departure` first.

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

## Mixed Label and Coordinate Indexes

Values can combine categorical label axes with coordinate axes such as time.

### Construction via `for` Comprehension

The most common way to create a mixed-index value is with a multi-binding `for` comprehension:

```
index Maneuver = { Departure, Correction, Insertion };
index TimeStep = range(0.0 s, 1.0 s, step: 0.5 s);

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

You can also use a map literal where each named-label entry contains a
comprehension over a coordinate index:

```
node v: Velocity[Maneuver, TimeStep] = {
    Maneuver.Departure: for t: TimeStep { @accel[Maneuver.Departure] * t },
    Maneuver.Correction: for t: TimeStep { @accel[Maneuver.Correction] * t },
    Maneuver.Insertion: for t: TimeStep { @accel[Maneuver.Insertion] * t },
};
```

### Mixed-Axis Element Access

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
    maximum(for t: TimeStep { @v[m, t] })
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

Named axes in `table[...]` accept the same full paths as indexed types, including module-qualified paths such as `mission.Maneuver`. Labels in ordinary table bodies are bare (`Departure`, not `Maneuver.Departure`) because `table[...]` explicitly supplies exactly one owner for each row or column axis. Rows are terminated with `;`.

### 2D Table

```
param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
    : Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;
};
```

The last index becomes columns, the second-to-last becomes rows. The header row starts with `:` and lists the column labels, followed by data rows with `RowLabel: value, value, ...;`. Commas separate cells; the semicolon is the explicit row (second-dimension) delimiter, so do not add a redundant trailing comma before it.

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

Each `[SliceLabel]` section contains its own header row and data rows. Named
slice labels use `Index.Variant` syntax (or `module.Index.Variant` when the
index is imported); `Fin` slice labels use `#N`.

### Finite-Index Tables

Use explicit `Fin(N)` axes for positional vectors and matrices. Their labels are
`#0, #1, ...` and are omitted from ordinary rows and columns:

```
// 1D Fin axis
param v: Dimensionless[Fin(3)] = table[Fin(3)] {
    1.0;
    2.0;
    3.0;
};

// 2D, both axes finite
param m: Dimensionless[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};

// 2D, mixed: named columns, finite rows
param mixed: Dimensionless[Fin(2), Maneuver] = table[Fin(2), Maneuver] {
    : Departure, Correction;
    1.0, 2.0;
    3.0, 4.0;
};

// 3D with a finite slice axis
param m3d: Dimensionless[Fin(2), Phase, Maneuver] = table[Fin(2), Phase, Maneuver] {
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

Slice labels (all but the last two axes) always require an explicit marker:
`[Index.Variant]` for named axes or `[#N]` for `Fin` axes. The same conventions
apply to multi-declaration shared axes: a `Fin` row axis has unlabeled rows, and
a `Fin` slice axis uses `[#N]` sections.

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
- The header row `: …;` has exactly one cell per column. For 1-D slots the cell must be `_`. Every 2-D slot cell must use the qualified `ExtraAxis.Variant` form (including the full module path when imported), because one heterogeneous header can concatenate columns owned by different axes.
- Data-row labels remain bare because the shared row axis is explicit in the table prefix.

Mixed 1-D / 2-D slots:

```
pub index Component = { ComponentA, ComponentB };
pub index OperationMode = { Safe, Nominal };

param      power_consumption:  Power[Component],
param      n_installed:        Int[Component],
const node mass_per_unit:      Mass[Component],
param      power_mode_active:  Bool[Component, OperationMode]
  = table[Component, (_, _, _, OperationMode)] {
      :            _,       _, _,      OperationMode.Safe, OperationMode.Nominal;
      ComponentA:  10.0 W,  1, 2.5 kg,               true,                  true;
      ComponentB:  12.0 W,  2, 3.1 kg,              false,                  true;
  };
```

Currently, at most one slot may carry an extra axis; multiple adjacent extra-axis slots are planned for a later extension.

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
      :            _,       OperationMode.Safe, OperationMode.Nominal;
      ComponentA:  5.0 W,                 true,                 false;
      ComponentB:  6.0 W,                false,                 false;

      [Phase.Cruise]
      :            _,       OperationMode.Safe, OperationMode.Nominal;
      ComponentA:  10.0 W,                true,                  true;
      ComponentB:  12.0 W,               false,                  true;
  };
```

Slice labels must qualify each shared axis in the declared order (`Phase.Launch`, not bare `Launch`), matching the convention used for single-decl 3D+ tables.

### Editor integration

Each slot in a multi-declaration is its own declaration for the purposes of navigation: `gotoDefinition`, `findReferences`, `rename`, and `hover` all land on the slot header, and each slot receives its own inlay hint at its name. Axis and qualified header/slice references participate in navigation and rename as well. The formatter preserves the multi-decl surface form while canonicalizing alignment and retaining every required axis qualifier. Cell-level inlay hints (projecting slot names into the header row of the source) remain future work.

- Multi-declarations are **pure syntactic sugar**: each slot desugars to an ordinary declaration with its own `table[SharedAxis] { … }` initializer. Cross-slot references work exactly as for any other declarations (`@other_slot[Variant]`).
- Attributes (`#[…]`) are not allowed on a multi-declaration. Visibility is
  written per slot and follows the ordinary declaration rules (`pub node` is
  valid; `pub param` and `pub(bind) node` are not).

## Coordinate Indexes

Coordinate indexes carry statically known quantity coordinates. Bounds and
spacing must be finite, compile-time quantities with the same dimension.
Runtime inputs and integer (`Int`) values are not accepted.

### Exact-Step `range`

Use `range` when the increment is authoritative:

```
index TimeStep = range(0.0 s, 1.0 s, step: 0.25 s);
index Countdown = range(1.0 s, -1.0 s, step: -0.5 s);
```

The step must be nonzero and point from `start` toward `end`. It must land on
the endpoint exactly within floating-point validation tolerance; Graphcal does
**not** clip or overshoot. For example,
`range(0.0 s, 1.0 s, step: 0.6 s)` is rejected. Interior coordinates are derived
directly from `start + position * step`, avoiding cumulative drift, and the
validated final coordinate preserves the declared endpoint.

### Count-Based `linspace`

Use `linspace` when the number of points is authoritative:

```
index Samples = linspace(0.0 s, 1.0 s, points: 5);
```

This produces exactly five coordinates and preserves both declared endpoints.
`points` must be a static positive `Nat`. A singleton is valid only when the
endpoints are identical:

```
index Origin = linspace(0.0 m, 0.0 m, points: 1);
```

Both constructors support ascending and descending coordinates. Concrete index
cardinalities are limited to 1,000,000 elements to prevent accidental massive
allocations.

## Structural Finite Indexes

`Fin(N)` is an explicit structural index with integer positions `0` through
`N - 1`. It is distinct from the `Nat` value `N`: Graphcal never converts a
bare Nat into an Index implicitly.

```
// A 3-element vector
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };

// A 2-by-3 matrix
param m: Dimensionless[Fin(2), Fin(3)] =
    for i: Fin(2), j: Fin(3) { 1.0 };
```

Write `Fin(N)` in every Index position, including indexed types, `for`
bindings, tables, and Index-sorted generic arguments. Obsolete forms such as
`Dimensionless[3]`, `for i: range(3)`, and `table[3]` are rejected with a
`Fin(3)` suggestion. `Fin(0)` and cardinalities above the practical limit are
invalid.

### Generic Sorts Stay Distinct

A `Nat` argument remains a Nat, while `Fin(...)` is an Index argument:

```
type Vector<N: Nat, D: Dim> {
    Vector(values: D[Fin(N)]),
}

type IndexedVector<I: Index, D: Dim> {
    IndexedVector(values: D[I]),
}

param a: Vector<3, Dimensionless>;
param b: IndexedVector<Fin(3), Dimensionless>;
```

Consequently, `Vector<Fin(3), Dimensionless>` and
`IndexedVector<3, Dimensionless>` are sort errors rather than implicit
conversions.

### Nat Arithmetic and Iteration

The cardinality inside `Fin(...)` may use Nat addition and multiplication, such
as `Fin(N + 1)` or `Fin(Rows * Cols)`. Multiplication binds more tightly than
addition. Nat expressions are normalized to canonical polynomial form.
Subtraction is deliberately unsupported; express the larger side additively,
for example use `D[Fin(N + 1)]` for an input and `D[Fin(N)]` for a smaller
output.

A `Fin(N)` loop variable has the bounded integer type `Fin(N)` and evaluates
with an integer representation at runtime. It can index the corresponding
value:

```
node doubled: Dimensionless[Fin(3)] =
    for i: Fin(3) { @v[i] * 2.0 };
```

Integer index expressions are allowed and statically bounds-checked when
possible:

```
param values: Velocity[Fin(4)] = for i: Fin(4) { 1.0 m/s };
node diffs: Velocity[Fin(3)] =
    for i: Fin(3) { @values[i + 1] - @values[i] };
```

`Fin` axes compose with named indexes:

```
index Phase = { Launch, Cruise };
node data: Dimensionless[Fin(3), Phase] =
    for i: Fin(3), p: Phase { 1.0 };
```

## Required Indexes

An index can be declared **without** specifying its labels or coordinates. These
are **required indexes** — they must be bound via a
[parameterized import](multi-file.md#index-bindings) when the file is used as a
library.

### Required Named Index

```
pub(bind) index Phase;
```

This declares a named index `Phase` with no variants. A file importing this library must bind it to a concrete named index.

Required indexes form the library's bindable interface and must carry
`pub(bind)` (see [Visibility, Bindability, and Input Ports](multi-file.md#visibility-bindability-and-input-ports)).
Omitting the annotation — or writing plain `pub` — is error `V002`.

### Required Coordinate Index

```
pub(bind) index Step: Time;
```

This declares a coordinate index `Step` constrained to dimension `Time`. The
importer must bind it to a concrete `range` or `linspace` index with the same
dimension.

### Using Required Indexes

Required indexes are used exactly like concrete indexes — as axes in type annotations, `for` comprehensions, index access, `match`, and map/table literals:

```
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

The file cannot be evaluated standalone. It must be imported with a binding that supplies a concrete index (see [Index Bindings](multi-file.md#index-bindings)).

## `unfold` (Recurrence Relations)

`unfold` computes values over an explicit coordinate index where each value
depends on the previous state:

```
node x: Dimensionless[TimeStep] = unfold(
    TimeStep,
    @x0,
    |prev_x, prev_t, t| prev_x * (1.0 + @rate * (t - prev_t))
);
```

The first coordinate receives `@x0`. At every later coordinate, the body
receives the previous value, the previous coordinate, and the current
coordinate. The axis belongs to the expression; the enclosing declaration's
type is not consulted to select it, so `unfold` can be nested or consumed
immediately:

```
node total: Dimensionless = sum(
    unfold(TimeStep, @x0, |prev_x, prev_t, t| prev_x + @rate * (t - prev_t))
);
```

Use the `prev_x` binding directly when computing the next step. A reference to
`@x` in its own initializer is an ordinary dependency cycle, including
`@x[prev_t]`, `@x[t]`, and future-coordinate forms. This is useful for
time-stepping simulations and discrete dynamic systems.

## Aggregation Over Any Index

Use aggregation functions directly on indexed values:

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
```

Built-in aggregation functions (`sum`, `minimum`, `maximum`, `mean`, `count`) work with any index type.
