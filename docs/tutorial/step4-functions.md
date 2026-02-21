---
icon: material/numeric-4-circle
---

# Step 4: Functions

In this step, you'll write reusable pure functions, including dimension-generic functions that work with any physical dimension.

## Pure Functions

Functions in Graphcal are pure: they take inputs and return outputs without side effects. The `@` sigil is **prohibited** inside function bodies, ensuring functions cannot depend on the computation graph.

### Simple Function

```
dimension Velocity = Length / Time;
dimension GravParam = Length^3 / Time^2;

const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
```

A single-expression function uses `= expr` syntax. No braces needed.

### Block-Body Function

For multi-step logic, use a block body:

```
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
}

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

### Function Composition

Functions can call other functions:

```
const R_EARTH: Length = 6371.0 km;

fn earth_radius() -> Length = R_EARTH;

fn circular_velocity_at_alt(alt: Length) -> Velocity =
    orbital_velocity(GM_EARTH, earth_radius() + alt);
```

## Dimension Generics

Functions can be generic over dimensions. This lets you write a single function that works with `Length`, `Velocity`, `Mass`, or any other dimension:

```
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;
```

- **`<D: Dim>`** declares a dimension type parameter
- **`D`** can be used as a dimension annotation in parameter types and the return type
- The compiler infers `D` at each call site

Using it:

```
param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node midpoint_alt: Length = lerp(@parking_alt, @target_alt, 0.5);
```

The compiler infers `D = Length` from the arguments.

## Purity Enforcement

The `@` sigil is forbidden in function bodies. This is a compile-time error:

```
fn bad_function() -> Mass = @dry_mass;  // ERROR: @ not allowed in fn
```

This restriction ensures functions are pure and reusable without hidden dependencies on the computation graph.

## Using Functions in the Graph

```
param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node v_parking: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @parking_alt);
node transfer: TransferResult = hohmann_dv(GM_EARTH, R_EARTH + @parking_alt, R_EARTH + @target_alt);
node midpoint_alt: Length = lerp(@parking_alt, @target_alt, 0.5);
```

## What You Learned

- **`fn`** for pure function declarations
- **Single-expression** (`= expr`) and **block-body** (`{ ... }`) syntax
- **Dimension generics** with `<D: Dim>` for reusable dimension-polymorphic functions
- **Purity enforcement**: `@` is prohibited in function bodies
- **Function composition**: functions calling other functions

## Next Step

In [Step 5](step5-multi-file-projects.md), you'll split your project across multiple files with `import` declarations.
