---
icon: material/format-list-bulleted-type
---

# Type System

This page is the formal reference for Graphcal's type system. It describes the three-level type stratification, the dimension algebra, typing rules for expressions, generics, and type equivalence.

For introductory material, see the [tutorial](../tutorial/index.md). For specific features, see [Dimensions & Units](dimensions-and-units.md), [Algebraic Data Types](algebraic-data-types.md), [Indexes](indexes.md), and [DAG Blocks](functions.md).

## Type-Level Domains and Notation

The capital letters in the definitions below are **metavariables**, not names
that exist implicitly in graphcal source:

- `D` ranges over **dimensions**: canonical products of base dimensions with
  rational exponents. `Dimensionless` is the identity dimension. In a generic
  declaration, `<D: Dim>` introduces a source-level variable over this domain.
- `S` ranges over the closed set of supported **time scales**: `UTC`, `TAI`,
  `TT`, `TDB`, `ET`, `GPST`, `GST`, `BDT`, and `QZSST`. `S` is semantic
  notation only; graphcal does not currently have a `TimeScale` generic kind.
- `A` ranges over names introduced by `type` declarations.
- `Gᵢ` ranges over generic arguments accepted by `A`'s corresponding generic
  parameter. Each argument is checked against that parameter's kind (`Dim`,
  `Type`, `Index`, or `Nat`).
- `Iᵢ` ranges over finite, ordered **index axes**. An axis can come from an
  `index` declaration or from an explicit structural index such as `Fin(3)` in
  `T[Fin(3)]`.

`Quantity(D)` and `Datetime(S)` use parentheses only as semantic notation; they
are not constructor calls or literal source syntax. `A<G₁, ..., Gₙ>` and
`T[I₁, ..., Iₘ]` mirror graphcal's source syntax. Angle brackets apply a
generic algebraic type, while square brackets index a value type over one or
more axes.

`Dim`, `Type`, `Index`, and `Nat` are **generic parameter kinds**, not value
types. They are defined precisely in [Generic Parameter Kinds](#generic-parameter-kinds).
In kinding notation:

```text
D : Dim        D is a dimension
S : TimeScale  S is a supported time scale (semantic notation only)
T : Type       T is a single-value type, called a ValueType below
I : Index      I is a finite, ordered index axis
N : Nat        N is a type-level natural number
```

The built-in and user-declared type constructors have these kind-level
signatures. The arrows are not graphcal function types:

```text
Quantity : Dim -> Type
Int      : Type
Bool     : Type
Datetime : TimeScale -> Type
A        : K₁ × ... × Kₙ -> Type     for generic kinds Kᵢ declared by A
Fin      : Nat -> Index               with the validity obligation N > 0
_[_]     : Type × one-or-more Index axes -> DeclType
```

Thus `Type` is the kind whose inhabitants are single-value types; this page
uses **ValueType** for those inhabitants. `Index` is the kind whose inhabitants
are axes, not labels or indexed collections.

## Type Stratification

Graphcal's type system is organized into three levels:

```text
Level 1: Primitive = Quantity(D) | Int | Bool | Datetime(S)
Level 2: ValueType (T : Type) = Primitive | A | A<G₁, ..., Gₙ>
Level 3: DeclType             = ValueType | ValueType[I₁, ..., Iₘ]  (m >= 1)
```

A bare `A` is a non-generic algebraic type (or a generic type whose arguments
all have defaults). Every `type` declaration defines this same nominal
algebraic-type concept, regardless of whether it has one constructor or many.
Constructors and their payload fields describe how values of `A` are formed;
they are not separate types and therefore do not appear as separate cases in
the `ValueType` definition.

- **Primitive** — An indivisible atomic datum.
- **ValueType** — A single logical value: either a primitive or an instance of
  a user-declared algebraic type. This is the type of one value that can be
  stored in a node, passed through a DAG parameter, or used inside expressions.
- **DeclType** — What can appear in type annotations of `param`, `node`,
  `const node`, and DAG parameter/output declarations: either a `ValueType` or
  a collection of one `ValueType` indexed by one or more axes.

### DAG Correspondence

The stratification connects directly to the computation model:

> A node in the evaluation DAG has type **ValueType**. A declaration of type `ValueType[Index]` expands to one DAG node per index label. A declaration of type `ValueType[I, J]` expands to one DAG node per label tuple `(i, j)`.

| Declaration type | DAG nodes |
|-----------------|-----------|
| `node x: Velocity` | 1 node |
| `node x: Velocity[Maneuver]` (3 labels) | 3 nodes |
| `node x: Velocity[Phase, Maneuver]` (2 x 3) | 6 nodes |

The `for` comprehension expands a single declaration into multiple DAG nodes. Each node is independently evaluable (modulo data dependencies), making indexed values naturally parallelizable. This also explains why arithmetic on indexed values requires explicit `for`: you are defining the computation for each individual DAG node, not operating on the collection as a whole.

## Name Universes

Graphcal keeps three declaration universes exclusive inside one scope:

- **Type universe**: algebraic types and dimensions.
- **Index universe**: named and coordinate index declarations.
- **Value universe**: `param`, `node`, `const node`, `assert`, plot, figure,
  layer, and `dag` declarations.

A leaf name can be declared in only one of those universes in the same scope.
For example, `type M { Mk }` and `index M = { A }` are a duplicate-name error,
as are `dim M = Length;` with either `type M { ... }` or `index M = { ... }`.

Constructors live in their own constructor namespace. This is why a record-like
declaration can still use the conventional `type T { T(...) }` spelling: the
type name and its constructor name do not compete in type, index, or value
positions.

Units are referenced only in unit syntax, such as unit literals and conversion
targets. A unit name can match a type, index, or value leaf without changing
which declaration a type, index, or value position resolves.

## Type Categories

### Primitives (Level 1)

The indivisible types. Each represents a single atomic datum.

| Semantic type | Representation | Carries a dimension? |
|---------------|----------------|----------------------|
| `Quantity(D)` | IEEE 754 binary64 magnitude in SI base units | Yes |
| `Int` | 64-bit signed integer | No |
| `Bool` | Boolean | No |
| `Datetime(S)` | High-precision epoch in time scale `S` | No |

`Quantity(D)` and `Datetime(S)` are semantic notation, not literal source
syntax. Their source spellings are:

```text
Quantity(D)    -> D
Datetime(UTC)  -> Datetime
Datetime(S)    -> Datetime<S>
```

!!! important "No `Quantity` source constructor"
    Write a quantity type as its dimension, such as `Length`,
    `Dimensionless`, or `Length / Time`. `Quantity<Length>` and bare
    `Quantity` are not Graphcal types.

#### Quantity Types

A quantity is a binary64 floating-point magnitude with a **dimension** known at
compile time. Its unit is value/display metadata, not part of its type.

```
param mass: Mass = 1200.0 kg;           // quantity of dimension Mass
param ratio: Dimensionless = 0.85;      // identity-dimension quantity
```

`Dimensionless` is the identity-dimension quantity type. When two quantities
of the same dimension are divided, the result has type `Dimensionless`.

Floating-point arithmetic follows IEEE 754 binary64 rules. The runtime detects
and reports NaN and infinity.

Only quantities carry physical dimensions. `Int`, `Bool`, and `Datetime(S)`
are separate primitive families, not quantities.

#### Int

`Int` is a 64-bit signed integer. It is always dimensionless and cannot carry a physical dimension.

```
param count: Int = 42;
const node seven: Int = 7;
```

Integer arithmetic uses checked operations -- overflow is a runtime error, not silent wraparound.

#### Bool

`Bool` is used in conditions and logical expressions.

```
param enabled: Bool = true;
node active: Bool = @enabled && @count > 0;
```

#### Datetime

`Datetime` represents a precise point in time. It is parameterized by a **time scale** that determines how the instant is interpreted. Bare `Datetime` defaults to UTC.

```
param launch: Datetime = datetime("2024-11-05T12:00:00 UTC");
param t_tt: Datetime<TT> = epoch("2024-11-05T12:00:00", TT);
```

Supported time scales: `UTC`, `TAI`, `TT`, `TDB`, `ET`, `GPST`, `GST`, `BDT`, `QZSST`.

Datetime values follow **point-vs-vector** semantics:

| Operation | Result | Notes |
|-----------|--------|-------|
| `Datetime - Datetime` | `Time` | Both must be the same time scale |
| `Datetime + Time` | `Datetime` | Add a duration |
| `Time + Datetime` | `Datetime` | Add a duration (commuted form) |
| `Datetime - Time` | `Datetime` | Subtract a duration |
| `Datetime == Datetime` | `Bool` | Equality comparison |
| `Datetime < Datetime` | `Bool` | Ordering comparison |

Datetime values cannot be added together, multiplied, divided, or used as the right-hand operand of duration subtraction (`Time - Datetime`).

Cross-scale operations are type errors: `Datetime<UTC> - Datetime<TT>` does not compile. Use explicit time scale conversion functions (`to_utc`, `to_tt`, etc.) first.

See [Built-in Reference](built-ins.md#datetime-functions) for the full list of datetime constructors, conversions, and extraction functions.

### Value Types (Level 2)

A `ValueType` is a single logical value: a primitive or an instance of one
nominal algebraic type declared with `type`. Every body-bearing `type`
declaration lists one or more constructors. Constructor count does not create
separate "struct" and "union" type categories.

Constructor payload fields must themselves be ValueTypes — you cannot put an
indexed type like `Velocity[Maneuver]` inside a constructor's payload. To index
structured data, index the algebraic type itself:
`Vec3<Velocity, ECI>[Maneuver]`.

#### Algebraic Type with One Constructor

A one-constructor type can be record-shaped. By convention, its constructor
has the same name as its type:

```
type Orbit {
    Orbit(sma: Length, ecc: Dimensionless, inc: Angle),
}

type Vec3<D: Dim, Frame: Type> {
    Vec3(x: D, y: D, z: D),
}
```

Because every value has the same constructor and payload shape, its fields can
be accessed directly with `@value.field`.

#### Algebraic Type with Multiple Constructors

The same `type` declaration syntax can list multiple constructors. Each
constructor has its own payload or is a bare unit constructor:

```
type ManeuverKind {
    Impulsive(delta_v: Velocity),
    LowThrust(thrust: Force, duration: Time),
    Coast,
}
```

Field access is rejected when a type has multiple constructors because no
single payload shape is guaranteed. Destructure the value through `match`
instead.

### Declaration Types (Level 3)

A DeclType is either a ValueType or an indexed collection of ValueTypes. This is what appears in type annotations:

```
param dry_mass: Mass = 1200.0 kg;                         // ValueType
param delta_v: Velocity[Maneuver] = { ... };              // Indexed ValueType
node matrix: Dimensionless[Row, Col] = for r, c { ... };  // Multi-indexed ValueType
```

`T[I]` is a type constructor that lifts a ValueType into a total map from index labels to values. Multi-indexing `T[I, J]` is a flat product-key map (not nested). Axis order is significant: `T[I, J]` and `T[J, I]` are different types.

## Domain Constraints

Type expressions can carry **domain constraints** that declare valid value ranges. Constraints are written as `(min: expr, max: expr)` after the base type:

```
param bus_mass: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;
param thrust: Force(min: 0.01 N) = 0.5 N;           // min only
param efficiency: Dimensionless(max: 1.0) = 0.85;    // max only
param count: Int(min: 1, max: 100) = 10;             // Int constraints
```

### Syntax

The constraint clause goes between the base type and the optional `[Index]` suffix:

```
T(min: expr, max: expr)           // both bounds
T(min: expr)                      // lower bound only
T(max: expr)                      // upper bound only
T(min: expr, max: expr)[I]        // constrained indexed type (element-wise)
```

Here `T` must be a quantity type or `Int`, and `I` is an index axis. Both
`min` and `max` are optional — you can specify one or both. Each bound
expression must evaluate to a value compatible with `T`.

### Supported Types

Domain constraints are valid on:

- **Quantity types** (including `Dimensionless`): `Mass(min: ...)`,
  `Velocity(max: ...)`, `Dimensionless(min: 0.0, max: 1.0)`, etc.
- **`Int`**: `Int(min: 1, max: 100)`

Domain constraints are **not** valid on `Bool`, `Datetime`, or any algebraic
type, regardless of its constructor count. Attempting to use constraints on
these types is a compile error.

### Indexed Types

For indexed types, constraints apply **element-wise** to each entry:

```
param delta_v: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = {
    Maneuver.Departure: 3200.0 m/s,
    Maneuver.Correction: 500.0 m/s,
    Maneuver.Insertion: 1800.0 m/s,
};
```

Each entry in the indexed value is independently checked against the constraint bounds.

### Constructor Payload Field Constraints

Constraints can also annotate the payload field types of a
constructor:

```
type SatelliteSpec {
    SatelliteSpec(
        mass: Mass(min: 100.0 kg, max: 2000.0 kg),
        altitude: Length(min: 200.0 km),
    ),
}

pub type ManeuverResult {
    Burn(dv: Velocity(min: 0.0 m/s, max: 10.0 km/s), duration: Time(min: 0.0 s)),
    Coast,
}
```

Field constraints fire at **construction time** for each
`Ctor(field: ...)` call:

- For a `const node` whose value is a constructor call, violations are caught at compile time as `DomainViolation`.
- For a `param` or `node` that constructs a value at runtime, a violation is reported as a per-node `EvalFailed` error keyed to the constructor and field (e.g., `field SatelliteSpec.mass above maximum (2000 kg)`).

### Generic Type Arguments

Domain constraints are **not** allowed on generic type arguments — they have no enforcement site after type erasure and ambiguous semantics. Put the constraint on the payload field of the constructor instead:

```
// REJECTED at compile time:
pub type Vec3<D: Dim> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m)> = ...;

// Use a non-generic field constraint instead:
pub type SignedLength { SignedLength(value: Length(min: 0.0 m)) }
```

### Runtime Checking

Domain constraints on `param` and `node` declarations are checked at
**runtime** after evaluation: a violation produces a per-node error and
downstream nodes receive a `DependencyFailed`. Constraints on `const node`
declarations and on constructor payload fields constructed inside a `const`
are checked at compile time, since the values are known statically.

### Compile-Time Validation

The following are always caught at compile time, regardless of where the
constraint sits (top-level declaration or constructor payload field):

- **Invalid target type**: Constraint on an unsupported type (e.g., `Bool(min: 0)`)
- **Invalid key**: Unknown constraint key (e.g., `Mass(step: 10)` — only `min` and `max` are valid)
- **Min exceeds max**: When both bounds are specified and `min > max`
- **Dimension mismatch**: When the bound's dimension doesn't match the type's dimension (e.g., `Mass(min: 1.0 m)`)
- **Generic type-arg constraint**: A constraint placed on a `TypeApplication` argument like `Vec3<Length(min: 0.0 m)>`

### Use Cases

Domain constraints are useful for:

- **Parameter sweeping/sampling**: Declaring valid ranges for design space exploration
- **Input validation**: Catching obviously wrong parameter values before they propagate through the graph
- **Documentation**: Making valid ranges explicit in the type annotation, visible in LSP hover

## Indexes and Indexed Types

An index declares a finite, ordered collection axis usable in `T[I]`. Graphcal
has named, coordinate, and structural finite indexes.

### Named Index

A named index declares a finite set of labels usable as a collection axis. The `index` keyword declares:

1. An **axis marker**: `Maneuver` can be used in `T[Maneuver]` to create indexed types.
2. A closed set of **index labels**: `Maneuver.Departure` identifies one position on that axis.

```
index Maneuver = { Departure, Correction, Insertion };
```

Named index labels use qualified syntax (`Maneuver.Departure`), distinguishing
them from algebraic-type constructors, which use bare syntax (`Nominal`). This
reflects a genuine semantic difference: labels identify positions within a
collection axis, while constructors form values of an algebraic type.

Named index labels are not ValueType values. They cannot be stored in nodes or params, compared with `==`, passed through DAG parameters, or used in constructor payloads. Labels appear only in index positions and index-pattern positions:

- Indexed type axes: `Velocity[Maneuver]`
- Element access: `@delta_v[Maneuver.Departure]`
- Map and table keys: `{ Maneuver.Departure: 2.46 km/s, ... }`
- `for` bindings and index access through their loop variables: `for m: Maneuver { @delta_v[m] }`
- `match` patterns over named-index loop variables: `match m { Maneuver.Departure => ..., ... }`

An algebraic type with only unit constructors (e.g., `type Foo { A, B }`) is
NOT automatically an index. The `index` keyword explicitly marks an
enumeration as usable in `T[I]`, preventing accidental use of marker types as
collection axes.

### Coordinate Index

A coordinate index is a finite sequence of quantity values in one dimension:

```
index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);
index Samples = linspace(0.0 s, 100.0 s, points: 1001);
```

`range` makes the exact increment authoritative; `linspace` makes the exact
point count authoritative. Their arguments are static and finite. A coordinate
loop variable has semantic type `Quantity(D)` and can participate in arithmetic
and indexing.

### Structural Finite Index

`Fin(N)` constructs an anonymous positional axis explicitly:

```
param vector: Dimensionless[Fin(3)] = for i: Fin(3) { 0.0 };
```

Its loop variable has the bounded integer type `Fin(N)`, which is accepted in
`Int` operations while retaining its bound for static index checks. The Nat `N`
and Index `Fin(N)` remain distinct sorts, so a bare `D[3]` is invalid.

### Index Capabilities

| Capability | Named (`Maneuver`) | Coordinate (`TimeStep`) | Finite (`Fin(3)`) |
|-----------|----------------------|--------------------------|-------------------|
| Loop variable type | index case variable | `Quantity(D)` | `Fin(3)` (bounded integer) |
| Indexing: `@x[i]` | Yes | Yes | Yes |
| Explicit map key | Yes | No | No |
| Equality comparison | No; use `match` | Yes | Yes |
| Pattern matching | Yes (qualified label) | No | No |
| Arithmetic | No | Yes (quantity) | Yes (integer) |
| Pass to DAG param | No | Yes (as quantity) | Yes (as `Int`) |

Coordinate-index loop variables are quantity values. Named-index loop variables
are index case variables: they can select an indexed entry and drive exhaustive
`match`, but they are not values.

### Construction of Indexed Values

**Map literal** (total -- all labels must be present):

```
param delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.05 km/s,
    Maneuver.Insertion: 1.48 km/s,
}
```

**Multi-axis map literal** (total -- all label tuples must be present):

```
param delta_v_budget: Velocity[Phase, Maneuver] = {
    (Phase.Launch, Maneuver.Departure): 2.46 km/s,
    (Phase.Launch, Maneuver.Correction): 0.0 m/s,
    (Phase.Launch, Maneuver.Insertion): 0.0 m/s,
    (Phase.Cruise, Maneuver.Departure): 0.0 m/s,
    (Phase.Cruise, Maneuver.Correction): 0.05 km/s,
    (Phase.Cruise, Maneuver.Insertion): 0.0 m/s,
    (Phase.Arrival, Maneuver.Departure): 0.0 m/s,
    (Phase.Arrival, Maneuver.Correction): 0.0 m/s,
    (Phase.Arrival, Maneuver.Insertion): 1.48 km/s,
}
```

Single-axis map literals use bare keys (`Maneuver.Departure: ...`); multi-axis map literals use tuple keys (`(Phase.Launch, Maneuver.Departure): ...`).

**`for` comprehension** (one value per label):

```
node fuel: Mass[Maneuver] = for m: Maneuver {
    @dry_mass * (exp(@delta_v[m] / @v_exhaust) - 1.0)
}
```

**Multi-axis `for`** (one value per label tuple):

```
node matrix: Dimensionless[Row, Col] = for r: Row, c: Col {
    @A[r, c] + @B[r, c]
}
```

### Consumption of Indexed Values

**Indexing** -- extracts a single element by providing all index labels:

```
@delta_v[Maneuver.Departure]                // Velocity[Maneuver] -> Velocity
@matrix[Row.R1, Col.C2]                    // Dimensionless[Row, Col] -> Dimensionless
```

No partial indexing -- all axes must be specified. To extract a "slice" along one axis, use explicit `for`:

```
node row1: Dimensionless[Col] = for c: Col { @matrix[Row.R1, c] }
```

**Aggregation** -- collapses one or more axes:

```
sum(for m: Maneuver { @fuel[m] })    // Mass[Maneuver] -> Mass
maximum(for m: Maneuver { @delta_v[m] }) // Velocity[Maneuver] -> Velocity
```

**Scan** -- ordered accumulation:

```
scan(
    for m: Maneuver { @delta_v[m] },
    0.0 m/s,
    |acc, val| acc + val,
)   // Velocity[Maneuver] -> Velocity[Maneuver]
```

### No Implicit Broadcasting

Arithmetic on indexed values requires explicit `for`. This is a deliberate safety decision:

```
// ERROR: cannot add Velocity[Maneuver] + Velocity[Maneuver]
node bad = @delta_v + @extra_dv;

// CORRECT: explicit element-wise operation
node good: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] + @extra_dv[m]
}
```

This prevents the class of silent broadcasting bugs common in NumPy and Excel, where mismatched shapes are silently resolved.

## Type Conversions

Graphcal has **no implicit type conversions**. You must use explicit conversion functions:

| Function | From | To | Example |
|----------|------|----|---------|
| `to_float(x)` | `Int` | `Dimensionless` | `to_float(42)` yields `42.0` |
| `to_int(x)` | `Dimensionless` | `Int` | `to_int(3.7)` yields `3` |
| `to_utc(x)` | `Datetime(S)` | `Datetime(UTC)` | Time scale conversion |
| `to_tai(x)` | `Datetime(S)` | `Datetime(TAI)` | Time scale conversion |
| `to_tt(x)` | `Datetime(S)` | `Datetime(TT)` | Time scale conversion |

The existing `to_float` name describes conversion to the floating-point
representation; it does not name a source-level `Float` type. `to_int`
truncates toward zero. Time scale conversion functions (`to_utc`, `to_tai`, `to_tt`, `to_tdb`, `to_et`, `to_gpst`, `to_gst`, `to_bdt`, `to_qzsst`) convert between time scales without changing the physical instant.

## Dimension Algebra

Dimensions are compile-time types that form an algebra over base dimensions. See [Dimensions & Units](dimensions-and-units.md) for the full reference on declaring dimensions and units. This section describes the algebraic rules the compiler enforces.

### Representation

Internally, a dimension is a product of base dimensions with rational exponents:

$$
D = L^{a_1} \cdot T^{a_2} \cdot M^{a_3} \cdot \ldots
$$

where each exponent $a_i$ is a rational number (stored as a reduced fraction). `Dimensionless` is the case where all exponents are zero.

### Arithmetic Rules

The compiler determines the dimension of the result of each arithmetic operation:

| Operation | Dimension Rule | Constraint |
|-----------|---------------|------------|
| `a + b` | Same as operands | `dim(a)` must equal `dim(b)` |
| `a - b` | Same as operands | `dim(a)` must equal `dim(b)` |
| `a * b` | Product | `dim(a) * dim(b)` -- exponents add |
| `a / b` | Quotient | `dim(a) / dim(b)` -- exponents subtract |
| `a ^ n` | Power | `dim(a) ^ n` -- exponents multiply by `n` |
| `a % b` | Same as operands | `dim(a)` must equal `dim(b)` |

Examples:

- `Length * Length` = `Length^2`
- `Length / Time` = `Velocity` (a prelude derived dimension)
- `Length / Length` = `Dimensionless`
- `(Mass * Length / Time^2) * (Length / Time)` = `Mass * Length^2 / Time^3`

### Equivalence

Two dimension expressions are equivalent if and only if they reduce to the same canonical form (same set of base dimension exponents). Named dimensions are transparent -- `Velocity` and `Length / Time` are the same type if `Velocity` is defined as `Length / Time`.

### Built-in Function Dimension Rules

Built-in math functions have specific dimension constraints:

| Function | Argument Dimension | Result Dimension |
|----------|--------------------|------------------|
| `sqrt(x)` | Any `D` | `D^(1/2)` (exponents halved) |
| `sin(x)`, `cos(x)`, `tan(x)` | `Angle` | `Dimensionless` |
| `asin(x)`, `acos(x)` | `Dimensionless` | `Angle` |
| `atan2(y, x)` | Both same `D` | `Angle` |
| `exp(x)`, `ln(x)`, `log10(x)` | `Dimensionless` | `Dimensionless` |
| `abs(x)` | Any `D` | `D` |
| `least(a, b)`, `greatest(a, b)` | Both same `D` | `D` |
| `floor(x)`, `ceil(x)`, `round(x)`, `trunc(x)` | Any `D` | `D` |

## Unit Conversion

The `->` operator converts between units of the same dimension. It binds at the lowest precedence.

```
node speed_kmh: Velocity = @speed -> km/h;
```

There is no type-cast operator. To change a value's phantom type parameter (e.g., re-label a reference frame), construct a new instance and assign each field explicitly:

```
node pos_body: Vec3<Length, Body> = Vec3<Length, Body>(
    x: @pos_eci.x,
    y: @pos_eci.y,
    z: @pos_eci.z,
);
```

The verbosity is intentional: every relabeling is a deliberate, field-by-field act, visible at the call site.

## Typing Rules for Expressions

This section lists the type of each expression form and the constraints the compiler enforces.

### Literals

| Expression | Type | Notes |
|-----------|------|-------|
| `42` | `Int` | No decimal point or exponent |
| `3.14` | `Dimensionless` | Floating-point literal without a unit |
| `400.0 km` | Dimension of the unit (`Length`) | Floating-point literal with a unit; integer literals cannot have units |
| `true`, `false` | `Bool` | |

### References

| Expression | Type |
|-----------|------|
| `@name` | Declared type of param/node/const node `name` |
| `BUILTIN_NAME` | Type of the built-in constant (`PI`, `E`, `TAU`, etc.) |
| `local_var` | Type of the loop variable or match binding |

### Arithmetic Operators

| Expression | Result Type | Constraint |
|-----------|-------------|------------|
| `a + b` | `dim(a)` | `dim(a) == dim(b)` |
| `a - b` | `dim(a)` | `dim(a) == dim(b)` |
| `a * b` | `dim(a) * dim(b)` | |
| `a / b` | `dim(a) / dim(b)` | |
| `a % b` | `dim(a)` | `dim(a) == dim(b)` |
| `a ^ n` | `dim(a) ^ n` | `n` is a numeric literal |
| `-a` | `dim(a)` | |

### Comparison and Logical Operators

| Expression | Result Type | Constraint |
|-----------|-------------|------------|
| `a == b`, `a != b` | `Bool` | `a` and `b` must have the same type |
| `a < b`, `a > b`, `a <= b`, `a >= b` | `Bool` | `a` and `b` must be quantities with the same dimension |
| `a && b`, `a \|\| b` | `Bool` | `a` and `b` must be `Bool` |
| `!a` | `Bool` | `a` must be `Bool` |

Comparisons are non-chaining: `a < b < c` is a parse error; write `a < b && b < c`.

### Conditional

```
if condition { then_expr } else { else_expr }
```

- `condition` must be `Bool`.
- `then_expr` and `else_expr` must have the same type.
- The result type is the type of the branches.

### Function Call

```
function_name(arg1, arg2, ...)
```

- Arguments are matched positionally against the function's parameter types.
- Generic dimensions and indexes in built-in or extern signatures are inferred
  from ordinary value arguments.
- Explicit non-empty function generic arguments are not supported. A form such
  as `sqrt<3>(x)` is rejected rather than silently ignoring `<3>`.
- The result type is the function's return type after generic substitution.

### Field Access

```
expr.field_name
```

- `expr` must have a record-shaped algebraic type: exactly one constructor with
  payload fields, using the same name as the type.
- `field_name` must be a payload field of that constructor.
- The result type is the declared field type, with generic parameters
  substituted.

### Index Access

```
expr[Index.Variant]        // access a specific element
expr[loop_var]              // access with a for-binding variable
expr[Index1.V1, Index2.V2] // multi-dimensional access
```

- `expr` must be an indexed type `T[I]` (or `T[I1, I2]` for multi-dimensional).
- All axes must be specified (no partial indexing).
- The result type is the element type `T`.

### Algebraic Value Construction

```
ConstructorName(field1: expr1, field2: expr2)
ConstructorName<Arg1, Arg2>(field1: expr1, field2: expr2)
ConstructorName                                   // unit constructor
```

- The constructor determines one enclosing algebraic result type; a
  constructor does not introduce a separate type.
- Each field expression must match the declared type of that payload field.
- Every payload field must be written as `field: expr`; shorthand `field` is
  not supported.
- Use explicit `field: @node_name` when passing graph nodes.
- Generic arguments instantiate the enclosing algebraic type's parameters.

### Index Label

```
IndexName.VariantName
```

- References a specific label of a named index.
- This is not a value expression and has no ValueType.
- It is legal only in index positions, such as element access, map/table keys, `#[expected_fail(...)]` keys, include index bindings, and named-index `match` patterns.

### Match Expression

```
match scrutinee {
    VariantA(field1: field1, field2: binding) => expr_a,
    VariantB => expr_b,
}
```

- `scrutinee` must be an algebraic-type value or a named-index loop variable.
- `match` is for exhaustive case analysis over closed finite alternatives. Use `if` for ordinary boolean predicates and comparisons.
- All members/labels must be covered (exhaustiveness check).
- For algebraic-type scrutinees, arms use constructor patterns (bare or
  module-qualified) and can bind payload fields explicitly with
  `field: variable` or `field: _`.
- For named-index loop variables, arms use qualified index-label patterns (`Index.Label` or `module.Index.Label`) and cannot bind fields.
- All arm expressions must have the same type.
- The result type is the common type of the arms.

### Map Literal

```
{ Index.Variant1: expr1, module.Index.Variant2: expr2, ... }
```

- All variants of the index must be covered.
- All value expressions must have the same type `T`.
- The result type is `T[Index]`; when the index is imported, the owner is the resolved module item, not just the leaf spelling.

For multi-axis map literals, use tuple keys:

```
{ (I1.V1, I2.V2): expr1, (I1.V1, I2.V3): expr2, ... }
```

- All label tuples must be present.
- The result type is `T[I1, I2]`.

### For Comprehension

```
for var: IndexName { body_expr }
for v1: Index1, v2: Index2 { body_expr }
```

- `var` is bound to each label of the index in turn.
- For named indexes, the loop variable is an index case variable. It is valid in index access (`@x[var]`) and named-index `match`, but not as a ValueType value.
- For coordinate indexes, the loop variable has `Quantity(D)` type.
- For `Fin(N)`, the loop variable has bounded integer type `Fin(N)`; ordinary integer operations widen their results to `Int`.
- `body_expr` is evaluated for each binding; its type is `T`.
- The result type is `T[IndexName]` (or `T[Index1, Index2]` for multiple bindings).

### Scan

```
scan(source, init, |acc, val| body)
```

- `source` must be an indexed type `T[I]`.
- `init` must have type `U` (the accumulator type).
- `acc` is bound to type `U`; `val` is bound to type `T`.
- `body` must have type `U`.
- The result type is `U[I]` (accumulated values for each index element).

The `|acc, val| body` is special syntax, not a function value.

### Unfold

```
unfold(init, |prev, curr| body)
```

- `init` must have type `T`.
- `prev` and `curr` are bound to the iteration context.
- `body` must have type `T`.
- The result type is `T[I]` where `I` is the index from context.

The `|prev, curr| body` is special syntax, not a function value.

## DAG Blocks

DAG blocks are named, reusable sub-DAGs. They are declarations, not values. There is no function type in the type system.

DAG block parameters and nodes use the same DeclTypes (ValueTypes or indexed types) as top-level declarations:

```
dag hohmann_transfer {
    param gm: Length^3 / Time^2;
    param r1: Length;
    param r2: Length;
    node dv1: Velocity = ...;
    node total_dv: Velocity = @dv1 + ...;
}
```

DAG blocks are instantiated with `include`, which embeds their nodes into the enclosing computation graph:

```
include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
).{ total_dv };
```

## Generics

Algebraic types can be parameterized by values from four type-level domains.
A generic parameter's annotation is its **kind**: it determines which arguments
are legal and how the parameter may be used in the declaration.

### Generic Parameter Kinds

| Kind | Syntax | Parameter ranges over | Valid uses |
|------|--------|-----------------------|------------|
| `Dim` | `<D: Dim>` | Any dimension, such as `Length` or `Length / Time` | Quantity field types and dimension expressions such as `D^2` |
| `Type` | `<T: Type>` | Any `ValueType`, such as `Bool`, `Length`, or `Vec3<Length, Eci>` | A payload field type, or a phantom/tag parameter if unused in any payload |
| `Index` | `<I: Index>` | A finite ordered index axis | An axis in an indexed type such as `D[I]` |
| `Nat` | `<N: Nat>` | A non-negative natural number used in type-level size arithmetic | A finite cardinality such as `D[Fin(N)]`, subject to the non-empty-axis rule |

These kinds are not themselves `ValueType`s. For example, `Index` denotes the
domain of collection axes; it does not mean an index label is a first-class
value. Likewise, `Type` denotes the `ValueType` domain and excludes both a bare
index axis and an indexed `DeclType` such as `Velocity[Maneuver]`.

A `Type` parameter is not inherently phantom. It is phantom only when the
declaration carries it for nominal distinction without using it in a
constructor payload. In `Vec3<D, Frame>`, for example, `D` determines the field
types while `Frame` is a phantom parameter.

### Generic Arguments and Defaults

Type annotations and constructors use the same generic-argument surface. Each
argument is checked positionally against the declared `Dim`, `Index`, `Nat`, or
`Type` kind; spelling or identifier casing never determines its kind. Ordinary
functions do not declare or consume these arguments, so a non-empty application
such as `sqrt<3>(x)` is rejected rather than silently ignored.

A `Nat` argument may be an integer literal, an in-scope `Nat` parameter, or a
polynomial expression using `+` and `*`:

```gcl
type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param matrix: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
```

Bare names and products such as `N` or `M * N` are intentionally ambiguous in
source syntax. The compiler resolves them only after it knows the corresponding
parameter kind.

Generic parameters of every kind can have defaults:

```gcl
index Component = { X, Y, Z };
type Unframed { Unframed }

type Vec3<
    D: Dim,
    I: Index = Component,
    F: Type = Unframed,
    N: Nat = 3,
> {
    Vec3(x: D, y: D, z: D),
}

// The omitted arguments are Component, Unframed, and 3.
param pos: Vec3<Length> = Vec3<Length>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
```

Defaulted parameters must form a trailing suffix, and each default may refer
only to parameters declared earlier in the list. Arguments may be omitted only
from that trailing defaulted portion. If every parameter has a default, type
and constructor names without an angle-bracket list apply all of them.

`Nat` and `Index` are distinct sorts. A bare Nat is never lifted into an Index:
use `Fin(3)` for an `Index` argument and `3` for a `Nat` argument. Accordingly,
`D[3]` is rejected with a suggestion to write `D[Fin(3)]`.

### Generic Parameter Unification

When using generic types, type parameters are unified from context:

```
param pos: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 1.0 km, y: 0.0 km, z: 0.0 km);
```

The compiler performs **unification**: it matches the declared type parameters (which may contain generic variables) against the concrete types to determine bindings for each generic variable.

If a generic variable appears multiple times, all occurrences must unify to the same concrete type.

### Dimension Expressions in Generics

Generic dimension parameters can appear in compound dimension expressions in type definitions.

### Structural Finite Indexes

`Fin(N)` explicitly constructs an Index with integer positions `0` through
`N - 1`:

```
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
param mat: Dimensionless[Fin(2), Fin(3)] =
    for i: Fin(2), j: Fin(3) { 1.0 };
node transposed: Dimensionless[Fin(3), Fin(2)] =
    for j: Fin(3), i: Fin(2) { @mat[i, j] };
```

Two finite structural indexes are equal exactly when their normalized
cardinality expressions are equal. Nat expressions support addition and
multiplication, with `*` binding more tightly than `+`; subtraction is not
supported. Express the larger side additively, for example use
`D[Fin(N + 1)]` for an input and `D[Fin(N)]` for its smaller output.

`Fin(0)` is invalid. Loop variables from `for i: Fin(N)` have bounded integer
type `Fin(N)` and can index values carrying that finite axis. They are accepted
where an `Int` is required; arithmetic results are ordinary `Int` values.

## Type Equivalence

Two types are equivalent if:

- **Quantities**: They have the same dimension in canonical form. Named dimensions are transparent (e.g., `Velocity` equals `Length / Time`).
- **Int**: Both are `Int`.
- **Bool**: Both are `Bool`.
- **Datetime**: Same time scale. `Datetime<UTC>` and `Datetime<TT>` are different types.
- **Algebraic types**: Same owner-qualified declared type name and equivalent
  generic arguments. Constructor count and payload shape belong to that
  nominal declaration; they do not create separate structural type identities.
- **Indexed**: Same element type, same indexes in the same order. `T[I, J]` and `T[J, I]` are different types.

There is no general subtyping. `Length` is not assignable to `Dimensionless`,
and `Vec3<Length, ECI>` is not assignable to `Vec3<Length, Unframed>` even if
both have the same fields. The bounded `Fin(N)` loop-local type is the explicit
integer-refinement exception: it is accepted where `Int` is required.

Named index labels do not participate in value type equivalence. They are index positions and patterns, not ValueTypes. Use `match` over a named-index loop variable for case analysis.

## Complete Entity Map

| Entity | Is a type? | First-class value? | DAG param? | DAG node? | Appears in expressions |
|--------|-----------|---------------------|------------|-----------|----------------------|
| Quantity value | ValueType | Yes | Yes | Yes | Yes |
| Int value | ValueType | Yes | Yes | Yes | Yes |
| Bool value | ValueType | Yes | Yes | Yes | Yes |
| Datetime value | ValueType | Yes | Yes | Yes | Yes |
| Algebraic value | ValueType | Yes | Yes | Yes | Yes |
| Algebraic constructor | No | No | No | No | Construction and match patterns |
| Named index label | No | No | No | No | Index positions and match patterns |
| Indexed value | DeclType | Yes | Yes | Yes | Via `for` |
| Coordinate index label | Quantity(D) | Yes | Yes (as quantity) | Yes (as quantity) | Indexing, arithmetic |
| `Fin` position | `Fin(N)` integer refinement | Yes | Yes (as `Int`) | Yes (as `Int`) | Indexing, arithmetic |
| Function | No | No | No | No | Calling only |
| Dimension | No; inhabits `Dim` | No | As generic `<D: Dim>` | As generic | In quantity type syntax |
| Time scale | No; inhabits semantic `TimeScale` | No | No | No | In `Datetime<TT>`-style type syntax |
| Unit | No (compile-time) | No | No | No | In literals and conversion targets |
| Index axis | No; inhabits `Index` | No | As generic `<I: Index>` | As generic | In indexed type syntax |
| Natural number | No; inhabits `Nat` | No | As generic `<N: Nat>` | As generic | In Nat expressions and `Fin(N)` cardinalities |

Named index labels use qualified syntax (`Maneuver.Departure`) while algebraic
constructors use bare or module-qualified syntax (`Nominal` or
`module.Nominal`). Labels belong to the index universe; constructors belong to
the constructor namespace and form values of their enclosing algebraic type.
