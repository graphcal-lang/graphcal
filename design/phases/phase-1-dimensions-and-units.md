# Phase 1: Dimensions and Units

> Add the dimension/unit type system. The core value proposition:
> "the compiler catches `km + kg`."

## Goal

Prove that compile-time dimensional analysis works through arithmetic.
Values carry physical dimensions as types and units as scaling factors.
The compiler rejects dimension mismatches before evaluation.

## Prerequisites

Phase 0 (Scalar Graph) must be complete. Phase 1 extends Phase 0:

- Bare `f64` literals become `Dimensionless` values.
- Existing Phase 0 `.gcl` files are **not** valid in Phase 1 (type annotations are now required).
  This is an accepted breaking change.
- The parser and evaluator are extended, not replaced.

## Design Decisions -- Locked

### From [04-dimensions-and-units](../04-dimensions-and-units.md)

- [x] **Internal representation:** Values are always stored internally in base SI units.
      E.g., `400 km` stored as `400_000.0` (meters). Display unit tracked as metadata.
- [x] **Dimension representation:** Dimensions as a vector of rational exponents over
      8 base dimensions. E.g., `Velocity = [Length: 1, Time: -1, ...]`.
      Multiply = add exponents, divide = subtract exponents.
- [x] **Base dimensions:** All 7 SI base dimensions + Angle (8 total):
      `Length`, `Time`, `Mass`, `Temperature`, `ElectricCurrent`, `Amount`,
      `LuminousIntensity`, `Angle`.
- [x] **Fractional exponents:** Rational (numerator/denominator) from the start.
      `sqrt` halves exponents; arbitrary rationals are supported.
- [x] **Temperature:** Base dimension included. Affine conversions (Celsius/Fahrenheit)
      deferred to a later phase. Kelvin works as a linear unit.
- [x] **Custom counting units:** Deferred. `unit launch;` auto-creating a dimension
      is not in Phase 1 scope.
- [x] **Display units:** If a value was written with a unit literal, display in that unit.
      If computed, display in base SI. Explicit `->` conversion overrides display unit.
      Clever unit inference deferred.
- [x] **User-defined dimensions in the same file:** Yes. `dimension MyDim;` and
      `dimension Velocity = Length / Time;` are supported in regular `.gcl` files.
- [x] **Unit prefixes:** Individual declarations (no prefix mechanism).
      `unit km: Length = 1000 m;` is declared individually.

### From [02-syntax-design](../02-syntax-design.md) (extensions)

- [x] **Type annotation syntax:** `param alt: Length = 400 km;` -- `: Type` after
      the name, before `=`. Confirmed.
- [x] **Literal-with-unit syntax:** `400 km` is parsed as `NUMBER UNIT_EXPR`.
      The unit expression binds tightly to the preceding number literal.
      `9.80665 m/s^2` is `NUMBER` followed by compound `UNIT_EXPR` (`m/s^2`).
      This is distinct from multiplication.
- [x] **Unit conversion operator:** `@fuel_mass -> lb` confirmed. `->` has the
      lowest precedence (just above `;`), so `@a + @b -> km` means `(@a + @b) -> km`.
- [x] **Inline dimension expressions in types:** Supported. `param gm: Length^3 / Time^2 = ...;`
      works directly without requiring a `dimension` declaration first.
- [x] **Type annotation policy:** Type annotations are **required** on all declarations
      (`param`, `node`, `const`). Literals inside expressions have their types inferred
      (`2.0` → `Dimensionless`, `400 km` → `Length`). The compiler checks that the
      inferred dimension of the RHS matches the declared type.

### From [03-primitives](../03-primitives.md) (subset)

- [x] **Dimensionless type:** Named `Dimensionless` (not `f64`). Bare numeric literals
      like `2.0` are inferred as `Dimensionless`. `exp()`, `ln()` require `Dimensionless`
      arguments.
- [x] **`i64` with dimensions:** Deferred. Phase 1 is `f64` only.

### Prelude (Phase 1 subset)

- [x] **How to ship the prelude:** Hard-coded in the compiler. Can migrate to
      auto-loaded `.gcl` prelude in Phase 4 (multi-file).
- [x] **Prelude contents:** See "Prelude Contents" section below.

## Syntax Supported in Phase 1

Everything from Phase 0, plus:

```ebnf
// New declarations
DimDecl      = "dimension" IDENT ("=" DimExpr)? ";"
UnitDecl     = "unit" IDENT ":" DimExpr ("=" Expr UNIT_IDENT)? ";"

// Extended declarations (type annotations REQUIRED)
ParamDecl    = "param" LOWER_IDENT ":" TypeExpr "=" Expr ";"
NodeDecl     = "node"  LOWER_IDENT ":" TypeExpr "=" Expr ";"
ConstDecl    = "const" UPPER_IDENT ":" TypeExpr "=" Expr ";"

// Type expressions (dimension expressions or Dimensionless)
TypeExpr     = "Dimensionless" | DimExpr
DimExpr      = DimTerm (("*" | "/") DimTerm)*
DimTerm      = IDENT ("^" INTEGER)?       // e.g., Length^2, Time^-1

// Unit-annotated literals (binds tightly to preceding number)
UnitLiteral  = NUMBER UNIT_EXPR
UNIT_EXPR    = UNIT_IDENT (("*" | "/") UNIT_IDENT ("^" INTEGER)?)*
             // e.g., km, m/s^2, km^3/s^2

// Unit conversion (lowest precedence, just above ;)
ConvertExpr  = Expr "->" UNIT_EXPR
```

## Prelude Contents

### Base Dimensions (8)

```gcl
dimension Length;
dimension Time;
dimension Mass;
dimension Temperature;
dimension ElectricCurrent;
dimension Amount;
dimension LuminousIntensity;
dimension Angle;
```

### Derived Dimensions (initial set)

```gcl
dimension Velocity = Length / Time;
dimension Acceleration = Length / Time^2;
dimension Force = Mass * Acceleration;
dimension Energy = Force * Length;
dimension Power = Energy / Time;
dimension Frequency = Dimensionless / Time;
dimension Pressure = Force / Length^2;
dimension Area = Length^2;
dimension Volume = Length^3;
```

### Base Units (SI)

```gcl
unit m: Length;
unit s: Time;
unit kg: Mass;
unit K: Temperature;
unit A: ElectricCurrent;
unit mol: Amount;
unit cd: LuminousIntensity;
unit rad: Angle;
```

### Derived and Prefixed Units (initial set)

```gcl
unit km: Length = 1000 m;
unit cm: Length = 0.01 m;
unit mm: Length = 0.001 m;
unit hour: Time = 3600 s;
unit min: Time = 60 s;
unit deg: Angle = (PI / 180) rad;
unit N: Force = 1 kg * m / s^2;
unit kN: Force = 1000 N;
unit J: Energy = 1 N * m;
unit kJ: Energy = 1000 J;
unit W: Power = 1 J / s;
unit kW: Power = 1000 W;
unit Pa: Pressure = 1 N / m^2;
unit kPa: Pressure = 1000 Pa;
unit MPa: Pressure = 1000000 Pa;
unit Hz: Frequency = 1 / s;
unit g: Mass = 0.001 kg;
```

### Built-in Constants

```gcl
PI: Dimensionless
E: Dimensionless
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Dimension type** | Represent dimensions as exponent vectors of 8 rationals; implement algebra |
| **Unit registry** | Unit registry with scaling factors to base SI; compound unit support |
| **Literal parser** | Parse `400 km`, `9.80665 m/s^2` as unit-annotated literals |
| **Type annotation parser** | Parse required `: Length`, `: Dimensionless`, `: Length^3 / Time^2` |
| **Dimension checker** | Infer dimensions through expressions; check algebra rules; verify declared type matches |
| **Conversion operator** | Implement `->` for unit conversion (display hint + dimension check) |
| **Prelude** | Hard-coded base/derived dimensions, units, constants |
| **Display** | Show values in literal unit, base SI for computed, or `->` target unit |
| **Error messages** | "Cannot add Length + Time", "exp() requires Dimensionless", "expected Mass, got Length" |

## Out of Scope

- Structs, multi-line bodies
- Functions
- Multi-file, imports
- Tables
- Spaces, indexes
- `i64`, `bool`, `Str`, `Datetime`, `Option`
- Affine temperature conversions (Celsius, Fahrenheit)
- Custom counting units (`unit launch;`)
- Unit prefix mechanism

## Milestone Test

```gcl
// orbital.gcl
dimension Velocity = Length / Time;

param alt: Length = 400 km;
param period: Time = 90 min;
const R_EARTH: Length = 6371 km;

node circumference: Length = 2.0 * PI * (@R_EARTH + @alt);
node speed: Velocity = @circumference / @period;
node speed_kmh: Velocity = @speed -> km/hour;
```

```sh
$ graphcal eval orbital.gcl
alt            = 400 km
period         = 90 min
R_EARTH        = 6371 km
circumference  = 42543.38 km
speed          = 7.091 km/s
speed_kmh      = 25527.0 km/hour
```

### Error cases that must work

```gcl
// error: dimension mismatch in addition
node bad: Length = @alt + @period;
//  error: cannot add Length + Time

// error: exp requires dimensionless
node also_bad: Dimensionless = exp(@alt);
//  error: exp() requires Dimensionless argument, got Length

// error: unknown unit
node x: Length = 100 foobar;
//  error: unknown unit `foobar`

// error: missing type annotation
node y = 42.0;
//  error: type annotation required on node declaration

// error: declared type does not match expression
node wrong: Mass = @alt + 100 km;
//  error: expected Mass, got Length
```

## Open Questions

All previously open questions have been resolved. See the "Design Decisions -- Locked"
section above for the full list of decisions.

Additional resolved questions:

- **Compound unit display format:** Use `/` notation matching user input (`m/s`, `km/hour`),
  not `m*s^-1` or `m s^{-1}`.
