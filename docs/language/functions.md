---
icon: material/function
---

# DAG Blocks (Reusable Computation)

Graphcal uses `dag` blocks as the single mechanism for defining reusable,
parameterized computation. A `dag` is a named sub-DAG that can be
instantiated as many times as you like, each instance with its own
parameter bindings.

## Declaration Syntax

A `dag` block defines a named, reusable sub-DAG with its own parameters and
nodes:

```
dim Velocity = Length / Time;
dim GravParam = Length^3 / Time^2;

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

The body uses the same `param` / `node` / `const node` / `@`-sigil syntax
as the file's top-level declarations.

### Multi-Output DAGs

A single `dag` can expose multiple outputs:

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

DAG blocks are instantiated using `include` declarations, which embed the
sub-DAG into the current computation graph. The argument list is mandatory
(it may be empty); outputs are projected via the `.{ ... }` brace list:

```
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
const node r_earth: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
).{ total_dv as transfer_dv, dv1 as departure_dv };
```

- Parameters are passed as named arguments.
- Output nodes are selected and optionally aliased with `as` inside the
  `.{ ... }` brace list.
- The selected outputs become regular nodes in the enclosing DAG and can
  be referenced with `@transfer_dv`, `@departure_dv`, etc.
- An `include` ends with `;`.

### Aliasing the Whole Include

If you only need a few outputs, the brace list is convenient. To bring all
of a DAG's outputs in under one prefix, alias the whole instantiation
instead:

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt) as parking;
node speed: Velocity = @parking.v;
```

Alias and brace list are mutually exclusive on a single `include`.

## Cross-File DAG Blocks

DAG blocks defined in another module are addressed by their full
dot-separated package path:

```
include lib.orbital.hohmann_transfer(gm: @gm_earth, r1: @r1, r2: @r2)
    .{ total_dv };
```

The path before `(` is absolute from the package root. See
[Multi-File Projects](multi-file.md) for the full path-resolution rules.

## Inline DAG Invocation (Expression Form)

Inside an expression, `@dag(args).out` is sugar for an anonymous
include — each call site is a fresh instantiation. Arguments are evaluated
in the surrounding expression scope, so they may reference loop variables:

```
index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

node dist: Length[Region] = { Region.A: 1.0 m, Region.B: 2.0 m };

node distances: Length[Region] = for r: Region {
    @id_len(v: @dist[r]).result
};
```

The thing immediately after `@` may be a DAG in scope by its leaf name or a
module-qualified DAG path such as `@module.dag(args).out`. The projection after
the call is still mandatory; `@dag(args)` and `@module.dag(args)` are rejected
because a DAG instance without an output projection is not a node.

## Import vs Include

The `import` and `include` keywords serve different purposes:

- **`import`** brings compile-time names into scope: `dim`, `unit`,
  `type`, `index`, `const node`, `dag`, `assert`. Importing runtime items
  (`param`, non-`const` `node`) is an error (M020).
- **`include`** instantiates a DAG, optionally with parameter bindings,
  and exposes its outputs as runtime nodes in the enclosing graph.

```
// constants.gcl
pub const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
pub const node r_earth: Length = 6371.0 km;

// main.gcl
import myproject.constants.{ gm_earth, r_earth };

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}

include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    .{ v as v_parking };
```

See [Multi-File Projects](multi-file.md) for full details on visibility
(`pub`, `pub(bind)`), required parameters and indexes, and parameterized
includes.

## Why DAG Blocks?

A `dag` block is a single mechanism that covers what other languages would
split between *pure functions* (single-output, expression-level) and
*parameterized library imports* (multi-output, file-level):

- Multiple outputs (not limited to a single return value).
- Same `param` / `node` semantics as top-level declarations.
- Same `@` sigil for referencing values within the DAG body.
- Composable with the file-level DAG via `include`.
- Strict scope isolation — every name a DAG uses must either be declared
  inside it, imported by it, or supplied as a `param`. There is no
  inheritance from the enclosing file's top-level scope.

The strict isolation rule means that to use a top-level constant inside a
`dag`, you either pass it in as a `param` at the include site or `import`
it explicitly inside the `dag` body.
