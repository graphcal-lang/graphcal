# Type System -- Spaces

> Layer 5: Semantic context tags preventing cross-context mixing (Sguaba-inspired).

## Status

**Decision level:** Mostly settled in concept. Some details around transforms and generics remain open.

## Summary

Inspired by [Sguaba](https://github.com/helsing-ai/sguaba), which uses Rust phantom types to prevent mixing vectors from different coordinate systems. The core insight: values can share the same dimension but live in **different semantic spaces** that must not be mixed.

A `space` declares a family of semantically distinct contexts. The `in` keyword tags a value with its space.

## Use Cases

| Domain | Space | Variants | Prevents |
| --- | --- | --- | --- |
| Coordinate frames | `Frame` | `Body`, `ECI`, `ECEF`, `LVLH` | Mixing reference frames |
| Spacecraft identity | `Craft` | `Chaser`, `Target`, `Depot` | Mixing per-vehicle budgets |
| Budget categories | `Budget` | `Allocated`, `Spent`, `Remaining` | Mixing budget columns |
| Time epochs | `Epoch` | `UTC`, `GPS`, `MissionElapsed` | Mixing time references |

## Syntax

### Declaration

```rust
space Frame {
    Body;
    ECI;
    ECEF;
    LVLH;
}
```

### Tagging Values

```rust
param sat_position: Vec3<Length> in Frame.ECI = ...;
param thrust_body: Vec3<Force> in Frame.Body = ...;
```

### Compile-Time Enforcement

```rust
node bad = @sat_position + @thrust_body;
//  error[S001]: space mismatch: Frame.ECI != Frame.Body
```

## Crossing Space Boundaries

Two mechanisms for intentional cross-space operations:

### 1. `.untagged` -- Manual Escape Hatch

```rust
node combined_mass: Mass =
    @chaser_mass.untagged
    + @target_mass.untagged;
```

The `.untagged` call is a signal to reviewers that a cross-space operation is intentional.

### 2. `Transform` -- Typed Conversion

```rust
node eci_to_body: Transform<Frame.ECI, Frame.Body> = {
    Transform.from_rotation(@attitude_quaternion)
};

node thrust_eci: Vec3<Force> in Frame.ECI = @eci_to_body.inverse() * @thrust_body;
```

## Interaction with Other Layers

- The `in` tag is **optional**. Untagged values are the default.
- Spaces are **orthogonal** to dimensions: `Vec3<Length> in Frame.ECI` has dimension `Length` and space `Frame.ECI`.
- Space tags compose with all other type layers.

## Open Questions

- **Space-generic functions:** Should functions be generic over spaces? E.g., `fn magnitude<S: Frame>(v: Vec3<Length> in S) -> Length`? This was deferred in the pure functions design.
- **Multi-space values:** Can a value belong to multiple spaces simultaneously? E.g., `in Frame.ECI in Craft.Chaser`?
- **Space arithmetic:** When two `Transform` types are composed, how does the type system track the chain? E.g., `Transform<A, B> * Transform<B, C> -> Transform<A, C>`?
- **`.untagged` auditing:** Should uses of `.untagged` be flagged in a lint pass or report? This would help code review.
- **Inheritance / hierarchy:** Can spaces have sub-spaces? E.g., `Frame.ECI` is a refinement of `Frame.Inertial`?
- **Runtime space selection:** Can the space variant be determined at runtime (e.g., from a parameter), or is it always compile-time?
- **Variant separators:** The examples show semicolons after each variant. Is this consistent with `index` (which uses commas)?

## Dependencies on Other Aspects

- **Dimensions** ([04](./04-dimensions-and-units.md)): Spaces are orthogonal to dimensions; they compose.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): `Transform` may be a built-in type or user-defined.
- **Pure Functions** ([12](./12-pure-functions.md)): Functions can accept space-tagged parameters.
- **Scoping** ([08](./08-scoping.md)): Space tags appear in type annotations at the `@` reference site.
