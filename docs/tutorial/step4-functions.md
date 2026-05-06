---
icon: material/numeric-4-circle
---

# Step 4: DAG Blocks (Reusable Computation)

In this step, you'll write reusable computation using `dag` blocks and
learn how to instantiate them with `include`.

## Defining a DAG Block

A `dag` block defines a reusable sub-DAG with its own parameters and
nodes. It uses the same `param` / `node` / `@` syntax you already know:

```
dim Velocity = Length / Time;
dim GravParam = Length^3 / Time^2;

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

A `dag` block is a named template — it does not execute until you
`include` it.

## Including a DAG Block

Use `include` to instantiate a DAG block. The argument list is mandatory
(it may be empty); outputs are projected via the `.{ ... }` brace list,
and the statement ends with `;`:

```
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
const node r_earth: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;

include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    .{ v as v_parking };
```

- **Named arguments**: `gm: @gm_earth` passes `@gm_earth` to the `gm`
  parameter. Arguments are evaluated in the surrounding scope.
- **Output selection**: `.{ v as v_parking }` selects the `v` node and
  renames it to `v_parking`.
- The included nodes become regular nodes in your computation graph.

## Multi-Output DAGs

A `dag` can expose multiple outputs:

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

Pick the outputs you need at the include site:

```
param target_alt: Length = 35786.0 km;

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
).{ total_dv as transfer_dv, dv1 as departure_dv };
```

## Aliasing the Whole Include

If you'd rather group the outputs under a single prefix, alias the entire
instantiation instead of using a brace list:

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt) as parking;
node v_parking: Velocity = @parking.v;
```

Alias and brace list are mutually exclusive on a single `include`.

## Using DAG Results in the Graph

Included outputs are regular graph nodes, referenced with `@`:

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    .{ v as v_parking };

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
).{ total_dv as transfer_dv };

node total: Velocity = @v_parking + @transfer_dv;
```

## DAG Bodies Are Isolated

A `dag` body sees only its own declarations, its own imports, and the
outputs of its own includes. There is no lexical inheritance from the
enclosing file's top-level scope. To use a top-level `const node` (or
any other compile-time name) inside a `dag`, either pass it in as a
`param` at the include site (as in the examples above) or `import` it
explicitly inside the `dag` body. See
[Multi-File Projects](../language/multi-file.md) for the full rules.

## What You Learned

- **`dag`** blocks for defining reusable computation templates
- **`include`** to instantiate a DAG block with named arguments
- **Output selection** with `.{ name as alias }` to pick and rename
  outputs
- **Whole-include aliasing** with `as` for grouping outputs under a
  prefix
- **Multiple outputs** from a single DAG block
- DAG blocks use the same `param` / `node` / `@` syntax as top-level
  declarations and have strict scope isolation

## Next Step

In [Step 5](step5-multi-file-projects.md), you'll split your project
across multiple files with `import` declarations.
