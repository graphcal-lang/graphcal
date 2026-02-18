---
icon: material/function
---

# Functions

Functions in Graphcal are pure, side-effect-free computations. They cannot access the computation graph, ensuring they are reusable and composable.

## Declaration Syntax

### Single-Expression Functions

```
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
```

### Block-Body Functions

```
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    let dv1 = sqrt(2.0 * gm * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * gm * r1 / (r2 * (r1 + r2)));
    TransferResult {
        dv1,
        dv2,
        total_dv: dv1 + dv2,
    }
}
```

### Zero-Argument Functions

```
fn earth_radius() -> Length = R_EARTH;
```

## Purity Enforcement

The `@` sigil is **forbidden** in function bodies. This is enforced at compile time:

```
fn bad() -> Mass = @dry_mass;  // ERROR: @ not allowed in fn body
```

This ensures functions depend only on their arguments and constants, making them:

- Deterministic
- Testable in isolation
- Safe to call from any context

Functions **can** reference constants and call other functions.

## Dimension Generics

Functions can be generic over dimensions:

```
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
```

- `<D: Dim>` declares a dimension type parameter
- `D` can be used anywhere a dimension annotation is expected
- The compiler infers `D` at each call site from the argument types

Multiple dimension parameters:

```
fn scale<D1: Dim, D2: Dim>(a: D1, b: D2) -> D1 = a * (b / b);
```

## Index Generics

Functions can be generic over indexes:

```
fn total<D: Dim, I: Index>(values: D[I]) -> D = sum(values);
```

- `<I: Index>` declares an index type parameter
- Works with any finite or range index

## Function Composition

Functions can call other functions:

```
fn earth_radius() -> Length = R_EARTH;

fn circular_velocity_at_alt(alt: Length) -> Velocity =
    orbital_velocity(GM_EARTH, earth_radius() + alt);
```

## Recursion Detection

Recursive function calls are detected and rejected at compile time:

```
fn factorial(n: Int) -> Int = if n == 0 { 1 } else { n * factorial(n - 1) };
// ERROR: recursive function call detected
```

This restriction exists because Graphcal targets finite computation graphs, not general-purpose programming.

## Calling Functions

Functions are called in `node` expressions, `const` expressions, or within other function bodies:

```
node v: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @parking_alt);
node mid: Length = lerp(@parking_alt, @target_alt, 0.5);
```
