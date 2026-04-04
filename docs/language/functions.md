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

## Nat Generics

Functions can be generic over nat range sizes:

```
fn transpose<M: Nat, N: Nat, D: Dim>(a: D[M, N]) -> D[N, M] =
    for j: range(N), i: range(M) { a[i, j] };

fn dot<N: Nat, D1: Dim, D2: Dim>(a: D1[N], b: D2[N]) -> D1 * D2 =
    sum(for i: range(N) { a[i] * b[i] });
```

- `<N: Nat>` declares a natural number type parameter
- `N` can be used in index position (`D[N]`) and in `for i: range(N)` loops
- The compiler infers `N` at each call site from the argument shapes
- `Nat` parameters are available as runtime `Int` values in the function body

### Nat Arithmetic

`Nat` expressions support addition, enabling functions that relate input and output sizes:

```
fn drop_last<N: Nat, D: Dim>(v: D[N + 1]) -> D[N] =
    for i: range(N) { v[i] };

fn pad_zero<N: Nat>(v: Dimensionless[N]) -> Dimensionless[N + 1] =
    for i: range(N + 1) { if i < N { v[i] } else { 0.0 } };
```

The compiler solves linear equations during unification: calling `drop_last` on a `Dimensionless[4]` vector solves `N + 1 = 4` to deduce `N = 3`. Subtraction is not supported — express the larger side with addition instead.

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
