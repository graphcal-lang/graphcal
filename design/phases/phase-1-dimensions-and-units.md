# Phase 1: Dimensions and Units

> Add the dimension/unit type system. The core value proposition:
> "the compiler catches `km + kg`."

## Goal

Prove that compile-time dimensional analysis works through arithmetic.
Values carry physical dimensions as types and units as scaling factors.
The compiler rejects dimension mismatches before evaluation.

## Prerequisites

Phase 0 (Scalar Graph) must be complete. Phase 1 extends Phase 0:

- `f64` literals without units become "dimensionless `f64`" -- existing Phase 0 files remain valid.
- The parser and evaluator are extended, not replaced.

## Design Decisions to Lock

### From [04-dimensions-and-units](../04-dimensions-and-units.md)

- [ ] **Internal representation:** Are values always stored internally in base SI units?
      E.g., `400 km` stored as `400000.0` (meters). This simplifies arithmetic but
      complicates display. Alternatively, store value + unit and convert on demand.
- [ ] **Dimension representation:** Dimensions as a vector of exponents over base
      dimensions (Length, Time, Mass, Angle, ...)?
      E.g., `Velocity = Length^1 * Time^-1` is the vector `[1, -1, 0, 0]`.
- [ ] **Base dimensions:** What is the minimal set? `Length`, `Time`, `Mass`, `Angle`.
      Others (`Temperature`, `ElectricCurrent`, `Amount`, `LuminousIntensity`)?
      Start minimal, extend later.
- [ ] **Fractional exponents:** Support `Length^(1/2)` for `sqrt(area)`? Or restrict
      to integer exponents and handle `sqrt` specially?
- [ ] **Temperature:** Affine conversions (Celsius/Fahrenheit) are non-trivial. Defer
      to a later phase or handle from the start?
- [ ] **Custom counting units:** `unit launch;` auto-creates a dimension. Include this
      in Phase 1, or defer?
- [ ] **Display units:** When printing `node speed: Velocity = ...`, which unit is shown?
      The "most natural" one? The one from the declaration? A default per dimension?
- [ ] **User-defined dimensions in the same file:** Can the user write `dimension MyDim;`
      in a regular `.graph` file, or only in the prelude?
      Phase 1 is single-file, so this matters.
- [ ] **Unit prefixes:** Support `kilo`, `mega`, etc. as a mechanism, or declare each
      prefixed unit individually?

### From [02-syntax-design](../02-syntax-design.md) (extensions)

- [ ] **Type annotation syntax:** `param alt: Length = 400 km;` -- confirm `: Type`
      after the name, before `=`.
- [ ] **Literal-with-unit syntax:** Is `400 km` parsed as `400` then `km`?
      What about `9.80665 m/s^2`? Is this `9.80665 * m / s^2`?
      Define the grammar precisely.
- [ ] **Unit conversion operator:** `@fuel_mass -> lb` confirmed? What is the
      precedence of `->`?
- [ ] **Inline dimension expressions in types:** `param gm: Length^3 / Time^2 = ...;`
      Is this supported, or must the user declare `dimension GravParam = Length^3 / Time^2;` first?
- [ ] **Type annotation policy:** Are type annotations on `param`/`node`/`const`
      optional (with inference), required, or lint-warned-if-omitted?
      The "Language for Agents" insights recommend explicit annotations.

### From [03-primitives](../03-primitives.md) (subset)

- [ ] **Dimensionless f64:** `f64` without a unit is dimensionless. Confirm that
      `exp()`, `ln()` require dimensionless arguments.
- [ ] **`i64` with dimensions:** Defer to a later phase. Phase 1 is `f64` only.

### Prelude (Phase 1 subset)

- [ ] **How to ship the prelude:** Hard-coded in the compiler? A built-in `.graph`
      file that's auto-loaded? Phase 1 is single-file, so the prelude must be
      implicit (no `use` yet).
- [ ] **Prelude contents:** At minimum: base dimensions (`Length`, `Time`, `Mass`, `Angle`),
      derived dimensions (`Velocity`, `Acceleration`, `Force`, `Energy`, `Power`),
      base units (`m`, `s`, `kg`, `rad`), derived units (`km`, `N`, `hour`, `deg`, etc.),
      constants (`pi`, `e`).

## Syntax Supported in Phase 1

Everything from Phase 0, plus:

```ebnf
// New declarations
DimDecl      = "dimension" IDENT ("=" DimExpr)? ";"
UnitDecl     = "unit" IDENT ":" DimExpr ("=" Expr UNIT_IDENT)? ";"

// Extended declarations (now with optional type annotations)
ParamDecl    = "param" IDENT (":" TypeExpr)? "=" Expr ";"
NodeDecl     = "node"  IDENT (":" TypeExpr)? "=" Expr ";"
ConstDecl    = "const" IDENT (":" TypeExpr)? "=" Expr ";"

// Type expressions
TypeExpr     = DimExpr                    // e.g., Length, Velocity, Length^3 / Time^2
DimExpr      = DimTerm (("*" | "/") DimTerm)*
DimTerm      = IDENT ("^" INTEGER)?       // e.g., Length^2, Time^-1

// Unit-annotated literals
UnitLiteral  = NUMBER UNIT_IDENT (("*" | "/") UNIT_IDENT ("^" INTEGER)?)*
             // e.g., 400 km, 9.80665 m/s^2, 3.98e5 km^3/s^2

// Unit conversion
ConvertExpr  = Expr "->" UNIT_IDENT (("*" | "/") UNIT_IDENT ("^" INTEGER)?)*
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Dimension checker** | Represent dimensions as exponent vectors; check algebra rules |
| **Unit system** | Unit registry with scaling factors; convert to/from base SI |
| **Literal parser** | Parse `400 km`, `9.80665 m/s^2` |
| **Type annotation parser** | Parse `: Length`, `: Length^3 / Time^2` |
| **Conversion operator** | Implement `->` for unit conversion |
| **Prelude** | Built-in dimensions, units, constants |
| **Dimension error messages** | "Cannot add Length + Mass", "exp() requires dimensionless argument" |

## Out of Scope

- Structs, multi-line bodies
- Functions
- Multi-file, imports
- Tables
- Spaces, indexes
- `i64`, `bool`, `Str`, `Datetime`, `Option`

## Milestone Test

```rust
// orbital.graph
dimension Velocity = Length / Time;

param alt: Length = 400 km;
param period: Time = 90 min;
const R_earth: Length = 6371 km;

node circumference: Length = 2.0 * pi * (@R_earth + @alt);
node speed: Velocity = @circumference / @period;
node speed_kmh = @speed -> km/hour;
```

```
$ cellgraph eval orbital.graph
alt            = 400 km
period         = 90 min
R_earth        = 6371 km
circumference  = 42543.38 km
speed          = 7.091 km/s
speed_kmh      = 25527.0 km/hour
```

### Error cases that must work

```rust
// error: dimension mismatch
node bad = @alt + @period;
//  error: cannot add Length + Time

// error: exp requires dimensionless
node also_bad = exp(@alt);
//  error: exp() requires dimensionless argument, got Length

// error: unknown unit
node x = 100 foobar;
//  error: unknown unit `foobar`
```

## Open Questions

- [ ] Should `dimensionless` be an explicit keyword/type, or is bare `f64` sufficient?
- [ ] How are compound units displayed in output? `m/s` vs `m*s^-1` vs `m s^{-1}`?
- [ ] Should the compiler suggest corrections? E.g., "did you mean `->` to convert units?"
      when dimensions match but units differ in an assignment.
