---
icon: material/numeric-3-circle
---

# Step 3: Structs

In this step, you'll learn to group related values into struct types.

## Struct Types

When a calculation produces multiple related values, group them into a
struct. In graphcal every `type` is an n-variant tagged union — a
struct is just a single-variant union whose sole constructor's name
matches the type's name:

```
dim GravParam = Length^3 / Time^2;

type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}
```

## Constructing a Struct

Build a struct value by calling its constructor:

```
node result: TransferResult = TransferResult(
    dv1: 100.0 m/s,
    dv2: 200.0 m/s,
    total_dv: 300.0 m/s,
    tof: 3600.0 s,
);
```

Graph node fields are passed with explicit `@` references:

```
node dv1: Velocity = 100.0 m/s;
node dv2: Velocity = 200.0 m/s;
node total_dv: Velocity = @dv1 + @dv2;

node result: TransferResult = TransferResult(
    dv1: @dv1,
    dv2: @dv2,
    total_dv: @total_dv,
    tof: 3600.0 s,
);
```

Constructor fields must always be explicit: `field: expr`.

## Field Access

Access struct fields with the `.` operator:

```
node total: Velocity = @result.total_dv;
node time_hours: Time = @result.tof -> hour;
```

## Putting It Together: Hohmann Transfer

A Hohmann transfer between two coplanar circular orbits naturally
produces several related values. Express each intermediate as its own
`node`, then group the outputs into a `TransferResult`:

```
dim GravParam = Length^3 / Time^2;

type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}

const node r_earth: Length = 6371.0 km;
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;

param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node r1: Length = @r_earth + @parking_alt;
node r2: Length = @r_earth + @target_alt;
node a: Length = (@r1 + @r2) / 2.0;

node v1: Velocity = sqrt(@gm_earth / @r1);
node v2: Velocity = sqrt(@gm_earth / @r2);
node dv1: Velocity = sqrt(2.0 * @gm_earth * @r2 / (@r1 * (@r1 + @r2))) - @v1;
node dv2: Velocity = @v2 - sqrt(2.0 * @gm_earth * @r1 / (@r2 * (@r1 + @r2)));

node transfer: TransferResult = TransferResult(
    dv1: @dv1,
    dv2: @dv2,
    total_dv: @dv1 + @dv2,
    tof: PI * sqrt(@a ^ 3.0 / @gm_earth),
);

node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> hour;
```

Every intermediate is a first-class node in the DAG. That makes them
visible in the LSP outline and in `graphcal eval` output, which is part
of why Graphcal exposes intermediates rather than hiding them inside a
local block.

In [Step 4](step4-functions.md), you'll learn how to package a chunk of
this graph into a reusable, parameterized `dag` block.

## What You Learned

- **`type`** declarations as n-variant tagged unions (single-variant
  for record-shaped data)
- **Constructor calls** for unified `type` declarations with
  `ConstructorName(field: value, ...)`
- **Field access** with `.` on values of single-variant unions
