---
icon: material/shape
---

# Algebraic Data Types

Every `type` declaration in graphcal is an **n-variant tagged union**.
A record-shaped struct is just a single-variant union whose sole
constructor's name matches the type's name; a unit marker is a
single unit constructor. The functional core never distinguishes
record from union — there is only one shape.

## Tagged Unions

A tagged union lists its **constructors** inside the braced body of a
`type` declaration. Each constructor has an optional payload
(declared with parens or braces) or is a bare unit constructor:

```
dim Force = Mass * Length / Time^2;

type ManeuverKind {
    Impulsive(delta_v: Velocity),
    LowThrust(thrust: Force, duration: Time),
    Coast,
}
```

Constructors live in a namespace that is distinct from the type
namespace — a single lexeme can name both a type and a constructor
without ambiguity. A future syntax sugar will let
`type Position { x: Length, y: Length }` desugar to
`type Position { Position(x: Length, y: Length) }`; until then the
explicit form is required.

## Records (Single-Variant Unions)

Record-shaped data is written as a single-variant union whose sole
constructor's name equals the type's name:

```
dim Velocity = Length / Time;

type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}
```

### Construction

Construction is always a constructor call — parens with named args:

```
node result: TransferResult = TransferResult(
    dv1: 100.0 m/s,
    dv2: 200.0 m/s,
    total_dv: 300.0 m/s,
    tof: 3600.0 s,
);
```

Field shorthand: when a node name matches the field name, you can
omit the value:

```
node dv1: Velocity = 100.0 m/s;
node result: TransferResult =
    TransferResult(dv1, dv2: 200.0 m/s, total_dv: 300.0 m/s, tof: 3600.0 s);
```

### Field Access

Field access works on a value of a single-variant union — there is
exactly one constructor, so the field set is unambiguous:

```
node total: Velocity = @result.total_dv;
node time_hours: Time = @result.tof -> hour;
```

For multi-variant unions, field access is rejected — destructure
through `match` instead.

## Unit Markers

A unit marker is a single-variant union whose constructor takes no
payload:

```
type Eci { Eci }
type Body { Body }
type Coasting { Coasting }
```

Unit markers are useful as phantom type parameters (e.g., reference
frames).

> **Note**: `type T;` (semicolon, no body) is **not** a unit marker — it
> declares a *required* type that importers must bind. See
> [Multi-File Projects → Visibility and Bindability](multi-file.md#visibility-and-bindability).

### Constructing Union Values

Construct a variant by its constructor name. The parens-with-named-args
form is the canonical syntax:

```
node maneuver: ManeuverKind = LowThrust(thrust: 0.5 N, duration: 3600.0 s);

node coast: ManeuverKind = Coast;
```

## Match Expressions

Use `match` to destructure union types:

```
node fuel_proxy: Force = match @maneuver {
    Impulsive { delta_v: _ } => 0.0 N,
    LowThrust { thrust, duration: _ } => thrust,
};
```

- Each arm matches a member and binds its fields
- `_` discards a field value
- Field shorthand: `thrust` binds the field to a local variable of the same name

### Exhaustiveness Checking

The compiler requires that all constructors are covered:

```
type Status {
    Nominal,
    Warning(code: Dimensionless),
}

// ERROR: non-exhaustive -- missing `Warning` arm
node code: Dimensionless = match @status {
    Nominal => 0.0,
};
```

### All Arms Must Agree

All match arms must produce the same type and dimension:

```
// ERROR: arms have different dimensions (Force vs Velocity)
node bad: Force = match @maneuver {
    Impulsive { delta_v } => delta_v,             // Velocity
    LowThrust { thrust, duration: _ } => thrust,  // Force
};
```

## Generic Types (Phantom Type Parameters)

Types can have generic parameters for type-safe phantom typing:

```
type Eci { Eci }
type Body { Body }

type Vec3<D: Dim, F: Type> {
    Vec3(x: D, y: D, z: D),
}
```

### Phantom Type Cast with `as`

Cast between phantom type instantiations using `as`:

```
node pos_eci: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 7000.0 km, y: 0.0 km, z: 0.0 km);
node pos_body: Vec3<Length, Body> = @pos_eci as Vec3<Length, Body>;
```

The `as` operator only changes the phantom type parameter; the underlying data is unchanged.

### Default Type Parameters

```
type Unframed { Unframed }

type Vec3<D: Dim, F: Type = Unframed> {
    Vec3(x: D, y: D, z: D),
}

// Equivalent to Vec3<Length, Unframed>
param pos: Vec3<Length> = Vec3<Length>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
```
