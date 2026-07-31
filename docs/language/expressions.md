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

| Operator | Quantity behavior | `Int` behavior | Dimension rule |
|----------|-------------------|----------------|----------------|
| `a + b` | Addition | Addition | Dimensions must match |
| `a - b` | Subtraction | Subtraction | Dimensions must match |
| `a * b` | Multiplication | Multiplication | Dimensions multiply |
| `a / b` | Division | Integer division | Dimensions divide |
| `a % b` | Remainder | Remainder | Dimensions must match |
| `a ^ n` | Exponentiation | Integer exponentiation (non-negative exponent) | Dimension raised to power |
| `-a` | Negation | Negation | Dimension preserved |

### Complex arithmetic

A complex quantity has type `Complex<D>`, where `D` is the shared dimension of
its real and imaginary components. Real quantities are never promoted
implicitly for addition or subtraction; write `to_complex(x)` explicitly.

| Expression | Result | Requirement |
|------------|--------|-------------|
| `z1 + z2`, `z1 - z2` | `Complex<D>` | Both operands are `Complex<D>` |
| `z1 * z2` | `Complex<D1 * D2>` | Complex operands may have different dimensions |
| `z1 / z2` | `Complex<D1 / D2>` | The divisor must be nonzero |
| `q * z`, `z * q` | `Complex<Dq * Dz>` | `q` is a real quantity |
| `z / q` | `Complex<Dz / Dq>` | `q` is a nonzero real quantity |
| `q / z` | `Complex<Dq / Dz>` | `z` is nonzero |
| `-z` | `Complex<D>` | |

For example:

```graphcal
node displacement: Complex<Length> = complex(3.0 m, 4.0 m);
node gain: Complex<Dimensionless> = complex(0.5, -0.25);
node transformed: Complex<Length> = @displacement * @gain;
node with_offset: Complex<Length> = @displacement + to_complex(1.0 m);
```

`%` and `^` are not defined for complex operands. `==` and `!=` compare real
and imaginary components exactly; complex values have no ordering, so `<`,
`>`, `<=`, and `>=` are rejected. Use `re`, `im`, `abs`, `phase`, and `conj`
for component and polar operations.

### Power exponents

Every quantity exponent must be dimensionless. If the base is
`Dimensionless`, the exponent may be any runtime `Dimensionless` quantity:
`@ratio ^ @runtime_exponent` is valid.

If the base has a physical dimension, its exponent must instead use exact
static syntax: an integer (`2`, `-2`, or `0`) or a parenthesized rational
(`(3/2)` or `(-1/3)`). The result dimension is raised by that exact rational.
Decimal/scientific syntax is not accepted for a dimensioned base: write
`(1/4)` instead of `0.25`, and `2` instead of `2.0` (`D020`).

Exact rational metadata is also used at runtime. For example,
`(-8.0) ^ (1/3)` evaluates to `-2.0`; an even-denominator root of a negative
base is not a real value and produces an evaluation error. Value-level
exponent zero is valid even though zero powers are omitted from dimension and
unit declarations.

`Int ^ Int` requires a non-negative exact integer exponent. A right-associated
expression that constant-folds to one is also accepted: `2 ^ 3 ^ 2` parses as
`2 ^ (3 ^ 2)` and evaluates as `2 ^ 9`.

!!! note "Finite quantities"
    Floating-point literals must be finite. Real or complex operations that
    would create `NaN` or `inf` are surfaced as errors instead of producing a
    runtime value.

## Comparison Operators

| Operator | Description | Operand Requirement |
|----------|-------------|-------------------|
| `==` | Equal | Same type and dimension |
| `!=` | Not equal | Same type and dimension |
| `<` | Less than | Same type and dimension |
| `>` | Greater than | Same type and dimension |
| `<=` | Less or equal | Same type and dimension |
| `>=` | Greater or equal | Same type and dimension |

All comparison operators return `Bool` for unindexed operands. Complex
quantities support exact `==` and `!=` when their dimensions match, but do not
support ordering comparisons.

Datetime arithmetic follows point-vs-duration rules: `Datetime + Time`,
`Time + Datetime`, and `Datetime - Time` produce `Datetime`, while
`Datetime - Datetime` produces a `Time` duration. `Time - Datetime` and
`Datetime + Datetime` are errors.

Comparison operators do not broadcast: every operand must be unindexed.
Passing an indexed value to `==`, `!=`, `<`, `>`, `<=`, or `>=` is a compile
error (`D019`). Write element-wise comparison explicitly with `for` and index
each collection in the body:

```gcl
index Case = { A, B };
node values: Length[Case] = {
    Case.A: 1.0 m,
    Case.B: 2.0 m,
};
node below_limit: Bool[Case] = for case: Case {
    @values[case] < 3.0 m
};
```

This makes the selected axes and element pairing visible in source instead of
silently resolving collection shapes.

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
Use `if cond { a } else { b }` when you need conditional evaluation.

## Unit Conversion (`->`)

Convert a value to a different unit of the same dimension:

```
node alt_m: Length = @altitude -> m;
node time_h: Time = @duration -> h;
node displacement_cm: Complex<Length> = @displacement -> cm;
```

For `Complex<D>`, the target scales both components and is display metadata;
the stored SI components and type do not change. The target otherwise follows
unit scoping rules: bare names for local, selectively imported, or prelude
units, and `alias.unit` for units of a module imported
with an alias (see
[Unit Scoping](dimensions-and-units.md#unit-scoping)).

`->` is non-chaining: an expression carries at most one conversion target.
`@x -> km -> m` is a parse error, and the parenthesized `(@x -> km) -> m` is
rejected by the dimension checker (`D012`).

A conversion must sit in a *display position* — a place its target can reach
the declaration's rendered value (declaration top level, `if`/`match`
branches, constructor fields, map entries, for-comprehension bodies,
`scan`/`unfold` inits). A conversion in any other position (arithmetic
operand, function argument, comparison, condition, assert body) has no
effect and is rejected (`D013`).

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

Access a field of a record-shaped, one-constructor algebraic value:

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

The thing immediately after `@` may be a DAG in scope or a module-qualified
DAG path such as `@module.dag(args).out`. The mandatory projection makes the
expression a graph reference. It may name either an explicitly exported node
or a param input port; projecting a param yields its effective bound/default
value.

See [Multi-File: Inline-DAG Call Expression](./multi-file.md#inline-dag-call-expression)
for the full semantics.

## Numeric Literals

| Form | Example | Type |
|------|---------|------|
| Integer | `42`, `1_000` | `Int` |
| Floating-point | `3.14`, `1.0e-3` | `Dimensionless` |
| Floating-point with unit | `200.0 km`, `9.8 m/s^2` | Dimension of the unit |
| Boolean | `true`, `false` | `Bool` |
