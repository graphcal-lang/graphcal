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

Exponents are non-zero integers (`Time^2`, `Length^-1`) or parenthesized rationals (`Length^(1/2)`, `m^(-3/2)`); the same exponent grammar applies to dimension and unit expressions, including conversion targets.

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

User unit definitions on bare `Temperature` are rejected (`D014`): the common temperature units (°C, °F) are *affine* scales with an offset, which a multiplicative `unit` definition cannot express — `unit C: Temperature = 1.0 K;` would print `300 K` as a meaningless `300 C`. Keep absolute temperatures in `K`, or model offsets explicitly in expressions. Compound dimensions involving Temperature (e.g. `Temperature / Time`) still accept unit definitions, since offsets cancel in differences and rates.

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

`->` also distributes element-wise over indexed values: `@x -> km` on a `Length[R]` (or multi-axis `Length[R, C]`) applies the display unit to every entry.

Compound targets support the `1/unit` reciprocal shorthand, matching how unit labels are displayed: `@f -> 1/min` is equivalent to `@f -> min^-1`. Only a literal `1` is allowed as the numerator.

The conversion sets the value's *display* unit; values are always stored in SI internally. Display metadata follows value reads: a value converted at its construction site renders the same way when read back through `@x`, a struct field, an index entry, a dag output projection, a `const`, or the branch of an `if`/`match` selected at runtime. The same applies to timezone displays on `Datetime` values.

`->` is non-chaining: an expression carries at most one conversion target. Both the bare chain `@alt -> km -> m` (a parse error) and the parenthesized form `(@alt -> km) -> m` (a `D012` dimension-check error) are rejected — only the outermost target could ever take effect, so an inner conversion is either a typo or dead code.

A conversion is only allowed where its display effect can land — the top level of a declaration body, an `if`/`match` branch, a constructor field initializer, a map-literal entry, a for-comprehension body, or a `scan`/`unfold` init. Anywhere else (arithmetic operands, function arguments, comparisons, conditions, assert bodies) the conversion would be silently inert, so it is rejected (`D013`).

A conversion must also be *resolvable* at runtime: if the target's scale cannot be computed (for example, a [dynamic unit](#dynamic-units) whose scale expression evaluates to zero or a negative value), the declaration fails with a per-node error rather than silently falling back to the base unit.

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
