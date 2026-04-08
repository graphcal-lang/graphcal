# Development Phases

Cellgraph is developed in phases. Each phase produces a usable artifact
(you can run a `.graph` file end-to-end), and design decisions within a
phase are locked before later phases begin. A valid Phase 0 file remains
valid in all later phases.

## Dependency Graph

```
Phase 0: Scalar Graph (f64, param/node/const node, @, single file)
    |
Phase 1: Dimensions & Units
    |
Phase 2: Structs & Multi-Line Nodes
    |
Phase 3: DAG Blocks (dag/include, replaces fn)
    |
Phase 4: Multi-File & Namespaces (import, private, prelude)
    |
Phase 5: Indexed Values (index, T[I], for, sum/reduce/scan)
    |
Phase 6: Scenarios & CLI Workflow
    |
    +------- MVP complete -------
    |
    +---> Phase 8: System Dynamics (range indexes, scan over time)
    |
    +---> Phase 9: Spaces
    +---> Phase 10: Tagged Unions & Match
    +---> Phase 11: Live View (TUI)
    +---> Phase 12: Spreadsheet Compat
    +---> Phase 13: Python Interop
```

## Phase Index

### MVP Path (Phases 0-6)

| Phase | File | Adds |
| --- | --- | --- |
| [0](./phase-0-scalar-graph.md) | Scalar Graph | `param`/`node`/`const node`, `@`, `f64`, single file |
| [1](./phase-1-dimensions-and-units.md) | Dimensions & Units | `dimension`, `unit`, `->`, type annotations |
| [2](./phase-2-structs.md) | Structs & Multi-Line Nodes | `type` (single-variant), block bodies, `let` |
| [3](./phase-3-pure-functions.md) | ~~Pure Functions~~ DAG Blocks | ~~`fn`~~ `dag`/`include` (fn removed; see [design 24](../24-inline-modules.md)) |
| [4](./phase-4-multi-file.md) | Multi-File & Namespaces | `import`, `private`, `project.graph`, prelude |
| [5](./phase-5-tables.md) | Indexed Values | `index`, `T[I]`, `for`, sum/reduce/scan |
| [6](./phase-6-scenarios.md) | Scenarios & CLI | `.scenario`, `cellgraph check`, assertions |

### Post-MVP

| Phase | File | Adds |
| --- | --- | --- |
| [8+](./phase-7-and-beyond.md) | Post-MVP | System dynamics, spaces, tagged unions, TUI, spreadsheet compat, Python interop |

## How to Use These Documents

Each phase document is a **sketchboard**: a working document to be refined
as the design matures. It contains:

1. **Goal** -- What this phase proves
2. **Design decisions to lock** -- Open questions that must be resolved before implementation
3. **Syntax to support** -- Exact grammar subset for this phase
4. **Implementation scope** -- What to build
5. **Out of scope** -- What is explicitly deferred
6. **Milestone test** -- A concrete `.graph` example that must work
7. **Open questions** -- Unresolved issues to discuss

Edit these files directly as you refine the design. Reference the
[aspect documents](../README.md) for deeper background on each topic.
