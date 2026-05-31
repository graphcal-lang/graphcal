---
icon: material/ruler
---

# Dimensions & Units

Graphcal separates **dimensions** (compile-time types) from **units** (value-level scaling factors). This page covers the dimension algebra, unit definitions, conversion, and the prelude.

## Dimensions

A dimension represents a physical quantity kind (e.g., length, time, mass). Dimensions form an algebra over base dimensions.

### Base Dimensions

The prelude provides 8 base dimensions:

| Base Dimension | Symbol |
|---------------|--------|
| `Length` | L |
| `Time` | T |
| `Mass` | M |
| `Temperature` | Θ |
| `ElectricCurrent` | I |
| `Amount` | N |
| `LuminousIntensity` | J |
| `Angle` | A |

Plus the special dimension `Dimensionless` (the identity element).

### Derived Dimensions

Define derived dimensions using algebraic expressions:

```
dim Velocity = Length / Time;
dim Acceleration = Length / Time^2;
dim Force = Mass * Length / Time^2;
dim Energy = Mass * Length^2 / Time^2;
dim GravParam = Length^3 / Time^2;
```

The allowed operations are:

| Operation | Syntax | Example |
|-----------|--------|---------|
| Multiplication | `A * B` | `Mass * Length` |
| Division | `A / B` | `Length / Time` |
| Exponentiation | `A^n` | `Time^2` |

Exponents are integer or rational literals.

### User-Defined Base Dimensions

Declare a new base dimension with the `base dim` syntax:

```
base dim Information;
```

Then build derived dimensions from it:

```
dim Bandwidth = Information / Time;
```

### Dimension Algebra Rules

Internally, dimensions are represented as products of base dimensions with rational exponents. The compiler performs:

- `Length * Length = Length^2`
- `Length / Length = Dimensionless`
- `(Mass * Length / Time^2) * (Length / Time) = Mass * Length^2 / Time^3`

Two expressions are dimensionally compatible if and only if they reduce to the same canonical form.

## Units

Units are value-level scaling factors tied to a specific dimension. They define how a numeric value maps to the SI base.

### Prelude Units

| Dimension | Units |
|-----------|-------|
| Length | `m`, `km` (1000 m), `cm` (0.01 m), `mm` (0.001 m) |
| Time | `s`, `min` (60 s), `hour` (3600 s) |
| Mass | `kg`, `g` (0.001 kg) |
| Temperature | `K` |
| ElectricCurrent | `A` |
| Amount | `mol` |
| LuminousIntensity | `cd` |
| Angle | `rad`, `deg` (π/180 rad) |
| Force | `N` (kg*m/s^2), `kN` (1000 N) |
| Energy | `J` (N*m), `kJ` (1000 J) |
| Power | `W` (J/s), `kW` (1000 W) |
| Pressure | `Pa` (N/m^2), `kPa` (1000 Pa), `MPa` (1e6 Pa) |
| Frequency | `Hz` (1/s) |

### Defining Custom Units

```
unit mile: Length = 1609.344 m;
unit knot: Velocity = 0.514444 m/s;
base unit bit: Information;         // canonical unit for a user-defined dimension
unit byte: Information = 8.0 bit;
unit kB: Information = 1000.0 byte;
```

A `base unit` declaration (`base unit bit: Information;` with no `= ...`) defines the canonical unit for a user-defined base dimension. Non-base units must always carry an `= ...` body.

Unit scale factors must be **positive and finite**. Static unit definitions such as `unit z: Length = 0.0 m;`, negative scales, and overflowing scales are rejected. Dynamic unit scales are checked at evaluation time with the same rule.

### Dynamic Units

A unit's scale factor can depend on runtime values (params or nodes) by using a parenthesized expression with `@`-references:

```
base dim Money;
base unit USD: Money;

param usd_per_eur: Dimensionless = 1.08;
unit EUR: Money = (@usd_per_eur) USD;
```

Here, 1 EUR = `usd_per_eur` USD. The scale factor is evaluated at runtime, so overriding `usd_per_eur` (e.g., via `--set usd_per_eur=1.20`) changes all EUR-denominated values accordingly.

Dynamic units behave identically to static units for dimension checking (compile-time). The scale is only resolved at evaluation time, after the referenced params have been computed.

### Using Units

Attach a unit to a numeric literal:

```
param altitude: Length = 200.0 km;
param duration: Time = 1.5 hour;
const node c: Velocity = 299792458.0 m/s;
```

Compound unit expressions are supported:

```
const node gm: GravParam = 3.986e5 km^3/s^2;
```

The SI value produced by a unit literal must remain finite. For example, a literal whose numeric value times its unit scale overflows is an error rather than `inf`.

## Unit Conversion

The `->` operator converts a value to a different unit of the same dimension:

```
param alt: Length = 200.0 km;
node alt_m: Length = @alt -> m;         // 200000.0 m
node alt_cm: Length = @alt -> cm;       // 20000000.0 cm
```

The source and target must share the same dimension. Attempting to convert between incompatible dimensions is a compile-time error.

## Dimension Inference

When you write an expression like `@a + @b`, the compiler infers the dimension of the result from the operands:

| Expression | Result Dimension |
|-----------|-----------------|
| `a + b` | Same as `a` and `b` (must match) |
| `a - b` | Same as `a` and `b` (must match) |
| `a * b` | Product of dimensions |
| `a / b` | Quotient of dimensions |
| `a ^ n` | Dimension raised to power `n` |
| `sqrt(a)` | Dimension raised to power 1/2 |

For example, `sqrt(Length^2 / Time^2)` infers `Length / Time` (= `Velocity`).
