---
icon: material/book-open-variant
---

# Language Reference

This section provides formal documentation of all Graphcal language features.

## Overview

Graphcal is a domain-specific language for engineering calculations built around a **directed acyclic graph (DAG)** of computations. Every `.gcl` file describes parameters (inputs), nodes (computed values), and constants, connected by explicit references.

The language has a layered type system:

| Layer | Feature | Purpose |
|-------|---------|---------|
| 1 | [Primitives](type-system.md) | `Float`, `Int`, `Bool` base types |
| 2 | [Dimensions](dimensions-and-units.md) | Physical dimension algebra (compile-time types) |
| 3 | [Units](dimensions-and-units.md) | Value-level scaling factors attached to dimensions |
| 4 | [Algebraic Data Types](algebraic-data-types.md) | Structs, union types, pattern matching |
| 5 | [Indexes](indexes.md) | Finite label sets for collections |
| 6 | [DAG Blocks](functions.md) | Reusable computation via `dag` blocks and `include` |

## Reference Pages

<div class="grid cards" markdown>

- :material-graph:{ .lg .middle } **Computation Model**

    ---

    DAG semantics, `param`/`node`/`const node`, the `@`-sigil.

    [:octicons-arrow-right-24: Computation model](computation-model.md)

- :material-format-list-bulleted-type:{ .lg .middle } **Type System**

    ---

    Primitive types: `Float`, `Int`, `Bool`, and conversions.

    [:octicons-arrow-right-24: Type system](type-system.md)

- :material-ruler:{ .lg .middle } **Dimensions & Units**

    ---

    Dimension algebra, unit definitions, conversion, prelude.

    [:octicons-arrow-right-24: Dimensions & units](dimensions-and-units.md)

- :material-shape:{ .lg .middle } **Algebraic Data Types**

    ---

    Structs, union types, `match` expressions, generics.

    [:octicons-arrow-right-24: ADTs](algebraic-data-types.md)

- :material-format-list-numbered:{ .lg .middle } **Indexes**

    ---

    Finite indexes, range indexes, `for`, `scan`, `unfold`.

    [:octicons-arrow-right-24: Indexes](indexes.md)

- :material-function:{ .lg .middle } **DAG Blocks**

    ---

    Reusable computation via `dag` blocks and `include`.

    [:octicons-arrow-right-24: DAG Blocks](functions.md)

- :material-code-parentheses:{ .lg .middle } **Expressions**

    ---

    Operators, precedence, `if`/`else`, literals.

    [:octicons-arrow-right-24: Expressions](expressions.md)

- :material-file-multiple:{ .lg .middle } **Multi-File Projects**

    ---

    `import` declarations, aliasing, circular detection.

    [:octicons-arrow-right-24: Multi-file](multi-file.md)

- :material-check-decagram:{ .lg .middle } **Assertions & Attributes**

    ---

    `assert` declarations, tolerance checks, `#[assumes(...)]`.

    [:octicons-arrow-right-24: Assertions](assertions.md)

- :material-chart-line:{ .lg .middle } **Plots**

    ---

    `plot` declarations, chart types, interactive visualization.

    [:octicons-arrow-right-24: Plots](plots.md)

- :material-package-variant:{ .lg .middle } **Built-in Reference**

    ---

    Prelude dimensions, units, constants, and functions.

    [:octicons-arrow-right-24: Built-ins](built-ins.md)

</div>
