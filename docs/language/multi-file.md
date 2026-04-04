---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal supports splitting projects across multiple files using `import` declarations. Import paths can be either **file paths** (quoted strings) or **module paths** (bare identifiers). There are two import styles: **selective imports** and **module imports**.

## Selective Imports

Selective imports bring specific names into the local scope:

```
import "./path/to/file.gcl" { name1, name2 };
```

- The path is a **string literal** relative to the importing file
- Braces list the specific names to import
- All top-level declarations can be imported: `param`, `node`, `const`, `dimension`, `unit`, `type`, `index`, `fn`

## Module Imports

Module imports bring an entire file in as a namespace, accessed via `::`:

```
import "./constants.gcl";                  // module named "constants"
import "./constants.gcl" as consts;        // module named "consts"
```

When no alias is given, the module name is derived from the filename stem (e.g., `constants.gcl` becomes `constants`). The filename stem must be a valid `lower_snake_case` identifier; otherwise, use `as` to provide an explicit alias.

Members are accessed with `::`:

```
import "./constants.gcl";
import "./params.gcl";
import "./lib.gcl";

node g: Acceleration = constants::G0;           // qualified const
node total: Mass = @params::dry_mass;           // qualified graph ref
node y: Dimensionless = lib::double(@x);        // qualified fn call
```

Module imports only resolve declarations that are actually referenced via `::` in the importing file. Unreferenced declarations are not imported.

## Path Resolution

Paths are resolved relative to the file containing the `import` declaration:

```
// In project/main.gcl:
import "./lib/constants.gcl" { G0 };     // resolves to project/lib/constants.gcl
import "../shared/units.gcl" { knot };   // resolves to shared/units.gcl
```

## Module Paths (Bare Imports)

For organized projects, you can import modules using bare identifier paths instead of quoted file paths. This requires a `graphcal.toml` manifest file at the project root.

### Setting up `graphcal.toml`

Create a `graphcal.toml` in your project root with a `[package]` section:

```toml
[package]
name = "nasa"
# source_dir = "src"  # optional, defaults to "src"
```

### Using module paths

With the manifest above, organize your source files under the `src/` directory and import them by package-qualified paths:

```
import nasa/rocket { delta_v };
import nasa/orbital/transfer as transfer;
import nasa/constants;
```

Module paths require at least two segments separated by `/`. The first segment must match the `package.name` from `graphcal.toml`.

### Resolution

`import nasa/rocket { delta_v }` resolves to `<project_root>/src/nasa/rocket.gcl`.

Nested paths work as expected: `import nasa/orbital/transfer` resolves to `<project_root>/src/nasa/orbital/transfer.gcl`.

### Custom source directory

Set `source_dir` in `graphcal.toml` to use a different directory:

```toml
[package]
name = "myproject"
source_dir = "lib"
```

Now `import myproject/helpers` resolves to `<project_root>/lib/myproject/helpers.gcl`.

### Parameterized module paths

Module paths work with all import forms, including parameterized imports:

```
import nasa/rocket(dry_mass: 800.0 kg) as stage_1;
import nasa/rocket(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

### Mixing file paths and module paths

File paths and module paths can be used in the same project:

```
import nasa/constants { G0 };
import "./local_helpers.gcl" { my_fn };
```

### Project layout example

```
my_project/
  graphcal.toml            # [package] name = "nasa"
  src/
    main.gcl               # entry point
    nasa/
      constants.gcl        # import nasa/constants { G0 };
      rocket.gcl            # import nasa/rocket { delta_v };
      orbital/
        transfer.gcl       # import nasa/orbital/transfer { dv };
```

## Import Aliasing

Rename imports with `as` to avoid name conflicts:

```
import "./file_a.gcl" { velocity as velocity_a };
import "./file_b.gcl" { velocity as velocity_b };

node diff: Velocity = @velocity_a - @velocity_b;
```

## What Can Be Imported

### Selective imports

| Declaration | How to Import | How to Reference |
|-------------|--------------|-----------------|
| `param` | `import "..." { name }` | `@name` |
| `node` | `import "..." { name }` | `@name` |
| `const` | `import "..." { NAME }` | `NAME` |
| `dimension` | `import "..." { DimName }` | `DimName` |
| `unit` | `import "..." { unit_name }` | `unit_name` |
| `type` | `import "..." { TypeName }` | `TypeName` |
| `index` | `import "..." { IndexName }` | `IndexName` |
| `fn` | `import "..." { fn_name }` | `fn_name(...)` |

Imported `param` and `node` declarations are referenced with `@` just like local ones.

### Module imports

| Declaration | How to Reference |
|-------------|-----------------|
| `param` | `@module::name` |
| `node` | `@module::name` |
| `const` | `module::NAME` |
| `fn` | `module::fn_name(...)` |

Dimension, unit, type, and index declarations cannot currently be referenced via module-qualified syntax. To use types or dimensions from another file, use selective imports.

## When to Use Each Style

- **Selective imports** are best when you need only a few names, or when you need to import types and dimensions
- **Module imports** are best when you import many values from a file and want to keep their origin clear

## Parameterized Imports (Module Instantiation)

You can instantiate a file with different parameter values by supplying **param bindings** in parentheses after the path. This inlines the dependency's computation graph into your file, replacing the specified param defaults with your expressions.

### Selective instantiation

```
import "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };

node result: Velocity = @delta_v;
```

Only `delta_v` is exposed to the importing file. Other declarations from `rocket.gcl` are computed internally but not directly accessible.

### Module instantiation

```
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;

node dv: Velocity = @r::delta_v;
node mr: Dimensionless = @r::mass_ratio;
```

All declarations from `rocket.gcl` are accessible via the `r::` prefix.

### Multiple instantiations

The same file can be instantiated multiple times with different parameters:

```
import "./rocket.gcl"(dry_mass: 800.0 kg, isp: 320.0 s) as stage_1;
import "./rocket.gcl"(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

Each instantiation creates an independent set of computations under its own namespace.

### Graph references in bindings

Binding expressions can reference `@` values from the importing file's scope:

```
param my_mass: Mass = 800.0 kg;
import "./rocket.gcl"(dry_mass: @my_mass) { delta_v };
```

The dependency's computation graph is merged into the importer's DAG, so the topological sort naturally handles evaluation order.

### Index Bindings

In addition to `param` bindings, parameterized imports can bind **indexes**. This enables library files that are generic over their label sets.

A library declares a [required index](indexes.md#required-indexes):

```
// lib/budget.gcl
index Phase;

param cost: Dimensionless[Phase];
node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

The importer binds it to a concrete index:

```
// main.gcl
index MyPhase = { Design, Build, Test };

import "./lib/budget.gcl"(
    Phase: MyPhase,
    cost: { MyPhase::Design: 10.0, MyPhase::Build: 20.0, MyPhase::Test: 5.0 },
) { total };

node result: Dimensionless = @total;  // 35.0
```

Index bindings use the same `Name: Value` syntax as param bindings, but the right-hand side must be the **name of a concrete index** (not an expression).

#### Kind matching

Named indexes can only be bound to named indexes, and range indexes can only be bound to range indexes. Binding a named index to a range or vice versa is a compile error.

#### Dimension matching for range indexes

When binding a required range index, the concrete range index must have the **same dimension**:

```
// lib.gcl
index Step: Time;   // requires dimension Time

// main.gcl
index MyStep = linspace(0.0 s, 10.0 s, step: 1.0 s);   // OK: dimension is Time
import "./lib.gcl"(Step: MyStep) { ... };

index DistStep = linspace(0.0 m, 100.0 m, step: 10.0 m);  // ERROR: dimension is Length
import "./lib.gcl"(Step: DistStep) { ... };     // dimension mismatch
```

### Strict Binding Mode

When a parameterized import has **any** bindings (param or index), **all** params and indexes with defaults must be explicitly bound. This prevents accidentally relying on stale default values when you intend to customize the module. Required indexes must **always** be bound, regardless of `#[allow_defaults]`.

```
// rocket.gcl has params: dry_mass (default), fuel_mass (default), isp (default)

// ERROR: only dry_mass is bound; fuel_mass and isp are not explicitly provided
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;

// OK: all params are explicitly bound
import "./rocket.gcl"(dry_mass: 800.0 kg, fuel_mass: 2800.0 kg, isp: 320.0 s) as r;
```

#### Opting out with `#[allow_defaults]`

If you intentionally want to bind only some params and let the rest use their defaults, add the `#[allow_defaults]` attribute to the import:

```
#[allow_defaults]
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;
// fuel_mass and isp keep their default values
```

The `#[allow_defaults]` attribute is only valid on `import` declarations with param bindings. Using it on other declarations (e.g., `param`, `node`) is a compile error.

### Required parameters

A param declared without a default value is **required** — it must be provided by the importer via a parameterized import binding. This is the primary mechanism for creating reusable "library" files:

```
// library: rocket_engine.gcl
param dry_mass: Mass;                     // required — must be provided
param isp: Velocity = 320.0 s;           // optional — has default

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
```

```
// consumer: main.gcl
import "./rocket_engine.gcl"(dry_mass: 800.0 kg) as engine;

node dv: Velocity = @engine::delta_v;
```

If a required param is not provided, the compiler emits error `O003`:

```
error[graphcal::O003]: required param `dry_mass` has no value
  ┌─ rocket_engine.gcl:2:1
  │
2 │ param dry_mass: Mass;
  │ ^^^^^^^^^^^^^^^^^^^^^ declared here without a default value
  │
  = help: provide a value via `--set 'dry_mass=<value>'`, `--input`,
          or a parameterized import binding
```

Required params can also be satisfied from the command line with `--set` or `--input`, which is useful for top-level entry-point files.

### Validation

- Binding names must be `param` or index (`index`) declarations in the imported file
- Binding a `node`, `const`, or unknown name is a compile error
- All required params (those without defaults) must be provided by bindings, `--set`, or `--input`
- All required indexes must be provided by bindings
- Index binding values must be the name of a concrete index in the importer's scope
- Named indexes can only be bound to named indexes; range indexes can only be bound to range indexes
- Range index dimensions must match between the required index and the bound index
- When any binding is provided, all params and indexes with defaults must be bound, unless `#[allow_defaults]` is present
- Dimension mismatches are caught by the normal dimension checker after merging

## Circular Import Detection

Graphcal detects circular imports at compile time:

```
// a.gcl
import "./b.gcl" { x };

// b.gcl
import "./a.gcl" { y };
// ERROR: circular import detected
```

## Project Organization Patterns

### Constants / Parameters / Main

A common pattern separates constants, parameters, and main logic:

```
project/
  constants.gcl   -- shared physical constants
  params.gcl      -- tunable input parameters
  main.gcl        -- computation graph
```

### Library / Application

For reusable functions:

```
project/
  lib/
    orbital.gcl   -- reusable orbital mechanics functions
    thermal.gcl   -- thermal analysis functions
  main.gcl        -- application-specific graph
```

### Reusable Templates with Required Parameters

Use required params to create library files that must be instantiated with specific values:

```
project/
  lib/
    rocket.gcl    -- template with required params (dry_mass, fuel_mass)
  main.gcl        -- instantiates rocket.gcl with different param values
```

```
// lib/rocket.gcl
param dry_mass: Mass;     // required
param fuel_mass: Mass;    // required
param isp: Time = 320 s;  // optional default

const G0: Acceleration = 9.80665 m/s^2;
node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```
// main.gcl
import "./lib/rocket.gcl"(dry_mass: 800.0 kg, fuel_mass: 2000.0 kg, isp: 320.0 s) as stage_1;
import "./lib/rocket.gcl"(dry_mass: 500.0 kg, fuel_mass: 1200.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

### Reusable Templates with Required Indexes

Use required indexes to create library files that are generic over their label sets:

```
project/
  lib/
    budget.gcl    -- template with required index (Phase) and required param (cost)
  main.gcl        -- instantiates budget.gcl with a concrete Phase index
```

```
// lib/budget.gcl
index Phase;

param cost: Dimensionless[Phase];
node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```
// main.gcl
index ProjectPhase = { Design, Build, Test };

import "./lib/budget.gcl"(
    Phase: ProjectPhase,
    cost: { ProjectPhase::Design: 10.0, ProjectPhase::Build: 20.0, ProjectPhase::Test: 5.0 },
) { total };

node project_cost: Dimensionless = @total;
```

## Assertions in Imported Files

When a file is imported, **all its assertions are automatically evaluated and
reported**, regardless of whether they are explicitly listed in the import. This
ensures that safety checks in library files are never silently skipped.

To use an imported assertion in `#[assumes(...)]`, you must import it by name.
See [Assertions](assertions.md#assertions-in-multi-file-projects) for details.

## Evaluation Entry Point

When running `graphcal eval`, the entry file is the one you pass on the command line. All `import` dependencies are resolved transitively from that file:

```bash
graphcal eval project/main.gcl
```

## Project Root and Import Sandboxing

Graphcal restricts imports to files within the **project root** directory tree. This prevents accidental or malicious access to files outside your project.

### Default behavior

By default, the project root is the **parent directory of the entry-point file**. A file can import siblings and descendants, but not files above its own directory:

```
workspace/
  shared/
    constants.gcl
  project/
    main.gcl        ← entry point; project root = project/
```

```bash
# In main.gcl, this import would FAIL:
# import "../shared/constants.gcl" { G0 };
# ERROR: import resolves outside the project root
```

### Widening the root with `graphcal.toml`

Place a `graphcal.toml` file in an ancestor directory to widen the project root to that directory. A `graphcal.toml` with a `[package]` section also enables [module paths](#module-paths-bare-imports). An empty `graphcal.toml` (without `[package]`) only widens the root:

```
workspace/
  graphcal.toml     ← project root marker
  shared/
    constants.gcl
  project/
    main.gcl        ← entry point
```

Now `main.gcl` can `import` from `../shared/constants.gcl` because the project root is `workspace/` (the directory containing `graphcal.toml`), not `project/`.

Graphcal searches upward from the entry-point file's directory for the nearest `graphcal.toml`. If none is found, the default behavior applies.

### Explicit root with `--root`

You can also set the project root explicitly on the command line:

```bash
graphcal eval --root ./workspace project/main.gcl
```

When `--root` is provided, automatic `graphcal.toml` discovery is skipped — the given directory is used directly. This is useful in CI/scripts or when placing a `graphcal.toml` file is inconvenient.

The `--root` flag is available for both `eval` and `typecheck` subcommands.
