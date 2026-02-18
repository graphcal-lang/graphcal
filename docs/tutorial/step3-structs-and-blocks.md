---
icon: material/numeric-3-circle
---

# Step 3: Structs & Blocks

In this step, you'll learn to group related values into struct types and use block expressions with `let` bindings for multi-step calculations.

## Struct Types

When a calculation produces multiple related values, you can group them into a struct:

```
dimension Velocity = Length / Time;
dimension GravParam = Length^3 / Time^2;

type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}
```

A `type` with a single set of fields defines a struct. Each field has a name and a dimension.

## Block Expressions

Complex calculations can use block expressions with `let` bindings for intermediate values:

```
const R_EARTH: Length = 6371.0 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    let a = (r1 + r2) / 2.0;

    let v1 = sqrt(GM_EARTH / r1);
    let v2 = sqrt(GM_EARTH / r2);
    let dv1 = sqrt(2.0 * GM_EARTH * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * GM_EARTH * r1 / (r2 * (r1 + r2)));

    TransferResult {
        dv1,
        dv2,
        total_dv: dv1 + dv2,
        tof: PI * sqrt(a ^ 3.0 / GM_EARTH),
    }
};
```

Key points:

- **`let` bindings** introduce local variables within a block
- The **last expression** in a block is the block's value (no trailing semicolon)
- **Struct construction** uses `TypeName { field1: value1, field2 }` syntax
- When a field name matches a local variable, you can use shorthand: `dv1` instead of `dv1: dv1`

## Field Access

Access struct fields with the `.` operator:

```
node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> hour;
```

## Putting It All Together

Here is the complete Hohmann transfer example:

```
dimension Velocity = Length / Time;
dimension GravParam = Length^3 / Time^2;

type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}

const R_EARTH: Length = 6371.0 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node transfer: TransferResult = {
    let r1 = R_EARTH + @parking_alt;
    let r2 = R_EARTH + @target_alt;
    let a = (r1 + r2) / 2.0;

    let v1 = sqrt(GM_EARTH / r1);
    let v2 = sqrt(GM_EARTH / r2);
    let dv1 = sqrt(2.0 * GM_EARTH * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * GM_EARTH * r1 / (r2 * (r1 + r2)));

    TransferResult {
        dv1,
        dv2,
        total_dv: dv1 + dv2,
        tof: PI * sqrt(a ^ 3.0 / GM_EARTH),
    }
};

node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> hour;
```

## What You Learned

- **`type`** declarations for struct types with typed fields
- **Block expressions** `{ let ...; ... }` for multi-step calculations
- **`let` bindings** for intermediate values within a block
- **Struct construction** with field shorthand
- **Field access** with `.` on struct-typed values

## Next Step

In [Step 4](step4-functions.md), you'll extract reusable logic into pure functions with dimension generics.
