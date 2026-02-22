---
icon: material/shape
---

# Algebraic Data Types

Graphcal uses a unified `type` declaration (inspired by Gleam) for both structs and tagged unions.

## Structs (Single-Variant Types)

A `type` with a single set of fields defines a struct:

```
dimension Velocity = Length / Time;

type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}
```

### Construction

```
node result: TransferResult = TransferResult {
    dv1: 100.0 m/s,
    dv2: 200.0 m/s,
    total_dv: 300.0 m/s,
    tof: 3600.0 s,
};
```

Field shorthand: when a local variable matches the field name, you can omit the value:

```
let dv1 = 100.0 m/s;
TransferResult { dv1, dv2: 200.0 m/s, total_dv: 300.0 m/s, tof: 3600.0 s }
```

### Field Access

```
node total: Velocity = @result.total_dv;
node time_hours: Time = @result.tof -> hour;
```

## Tagged Unions (Multi-Variant Types)

A `type` with multiple variant blocks defines a tagged union:

```
dimension Force = Mass * Length / Time^2;

type ManeuverKind {
    Impulsive { delta_v: Velocity }
    LowThrust { thrust: Force, duration: Time }
}
```

### Bare Variants

Variants without fields are declared without braces:

```
type Status {
    Nominal
    Warning { code: Dimensionless }
}
```

### Constructing Variants

Construct a variant by name:

```
node maneuver: ManeuverKind = LowThrust {
    thrust: 0.5 N,
    duration: 3600.0 s,
};

node status: Status = Nominal;
```

## Match Expressions

Use `match` to destructure tagged unions:

```
node fuel_proxy: Force = match @maneuver {
    Impulsive { delta_v: _ } => 0.0 N,
    LowThrust { thrust, duration: _ } => thrust,
};
```

- Each arm matches a variant and binds its fields
- `_` discards a field value
- Field shorthand: `thrust` binds the field to a local variable of the same name

### Exhaustiveness Checking

The compiler requires that all variants are covered:

```
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
    Impulsive { delta_v } => delta_v,      // Velocity
    LowThrust { thrust, duration: _ } => thrust,  // Force
};
```

## Generic Types (Phantom Type Parameters)

Types can have generic parameters for type-safe phantom typing:

```
type Eci {}
type Body {}

#[derive(Add, Sub, Neg)]
type Vec3<D: Dim, F: Type> {
    x: D,
    y: D,
    z: D,
}
```

### `#[derive]` Attribute

`#[derive(Add, Sub, Neg)]` generates component-wise arithmetic operators for the type.

### Phantom Type Cast with `as`

Cast between phantom type instantiations using `as`:

```
node pos_eci: Vec3<Length, Eci> = Vec3 { x: 7000.0 km, y: 0.0 km, z: 0.0 km };
node pos_body: Vec3<Length, Body> = @pos_eci as Vec3<Length, Body>;
```

The `as` operator only changes the phantom type parameter; the underlying data is unchanged.

### Default Type Parameters

```
type Unframed {}

#[derive(Add, Sub, Neg)]
type Vec3<D: Dim, F: Type = Unframed> {
    x: D,
    y: D,
    z: D,
}

// Equivalent to Vec3<Length, Unframed>
param pos: Vec3<Length> = Vec3 { x: 1.0 m, y: 2.0 m, z: 3.0 m };
```
