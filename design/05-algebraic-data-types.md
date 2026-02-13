# Type System -- Algebraic Data Types

> Layer 4: User-defined structs and tagged unions (Gleam-style `type`).

## Status

**Decision level:** Mostly settled. Gleam-style unified `type` keyword chosen.

## Summary

Following [Gleam](https://gleam.run), Cellgraph uses a single `type` keyword for both structs and tagged unions. A type with one variant is a struct; a type with multiple variants is a tagged union. There is no separate `struct` vs `enum` keyword.

## Syntax

### Struct (Single Variant)

```rust
type Orbit {
    semi_major_axis: Length,
    eccentricity: f64,
    inclination: Angle,
}
```

### Tagged Union (Multiple Variants)

```rust
type ManeuverKind {
    Impulsive(delta_v: Velocity)
    LowThrust(thrust: Force, duration: Time)
    GravityAssist(body: Str, periapsis: Length)
}
```

### Bare Variants (No Fields)

```rust
type Status {
    Nominal
    Warning(message: Str)
    Critical(message: Str, code: i64)
}
```

## Usage in Nodes

Struct construction and field access:

```rust
node transfer: TransferResult = {
    let dv1 = ...;
    let dv2 = ...;
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
};

// Field access
node total = @transfer.total_dv;
```

## Open Questions

- **Pattern matching:** How are tagged unions consumed? Is there a `match` expression?

  ```rust
  node fuel = match @maneuver_kind {
      Impulsive(dv) => rocket_fuel(@dry_mass, dv, @v_exhaust),
      LowThrust(thrust, dur) => thrust * dur / @v_exhaust,
      GravityAssist(_, _) => 0 kg,
  };
  ```

- **Exhaustiveness checking:** If `match` is supported, should it enforce exhaustive patterns?
- **Generic types:** Can types be generic? E.g., `type Pair<A, B> { first: A, second: B }`?
- **Dimensional fields:** Can struct fields carry dimensions? This is already shown in examples (`sma: Length`), but the interaction with the dimension algebra needs specification (e.g., can you do `@orbit.sma + 100 km`?).
- **Recursive types:** Are recursive types allowed (e.g., tree structures)? Probably not needed for engineering calculations, but should be explicitly excluded or deferred.
- **Type methods:** Can types have associated functions/methods? Or is everything done through free functions?
- **Visibility of fields:** Are struct fields always public, or can individual fields be private?
- **Variant separators:** The examples show variants without commas or semicolons between them. Is this the final syntax?

## Dependencies on Other Aspects

- **Syntax** ([02](./02-syntax-design.md)): The `type` keyword and construction syntax.
- **Spaces** ([06](./06-spaces.md)): Struct fields can be space-tagged.
- **Pure Functions** ([12](./12-pure-functions.md)): Functions can accept and return user-defined types.
- **Tables** ([10](./10-tables-and-autofill.md)): `scan` uses structs for co-evolved state.
- **Live View** ([13](./13-live-view.md)): Structs expand into sub-rows in the grid.
