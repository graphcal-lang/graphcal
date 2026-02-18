---
icon: material/numeric-5-circle
---

# Step 5: Multi-File Projects

In this step, you'll learn to split your project across multiple files using `use` imports.

## Why Multiple Files?

As projects grow, it helps to separate concerns:

- **Constants** in one file (shared across the project)
- **Parameters** in another (easy to find and tune)
- **Main calculations** in the entry point

## Project Structure

```
rocket_project/
  constants.gcl
  params.gcl
  main.gcl
```

### `constants.gcl`

```
dimension Acceleration = Length / Time^2;
const G0: Acceleration = 9.80665 m/s^2;
```

### `params.gcl`

```
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
```

### `main.gcl`

```
use "./constants.gcl" { G0 };
use "./params.gcl" { dry_mass, fuel_mass, isp };

dimension Velocity = Length / Time;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

## Running a Multi-File Project

Point `graphcal eval` at the entry file:

```bash
$ graphcal eval rocket_project/main.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
G0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

Graphcal resolves `use` paths relative to the importing file.

## The `use` Statement

```
use "./path/to/file.gcl" { name1, name2 };
```

- The path is **relative** to the file containing the `use` statement
- The braces list the names to import (constants, params, nodes, types, dimensions, units, indexes, functions)
- Imported params and nodes are referenced with `@` just like local ones

## Import Aliasing

If two files export the same name, use `as` to rename:

```
use "./file_a.gcl" { velocity as velocity_a };
use "./file_b.gcl" { velocity as velocity_b };
```

## What Gets Imported

You can import any top-level declaration:

| Declaration | Import | Reference |
|-------------|--------|-----------|
| `param` | `use "..." { name }` | `@name` |
| `node` | `use "..." { name }` | `@name` |
| `const` | `use "..." { NAME }` | `NAME` |
| `dimension` | `use "..." { DimName }` | `DimName` |
| `unit` | `use "..." { unit_name }` | `unit_name` |
| `type` | `use "..." { TypeName }` | `TypeName` |
| `index` | `use "..." { IndexName }` | `IndexName` |
| `fn` | `use "..." { fn_name }` | `fn_name(...)` |

## Circular Import Detection

Graphcal detects circular imports at compile time:

```
// a.gcl
use "./b.gcl" { x };
// b.gcl
use "./a.gcl" { y };  // ERROR: circular import
```

## What You Learned

- **`use`** statements to import declarations from other files
- **Relative paths** for file references
- **Import aliasing** with `as`
- **Circular import detection** at compile time
- A practical **project organization** pattern

## Next Step

In [Step 6](step6-indexed-values.md), you'll work with indexed collections for multi-element calculations.
