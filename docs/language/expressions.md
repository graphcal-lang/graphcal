---
icon: material/code-parentheses
---

# Expressions

This page covers all expression forms in Graphcal: operators, precedence, and conditionals.

## Operator Precedence

From lowest to highest precedence:

| Precedence | Operator | Description | Associativity |
|-----------|----------|-------------|---------------|
| 0 | `->` `as` | Unit conversion / phantom type cast (mutually exclusive) | n/a |
| 1 | `if`/`else` | Conditional expression | Right |
| 2 | `\|\|` | Logical OR | Left |
| 3 | `&&` | Logical AND | Left |
| 4 | `==` `!=` `<` `>` `<=` `>=` | Comparison | Non-chaining |
| 5 | `+` `-` | Addition, subtraction | Left |
| 6 | `*` `/` `%` | Multiplication, division, modulo | Left |
| 7 | `-` `!` | Unary negation, logical NOT | Prefix |
| 8 | `^` | Exponentiation | Right |
| 9 | `.` `[...]` | Field access, index access | Left |

Parentheses `()` can be used to override precedence. `->` and `as` bind at
the lowest level: an expression carries at most one of them, so the
trailing form (`expr -> unit` or `expr as Type`) wraps everything to its
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

The thing immediately after `@` must be a single in-scope identifier (the DAG
itself); qualified forms like `@module.dag(args).out` are rejected. To call a
DAG from another module, bring it into scope first with `import <pkg>.{dag};`.

See [Multi-File: Inline-DAG Call Expression](./multi-file.md#inline-dag-call-expression)
for the full semantics.

## Numeric Literals

| Form | Example | Type |
|------|---------|------|
| Integer | `42`, `1_000` | `Int` |
| Float | `3.14`, `1.0e-3` | `Float` (Dimensionless) |
| Float with unit | `200.0 km`, `9.8 m/s^2` | `Float` (with dimension) |
| Boolean | `true`, `false` | `Bool` |
