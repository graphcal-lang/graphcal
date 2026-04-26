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

Every `.gcl` file in Graphcal is a **package**. As long as no
`graphcal.toml` manifest is present, each file is a *virtual* package
whose name equals the file's stem — `constants.gcl` is the package
named `constants`. You can `import` from it by that name from any
sibling file in the same directory.

This is the simplest way to start. Later in this step, we'll
"promote" the project to a real (manifest-backed) package once the
file count grows.

## Project Structure

```text
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
pub param dry_mass: Mass = 1200.0 kg;
pub param fuel_mass: Mass = 2800.0 kg;
pub param isp: Time = 320.0 s;
```

### `main.gcl`

```graphcal
import constants.{g0};
import params.{dry_mass, fuel_mass, isp};

dim Velocity = Length / Time;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

The path before `.{...}` is the package — here, the file's own stem.
The brace list lists the names being brought into scope.

## Running a Multi-File Project

Point `graphcal eval` at the entry file:

```bash
$ graphcal eval rocket_project/main.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
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
| `type`           | `import file.{TypeName}`            | `TypeName`       |
| `index`          | `import file.{IndexName}`           | `IndexName`      |
| `dag`            | `import file.{dag_name}`            | `include`-d, or called as `@dag_name(...).out` |

## Promoting to a Real Package

Once a project grows beyond a handful of files in one directory, add
a `graphcal.toml` manifest at the project root. This makes the
package "real" — its name comes from the manifest instead of from
each file's stem, and source files live under a dedicated source
directory.

```text
rocket_project/
  graphcal.toml         # [package] name = "rocket_project"
  src/
    rocket_project/
      constants.gcl
      params.gcl
      main.gcl
```

```toml
# graphcal.toml
[package]
name = "rocket_project"
# source_dir = "src"  # optional, defaults to "src"
```

After promotion, every import in `main.gcl` is rewritten to the full
package path:

```graphcal
import rocket_project.constants.{g0};
import rocket_project.params.{dry_mass, fuel_mass, isp};
```

The semantics are identical to the virtual-package version — only
the prefix changes. The LSP rename refactor handles this rewrite
mechanically.

## Circular Import Detection

Graphcal detects circular imports at compile time:

```graphcal
// a.gcl
import b.{x};

// b.gcl
import a.{y};   // ERROR: circular import
```

## Assertions Are Always Checked

When you import a file, **all of its assertions are automatically
evaluated**, even if you don't import them by name. This ensures
that safety invariants in library files are never silently skipped.
See
[Assertions](../language/assertions.md#assertions-in-multi-file-projects)
for details.

## What You Learned

- Every `.gcl` file is a **package** — virtual (file stem) or real
  (manifest-backed).
- The three `import` forms — bare, aliased, and brace list — bring
  exactly the names you write into scope.
- `import` is for compile-time names; runtime values cross file
  boundaries via `include`.
- A project graduates from a virtual package to a real package by
  adding a `graphcal.toml` and rewriting prefixes.
- Circular imports and assertion checks are handled automatically.

## Next Step

In [Step 6](step6-indexed-values.md), you'll work with indexed
collections for multi-element calculations.
