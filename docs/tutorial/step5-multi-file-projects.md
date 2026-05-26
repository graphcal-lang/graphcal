---
icon: material/numeric-5-circle
---

# Step 5: Multi-File Projects

In this step, you'll learn to split your project across multiple files
using `import` declarations.

## Why Multiple Files?

As projects grow, it helps to separate concerns:

- **Constants** in one file (shared across the project)
- **Parameters** in another (easy to find and tune)
- **Main calculations** in the entry point

## Files Are Packages

Every `.gcl` file in Graphcal is a **package**. Without a
`graphcal.toml` manifest, the file is a *virtual* package — a
standalone Graphcal script. The package contains exactly one module:
the file itself. You can self-reference its top-level decls from
inline DAGs (e.g. `import dynamics.{type T};` from inside `dynamics.gcl`),
but you **cannot** import a sibling file. The first multi-file step
in any Graphcal project is to add a manifest.

In other words: virtual = one file. The moment you want a second
file, you add a `graphcal.toml` and graduate to a real package. The
rest of this step walks through that promotion end to end.

## Project Structure

A multi-file project always has a `graphcal.toml` manifest at the
root and source files arranged under the package's source directory:

```text
rocket_project/
  graphcal.toml                # [package] name = "rocket_project"
  src/
    rocket_project/
      constants.gcl
      params.gcl
      main.gcl
```

### `constants.gcl`

```graphcal
pub dim Acceleration = Length / Time^2;
pub const node g0: Acceleration = 9.80665 m/s^2;
```

### `params.gcl`

```graphcal
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
```

### `main.gcl`

```graphcal
import rocket_project.constants.{g0};
include rocket_project.params().{dry_mass, fuel_mass, isp};

dim Velocity = Length / Time;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

The path before `.{...}` is absolute from the package root. The
first segment is the package name (from `graphcal.toml`); subsequent
segments walk the directory tree under `source_dir`.

Note `params` uses `include`, not `import`: `params.gcl` exposes
`param`s (runtime values), and runtime values cross file boundaries
only through DAG instantiation. `import` brings compile-time names
only.

## Running a Multi-File Project

Point `graphcal eval` at the entry file:

```bash
$ graphcal eval rocket_project/src/rocket_project/main.gcl
g0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

Graphcal resolves each `import` against the package tree.

## The `import` Statement

There are three forms; pick the one that matches what you want to
bring into scope:

```graphcal
import constants;                  // brings module `constants`
import constants as c;             // brings module under alias `c`
import constants.{g0, g_mars};     // brings only `g0` and `g_mars`
```

The brace form is the most common in practice — it makes every
imported name explicit.

## Import Aliasing

If two files export the same name, rename one or both with `as`:

```graphcal
import file_a.{velocity as velocity_a};
import file_b.{velocity as velocity_b};
```

You can also alias a whole module:

```graphcal
import very.long.package.path as p;
node y: Length = p.helper(...);
```

## What Gets Imported

`import` brings only **compile-time** names. To use a runtime value
(like a `param` or non-`const` `node`) from another file, *include*
the producing DAG instead of importing the value (see
[Multi-File Projects](../language/multi-file.md#the-include-form)).

| Declaration kind | How to import                       | How to reference |
|------------------|-------------------------------------|------------------|
| `const node`     | `import file.{name}`                | `@name`          |
| `dim`            | `import file.{DimName}`             | `DimName`        |
| `unit`           | `import file.{unit_name}`           | `unit_name`      |
| `type`           | `import file.{type TypeName}`       | `TypeName`       |
| `index`          | `import file.{IndexName}`           | `IndexName`      |
| `dag`            | `import file.{dag_name}`            | `include`-d, or called as `@dag_name(...).out` |

## When a Single File Suffices

If your whole calculation fits in one file, you don't need a manifest
at all. A standalone `rocket.gcl` script behaves like a virtual
package — its only externally addressable name is its own stem.
References from inline DAGs back to top-level decls use that
self-reference path:

```graphcal
// rocket.gcl  (standalone script, no graphcal.toml)
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    import rocket.{type OrbitType};   // file's own name
    param o: OrbitType;
    // ...
}
```

The moment you split into a second file, add a `graphcal.toml` at
the project root and arrange the files under `<source_dir>/<pkg>/`
as shown above. Sibling-file `import`s are rejected with a clear
error from any file that is not itself inside the package namespace —
including a file sitting next to a `graphcal.toml` but outside its
`<source_dir>/<pkg>/` directory.

## Circular Import Detection

Graphcal detects circular imports at compile time. In a real package
with two modules `<pkg>.a` and `<pkg>.b`:

```graphcal
// src/<pkg>/a.gcl
import <pkg>.b.{x};

// src/<pkg>/b.gcl
import <pkg>.a.{y};   // ERROR: circular import
```

## Assertions Are Always Checked

When you import a file, **all of its assertions are automatically
evaluated**, even if you don't import them by name. This ensures
that safety invariants in library files are never silently skipped.
See
[Assertions](../language/assertions.md#assertions-in-multi-file-projects)
for details.

## What You Learned

- Every `.gcl` file is a **package** — virtual (single-file
  standalone script) or real (manifest-backed multi-file project).
- A virtual package has exactly one file. Multi-file projects always
  have a `graphcal.toml`.
- The three `import` forms — bare, aliased, and brace list — bring
  exactly the names you write into scope.
- `import` is for compile-time names; runtime values cross file
  boundaries via `include`.
- Circular imports and assertion checks are handled automatically.

## Next Step

In [Step 6](step6-indexed-values.md), you'll work with indexed
collections for multi-element calculations.
