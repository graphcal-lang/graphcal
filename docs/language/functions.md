---
icon: material/function
---

# DAG Blocks (Reusable Computation)

Graphcal uses `dag` blocks as the single mechanism for defining reusable, parameterized computation. DAG blocks replace the previous `fn` keyword, which is no longer supported.

> **Migration note:** The `fn` keyword has been removed. Using `fn` produces a parse error: "fn is no longer supported; use dag blocks instead." See the migration examples below.

## Declaration Syntax

A `dag` block defines a named, reusable sub-DAG with its own parameters and nodes:

```
dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

### Multi-Node DAGs

Unlike the old `fn` (which was limited to a single return value), a `dag` can expose multiple outputs:

```
dag hohmann_transfer {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;

    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}
```

## Using DAG Blocks with `include`

DAG blocks are instantiated using `include` declarations, which embed the sub-DAG into the current computation graph:

```
include hohmann_transfer(gm: GM_EARTH, r1: R_EARTH + @parking_alt, r2: R_EARTH + @target_alt) {
    total_dv as transfer_dv,
    dv1 as departure_dv,
}
```

- Parameters are passed as named arguments
- Output nodes are selected and optionally aliased with `as`
- The instantiated nodes become part of the enclosing DAG

### Accessing All Outputs

```
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @parking_alt) {
    v as v_parking,
}
```

## Cross-File DAG Blocks

DAG blocks defined in other files are included using a DAG path:

```
include "./lib/orbital.gcl"/hohmann_transfer(gm: GM_EARTH, r1: @r1, r2: @r2) {
    total_dv,
}
```

The syntax is `include "path"/dag_name(params) { outputs }`.

## Import vs Include

The `import` and `include` keywords serve different purposes:

- **`import`** brings compile-time definitions into scope: `const`, `type`, `dim`, `unit`, `index`, `dag`
- **`include`** instantiates a DAG (inline or from a file) into the current computation graph

```
import "./constants.gcl" { GM_EARTH, R_EARTH };

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}

include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @parking_alt) {
    v as v_parking,
}
```

## Migration from `fn`

### Single-expression function

Before (`fn`):

```
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);

node v: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @parking_alt);
```

After (`dag` + `include`):

```
dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node result: Velocity = sqrt(@gm / @r);
}

include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @parking_alt) {
    result as v,
}
```

### Block-body function

Before (`fn`):

```
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    let dv1 = sqrt(2.0 * gm * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * gm * r1 / (r2 * (r1 + r2)));
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}

node transfer: TransferResult = hohmann_dv(GM_EARTH, R_EARTH + @parking_alt, R_EARTH + @target_alt);
```

After (`dag` + `include`):

```
dag hohmann_transfer {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;

    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}

include hohmann_transfer(gm: GM_EARTH, r1: R_EARTH + @parking_alt, r2: R_EARTH + @target_alt) {
    total_dv as transfer_total_dv,
    dv1 as transfer_dv1,
    dv2 as transfer_dv2,
}
```

### Dimension-generic function

Before (`fn`):

```
fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;

node midpoint: Length = lerp(@parking_alt, @target_alt, 0.5);
```

After: Use built-in functions for simple generic operations, or define a `dag` for domain-specific patterns. Simple generic computations like `lerp` are candidates for built-in functions rather than user-defined DAGs.

## Why DAG Blocks Instead of Functions

The `dag` block unifies two concepts that were previously separate:

1. **Pure functions** (`fn`): Single-output, expression-level, with generics
2. **Parameterized imports**: Multi-output, file-level, with param bindings

Both solve the same problem -- accepting inputs, computing derived values, producing outputs. The `dag` block provides a single, consistent mechanism:

- Multiple outputs (not limited to a single return value)
- Same `param`/`node` semantics as top-level declarations
- Same `@` sigil for referencing values within the DAG
- Composable with the file-level DAG via `include`
