---
icon: material/numeric-4-circle
---

# Step 4: DAG Blocks (Reusable Computation)

In this step, you'll write reusable computation using `dag` blocks and learn how to instantiate them with `include`.

## Defining a DAG Block

A `dag` block defines a reusable sub-DAG with its own parameters and nodes. It uses the same `param`/`node`/`@` syntax you already know:

```
dim Velocity = Length / Time;
dim GravParam = Length^3 / Time^2;

const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

A `dag` block is a named template -- it doesn't execute until you `include` it.

## Including a DAG Block

Use `include` to instantiate a DAG block with specific arguments:

```
const node R_EARTH: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;

include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @parking_alt) {
    v as v_parking,
}
```

- **Named arguments**: `gm: GM_EARTH` passes `GM_EARTH` to the `gm` parameter
- **Output selection**: `{ v as v_parking }` selects the `v` node and renames it to `v_parking`
- The included nodes become part of your computation graph

## Multi-Output DAGs

Unlike single-return-value functions in other languages, a `dag` can expose multiple outputs:

```
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
}

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

Include it and pick the outputs you need:

```
param target_alt: Length = 35786.0 km;

include hohmann_transfer(gm: GM_EARTH, r1: R_EARTH + @parking_alt, r2: R_EARTH + @target_alt) {
    total_dv as transfer_dv,
    dv1 as departure_dv,
}
```

## Using DAG Results in the Graph

The included outputs are regular graph nodes, referenced with `@`:

```
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @parking_alt) {
    v as v_parking,
}

include hohmann_transfer(gm: GM_EARTH, r1: R_EARTH + @parking_alt, r2: R_EARTH + @target_alt) {
    total_dv as transfer_dv,
}

node total: Velocity = @v_parking + @transfer_dv;
```

## What You Learned

- **`dag`** blocks for defining reusable computation templates
- **`include`** to instantiate a DAG block with named arguments
- **Output selection** with `{ node_name as alias }` to pick and rename outputs
- **Multiple outputs** from a single DAG block
- DAG blocks use the same `param`/`node`/`@` syntax as top-level declarations

## Next Step

In [Step 5](step5-multi-file-projects.md), you'll split your project across multiple files with `import` declarations.
