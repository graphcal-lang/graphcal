---
icon: material/format-list-bulleted-type
---

# Type System

This page is the formal reference for Graphcal's type system. It describes the three-level type stratification, the dimension algebra, typing rules for expressions, generics, and type equivalence.

For introductory material, see the [tutorial](../tutorial/index.md). For specific features, see [Dimensions & Units](dimensions-and-units.md), [Algebraic Data Types](algebraic-data-types.md), [Indexes](indexes.md), and [Functions](functions.md).

## Type Stratification

Graphcal's type system is organized into three levels:

```
Level 1: Primitive  = Scalar(Dim) | Int | Bool | Datetime(TimeScale)
Level 2: ValueType  = Primitive
                    | Struct(name, fields: [ValueType])
                    | Union(name, members: [Type])
Level 3: DeclType   = ValueType
                    | Indexed(ValueType, [Index])   -- written T[I] or T[I, J, ...]

Expression-level:
  Label(IndexName)  -- not a ValueType or DeclType; exists only in expressions
```

- **Primitive** — An indivisible atomic datum.
- **ValueType** — A single logical value. Primitives plus algebraic compositions (structs, union types). This is the type of one value: you can pass it to a function, return it, store it.
- **DeclType** — What can appear in type annotations of `param`, `node`, and `const node` declarations, and in function parameter/return types. Either a ValueType or an indexed collection of ValueTypes.
- **Label(IndexName)** — An expression-level type for named index labels (e.g., `Maneuver::Departure`). Labels are real values that can be compared, matched, and passed to functions, but they cannot appear in declaration type annotations. They exist only within expression contexts such as `for` loop bodies, function parameters, and `let` bindings.

### DAG Correspondence

The stratification connects directly to the computation model:

> A node in the evaluation DAG has type **ValueType**. A declaration of type `ValueType[Index]` expands to one DAG node per index label. A declaration of type `ValueType[I, J]` expands to one DAG node per label tuple `(i, j)`.

| Declaration type | DAG nodes |
|-----------------|-----------|
| `node x: Velocity` | 1 node |
| `node x: Velocity[Maneuver]` (3 labels) | 3 nodes |
| `node x: Velocity[Phase, Maneuver]` (2 x 3) | 6 nodes |

The `for` comprehension expands a single declaration into multiple DAG nodes. Each node is independently evaluable (modulo data dependencies), making indexed values naturally parallelizable. This also explains why arithmetic on indexed values requires explicit `for`: you are defining the computation for each individual DAG node, not operating on the collection as a whole.

## Type Kinds

### Primitives (Level 1)

The indivisible types. Each represents a single atomic datum.

| Type | Representation | Dimension? |
|------|---------------|------------|
| `Scalar(Dim)` | 64-bit float in SI base units | Yes |
| `Int` | 64-bit signed integer | No (dimensionless) |
| `Bool` | Boolean | No (dimensionless) |
| `Datetime(TimeScale)` | High-precision epoch | No (point in time) |

#### Scalar Types

A scalar is a float value paired with a **dimension** at compile time. The dimension determines what physical quantity the value represents.

```
param mass: Mass = 1200.0 kg;           // Float with dimension Mass
param ratio: Dimensionless = 0.85;      // Float with dimension Dimensionless
```

`Dimensionless` is the identity dimension (no physical quantity). When two values of the same dimension are divided, the result is `Dimensionless`.

Float arithmetic follows IEEE 754 double-precision rules. The runtime detects and reports NaN and infinity.

Only `Scalar` carries a physical dimension. `Int` and `Bool` are non-scalable -- you cannot multiply an integer by an arbitrary unit scale factor and get a meaningful integer back.

#### Int

`Int` is a 64-bit signed integer. It is always dimensionless and cannot carry a physical dimension.

```
param count: Int = 42;
const node SEVEN: Int = 7;
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
| `Datetime - Datetime` | `Time` (scalar) | Both must be the same time scale |
| `Datetime + Time` | `Datetime` | Add a duration |
| `Datetime - Time` | `Datetime` | Subtract a duration |
| `Datetime == Datetime` | `Bool` | Equality comparison |
| `Datetime < Datetime` | `Bool` | Ordering comparison |

Datetime values cannot be added together, multiplied, or divided.

Cross-scale operations are type errors: `Datetime<UTC> - Datetime<TT>` does not compile. Use explicit time scale conversion functions (`to_utc`, `to_tt`, etc.) first.

See [Built-in Reference](built-ins.md#datetime-functions) for the full list of datetime constructors, conversions, and extraction functions.

### Value Types (Level 2)

A ValueType is a single logical value: a primitive, a struct instance, or a union type member. All struct fields and union type member fields must themselves be ValueTypes.

#### Structs

A struct is a product type with named fields. All fields must be ValueTypes -- you cannot put an indexed type like `Velocity[Maneuver]` inside a struct field. To index structured data, index the struct itself: `Vec3<Velocity, ECI>[Maneuver]`.

```
type Orbit {
    sma: Length,
    ecc: Dimensionless,
    inc: Angle,
}

#[derive(Add, Sub, Neg)]
type Vec3<D: Dim, Frame: Type> {
    x: D,
    y: D,
    z: D,
}
```

#### Union Types

A union type is a sum type that combines multiple existing types. Each member is defined as its own type first, then combined with the `=` and `|` syntax. Members can be unit types (no fields) or struct types.

```
type Impulsive { delta_v: Velocity }
type LowThrust { thrust: Force, duration: Time }
type ManeuverKind = Impulsive | LowThrust;
```

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
Type(min: expr, max: expr)           // both bounds
Type(min: expr)                      // lower bound only
Type(max: expr)                      // upper bound only
Type(min: expr, max: expr)[Index]    // constrained indexed type (element-wise)
```

Both `min` and `max` are optional — you can specify one or both. The bound expressions must evaluate to a value compatible with the type's dimension.

### Supported Types

Domain constraints are valid on:

- **Scalar types** (any dimension): `Mass(min: ...)`, `Velocity(max: ...)`, etc.
- **`Dimensionless`**: `Dimensionless(min: 0.0, max: 1.0)`
- **`Int`**: `Int(min: 1, max: 100)`

Domain constraints are **not** valid on `Bool`, `Datetime`, struct types, or union types. Attempting to use constraints on these types is a compile error.

### Indexed Types

For indexed types, constraints apply **element-wise** to each entry:

```
param delta_v: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = {
    Maneuver::Departure: 3200.0 m/s,
    Maneuver::Correction: 500.0 m/s,
    Maneuver::Insertion: 1800.0 m/s,
};
```

Each entry in the indexed value is independently checked against the constraint bounds.

### Runtime Checking

Domain constraints are checked at **runtime** after evaluation. They are not enforced at compile time because parameter values are not known until runtime.

- If a value violates its constraint, it is reported as a per-node error (similar to division by zero or other runtime errors).
- The constraint check occurs after the value is computed, before it is used by downstream nodes.

### Compile-Time Validation

The following are caught at compile time:

- **Invalid target type**: Constraint on an unsupported type (e.g., `Bool(min: 0)`)
- **Invalid key**: Unknown constraint key (e.g., `Mass(step: 10)` — only `min` and `max` are valid)
- **Min exceeds max**: When both bounds are specified and `min > max`
- **Dimension mismatch**: When the bound's dimension doesn't match the type's dimension (e.g., `Mass(min: 1.0 m)`)

### Use Cases

Domain constraints are useful for:

- **Parameter sweeping/sampling**: Declaring valid ranges for design space exploration
- **Input validation**: Catching obviously wrong parameter values before they propagate through the graph
- **Documentation**: Making valid ranges explicit in the type annotation, visible in LSP hover

## Indexes and Indexed Types

An index declares a finite, ordered set of labels usable as collection axes in `T[I]`. Two flavors exist.

### Named Index

A named index declares a finite set of labels usable as a collection axis. The `index` keyword declares:

1. An **expression-level type**: `Maneuver::Departure` has type `Label(Maneuver)` — a dedicated type kind, distinct from union types. Labels exist only within expressions, not in declaration type annotations.
2. An **axis marker**: `Maneuver` can be used in `T[Maneuver]` to create indexed types.

```
index Maneuver = { Departure, Correction, Insertion };
```

Named index labels use qualified syntax (`Maneuver::Departure`), distinguishing them from union type members which use bare syntax (`Nominal`). This reflects a genuine semantic difference: labels identify positions within a collection axis, while union type members are constructors of a sum type.

Named index labels are proper runtime values within expressions:

- Pass to functions: `fn f(m: Maneuver) -> Velocity` works.
- Return from functions: `fn pick() -> Maneuver` works.
- Store in local variables: `let x = Maneuver::Departure` works.
- Compare: `m == Maneuver::Departure` works.
- Pattern match: `match m { Maneuver::Departure => ..., ... }` works.
- Use in struct fields: `type Config { phase: Phase, maneuver: Maneuver }` works.

However, labels cannot be the type of a `param`, `node`, or `const node` declaration — they exist only within expression contexts.

A regular fieldless union type (`type Foo = A | B;`) is NOT automatically an index. The `index` keyword explicitly marks it as usable in `T[I]`, preventing accidental use of marker types as collection axes.

### Range Index

A range index is a finite sequence of scalar values in a specific dimension:

```
index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);
```

Range index labels are scalar values, not union type members. The loop variable in `for t: TimeStep { ... }` acts as a `Scalar(Time)` -- it can be used in arithmetic and for indexing.

### Named vs Range Index Capabilities

| Capability | Named index (`Maneuver`) | Range index (`TimeStep`) |
|-----------|--------------------------|--------------------------|
| Loop variable type | `Label(Maneuver)` (ValueType) | `Scalar(Dim)` (Primitive) |
| Indexing: `@x[m]` | Yes | Yes |
| Map literal key | Yes | No (range labels are implicit) |
| Equality comparison | Yes (as Label) | Yes (as Scalar) |
| Pattern matching | Yes (qualified: `Maneuver::X => ...`) | No |
| Arithmetic | No (not a scalar) | Yes |
| Pass to function | Yes | Yes (as scalar) |

Both loop variable types are runtime values -- named index labels are `Label` values (expression-level), range index labels are scalar values (Primitive).

### Construction of Indexed Values

**Map literal** (total -- all labels must be present):

```
param delta_v: Velocity[Maneuver] = {
    Maneuver::Departure: 2.46 km/s,
    Maneuver::Correction: 0.05 km/s,
    Maneuver::Insertion: 1.48 km/s,
}
```

**Multi-axis map literal** (total -- all label tuples must be present):

```
param delta_v_budget: Velocity[Phase, Maneuver] = {
    (Phase::Launch, Maneuver::Departure): 2.46 km/s,
    (Phase::Launch, Maneuver::Correction): 0.0 m/s,
    (Phase::Launch, Maneuver::Insertion): 0.0 m/s,
    (Phase::Cruise, Maneuver::Departure): 0.0 m/s,
    (Phase::Cruise, Maneuver::Correction): 0.05 km/s,
    (Phase::Cruise, Maneuver::Insertion): 0.0 m/s,
    (Phase::Arrival, Maneuver::Departure): 0.0 m/s,
    (Phase::Arrival, Maneuver::Correction): 0.0 m/s,
    (Phase::Arrival, Maneuver::Insertion): 1.48 km/s,
}
```

Single-axis map literals use bare keys (`Maneuver::Departure: ...`); multi-axis map literals use tuple keys (`(Phase::Launch, Maneuver::Departure): ...`).

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
@delta_v[Maneuver::Departure]                // Velocity[Maneuver] -> Velocity
@matrix[Row::R1, Col::C2]                    // Dimensionless[Row, Col] -> Dimensionless
```

No partial indexing -- all axes must be specified. To extract a "slice" along one axis, use explicit `for`:

```
node row1: Dimensionless[Col] = for c: Col { @matrix[Row::R1, c] }
```

**Aggregation** -- collapses one or more axes:

```
sum(for m: Maneuver { @fuel[m] })    // Mass[Maneuver] -> Mass
max(for m: Maneuver { @delta_v[m] }) // Velocity[Maneuver] -> Velocity
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
| `to_float(x)` | `Int` | `Dimensionless` (Float) | `to_float(42)` yields `42.0` |
| `to_int(x)` | `Dimensionless` (Float) | `Int` | `to_int(3.7)` yields `3` |
| `to_utc(x)` | `Datetime(any)` | `Datetime<UTC>` | Time scale conversion |
| `to_tai(x)` | `Datetime(any)` | `Datetime<TAI>` | Time scale conversion |
| `to_tt(x)` | `Datetime(any)` | `Datetime<TT>` | Time scale conversion |

`to_int` truncates toward zero (like Rust's `as` cast). Time scale conversion functions (`to_utc`, `to_tai`, `to_tt`, `to_tdb`, `to_et`, `to_gpst`, `to_gst`, `to_bdt`, `to_qzsst`) convert between time scales without changing the physical instant.

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
- `Length / Time` = `Velocity` (if `dimension Velocity = Length / Time;`)
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
| `asin(x)`, `acos(x)`, `atan2(y, x)` | `Dimensionless` | `Angle` |
| `exp(x)`, `ln(x)`, `log10(x)` | `Dimensionless` | `Dimensionless` |
| `abs(x)` | Any `D` | `D` |
| `min(a, b)`, `max(a, b)` | Both same `D` | `D` |
| `floor(x)`, `ceil(x)`, `round(x)` | `Dimensionless` | `Dimensionless` |

## Unit Conversion and Cast

The `->` operator converts between units of the same dimension. The `as` operator casts to a type (stripping or reinterpreting type information). Both bind at the lowest precedence.

```
node speed_kmh: Velocity = @speed -> km/hour;    // unit conversion
node raw: Vec3<Length, Unframed> = @v as Vec3<Length, Unframed>;  // type cast
```

These are mutually exclusive: an expression can have `->` or `as`, not both.

## Typing Rules for Expressions

This section lists the type of each expression form and the constraints the compiler enforces.

### Literals

| Expression | Type | Notes |
|-----------|------|-------|
| `42` | `Int` | No decimal point or exponent |
| `3.14` | `Dimensionless` | Float without unit |
| `400.0 km` | Dimension of the unit (`Length`) | Float with unit; integer literals cannot have units |
| `true`, `false` | `Bool` | |

### References

| Expression | Type |
|-----------|------|
| `@name` | Declared type of param/node `name` |
| `CONST_NAME` | Declared type of `const node` `CONST_NAME` |
| `local_var` | Type of the `let` binding or function parameter |

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

For struct types with `#[derive(Add)]`, `#[derive(Sub)]`, or `#[derive(Neg)]`, the corresponding operators are also allowed. Both operands must be the same struct type with the same type arguments, and the result is that struct type. The operation is applied component-wise.

### Comparison and Logical Operators

| Expression | Result Type | Constraint |
|-----------|-------------|------------|
| `a == b`, `a != b` | `Bool` | `a` and `b` must have the same type |
| `a < b`, `a > b`, `a <= b`, `a >= b` | `Bool` | `a` and `b` must have the same scalar dimension |
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

### Block Expression

```
{
    let x: Type = expr1;
    let y = expr2;
    result_expr
}
```

- `let` bindings introduce local variables.
- Type annotations on `let` bindings are optional; if present, the inferred type of the initializer must match.
- The type of the block is the type of the final expression.

### Function Call

```
function_name(arg1, arg2, ...)
```

- Arguments are matched positionally against the function's parameter types.
- For generic functions, type parameters are inferred from the argument types (see [Generics](#generics) below).
- The result type is the function's return type after generic substitution.

### Field Access

```
expr.field_name
```

- `expr` must be a struct type.
- `field_name` must be a field of that struct.
- The result type is the declared type of the field (with generic parameters substituted).

### Index Access

```
expr[Index::Variant]        // access a specific element
expr[loop_var]              // access with a for-binding variable
expr[Index1::V1, Index2::V2] // multi-dimensional access
```

- `expr` must be an indexed type `T[I]` (or `T[I1, I2]` for multi-dimensional).
- All axes must be specified (no partial indexing).
- The result type is the element type `T`.

### Struct Construction

```
TypeName { field1: expr1, field2 }
TypeName<Arg1, Arg2> { field1: expr1, field2 }
MemberName                                        // unit type (no fields)
```

- Each field expression must match the declared type of that field.
- Field shorthand (`field2` without `: expr`) uses a local variable of the same name.
- The result type is the struct/union type.

### Variant Literal

```
IndexName::VariantName
```

- References a specific label of a named index.
- The result type is `Label(IndexName)`.
- Named index labels are ValueType values and can be used anywhere a value is expected.

### Match Expression

```
match scrutinee {
    VariantA { field1, field2: binding } => expr_a,
    VariantB => expr_b,
}
```

- `scrutinee` must be a union type or a `Label` type.
- All members/labels must be covered (exhaustiveness check).
- For union type scrutinees, arms use bare member names and can bind fields.
- For `Label` scrutinees, arms use qualified names (`Index::Label`) and cannot bind fields (labels are fieldless).
- All arm expressions must have the same type.
- The result type is the common type of the arms.

### Map Literal

```
{ Index::Variant1: expr1, Index::Variant2: expr2, ... }
```

- All variants of the index must be covered.
- All value expressions must have the same type `T`.
- The result type is `T[Index]`.

For multi-axis map literals, use tuple keys:

```
{ (I1::V1, I2::V2): expr1, (I1::V1, I2::V3): expr2, ... }
```

- All label tuples must be present.
- The result type is `T[I1, I2]`.

### For Comprehension

```
for var: IndexName { body_expr }
for v1: Index1, v2: Index2 { body_expr }
```

- `var` is bound to each label of the index in turn.
- For named indexes, the loop variable has the `Label(IndexName)` type.
- For range indexes, the loop variable has `Scalar(Dim)` type.
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

## Functions

Functions are declarations, not values. There is no function type in the type system. No higher-order functions are supported.

Functions can accept and return DeclTypes (ValueTypes or indexed types):

```
// ValueType params and return
fn hohmann(gm: Length^3 / Time^2, r1: Length, r2: Length) -> TransferResult { ... }

// Indexed type params and return
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
fn normalize<I: Index>(v: Dimensionless[I]) -> Dimensionless[I] = for i: I {
    v[i] / sum(v)
};
```

Functions **cannot** accept other functions as arguments (no higher-order functions), nor dimensions or units as runtime values. Dimensions and units appear as generic parameters (`<D: Dim>`) or in compile-time expressions only.

Named index labels are passable to functions as `Label` values:

```
fn maneuver_fuel(m: Maneuver, params: MissionParams) -> Mass {
    match m {
        Maneuver::Departure => compute_departure_fuel(params),
        Maneuver::Correction => compute_correction_fuel(params),
        Maneuver::Insertion => compute_insertion_fuel(params),
    }
}
```

## Generics

Functions and types can be generic over dimensions, indexes, natural numbers, and phantom types.

### Generic Constraints

| Constraint | Syntax | Meaning |
|-----------|--------|---------|
| `Dim` | `<D: Dim>` | `D` stands for any dimension |
| `Index` | `<I: Index>` | `I` stands for any index |
| `Nat` | `<N: Nat>` | `N` stands for a natural number (type-level size) |
| `Type` | `<F: Type>` | `F` stands for any type (phantom/tag) |

### Default Type Parameters

Generic parameters can have defaults:

```
#[derive(Add, Sub, Neg)]
type Vec3<D: Dim, F: Type = Unframed> {
    x: D,
    y: D,
    z: D,
}

// These are equivalent:
param pos: Vec3<Length> = ...;
param pos: Vec3<Length, Unframed> = ...;
```

### Generic Type Inference

When calling a generic function, type parameters are inferred from the argument types:

```
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;

// D is inferred as Length from the arguments:
node mid: Length = lerp(@start, @end, 0.5);
```

The compiler performs **unification**: it matches the declared parameter types (which may contain generic variables) against the inferred argument types (which are concrete) to determine bindings for each generic variable.

If a generic variable appears multiple times, all occurrences must unify to the same concrete type. For example, in `lerp<D: Dim>(a: D, b: D, ...)`, both `a` and `b` must have the same dimension.

### Dimension Expressions in Generics

Generic dimension parameters can appear in compound dimension expressions:

```
fn kinetic_energy<D: Dim>(mass: Mass, speed: D) -> Mass * D^2 =
    0.5 * mass * speed ^ 2;
```

During unification, the compiler solves for the generic variable. If the parameter type is `D` and the argument type is `Length`, then `D = Length`, and the return type `Mass * D^2` becomes `Mass * Length^2` (= `Energy`).

### Nat Generics and Range Indexes

`Nat` generics allow functions to be parameterized over array sizes. Integer literals in index position create anonymous **nat range** indexes:

```
// A 3-element vector (internally uses range(3))
param v: Dimensionless[3] = for i: range(3) { 1.0 };

// A generic transpose function
fn transpose<M: Nat, N: Nat, D: Dim>(a: D[M, N]) -> D[N, M] =
    for j: range(N), i: range(M) { a[i, j] };

// M and N are inferred from the argument shape
param mat: Dimensionless[2, 3] = for i: range(2), j: range(3) { 1.0 };
node transposed: Dimensionless[3, 2] = transpose(@mat);
```

During unification, `Nat` parameters are matched by identity: `range(M)` unifies with `range(3)` to bind `M = 3`. Two nat ranges are equal if and only if their sizes are equal.

`Nat` expressions support addition, which enables functions that relate input and output sizes:

```
fn drop_last<N: Nat, D: Dim>(v: D[N + 1]) -> D[N] =
    for i: range(N) { v[i] };

// When calling drop_last on a Dimensionless[4] vector,
// the compiler solves N + 1 = 4 to deduce N = 3.
```

Expressions are normalized to a canonical linear form (`c + a₁·x₁ + …`) and equality is decided by comparing coefficients. Subtraction is not supported — instead, express the larger side with addition.

Loop variables from `for i: range(N)` have type `Int` and can be used to index into nat-range-indexed values. `Nat` parameters are also available as runtime `Int` values in function bodies (e.g., `if i < N`).

When a `Nat` parameter appears only in the return type (not in any argument), it cannot be inferred. Use explicit generic arguments (turbofish syntax) to provide the value:

```
fn eye<N: Nat>() -> Dimensionless[N, N] =
    for i: range(N), j: range(N) { if i == j { 1.0 } else { 0.0 } };

node I3: Dimensionless[3, 3] = eye<3>();
```

## Type Equivalence

Two types are equivalent if:

- **Scalars**: They have the same dimension in canonical form. Named dimensions are transparent (e.g., `Velocity` equals `Length / Time`).
- **Int**: Both are `Int`.
- **Bool**: Both are `Bool`.
- **Datetime**: Same time scale. `Datetime<UTC>` and `Datetime<TT>` are different types.
- **Labels**: Same index name. `Label(Maneuver)` and `Label(Phase)` are different types.
- **Structs**: Same struct name and all type arguments are equivalent.
- **Union types**: Same type name and same members.
- **Indexed**: Same element type, same indexes in the same order. `T[I, J]` and `T[J, I]` are different types.

There is no subtyping. `Length` is not assignable to `Dimensionless`, and `Vec3<Length, ECI>` is not assignable to `Vec3<Length, Unframed>` even if both have the same fields.

Cross-index label equality is a type error: `m == p` where `m: Maneuver` and `p: Phase` does not compile because they are different `Label` types.

## Complete Entity Map

| Entity | Is a type? | First-class value? | Pass to `fn`? | Return from `fn`? | Appears in expressions |
|--------|-----------|---------------------|---------------|-------------------|----------------------|
| Scalar value | ValueType | Yes | Yes | Yes | Yes |
| Int value | ValueType | Yes | Yes | Yes | Yes |
| Bool value | ValueType | Yes | Yes | Yes | Yes |
| Datetime value | ValueType | Yes | Yes | Yes | Yes |
| Struct instance | ValueType | Yes | Yes | Yes | Yes |
| Union type member | ValueType | Yes | Yes | Yes | Yes |
| Named index label | Label(IndexName) (expression-level) | Yes | Yes | Yes | Yes |
| Indexed value | DeclType | Yes | Yes | Yes | Via `for` |
| Range index label | Scalar(Dim) | Yes | Yes (as scalar) | Yes (as scalar) | Indexing, arithmetic |
| Function | No | No | No | No | Calling only |
| Dimension | No (compile-time) | No | As generic `<D: Dim>` | As generic | No |
| Unit | No (compile-time) | No | No | No | In literals only |
| Index | No (compile-time) | No | As generic `<I: Index>` | As generic | No |

Named index labels have a dedicated `Label(IndexName)` type kind, distinct from union type members. Labels use qualified syntax (`Maneuver::Departure`) while union type members use bare syntax (`Nominal`).

## Derived Operations

Struct types can derive arithmetic operators using the `#[derive(...)]` attribute:

```
#[derive(Add, Sub, Neg)]
type Vec3<D: Dim> {
    x: D,
    y: D,
    z: D,
}
```

| Derive | Enables | Operand Constraint | Result |
|--------|---------|--------------------|--------|
| `Add` | `a + b` | Both operands same struct type with same type args | Same struct type |
| `Sub` | `a - b` | Both operands same struct type with same type args | Same struct type |
| `Neg` | `-a` | Operand is the struct type | Same struct type |

Operations are applied component-wise to all fields. All fields must have types that support the corresponding operation (e.g., for `#[derive(Add)]`, all fields must be addable).
