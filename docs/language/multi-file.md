---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal supports splitting projects across multiple files using `use` imports.

## The `use` Statement

```
use "./path/to/file.gcl" { name1, name2 };
```

- The path is a **string literal** relative to the importing file
- Braces list the specific names to import
- All top-level declarations can be imported: `param`, `node`, `const`, `dimension`, `unit`, `type`, `index`, `fn`

## Path Resolution

Paths are resolved relative to the file containing the `use` statement:

```
// In project/main.gcl:
use "./lib/constants.gcl" { G0 };     // resolves to project/lib/constants.gcl
use "../shared/units.gcl" { knot };   // resolves to shared/units.gcl
```

## Import Aliasing

Rename imports with `as` to avoid name conflicts:

```
use "./file_a.gcl" { velocity as velocity_a };
use "./file_b.gcl" { velocity as velocity_b };

node diff: Velocity = @velocity_a - @velocity_b;
```

## What Can Be Imported

| Declaration | How to Import | How to Reference |
|-------------|--------------|-----------------|
| `param` | `use "..." { name }` | `@name` |
| `node` | `use "..." { name }` | `@name` |
| `const` | `use "..." { NAME }` | `NAME` |
| `dimension` | `use "..." { DimName }` | `DimName` |
| `unit` | `use "..." { unit_name }` | `unit_name` |
| `type` | `use "..." { TypeName }` | `TypeName` |
| `index` | `use "..." { IndexName }` | `IndexName` |
| `fn` | `use "..." { fn_name }` | `fn_name(...)` |

Imported `param` and `node` declarations are referenced with `@` just like local ones.

## Circular Import Detection

Graphcal detects circular imports at compile time:

```
// a.gcl
use "./b.gcl" { x };

// b.gcl
use "./a.gcl" { y };
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

When running `graphcal eval`, the entry file is the one you pass on the command line. All `use` dependencies are resolved transitively from that file:

```bash
graphcal eval project/main.gcl
```
