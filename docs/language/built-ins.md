---
icon: material/package-variant
---

# Built-in Reference

This page lists all dimensions, units, constants, and functions provided by the Graphcal prelude. These are available in every `.gcl` file without any `use` imports.

## Built-in Constants

| Name | Type | Value |
|------|------|-------|
| `PI` | `Dimensionless` | 3.14159265358979... |
| `E` | `Dimensionless` | 2.71828182845904... |

## Built-in Functions

### Math Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `sqrt(x)` | `D^2 -> D` | Square root (dimension halved) |
| `abs(x)` | `D -> D` | Absolute value |
| `floor(x)` | `Dimensionless -> Dimensionless` | Round toward negative infinity |
| `ceil(x)` | `Dimensionless -> Dimensionless` | Round toward positive infinity |
| `exp(x)` | `Dimensionless -> Dimensionless` | Exponential (e^x) |
| `ln(x)` | `Dimensionless -> Dimensionless` | Natural logarithm |

### Trigonometric Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `sin(x)` | `Angle -> Dimensionless` | Sine |
| `cos(x)` | `Angle -> Dimensionless` | Cosine |
| `tan(x)` | `Angle -> Dimensionless` | Tangent |
| `asin(x)` | `Dimensionless -> Angle` | Inverse sine |
| `acos(x)` | `Dimensionless -> Angle` | Inverse cosine |
| `atan2(y, x)` | `(D, D) -> Angle` | Two-argument inverse tangent |

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
