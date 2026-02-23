---
icon: material/package-variant
---

# Built-in Reference

This page lists all dimensions, units, constants, and functions provided by the Graphcal prelude. These are available in every `.gcl` file without any `import` declarations.

## Built-in Constants

| Name | Type | Value |
|------|------|-------|
| `PI` | `Dimensionless` | 3.14159265358979... |
| `E` | `Dimensionless` | 2.71828182845904... |
| `TAU` | `Dimensionless` | 6.28318530717958... (2*PI) |
| `SQRT2` | `Dimensionless` | 1.41421356237309... |
| `LN2` | `Dimensionless` | 0.69314718055994... |
| `LN10` | `Dimensionless` | 2.30258509299404... |

## Built-in Functions

### Math Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `sqrt(x)` | `D -> D^(1/2)` | Square root (dimension halved) |
| `cbrt(x)` | `D -> D^(1/3)` | Cube root (dimension divided by 3) |
| `abs(x)` | `D -> D` | Absolute value |
| `sign(x)` | `D -> Dimensionless` | Sign of value (1.0, -1.0, or NaN) |
| `round(x)` | `D -> D` | Round to nearest integer |
| `trunc(x)` | `D -> D` | Truncate toward zero |
| `floor(x)` | `D -> D` | Round toward negative infinity |
| `ceil(x)` | `D -> D` | Round toward positive infinity |
| `clamp(x, min, max)` | `(D, D, D) -> D` | Clamp value to range |
| `hypot(a, b)` | `(D, D) -> D` | Hypotenuse (sqrt(a^2 + b^2)) |
| `exp(x)` | `Dimensionless -> Dimensionless` | Exponential (e^x) |
| `expm1(x)` | `Dimensionless -> Dimensionless` | exp(x) - 1 (numerically stable for small x) |
| `ln(x)` | `Dimensionless -> Dimensionless` | Natural logarithm |
| `log1p(x)` | `Dimensionless -> Dimensionless` | ln(1 + x) (numerically stable for small x) |
| `log(x, base)` | `(Dimensionless, Dimensionless) -> Dimensionless` | Logarithm with arbitrary base |
| `log2(x)` | `Dimensionless -> Dimensionless` | Base-2 logarithm |
| `log10(x)` | `Dimensionless -> Dimensionless` | Base-10 logarithm |

### Trigonometric Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `sin(x)` | `Angle -> Dimensionless` | Sine |
| `cos(x)` | `Angle -> Dimensionless` | Cosine |
| `tan(x)` | `Angle -> Dimensionless` | Tangent |
| `asin(x)` | `Dimensionless -> Angle` | Inverse sine |
| `acos(x)` | `Dimensionless -> Angle` | Inverse cosine |
| `atan(x)` | `Dimensionless -> Angle` | Inverse tangent |
| `atan2(y, x)` | `(D, D) -> Angle` | Two-argument inverse tangent |

### Hyperbolic Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `sinh(x)` | `Dimensionless -> Dimensionless` | Hyperbolic sine |
| `cosh(x)` | `Dimensionless -> Dimensionless` | Hyperbolic cosine |
| `tanh(x)` | `Dimensionless -> Dimensionless` | Hyperbolic tangent |
| `asinh(x)` | `Dimensionless -> Dimensionless` | Inverse hyperbolic sine |
| `acosh(x)` | `Dimensionless -> Dimensionless` | Inverse hyperbolic cosine |
| `atanh(x)` | `Dimensionless -> Dimensionless` | Inverse hyperbolic tangent |

### Comparison Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `min(a, b)` | `(D, D) -> D` | Minimum of two values |
| `max(a, b)` | `(D, D) -> D` | Maximum of two values |

### Type Conversion Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `to_float(x)` | `Int -> Dimensionless` | Convert integer to float |
| `to_int(x)` | `Dimensionless -> Int` | Convert float to integer (truncates toward zero) |

### Aggregation Functions (Indexed Values)

These functions operate on `for` comprehensions or indexed values:

| Function | Signature | Description |
|----------|-----------|-------------|
| `sum(...)` | `D[I] -> D` | Sum of all elements |
| `max(...)` | `D[I] -> D` | Maximum element |
| `min(...)` | `D[I] -> D` | Minimum element |
| `mean(...)` | `D[I] -> D` | Arithmetic mean |
| `count(...)` | `D[I] -> Dimensionless` | Number of elements |

## Prelude Base Dimensions

| Dimension | Description |
|-----------|-------------|
| `Length` | Spatial distance |
| `Time` | Temporal duration |
| `Mass` | Mass |
| `Temperature` | Thermodynamic temperature |
| `ElectricCurrent` | Electric current |
| `Amount` | Amount of substance |
| `LuminousIntensity` | Luminous intensity |
| `Angle` | Plane angle |
| `Dimensionless` | No physical dimension |

## Prelude Derived Dimensions

| Dimension | Definition |
|-----------|-----------|
| `Velocity` | `Length / Time` |
| `Acceleration` | `Length / Time^2` |
| `Force` | `Mass * Length / Time^2` |
| `Energy` | `Mass * Length^2 / Time^2` |
| `Power` | `Mass * Length^2 / Time^3` |
| `Pressure` | `Mass / (Length * Time^2)` |
| `Frequency` | `1 / Time` |
| `Area` | `Length^2` |
| `Volume` | `Length^3` |

## Prelude Units

### Length

| Unit | Definition |
|------|-----------|
| `m` | Base unit (meter) |
| `km` | 1000 m |
| `cm` | 0.01 m |
| `mm` | 0.001 m |

### Time

| Unit | Definition |
|------|-----------|
| `s` | Base unit (second) |
| `min` | 60 s |
| `hour` | 3600 s |

### Mass

| Unit | Definition |
|------|-----------|
| `kg` | Base unit (kilogram) |
| `g` | 0.001 kg |

### Temperature

| Unit | Definition |
|------|-----------|
| `K` | Base unit (kelvin) |

### Electric Current

| Unit | Definition |
|------|-----------|
| `A` | Base unit (ampere) |

### Amount of Substance

| Unit | Definition |
|------|-----------|
| `mol` | Base unit (mole) |

### Luminous Intensity

| Unit | Definition |
|------|-----------|
| `cd` | Base unit (candela) |

### Angle

| Unit | Definition |
|------|-----------|
| `rad` | Base unit (radian) |
| `deg` | pi/180 rad |

### Force

| Unit | Definition |
|------|-----------|
| `N` | 1 kg*m/s^2 |
| `kN` | 1000 N |

### Energy

| Unit | Definition |
|------|-----------|
| `J` | 1 N*m |
| `kJ` | 1000 J |

### Power

| Unit | Definition |
|------|-----------|
| `W` | 1 J/s |
| `kW` | 1000 W |

### Pressure

| Unit | Definition |
|------|-----------|
| `Pa` | 1 N/m^2 |
| `kPa` | 1000 Pa |
| `MPa` | 1000000 Pa |

### Frequency

| Unit | Definition |
|------|-----------|
| `Hz` | 1/s |
