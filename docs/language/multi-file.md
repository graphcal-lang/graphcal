---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal supports splitting projects across multiple files using `import` declarations. There are two import styles: **selective imports** and **module imports**.

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

Place an empty `graphcal.toml` file in an ancestor directory to widen the project root to that directory:

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
