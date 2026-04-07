# Cellgraph Language Design -- Decomposed Aspects

> [!WARNING]
> This directory may contain outdated design documents.

Each aspect of the language design is documented in a separate file for independent discussion. Files are numbered for reference but can be read in any order.

## Dependency Map

```
                    01-computation-model
                     /        |        \
              02-syntax    08-scoping   10-tables
             /  |    \        |            |    \
           03  04  05-ADT  09-namespace  11-sys  07-indexes
               |    |                    dynamics
             06-spaces  12-pure-functions
                            |
                     (uses 04, 06, 08)

         13-live-view  14-spreadsheet  15-python  16-git  17-errors
         (rendering)   (import/export) (PyO3)   (scenarios) (diagnostics)
```

## Index

### Core Language

| # | Aspect | Status | Description |
| --- | --- | --- | --- |
| [01](./01-computation-model.md) | Computation Model | Mostly settled | DAG of `param`/`node`/`const`, reactive evaluation, incremental recomputation |
| [02](./02-syntax-design.md) | Syntax Design | Mostly settled | Keywords, expressions, statement forms |
| [08](./08-scoping.md) | Scoping (`@` Sigil) | Settled | `@name` = graph scope, bare `name` = local scope |
| [09](./09-namespace.md) | Namespace & Multi-File | Mostly settled | Modules, imports, visibility, prelude |

### Type System

| # | Aspect | Status | Description |
| --- | --- | --- | --- |
| [20](./20-type-system-stratification.md) | **Type System Stratification** | Proposal | Three-level model: Primitive → ValueType → DeclType. Unifies docs 03-07. |
| [21](./21-separate-label-indexes-from-tagged-unions.md) | **Separate Label Indexes from Tagged Unions** | Draft | Revert index/tagged-union unification. Dedicated `Label(IndexName)` type kind. |
| [03](./03-primitives.md) | Primitives (Level 1) | Mostly settled | `f64`, `i64`, `bool`, `Str`, `Datetime`, `Option<T>` |
| [04](./04-dimensions-and-units.md) | Dimensions & Units | Mostly settled | Dimensions as types, units as values (Numbat-inspired) |
| [05](./05-algebraic-data-types.md) | Algebraic Data Types (Level 2) | Mostly settled | Unified `type` for structs and tagged unions (Gleam-style) |
| [06](./06-spaces.md) | Spaces | Mostly settled | Semantic context tags preventing cross-space mixing (Sguaba-inspired) |
| [07](./07-indexes.md) | Indexes | Mostly settled | Finite label sets as table axes |

### Data & Simulation

| # | Aspect | Status | Description |
| --- | --- | --- | --- |
| [10](./10-tables-and-autofill.md) | Tables & Autofill | Partially settled | N-dimensional labeled tables with map/scan/reduce |
| [11](./11-system-dynamics.md) | System Dynamics | Mostly settled | Temporal simulation via `scan` pattern (Vensim replacement) |
| [12](./12-pure-functions.md) | Pure Functions | Superseded by 24 | `fn` keyword, purity via `@` prohibition |
| [24](./24-inline-modules.md) | **Inline DAGs, `const` Modifier, DAG-as-Module** | Draft | Replace `fn` with `dag` blocks, `const` as modifier, `import`/`include` split, unified module path system |

### Tooling & Ecosystem

| # | Aspect | Status | Description |
| --- | --- | --- | --- |
| [13](./13-live-view.md) | Live View & Rendering | Conceptual | Auto-rendered grid/DAG visualization (Mermaid-style) |
| [14](./14-spreadsheet-compatibility.md) | Spreadsheet Compatibility | Conceptual | Excel import/export, `.sheetmap` bidirectional sync |
| [15](./15-python-interop.md) | Python Interop | Conceptual | PyO3 bindings, `#[python]` nodes, parameter sweeps |
| [16](./16-git-workflow.md) | Git Workflow & Scenarios | Mostly settled | `.graph` as source of truth, `.scenario` overlays |
| [17](./17-error-messages.md) | Error Messages & Diagnostics | Design settled | Error codes, format, miette-based diagnostic system |
| [18](./18-non-si-dimensions.md) | Non-SI Base Dimensions | Proposal | User-defined base dimensions (Information, Currency, etc.) |
| [19](./19-assertions-and-testing.md) | Assertions & Testing | Design in progress | `assert` declarations, `#[...]` attributes, `#[assumes]`, tolerance syntax |
| [22](./22-vega-lite-plotting.md) | Vega-Lite Plotting | Proposal | Replace Plotly with Vega-Lite grammar via Altair-inspired API |

## How to Use These Files

Each file follows a consistent structure:

1. **Status** -- How settled the design is
2. **Summary** -- What this aspect covers
3. **Current design** -- What's been decided
4. **Open questions** -- What needs discussion
5. **Dependencies** -- How this aspect relates to others

Pick any file and discuss its open questions independently. Cross-references between files use relative links.
