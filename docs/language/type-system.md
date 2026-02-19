---
icon: material/format-list-bulleted-type
---

# Type System

This page is the formal reference for Graphcal's type system. It describes all type kinds, the dimension algebra, typing rules for expressions, generics, and type equivalence.

For introductory material, see the [tutorial](../tutorial/index.md). For specific features, see [Dimensions & Units](dimensions-and-units.md), [Algebraic Data Types](algebraic-data-types.md), [Indexes](indexes.md), and [Functions](functions.md).

## Type Kinds

Every value in Graphcal has one of the following types:

| Type Kind | Examples | Description |
|-----------|----------|-------------|
| Scalar (Float + Dimension) | `Length`, `Velocity`, `Dimensionless` | 64-bit float annotated with a physical dimension |
| `Int` | `42`, `-7` | 64-bit signed integer; always dimensionless |
| `Bool` | `true`, `false` | Boolean |
| Struct | `TransferResult`, `Vec3<Length, ECI>` | Product type with named fields |
| Tagged union | `ManeuverKind` | Sum type with named variants |
| Indexed | `Velocity[Maneuver]`, `Length[Phase, Step]` | Collection over a finite index set |

### Scalar Types

A scalar is a `Float` value paired with a **dimension** at compile time. The dimension determines what physical quantity the value represents.

```
param mass: Mass = 1200.0 kg;           // Float with dimension Mass
param ratio: Dimensionless = 0.85;      // Float with dimension Dimensionless
```

`Dimensionless` is the identity dimension (no physical quantity). When two values of the same dimension are divided, the result is `Dimensionless`.

Float arithmetic follows IEEE 754 double-precision rules. The runtime detects and reports NaN and infinity.

### Int

`Int` is a 64-bit signed integer. It is always dimensionless and cannot carry a physical dimension.

```
param count: Int = 42;
const SEVEN: Int = 7;
```

Integer arithmetic uses checked operations -- overflow is a runtime error, not silent wraparound.

### Bool

`Bool` is used in conditions and logical expressions.

```
param enabled: Bool = true;
node active: Bool = @enabled && @count > 0;
```

## Type Conversions

Graphcal has **no implicit type conversions**. You must use explicit conversion functions:

| Function | From | To | Example |
|----------|------|----|---------|
| `to_float(x)` | `Int` | `Dimensionless` (Float) | `to_float(42)` yields `42.0` |
| `to_int(x)` | `Dimensionless` (Float) | `Int` | `to_int(3.7)` yields `3` |

`to_int` truncates toward zero (like Rust's `as` cast).

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
| `sqrt(x)` | Any `D^2` | `D` (exponents halved) |
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
| `CONST_NAME` | Declared type of const `CONST_NAME` |
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

For struct types with `derive(Add)`, `derive(Sub)`, or `derive(Neg)`, the corresponding operators are also allowed. Both operands must be the same struct type with the same type arguments, and the result is that struct type. The operation is applied component-wise.

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
- The index argument must match the index of the indexed type.
- The result type is the element type `T`.

### Struct Construction

```
TypeName { field1: expr1, field2 }
TypeName<Arg1, Arg2> { field1: expr1, field2 }
VariantName                                       // bare variant (no fields)
```

- Each field expression must match the declared type of that field.
- Field shorthand (`field2` without `: expr`) uses a local variable of the same name.
- The result type is the struct/union type.

### Variant Literal

```
IndexName::VariantName
```

- References a specific variant of a named index.
- Used in index access and match patterns.

### Match Expression

```
match scrutinee {
    VariantA { field1, field2: binding } => expr_a,
    VariantB => expr_b,
}
```

- `scrutinee` must be a tagged union type.
- All variants of the union must be covered (exhaustiveness check).
- All arm expressions must have the same type.
- The result type is the common type of the arms.

### Map Literal

```
{ Index::Variant1: expr1, Index::Variant2: expr2, ... }
```

- All variants of the index must be covered.
- All value expressions must have the same type `T`.
- The result type is `T[Index]`.

### For Comprehension

```
for var: IndexName { body_expr }
for v1: Index1, v2: Index2 { body_expr }
```

- `var` is bound to each variant of the index in turn.
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

### Unfold

```
unfold(init, |prev, curr| body)
```

- `init` must have type `T`.
- `prev` and `curr` are bound to the iteration context.
- `body` must have type `T`.
- The result type is `T[I]` where `I` is the index from context.

## Generics

Functions and types can be generic over dimensions, indexes, and phantom types.

### Generic Constraints

| Constraint | Syntax | Meaning |
|-----------|--------|---------|
| `Dim` | `<D: Dim>` | `D` stands for any dimension |
| `Index` | `<I: Index>` | `I` stands for any index |
| `Type` | `<F: Type>` | `F` stands for any type (phantom/tag) |

### Default Type Parameters

Generic parameters can have defaults:

```
type Vec3<D: Dim, F: Type = Unframed> derive(Add, Sub, Neg) {
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

## Type Equivalence

Two types are equivalent if:

- **Scalars**: They have the same dimension in canonical form. Named dimensions are transparent (e.g., `Velocity` equals `Length / Time`).
- **Int**: Both are `Int`.
- **Bool**: Both are `Bool`.
- **Structs**: Same struct name and all type arguments are equivalent.
- **Indexed**: Same element type and same index (or indexes, for multi-dimensional).

There is no subtyping. `Length` is not assignable to `Dimensionless`, and `Vec3<Length, ECI>` is not assignable to `Vec3<Length, Unframed>` even if both have the same fields.

## Derived Operations

Struct types can derive arithmetic operators:

```
type Vec3<D: Dim> derive(Add, Sub, Neg) {
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

Operations are applied component-wise to all fields. All fields must have types that support the corresponding operation (e.g., for `derive(Add)`, all fields must be addable).
