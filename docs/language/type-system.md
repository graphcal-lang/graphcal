---
icon: material/format-list-bulleted-type
---

# Type System

Graphcal has three primitive types. Every value in the language is one of these primitives, optionally annotated with a physical dimension and wrapped in a struct, tagged union, or indexed collection.

## Primitive Types

| Type | Description | Literal Examples |
|------|-------------|-----------------|
| `Float` | 64-bit floating-point number | `3.14`, `1.0e-3`, `1200.0 kg` |
| `Int` | 64-bit signed integer (dimensionless only) | `42`, `1_000`, `-7` |
| `Bool` | Boolean | `true`, `false` |

### Float

`Float` is the primary numeric type. It can carry a physical dimension:

```
param mass: Mass = 1200.0 kg;       // Float with dimension Mass
param ratio: Dimensionless = 0.85;  // Float with dimension Dimensionless
```

Float arithmetic follows IEEE 754 double-precision rules. The runtime detects and reports NaN and infinity.

### Int

`Int` is a 64-bit signed integer. It is always `Dimensionless`:

```
param count: Int = 42;
const SEVEN: Int = 7;
node clamped: Int = if @count > SEVEN { SEVEN } else { @count };
```

Integer arithmetic uses checked operations -- overflow is a runtime error, not silent wraparound.

### Bool

`Bool` is used in conditions:

```
param enabled: Bool = true;
node status: Dimensionless = if @enabled { 1.0 } else { 0.0 };
```

## Type Conversions

Graphcal has **no implicit type conversions**. You must use explicit conversion functions:

| Function | From | To | Example |
|----------|------|----|---------|
| `to_float(x)` | `Int` | `Dimensionless` (Float) | `to_float(42)` yields `42.0` |
| `to_int(x)` | `Dimensionless` (Float) | `Int` | `to_int(3.7)` yields `3` |

`to_int` truncates toward zero (like Rust's `as` cast).

## Dimensionless

`Dimensionless` is the dimension annotation for plain numbers without physical dimension. It applies to `Float` values only:

```
param ratio: Dimensionless = 0.85;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
```

When dividing two values of the same dimension, the result is `Dimensionless`.

## The Type Hierarchy

```
Value
  Float (with Dimension)
  Int (always Dimensionless)
  Bool
```

`Float` values additionally carry:

- A **dimension** (compile-time type, e.g., `Mass`, `Length / Time`)
- An optional **display unit** (e.g., `km`, `kg`) for formatting

`Int` values are always dimensionless. `Bool` values have no dimension.
