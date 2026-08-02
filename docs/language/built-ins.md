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
| `abs(x)` | `D -> D` or `Complex<D> -> D` | Absolute value or complex magnitude |
| `sign(x)` | `D -> Dimensionless` | Sign of value (-1.0, 0.0, or 1.0) |
| `round(x)` | `Dimensionless -> Dimensionless` | Round to nearest integer |
| `trunc(x)` | `Dimensionless -> Dimensionless` | Truncate toward zero |
| `floor(x)` | `Dimensionless -> Dimensionless` | Round toward negative infinity |
| `ceil(x)` | `Dimensionless -> Dimensionless` | Round toward positive infinity |
| `clamp(x, min, max)` | `(D, D, D) -> D` | Clamp value to range |
| `hypot(a, b)` | `(D, D) -> D` | Hypotenuse (sqrt(a^2 + b^2)) |
| `exp(x)` | `Dimensionless -> Dimensionless` or `Complex<Dimensionless> -> Complex<Dimensionless>` | Real or complex exponential |
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

### Complex Functions

`Complex<D>` is the built-in complex type over dimension `D`. Both components
are stored in SI base units and must have the same dimension. Construction is
explicit; there is no `a + bi` literal syntax.

| Function | Signature | Description |
|----------|-----------|-------------|
| `complex(re, im)` | `(D, D) -> Complex<D>` | Construct from rectangular components |
| `polar(magnitude, phase)` | `(D, Angle) -> Complex<D>` | Construct from a non-negative magnitude and phase |
| `to_complex(x)` | `D -> Complex<D>` | Promote a real quantity with zero imaginary component |
| `re(z)` | `Complex<D> -> D` | Extract the real component |
| `im(z)` | `Complex<D> -> D` | Extract the imaginary component |
| `abs(z)` | `Complex<D> -> D` | Return the Euclidean magnitude |
| `phase(z)` | `Complex<D> -> Angle` | Return the principal phase from `atan2(im, re)` |
| `conj(z)` | `Complex<D> -> Complex<D>` | Return the complex conjugate |
| `exp(z)` | `Complex<Dimensionless> -> Complex<Dimensionless>` | Return `e^z`; dimensioned input is rejected |

```graphcal
node displacement: Complex<Length> = complex(3.0 m, 4.0 m);
node same_polar: Complex<Length> = polar(5.0 m, 53.130102 deg);
node magnitude: Length = abs(@displacement); // 5 m
node direction: Angle = phase(@displacement);
node shifted: Complex<Length> = @displacement + to_complex(1.0 m);
```

`polar` rejects a negative magnitude instead of silently rotating the phase by
π. Arithmetic and complex function results must remain finite. Complex division
by zero is an evaluation error. See
[Complex Arithmetic](expressions.md#complex-arithmetic) for the mixed
real/complex operation matrix.

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
| `to_int(x)` | `Dimensionless -> Int` | Convert an integer-valued `Dimensionless` quantity exactly; reject fractional, non-finite, or out-of-range values |
| `to_int(k)` | `Key<Fin(N)> -> Int` | Extract the integer position of a `Fin` index key; named and coordinate keys are rejected |

`to_int` never chooses a rounding policy implicitly. Apply `trunc`, `floor`,
`ceil`, or `round` before converting when that is the intended policy:

```graphcal
node exact: Int = to_int(3.0);
node toward_zero: Int = to_int(trunc(3.7));
node nearest: Int = to_int(round(3.7));
```

Integrality is checked on the represented binary64 value. A conversion cannot
detect precision that was already lost while computing that value.

### Index Key Functions

These functions construct and consume [index keys](indexes.md#index-keys) of
type `Key<I>`. `C` is a coordinate axis over dimension `D`; `c` is a static
Nat constant; `e` is a runtime `Int`; `q` is a quantity of dimension `D`.

| Function | Signature | Description |
|----------|-----------|-------------|
| `key(Fin(N), c)` | `(Fin(N), static Nat) -> Key<Fin(N)>` | Static `Fin` key; the position is range-checked while the program is compiled |
| `fin_key(Fin(N), e)` | `(Fin(N), Int) -> Key<Fin(N)>` | Runtime `Fin` key; an out-of-range position is a per-node evaluation error |
| `floor_key(C, q)` | `(C, D) -> Key<C>` | Greatest grid point `<= q`; fails when `q` is below the axis range |
| `ceil_key(C, q)` | `(C, D) -> Key<C>` | Least grid point `>= q`; fails when `q` is above the axis range |
| `nearest_key(C, q)` | `(C, D) -> Key<C>` | Closest grid point; total — a midpoint tie resolves toward the axis start |
| `coord(k)` | `Key<C> -> D` | Extract the quantity coordinate of a coordinate-axis key |
| `to_int(k)` | `Key<Fin(N)> -> Int` | Extract the integer position of a `Fin` key |

Named axes need no former — a qualified label such as `Maneuver.Departure`
is itself a constant of type `Key<Maneuver>` — and have no extraction: a
named key is opaque apart from equality and `match`. There is no exact
quantity-to-key cast for coordinate axes; select a grid point with an
explicit search policy instead.

```graphcal
index TimeStep = range(0.0 s, 10.0 s, step: 0.1 s);

param event_time: Time = 3.47 s;
node before_event: Key<TimeStep> = floor_key(TimeStep, @event_time);
node event_coord: Time = coord(@before_event);

param readings: Pressure[Fin(8)] = for i: Fin(8) { 100.0 Pa };
param requested: Int = 5;
node channel: Key<Fin(8)> = fin_key(Fin(8), @requested);
node selected: Pressure = @readings[@channel];
```

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

- one-argument `datetime` requires the RFC 3339 form
  `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (RFC 3339 also permits lowercase
  `t` and `z`); numeric offset hours and minutes must be within `00..=23` and
  `00..=59`, respectively;
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
| `product(values)` | `D[I] -> D^\|I\|` | Product of all elements; cardinality determines the result dimension |
| `maximum(values)` | `D[I] -> D` | Maximum element |
| `minimum(values)` | `D[I] -> D` | Minimum element |
| `argmax(values)` | `D[I] -> Key<I>` | Key of the maximum element; ties resolve to the first extremum in index order |
| `argmin(values)` | `D[I] -> Key<I>` | Key of the minimum element; ties resolve to the first extremum in index order |
| `mean(values)` | `D[I] -> D` | Arithmetic mean |
| `rss(values)` | `D[I] -> D` | Root sum square, computed with scaled accumulation |
| `count(values)` | `T[I] -> Int` | Exact number of elements |

`argmax` and `argmin` return the extremum's *location* as an
[index key](indexes.md#index-keys) rather than its value, so the result can
re-index the source or any other value on the same axis. Because axes are
never empty and ties are deterministic, the identity
`@x[argmax(@x)] == maximum(@x)` always holds:

```graphcal
index Maneuver = { Departure, Correction, Insertion };
param delta_v: Velocity[Maneuver] = {
    Maneuver.Departure: 2.46 km/s,
    Maneuver.Correction: 0.12 km/s,
    Maneuver.Insertion: 1.83 km/s,
};
param fuel_margin: Dimensionless[Maneuver] = {
    Maneuver.Departure: 1.1,
    Maneuver.Correction: 1.2,
    Maneuver.Insertion: 1.3,
};

node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];        // 2.46 km/s
node critical_margin: Dimensionless = @fuel_margin[@critical];
```

`product` makes chained reductions explicit without deeply nested binary
multiplication. A dimensioned product requires a concrete axis cardinality so
its output exponent is known; a dimensionless product does not. `rss` is the
unit-safe quadrature operation for uncertainty budgets and Euclidean component
combinations, and avoids avoidable intermediate overflow/underflow. Keep a
budget's deterministic means and statistical sigmas as separate values, then
combine each according to its semantics:

```graphcal
index BudgetLine = { Structure, Payload };
param means: Mass[BudgetLine] = {
    BudgetLine.Structure: 100.0 kg,
    BudgetLine.Payload: 50.0 kg,
};
param sigmas: Mass[BudgetLine] = {
    BudgetLine.Structure: 3.0 kg,
    BudgetLine.Payload: 4.0 kg,
};
node budget_mean: Mass = sum(@means);   // 150 kg
node budget_sigma: Mass = rss(@sigmas); // 5 kg
```

Convert a count explicitly when quantity arithmetic needs a dimensionless
scalar: `sum(@values) / to_float(count(@values))`. Direct multi-axis aggregation
is rejected; reduce one selected axis at a time with an explicit `for`, such as
`product(for row: Row { product(for column: Column { @a[row, column] }) })`.

### Frame-Tagged Vectors and Attitudes

Reference frames are project-specific nominal types, so Graphcal does not
provide global `Eci`, `Body`, or similarly named frame values. Define the
frames and wrappers in project vocabulary, then use the array linear-algebra
built-ins for their numeric kernels:

```graphcal
type Eci { Eci }
type Body { Body }
type Vec3<D: Dim, Frame: Type> { Vec3(x: D, y: D, z: D) }
type Rotation<From: Type, To: Type> {
    Rotation(elements: Dimensionless[Fin(3), Fin(3)])
}
type Quaternion<From: Type, To: Type> {
    Quaternion(w: Dimensionless, x: Dimensionless, y: Dimensionless, z: Dimensionless)
}

node r: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
node q: Quaternion<Eci, Body> =
    Quaternion<Eci, Body>(w: 1.0, x: 0.0, y: 0.0, z: 0.0);
```

The phantom `Frame` / `From` / `To` arguments prevent accidental frame
reinterpretation at type-check time. The
[`linear_algebra_ergonomics.gcl`](https://github.com/graphcal-lang/graphcal/blob/main/tests/fixtures/valid/linear_algebra_ergonomics.gcl)
fixture demonstrates an explicit matrix-vector frame transition. Projects can
package numeric component operations in reusable DAGs with required type-level inputs;
see
[DAG Blocks](functions.md#type-level-inputs-for-reusable-math).

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
