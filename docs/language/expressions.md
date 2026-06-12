---
icon: material/code-parentheses
---

# Expressions

This page covers all expression forms in Graphcal: operators, precedence, and conditionals.

## Operator Precedence

From lowest to highest precedence:

| Precedence | Operator | Description | Associativity |
|-----------|----------|-------------|---------------|
| 0 | `->` | Unit conversion | n/a |
| 1 | `if`/`else` | Conditional expression | Right |
| 2 | `\|\|` | Logical OR | Left |
| 3 | `&&` | Logical AND | Left |
| 4 | `==` `!=` `<` `>` `<=` `>=` | Comparison | Non-chaining |
| 5 | `+` `-` | Addition, subtraction | Left |
| 6 | `*` `/` `%` | Multiplication, division, modulo | Left |
| 7 | `-` `!` | Unary negation, logical NOT | Prefix |
| 8 | `^` | Exponentiation | Right |
| 9 | `.` `[...]` | Field access, index access | Left |

Parentheses `()` can be used to override precedence. `->` binds at the
lowest level, so the trailing form `expr -> unit` wraps everything to its
left.

## Arithmetic Operators

| Operator | Float Behavior | Int Behavior | Dimension Rule |
|----------|---------------|-------------|----------------|
| `a + b` | Addition | Addition | Dimensions must match |
| `a - b` | Subtraction | Subtraction | Dimensions must match |
| `a * b` | Multiplication | Multiplication | Dimensions multiply |
| `a / b` | Division | Integer division | Dimensions divide |
| `a % b` | Remainder | Remainder | Dimensions must match |
| `a ^ n` | Exponentiation | Not supported | Dimension raised to power |
| `-a` | Negation | Negation | Dimension preserved |

!!! note "Exponent restriction"
    The exponent in `^` must be **compile-time-known** so the resulting dimension can be resolved before evaluation. In practice that means a numeric literal (integer or float, optionally with a leading unary `-`); for `Int ^ Int`, any expression that constant-folds to a non-negative integer is also accepted (e.g., `2 ^ 3 ^ 2` parses as `2 ^ (3 ^ 2)` and folds to `2 ^ 9 = 512`). Variable exponents whose value would only be known at runtime are not allowed.

!!! note "Finite scalars"
    Floating-point literals must be finite. Scalar operations that would create `NaN` or `inf` are surfaced as errors instead of producing a runtime value.

## Comparison Operators

| Operator | Description | Operand Requirement |
|----------|-------------|-------------------|
| `==` | Equal | Same type and dimension |
| `!=` | Not equal | Same type and dimension |
| `<` | Less than | Same type and dimension |
| `>` | Greater than | Same type and dimension |
| `<=` | Less or equal | Same type and dimension |
| `>=` | Greater or equal | Same type and dimension |

All comparison operators return `Bool` for scalar operands.

Comparisons broadcast element-wise over indexed operands: `T[I] op T[I]`
zips the two collections per key, and `T[I] op scalar` applies the scalar to
every key — both return `Bool[I]`. Indexed operands must share the same
axes in the same order; mismatched axes are a compile error (`D011`).

## Logical Operators

| Operator | Description |
|----------|-------------|
| `a \|\| b` | Logical OR |
| `a && b` | Logical AND |
| `!a` | Logical NOT |

Operands must be `Bool`.

`&&` and `||` always evaluate **both** operands (no short-circuit).
In a reactive calculation graph every sub-expression should be valid regardless of control flow,
so Graphcal surfaces errors eagerly rather than hiding them behind a short-circuit.
Use `if`-`then`-`else` when you need conditional evaluation.

## Unit Conversion (`->`)

Convert a value to a different unit of the same dimension:

```
node alt_m: Length = @altitude -> m;
node time_h: Time = @duration -> hour;
```

## Phantom Type Change (Explicit Reconstruction)

There is no phantom-type cast operator. To change a phantom type parameter,
construct a new instance and assign each field explicitly:

```
node pos_body: Vec3<Length, Body> = Vec3<Length, Body>(
    x: @pos_eci.x,
    y: @pos_eci.y,
    z: @pos_eci.z,
);
```

The verbosity is intentional: re-labeling (e.g., changing a reference frame) is a
deliberate, field-by-field act, not a silent reinterpretation.

## Field Access (`.`)

Access a field of a struct-typed value:

```
node dv: Velocity = @transfer.total_dv;
```

## Index Access (`[...]`)

Access an element of an indexed value:

```
node first: Velocity = @delta_v[Maneuver.Departure];
```

## If/Else Expressions

Conditional expressions:

```
node clamped: Int = if @a > @seven { @seven } else { @a };
```

Both branches must have the same type and dimension. The `else` branch is required.

## Inline DAG Invocation (`@dag(args).out`)

A `dag` can be invoked as an expression, producing a fresh sub-graph at every
syntactic call site:

```
node doubled: Length = @scale(factor: 2.0, v: @src).result;
```

The projected output after `.` is mandatory. Arguments are evaluated in the
surrounding scope, so they may reference loop variables from an enclosing
`for` comprehension or other local binders.

The thing immediately after `@` may be a DAG in scope or a module-qualified DAG path such as `@module.dag(args).out`. The projected output after the call is what makes the expression a graph reference.
The projected output must be a public node of the called DAG.

See [Multi-File: Inline-DAG Call Expression](./multi-file.md#inline-dag-call-expression)
for the full semantics.

## Numeric Literals

| Form | Example | Type |
|------|---------|------|
| Integer | `42`, `1_000` | `Int` |
| Float | `3.14`, `1.0e-3` | `Float` (Dimensionless) |
| Float with unit | `200.0 km`, `9.8 m/s^2` | `Float` (with dimension) |
| Boolean | `true`, `false` | `Bool` |
