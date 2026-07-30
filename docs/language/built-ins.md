---
icon: material/package-variant
---

# Built-in Reference

This page lists all dimensions, units, constants, and functions provided by the Graphcal prelude. These are available in every `.gcl` file without any `import` declarations.

The set of bare-callable functions is closed: user code cannot add to it. Externally-provided functions are declared with `import plugin` blocks and called qualified through their alias — see [Extern Functions (Plugins)](extern-functions.md).

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
| `sign(x)` | `D -> Dimensionless` | Sign of value (-1.0, 0.0, or 1.0) |
| `round(x)` | `Dimensionless -> Dimensionless` | Round to nearest integer |
| `trunc(x)` | `Dimensionless -> Dimensionless` | Truncate toward zero |
| `floor(x)` | `Dimensionless -> Dimensionless` | Round toward negative infinity |
| `ceil(x)` | `Dimensionless -> Dimensionless` | Round toward positive infinity |
| `clamp(x, min, max)` | `(D, D, D) -> D` | Clamp value to range |
| `hypot(a, b)` | `(D, D) -> D` | Hypotenuse (sqrt(a^2 + b^2)) |
| `exp(x)` | `Dimensionless -> Dimensionless` | Exponential (e^x) |
| `expm1(x)` | `Dimensionless -> Dimensionless` | exp(x) - 1 (numerically stable for small x) |
| `ln(x)` | `Dimensionless -> Dimensionless` | Natural logarithm |
| `log1p(x)` | `Dimensionless -> Dimensionless` | ln(1 + x) (numerically stable for small x) |
| `log(x, base)` | `(Dimensionless, Dimensionless) -> Dimensionless` | Logarithm with arbitrary base |
| `log2(x)` | `Dimensionless -> Dimensionless` | Base-2 logarithm |
| `log10(x)` | `Dimensionless -> Dimensionless` | Base-10 logarithm |

The rounding functions (`round`, `trunc`, `floor`, `ceil`) reject dimensioned
arguments: "round to an integer" has no unit-independent meaning for a physical
quantity -- an integer number of meters is not an integer number of
centimeters. To round a quantity, divide by an explicit granularity, round the
dimensionless ratio, and scale back:

```graphcal
// Round down to whole centimeters.
node whole_cm: Length = floor(@x / 1.0 cm) * 1.0 cm;
```

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

### Selection Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `least(a, b)` | `(D, D) -> D` | Lesser of two values |
| `greatest(a, b)` | `(D, D) -> D` | Greater of two values |

These selectors always take exactly two values. To reduce an indexed value, use
`minimum(values)` or `maximum(values)` instead.

### Type Conversion Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `to_float(x)` | `Int -> Dimensionless` | Convert an integer to a binary64 `Dimensionless` quantity |
| `to_int(x)` | `Dimensionless -> Int` | Convert a finite `Dimensionless` quantity to an integer (truncates toward zero; rejects values outside `i64` range) |

### Datetime Functions

#### Constructors

| Function | Signature | Description |
|----------|-----------|-------------|
| `datetime("...")` | `OffsetDateTimeLiteral -> Datetime` | Parse RFC 3339 with an explicit `Z` or numeric offset |
| `datetime("...", "tz")` | `(CivilDateTimeLiteral, TimezoneLiteral) -> Datetime` | Interpret local civil time in an IANA timezone |
| `epoch<S>("...")` | `CivilDateTimeLiteral -> Datetime<S>` | Interpret scale-free calendar coordinates in static time scale `S` |
| `from_jd(x)` | `Dimensionless -> Datetime` | Construct from Julian Date (UTC) |
| `from_mjd(x)` | `Dimensionless -> Datetime` | Construct from Modified Julian Date (UTC) |
| `from_unix(x)` | `Dimensionless -> Datetime` | Construct from Unix timestamp in seconds |

Each constructor has exactly one interpretation source:

- one-argument `datetime` requires `Z` or a numeric offset;
- timezone-based `datetime` requires local civil coordinates with no embedded
  offset, timezone, or time-scale suffix;
- `epoch<S>` requires local calendar coordinates with no embedded offset,
  timezone, or time-scale suffix, and `S` must be one supported bare time
  scale.

Datetime literals are parsed during checking. Invalid calendar dates and
missing or contradictory interpretation sources are compile errors, not
runtime failures. Local civil times are also resolved against Graphcal's
bundled tzdb during checking: nonexistent times in DST gaps and repeated times
in DST folds are rejected rather than shifted or selected silently. To choose a
specific instant, use one-argument `datetime` with an explicit numeric offset.
The obsolete positional form `epoch("...", TT)` is rejected; write
`epoch<TT>("...")`.

#### Time Scale Conversions

| Function | Signature | Description |
|----------|-----------|-------------|
| `to_utc(x)` | `Datetime(any) -> Datetime<UTC>` | Convert to UTC |
| `to_tai(x)` | `Datetime(any) -> Datetime<TAI>` | Convert to TAI |
| `to_tt(x)` | `Datetime(any) -> Datetime<TT>` | Convert to Terrestrial Time |
| `to_tdb(x)` | `Datetime(any) -> Datetime<TDB>` | Convert to Barycentric Dynamical Time |
| `to_et(x)` | `Datetime(any) -> Datetime<ET>` | Convert to Ephemeris Time |
| `to_gpst(x)` | `Datetime(any) -> Datetime<GPST>` | Convert to GPS Time |
| `to_gst(x)` | `Datetime(any) -> Datetime<GST>` | Convert to Galileo System Time |
| `to_bdt(x)` | `Datetime(any) -> Datetime<BDT>` | Convert to BeiDou Time |
| `to_qzsst(x)` | `Datetime(any) -> Datetime<QZSST>` | Convert to QZSS Time |

#### Numeric Conversions

| Function | Signature | Description |
|----------|-----------|-------------|
| `to_jd(x)` | `Datetime -> Dimensionless` | Convert to Julian Date (UTC days) |
| `to_mjd(x)` | `Datetime -> Dimensionless` | Convert to Modified Julian Date (UTC days) |
| `to_unix(x)` | `Datetime -> Dimensionless` | Convert to Unix timestamp (seconds since 1970-01-01) |

#### Extraction Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `year(x)` | `Datetime<S> -> Int` | Extract year |
| `month(x)` | `Datetime<S> -> Int` | Extract month (1-12) |
| `day(x)` | `Datetime<S> -> Int` | Extract day of month (1-31) |
| `hour(x)` | `Datetime<S> -> Int` | Extract hour (0-23) |
| `minute(x)` | `Datetime<S> -> Int` | Extract minute (0-59) |
| `second(x)` | `Datetime<S> -> Int` | Extract second (0-59) |
| `weekday(x)` | `Datetime<S> -> Int` | ISO weekday (1=Monday, 7=Sunday) |
| `day_of_year(x)` | `Datetime<S> -> Int` | Day of year (1-366) |

All calendar extractors use the input's declared time scale. For example,
`hour(epoch<TT>("2024-11-05T12:00:00"))` is `12`, even though the same instant
has a different UTC coordinate. A display timezone applied with `->` remains
formatting metadata and does not change extractor results.

#### Datetime Domain Constraints

`Datetime<S>` supports inclusive `min:` and `max:` constraints. Bounds are
compile-time constants and must have exactly the constrained time scale:

```graphcal
param event: Datetime<TT>(
    min: epoch<TT>("2025-01-01T00:00:00"),
    max: epoch<TT>("2025-12-31T23:59:59"),
) = epoch<TT>("2025-06-01T12:00:00");
```

Cross-scale bounds require an explicit conversion such as
`to_tt(datetime("...Z"))`. Constraints apply element-wise to indexed datetime
values and to constrained algebraic payload fields. See
[Type System — Domain Constraints](type-system.md#domain-constraints).

#### Timezone Display

The `->` operator can display a datetime in a specific timezone without changing the underlying value:

```
node meeting_ny: Datetime = @meeting -> "America/New_York";
```

Quoted timezone spellings are contextual `TimezoneLiteral`s, not first-class
`String` values. They are validated and canonicalized while the program is
checked, then carried through compilation and evaluation as opaque IANA
identifiers.

Timezone construction, validation, and display all use the same IANA timezone
database bundled with the Graphcal toolchain. Graphcal never consults the
host's zoneinfo installation, so a given Graphcal release resolves timezone
rules identically across supported platforms. Invalid-timezone diagnostics
report the bundled IANA tzdb release; updating those rules is an explicit
Graphcal dependency update.

### Aggregation Functions (Indexed Values)

These functions operate on rank-one `for` comprehensions or indexed values.
`D` is a quantity dimension; `T` may be any non-indexed value type.

| Function | Signature | Description |
|----------|-----------|-------------|
| `sum(values)` | `D[I] -> D` | Sum of all elements |
| `maximum(values)` | `D[I] -> D` | Maximum element |
| `minimum(values)` | `D[I] -> D` | Minimum element |
| `mean(values)` | `D[I] -> D` | Arithmetic mean |
| `count(values)` | `T[I] -> Int` | Exact number of elements |

Convert a count explicitly when quantity arithmetic needs a dimensionless
scalar: `sum(@values) / to_float(count(@values))`. Direct multi-axis aggregation
is rejected; reduce one selected axis at a time with an explicit `for`.

### Linear Algebra (Indexed Quantities)

Linear-algebra built-ins operate on rank-one vectors and rank-two matrices.
Axes are part of the type and contractions require the same typed axis, not
merely the same number of entries. `Fin(N)` axes match structurally; named axes
match by their declaration identity.

| Function | Signature | Description |
|----------|-----------|-------------|
| `dot(a, b)` | `(D1[I], D2[I]) -> D1 * D2` | Dot product |
| `matmul(a, b)` | `(D1[I, J], D2[J, K]) -> (D1 * D2)[I, K]` | Matrix product over `J` |
| `transpose(a)` | `D[I, J] -> D[J, I]` | Swap matrix axes |
| `trace(a)` | `D[I, I] -> D` | Sum the diagonal of a square typed-axis matrix |
| `norm(v)` | `D[I] -> D` | Euclidean vector norm |
| `cross(a, b)` | `(D1[I], D2[I]) -> (D1 * D2)[I]` | Three-component cross product; `I` must have exactly three entries |
| `outer(a, b)` | `(D1[I], D2[J]) -> (D1 * D2)[I, J]` | Outer product |
| `solve(a, b)` | `(D1[I, I], D2[I]) -> (D2 / D1)[I]` | Solve the square system `a * x = b` |
| `inverse(a)` | `D[I, I] -> D^-1[I, I]` | Matrix inverse |
| `det(a)` | `D[I, I] -> D^\|I\|` | Determinant; the axis cardinality becomes the dimension exponent |

```graphcal
param A: Length[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0 m, 2.0 m, 3.0 m;
    4.0 m, 5.0 m, 6.0 m;
};
param B: Dimensionless[Fin(3), Fin(2)] = table[Fin(3), Fin(2)] {
    7.0, 8.0;
    9.0, 10.0;
    11.0, 12.0;
};

node AB: Length[Fin(2), Fin(2)] = matmul(@A, @B);
node A_t: Length[Fin(3), Fin(2)] = transpose(@A);
```

There is no implicit axis selection or broadcasting. For example, `matmul`
always contracts the second axis of its first argument with the first axis of
its second argument. Distinct named axes with equal cardinality are rejected.

`solve` and `inverse` use scaled partial-pivoting LU, reject singular or
numerically ill-conditioned matrices, and verify the residual before returning
a result. These failures are explicit evaluation errors; they never return
`NaN` or infinity. `det` remains defined for singular matrices and returns
zero. Because a determinant's physical dimension is `D` raised to the matrix
order, `det` requires a concrete axis cardinality.

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
| `h` | 3600 s |

`h` is the only prelude spelling for the hour unit; `hour(value)` remains the
datetime extractor.

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
