# Syntax Design

> The surface syntax of `.graph` files: keywords, expressions, statement structure.

## Status

**Decision level:** Mostly settled. Rust-inspired DSL syntax chosen.

## Summary

Cellgraph uses a Rust-inspired DSL as its surface syntax. The `.graph` file is the canonical source of truth. Three approaches were considered; Approach B (Rust-inspired DSL) was chosen, with Python API (Approach C) as a complementary interface.

## Keyword Inventory

| Keyword | Role | Example |
| --- | --- | --- |
| `param` | User-adjustable input | `param mass = 5000 kg;` |
| `node` | Computed value | `node thrust = @mass * @G0;` |
| `const` | Immutable constant | `const G0 = 9.80665 m/s^2;` |
| `let` | Local variable (inside node/fn body) | `let r1 = @R_earth + @alt;` |
| `fn` | Pure reusable function | `fn speed(d: Length, t: Time) -> Velocity = d / t;` |
| `type` | Struct or tagged union | `type Orbit { sma: Length, ecc: f64 }` |
| `dimension` | Physical dimension | `dimension Velocity = Length / Time;` |
| `unit` | Unit within a dimension | `unit km: Length = 1000 m;` |
| `tag` | Semantic context tag family | `tag Frame { Body, ECI }` |
| `as` | Explicit type cast (tag strip) | `@p_O2 as Pressure` |
| `cat` | Finite label set (table axis) | `cat Region { LEO, GTO }` |
| `range` | Range index (numeric axis) | `range TimeStep(0.0 s, 1.0 s, step: 0.5 s);` |
| `table` | Table schema declaration | `table maneuvers { name: Str, dv: Velocity }` |
| `import` | Import from another module | `import orbit.transfer.{ transfer };` |
| `private` | Restrict visibility to current file | `private node _helper = ...;` |
| `project` | Project root declaration | `project mission_design { ... }` |

## Expression Syntax

- Arithmetic: `+`, `-`, `*`, `/`, `^` (exponentiation)
- Comparison: `<`, `>`, `<=`, `>=`, `==`, `!=`
- Logical: `&&`, `||`, `!`
- Unit conversion: `->` (e.g., `@fuel_mass -> lb`)
- Struct construction: `TransferResult { dv1, dv2, total_dv: dv1 + dv2 }`
- Field access: `@transfer.dv1`
- Function calls: `sqrt(gm / r)`
- Conditionals: `if cond { a } else { b }` (expression, not statement)

## Statement Forms

**Single-expression (trailing `;`):**

```gcl
param alt = 400 km;
const G0 = 9.80665 m/s^2;
node speed = @distance / @time;
fn lerp<D: Dim>(a: D, b: D, t: f64) -> D = a + (b - a) * t;
```

**Block form (braces, no trailing `;`):**

```gcl
node transfer: TransferResult = {
    let r1 = @R_earth + @parking_alt;
    let r2 = @R_earth + @target_alt;
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
};
```

Note: The current design has block-form `node` with a trailing `;`, while block-form `fn` and `type` do not. This inconsistency may warrant revisiting.

## Comments and Documentation

```gcl
// Line comment
/// Documentation comment (attached to the next declaration)

/// Circular orbital velocity at a given radius.
fn orbital_velocity(gm: Length^3 / Time^2, r: Length) -> Velocity {
    sqrt(gm / r)
}
```

## Open Questions

- **Semicolon rules:** Block-form `node` currently requires trailing `;` but `fn` and `type` do not. Should this be unified?
- **String syntax:** Single-quoted vs double-quoted? Interpolation support?
- **Pattern matching:** Should `match` expressions be supported for tagged unions?
- **Attributes:** The `#[...]` attribute system is designed in [19-assertions-and-testing.md](./19-assertions-and-testing.md). Initial attributes: `#[assumes(...)]` for engineering assumptions, `#[lazy]` for deferred evaluation.
- **Numeric literals:** Separator support (e.g., `200_000 m`)? Scientific notation (e.g., `3.98e5 km^3/s^2`)?
- **Operator precedence:** Should `^` bind tighter than unary `-`? Should unit attachment (`400 km`) have specific precedence?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Defines the semantics behind `param`/`node`/`const`.
- **Type System layers** ([03](./03-primitives.md)--[07](./07-indexes.md)): Type annotations in declarations.
- **Scoping** ([08](./08-scoping.md)): The `@` sigil for graph references.
- **Pure Functions** ([12](./12-pure-functions.md)): The `fn` keyword and its rules.
