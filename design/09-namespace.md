# Namespace and Multi-File Management

> How projects are organized across files: modules, imports, visibility.

## Status

**Decision level:** Mostly settled. Key principles established.

## Summary

One graph per project, split across files. Files are organizational units, not isolation boundaries. Explicit imports are enforced. Public-by-default. Prelude is the single auto-import exception.

## Project Structure

```
mission/
+-- project.graph           # project root
+-- prelude.graph            # dimensions, units, constants (auto-imported)
+-- constants.graph
+-- orbit/
|   +-- transfer.graph
|   +-- station_keeping.graph
+-- propulsion/
|   +-- engines.graph
|   +-- fuel_budget.graph
+-- scenarios/
    +-- baseline.scenario
    +-- high_isp.scenario
```

File path determines module name:

```
orbit/transfer.graph       ->  module orbit.transfer
propulsion/engines.graph   ->  module propulsion.engines
```

## Project Root

```gcl
project mission_design {
    version: "0.1.0",
    prelude: "prelude.graph",
}
```

## Imports

Explicit `use` statements are required for cross-file references:

```gcl
use orbit.transfer.{ transfer };
use constants.{ dry_mass_budget };
```

Glob imports are allowed but discouraged:

```gcl
use propulsion.engines.*;   // glob -- use sparingly
```

Missing import produces a helpful error:

```
error[N002]: unknown graph reference `@transfer`
  --> propulsion/fuel_budget.graph:7:42
   = help: did you mean `orbit.transfer.transfer`?
   = help: add `use orbit.transfer.{ transfer };`
```

## Prelude

The prelude (`prelude.graph`) is auto-imported into every file. It contains project-wide vocabulary: dimensions, units, indexes, and shared constants.

## Visibility

Public-by-default. Use `private` to hide internal helpers:

```gcl
// Public (default)
param dry_mass = 1200 kg;
node fuel_mass = @dry_mass * (exp(@total_dv / @v_exhaust) - 1);

// Private
private node _debug_ratio = @transfer.dv1 / @transfer.dv2;
```

## Resolution Order for `@name`

1. **Current file** -- params, nodes, consts, tables in this file
2. **Explicit `use` imports** -- names brought in by `use` declarations
3. **Prelude** -- dimensions, units, constants from `prelude.graph`
4. **Ambiguity** -- compile error, require qualification

## Open Questions

- **Circular imports:** Are circular dependencies between files allowed? If A uses B and B uses A, is that a compile error, or resolved by the DAG (since the graph must be acyclic, circular file imports could still produce an acyclic DAG)?
- **Re-exports:** Can a module re-export names from another module? E.g., `pub use orbit.transfer.{ transfer };` so consumers can import from a single entry point.
- **Qualified references:** Can you use `@orbit.transfer.total_dv` without a `use` statement? Or must all cross-module references go through `use`?
- **Multiple preludes:** Can a project have multiple prelude files (e.g., one for dimensions, one for constants)? Or exactly one?
- **External packages:** Is there a package/dependency system for sharing `.graph` libraries across projects? Or is this deferred?
- **File discovery:** Does the compiler discover all `.graph` files in the project directory, or must they be registered somewhere?
- **Naming conventions:** Are there enforced naming conventions for files and modules? Snake_case? Kebab-case?

## Dependencies on Other Aspects

- **Scoping** ([08](./08-scoping.md)): `@` resolution follows the import chain.
- **Syntax** ([02](./02-syntax-design.md)): `use` and `private` keywords.
- **Computation Model** ([01](./01-computation-model.md)): The graph spans multiple files.
- **Git Workflow** ([16](./16-git-workflow.md)): Multi-file projects are the reason for Git integration.
