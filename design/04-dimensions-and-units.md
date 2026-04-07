# Type System -- Dimensions and Units

> Layers 2 and 3: Physical dimensions as types, units as values.

## Status

**Decision level:** Mostly settled. Numbat-inspired design chosen.

## Summary

**Dimensions are types. Units are values.** This separation is the key insight, inspired by [Numbat](https://numbat.dev). When you write `param altitude: Length = 400 km`, the type is `Length` (dimension), and `km` is a scaling factor (unit). This means `400 km + 200_000 m` is well-typed (both `Length`) and the compiler handles conversion.

## Dimensions (Layer 2 -- Compile-Time Types)

Dimensions form an algebra over base physical quantities:

```gcl
// Base dimensions (defined in prelude)
base dimension Length;
base dimension Time;
base dimension Mass;
base dimension Angle;

// Derived dimensions -- algebraic combinations
dimension Velocity = Length / Time;
dimension Acceleration = Length / Time^2;
dimension Force = Mass * Acceleration;
dimension Energy = Force * Length;
dimension Power = Energy / Time;
dimension SpecificImpulse = Time;   // conventional
```

### Dimension Algebra Rules

| Operation | Dimension Rule |
| --- | --- |
| `a + b` | Dimensions must match |
| `a * b` | Dimensions multiply (exponents add) |
| `a / b` | Dimensions divide (exponents subtract) |
| `a ^ n` | Dimension raised to power |
| `sqrt(a)` | Dimension^(1/2) |
| `exp(a)` | `a` must be dimensionless |
| `ln(a)` | `a` must be dimensionless |
| `sin(a)` | `a` must be `Angle`, result is dimensionless |

### Type Inference

Full inference is supported -- most type annotations are optional:

```gcl
param alt = 400 km;                             // inferred: Length
param t = 90 min;                               // inferred: Time
node speed = 2 * pi * (@R_earth + @alt) / @t;   // inferred: Velocity
```

## Units (Layer 3 -- Value-Level Scaling Factors)

Units are scaling factors within a dimension:

```gcl
// Base units (defined in prelude)
unit m: Length;
unit s: Time;
unit kg: Mass;
unit rad: Angle;

// Derived units
unit km: Length = 1000 m;
unit hour: Time = 3600 s;
unit deg: Angle = (pi / 180) rad;
unit N: Force = kg * m / s^2;
unit kN: Force = 1000 N;
```

### Custom Counting Units

Units can auto-create their own dimension for domain-specific counting:

```gcl
unit launch;          // creates dimension Launch
unit crew_member;     // creates dimension CrewMember
```

### Unit Conversion

Explicit conversion uses the `->` operator:

```gcl
node fuel_mass_lb = @fuel_mass -> lb;
node tof_hours = @transfer.tof -> hour;
```

## Prelude

The entire dimension and unit system is defined in the standard library prelude, written in the language itself, and extensible by users.

## Open Questions

- **Fractional dimension exponents:** Can dimensions have fractional exponents beyond `sqrt`? E.g., `Length^(1/3)` for some engineering formulas.
- **Temperature handling:** Temperature conversions are affine (not linear). How should Celsius/Fahrenheit be handled alongside Kelvin? Numbat handles this; the approach should be documented.
- **Display units:** When rendering a value, which unit should be used? The one from the declaration? The "most natural" one? User-configurable per output?
- **Dimension aliases:** Can users create aliases for complex dimensions (e.g., `dimension Torque = Force * Length` where `Torque` and `Energy` have the same physical dimension but different semantics)? Should they be treated as the same or distinct?
- **User-defined base dimensions:** Can users add new base dimensions beyond the standard physical ones? See [18-non-si-dimensions.md](./18-non-si-dimensions.md) for a detailed proposal.
- **Unit prefixes:** Should SI prefixes (`kilo`, `mega`, `milli`, etc.) be supported as a mechanism, or must each prefixed unit be declared individually?
- **Internal representation:** Are values always stored in base SI units internally? Or is the internal representation configurable per project?

## Dependencies on Other Aspects

- **Primitives** ([03](./03-primitives.md)): Dimensions annotate primitive types.
- **Pure Functions** ([12](./12-pure-functions.md)): Dimension generics (`<D: Dim>`).
- **Syntax** ([02](./02-syntax-design.md)): Literal syntax for values with units (`400 km`).
- **Spaces** ([06](./06-spaces.md)): Dimensions and spaces are orthogonal layers that compose.
