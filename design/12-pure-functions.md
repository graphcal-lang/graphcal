# Pure Functions

> Reusable computation logic via `fn`, enforced pure by the `@` prohibition rule.

## Status

**Decision level:** Design complete (see [pure-functions-syntax-design.md](../2026-02-12_pure-functions-syntax-design.md)). Awaiting implementation.

## Summary

Functions are reusable computation templates. They are **not** graph nodes -- they don't appear in the DAG. Purity is enforced structurally: `@` references are a compile error inside `fn` bodies.

## Keyword

`fn` was chosen over `pure` and `pure fn`. See the full analysis in the dedicated design document.

## Two Forms

**Block form** (multi-line, no trailing `;`):

```rust
fn hohmann_transfer(gm: Length^3 / Time^2, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}
```

**Short form** (single expression, with trailing `;`):

```rust
fn lerp<D: Dim>(a: D, b: D, t: f64) -> D = a + (b - a) * t;
```

## Purity Enforcement

`@` is a compile error inside `fn` bodies:

```rust
fn bad(r: Length) -> Velocity {
    sqrt(@GM_earth / r)
//       ^^^^^^^^^ error[F001]: graph reference not allowed
//                 help: pass GM_earth as a parameter
}
```

## Dimension Generics

```rust
fn abs<D: Dim>(x: D) -> D = if x < 0 { -x } else { x };
fn clamp<D: Dim>(value: D, low: D, high: D) -> D { ... }
```

## Space-Tagged Parameters

```rust
fn rotate_to_body(v: Vec3<Force> in Frame.ECI, q: Quaternion) -> Vec3<Force> in Frame.Body {
    q * v
}
```

## Visibility and Import

Same rules as other declarations -- public by default, `private` for hiding. Prelude functions are auto-imported.

## Explicitly NOT Supported (Initially)

| Feature | Rationale |
| --- | --- |
| Recursion | Engineering calcs are straight-line. Add later with `rec fn`. |
| Higher-order functions | No functions-as-parameters. Lambdas in `scan`/`map` suffice. |
| Overloading | Use generics instead. |
| Closures capturing graph scope | Violates purity. |

## Open Questions

- **Local `fn` inside `node` bodies:** Should scoped helper functions be allowed inside multi-line node bodies?
- **Space-generic functions (`<S: Space>`):** Deferred or include now?
- **`Scalar` constraint:** A constraint for "dimensionless numeric" alongside `Dim` -- needed from day one?
- **Return type inference:** Should it be encouraged or discouraged? Explicit return types aid readability.
- **Standard library functions:** What functions ship in the prelude? `sqrt`, `exp`, `ln`, `sin`, `cos`, `abs`, `min`, `max`, `clamp`?

## Dependencies on Other Aspects

- **Scoping** ([08](./08-scoping.md)): `@` prohibition is the purity mechanism.
- **Dimensions** ([04](./04-dimensions-and-units.md)): `<D: Dim>` generics.
- **Spaces** ([06](./06-spaces.md)): Space-tagged parameters.
- **Syntax** ([02](./02-syntax-design.md)): `fn` keyword, two forms.
- **Namespace** ([09](./09-namespace.md)): Functions follow the same visibility/import rules.
