# Phase 4: Multi-File and Namespaces

> Split a project across multiple `.graph` files with `import` declarations.

## Goal

Prove that projects can scale beyond a single file. A `project.graph`
root file defines the project. Files are modules. Cross-file references
use explicit `import` declarations. A prelude provides shared vocabulary.

## Prerequisites

Phases 0-3 must be complete. All single-file features (scalars, dimensions,
structs, functions) work. Phase 4 adds the organizational layer.

## Design Decisions to Lock

### From [09-namespace](../09-namespace.md)

- [ ] **Module naming:** File path determines module name.
      `orbit/transfer.graph` -> `orbit.transfer`. Confirm.
- [ ] **Project root:** `project.graph` file with project metadata.
      What is the syntax?

      ```
      project mission_design {
          version: "0.1.0",
          prelude: "prelude.graph",
      }
      ```

      Or simpler? Is `project.graph` required, or can a single `.graph` file
      be evaluated standalone (backward compat with Phase 0-3)?
- [ ] **Prelude loading:** The prelude file's declarations are auto-imported
      into every file. How does this work mechanically? Is the prelude
      parsed first, then its namespace merged into every file's scope?
- [ ] **User prelude vs built-in prelude:** Phase 1 had built-in dimensions/units.
      Phase 4 should allow users to define their own prelude that extends
      the built-in one. How do they compose?
- [ ] **Import syntax:** `import orbit.transfer.{ transfer, parking_alt };`
      Confirm selective import with `{ }`.
- [ ] **Glob imports:** `import module.*;` -- allowed but discouraged?
      Or lint-warned? Or forbidden entirely?
- [ ] **No import aliasing:** `import orbit.transfer as ot;` is NOT supported.
      Confirm this per the "Language for Agents" insight.
- [ ] **No re-exports:** A module cannot re-export names from another module.
      Confirm this.
- [ ] **Circular imports:** Compile error. Not just circular node dependencies
      (already caught by DAG acyclicity), but circular file-level imports.
- [ ] **Qualified references:** Can you write `@orbit.transfer.total_dv` without
      an `import` statement? Or must all cross-module references go through `import`?
      Recommendation: require `import` for clarity, but allow qualified `@mod.name`
      as well.
- [ ] **Resolution order:** Current file -> `import` imports -> prelude -> ambiguity error.
      Confirm.

### From [08-scoping](../08-scoping.md) (multi-file)

- [ ] **`@` with module paths:** `@orbit.transfer.total_dv` -- how is this parsed?
      Is `orbit.transfer` a module path and `total_dv` a name? Or is
      `orbit` a local name and `.transfer.total_dv` field access?
      Rule: `@` starts a graph reference. After `@`, the parser greedily
      resolves the longest module path, then the final segment is the name,
      then any further `.` is field access.
- [ ] **Ambiguity between field access and module path:** `@a.b.c` could be
      field `c` on field `b` on node `a`, or name `c` in module `a.b`.
      How is this disambiguated?

### Visibility

- [ ] **Public-by-default:** Top-level `param`, `node`, `const`, `type`, `fn`
      are importable by other files. Confirm.
- [ ] **`private` keyword:** `private node _helper = ...;` hides from other files.
      Does `private` apply to `type`, `fn`, `dimension`, `unit` as well?
- [ ] **Private fields:** Are struct fields always public, or can individual
      fields be `private`? Recommendation: all fields public (structs are data).

### File Discovery

- [ ] **Automatic vs explicit:** Does the compiler discover all `.graph` files
      under the project root, or must they be listed somewhere?
      Recommendation: auto-discover (like Go packages).
- [ ] **Naming conventions:** Any enforced conventions for file/directory names?
      Snake_case recommended?

## Syntax Supported in Phase 4

Everything from Phase 3, plus:

```ebnf
// Project root (project.graph only)
ProjectDecl  = "project" IDENT "{" ProjectField* "}"
ProjectField = IDENT ":" Literal ","

// Import
ImportDecl   = "import" ModulePath "." "{" ImportList "}" ";"
             | "import" ModulePath "." "*" ";"
ModulePath   = IDENT ("." IDENT)*
ImportList   = IDENT ("," IDENT)* ","?

// Visibility modifier (prefix on any declaration)
Visibility   = "private"

// Extended graph reference (module-qualified)
GRAPH_REF    = "@" IDENT ("." IDENT)*
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **File discovery** | Walk project directory, find all `.graph` files |
| **Module namer** | Map file paths to module names |
| **Project root parser** | Parse `project.graph` for metadata and prelude path |
| **Prelude loader** | Parse prelude, inject into every file's scope |
| **Import resolver** | Parse `import` statements, resolve to target declarations |
| **Cross-file `@` resolution** | Resolve qualified `@mod.name` references |
| **Visibility checker** | Enforce `private` -- reject imports of private names |
| **Circular import detector** | Detect and report circular file-level dependencies |
| **Ambiguity reporter** | Detect when `@name` matches multiple imports |

## Out of Scope

- External packages / dependency management
- Package registry
- Versioning of imports

## Milestone Test

```
mission/
  project.graph
  prelude.graph
  orbit/transfer.graph
  propulsion/fuel_budget.graph
```

```gcl
// project.graph
project mission {
    version: "0.1.0",
    prelude: "prelude.graph",
}
```

```gcl
// prelude.graph
dimension Velocity = Length / Time;
dimension Acceleration = Length / Time^2;
const G0: Acceleration = 9.80665 m/s^2;
const R_earth: Length = 6371 km;
const GM_earth = 398600.4418 km^3/s^2;
```

```gcl
// orbit/transfer.graph
param parking_alt: Length = 200 km;
param target_alt: Length = 35786 km;

node transfer: TransferResult = {
    let r1 = @R_earth + @parking_alt;
    let r2 = @R_earth + @target_alt;
    hohmann_dv(@GM_earth, r1, r2)
};
```

```gcl
// propulsion/fuel_budget.graph
import orbit.transfer.{ transfer };

param dry_mass: Mass = 1200 kg;
param isp: SpecificImpulse = 320 s;

node v_exhaust: Velocity = @isp * @G0;
node fuel_mass: Mass = @dry_mass * (exp(@transfer.total_dv / @v_exhaust) - 1.0);
```

```sh
$ cellgraph eval mission/
...
fuel_mass = 2847 kg
```

### Error cases that must work

```gcl
// error: missing import
node x = @transfer.total_dv;
//  error[N002]: unknown graph reference `@transfer`
//  help: add `import orbit.transfer.{ transfer };`

// error: ambiguous reference
//  (when two imports bring the same name)

// error: importing private name
import orbit.transfer.{ _debug_helper };
//  error: `_debug_helper` is private
```

## Open Questions

- [ ] Should single-file evaluation still work without a `project.graph`?
      E.g., `cellgraph eval single.graph` uses built-in prelude only.
      This preserves backward compatibility with Phases 0-3.
- [ ] Should unused imports be a warning or error?
- [ ] Should the prelude be allowed to import from non-prelude files?
      Recommendation: no (prelude is foundational).
