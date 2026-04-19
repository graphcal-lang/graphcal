---
icon: material/code-parentheses
---

# Expressions

This page covers all expression forms in Graphcal: operators, precedence, and conditionals.

## Operator Precedence

From lowest to highest precedence:

| Precedence | Operator | Description | Associativity |
|-----------|----------|-------------|---------------|
| 1 | `\|\|` | Logical OR | Left |
| 2 | `&&` | Logical AND | Left |
| 3 | `==` `!=` `<` `>` `<=` `>=` | Comparison | Left |
| 4 | `+` `-` | Addition, subtraction | Left |
| 5 | `*` `/` `%` | Multiplication, division, modulo | Left |
| 6 | `-` `!` | Unary negation, logical NOT | Prefix |
| 7 | `^` | Exponentiation | Right |
| 8 | `->` | Unit conversion | Left |
| 9 | `as` | Phantom type cast | Left |
| 10 | `.` `[...]` | Field access, index access | Left |

Parentheses `()` can be used to override precedence.

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
    The exponent in `^` must be a **literal** number (integer or float). Variable exponents are not allowed because the resulting dimension would not be known at compile time.

## Comparison Operators

| Operator | Description | Operand Requirement |
|----------|-------------|-------------------|
| `==` | Equal | Same type and dimension |
| `!=` | Not equal | Same type and dimension |
| `<` | Less than | Same type and dimension |
| `>` | Greater than | Same type and dimension |
| `<=` | Less or equal | Same type and dimension |
| `>=` | Greater or equal | Same type and dimension |

All comparison operators return `Bool`.

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

## Phantom Type Cast (`as`)

Cast between different phantom type instantiations:

```
node pos_body: Vec3<Length, Body> = @pos_eci as Vec3<Length, Body>;
```

## Field Access (`.`)

Access a field of a struct-typed value:

```
node dv: Velocity = @transfer.total_dv;
```

## Index Access (`[...]`)

Access an element of an indexed value:

```
node first: Velocity = @delta_v[Maneuver::Departure];
```

## If/Else Expressions

Conditional expressions:

```
node clamped: Int = if @a > @seven { @seven } else { @a };
```

Both branches must have the same type and dimension. The `else` branch is required.

## Inline DAG Invocation (`@dag(args)::out`)

A `dag` can be invoked as an expression, producing a fresh sub-graph at every
syntactic call site:

```
node doubled: Length = @scale(factor: 2.0, v: @src)::result;
```

The projected output after `::` is mandatory. Arguments are evaluated in the
surrounding scope, so they may reference loop variables from an enclosing
`for` comprehension or other local binders.

See [Multi-File: Inline DAG Invocation](./multi-file.md#inline-dag-invocation)
for the full semantics.

## Numeric Literals

| Form | Example | Type |
|------|---------|------|
| Integer | `42`, `1_000` | `Int` |
| Float | `3.14`, `1.0e-3` | `Float` (Dimensionless) |
| Float with unit | `200.0 km`, `9.8 m/s^2` | `Float` (with dimension) |
| Boolean | `true`, `false` | `Bool` |
