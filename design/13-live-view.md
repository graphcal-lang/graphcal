# Live View and Rendering

> Auto-rendered visualization of the computation graph (Mermaid-style).

## Status

**Decision level:** Conceptual. Rendering rules outlined, but implementation details are open.

## Summary

Write text, get auto-rendered live view. Zero layout control -- like Mermaid.js. The `.graph` file is the source of truth; the grid/plot is an auto-generated view.

## Rendering Rules

1. Params appear first (declaration order)
2. Nodes in topological order
3. Structs expand into sub-rows
4. Tables render as embedded grids
5. Dependency DAG always visible
6. Scans over time axes render as time series plots

## Example Rendered View

```
+-- PARAMETERS ---------+-- Value ---+-- Description -------+
|  parking_alt           | [ 200.0 ]  |  Parking orbit km    |
|  target_alt            | [35786.0]  |  GEO altitude km     |
|  isp                   | [ 320.0 ]  |  Specific imp. s     |
+-- RESULTS ------------+-- Value ---+-- Depends on ---------+
|  v_exhaust             |  3138.1    |  isp                 |
|  +- transfer           |            |                      |
|  |   .dv1              |  2.457     |  parking_alt         |
|  |   .dv2              |  1.478     |  target_alt          |
|  total_dv              |  3.935     |  transfer            |
+------------------------+------------+----------------------+
```

## Evaluation Model for Live View

The live view drives the **hybrid eager/lazy evaluation** strategy described in [01-computation-model.md](./01-computation-model.md):

- **Displayed nodes are eagerly recomputed.** When a `param` changes (e.g., via a slider in Edit mode), all nodes currently visible in the rendered grid are recomputed immediately. This provides the spreadsheet-style responsiveness users expect.
- **Non-visible nodes are lazily evaluated.** Nodes outside the current view (scrolled off-screen, collapsed sections, or in other modules) are only evaluated when demanded (e.g., when the user scrolls to them or when a visible node depends on them).
- **Early cutoff prevents cascading updates.** If a recomputed node produces the same value as before, its dependents are not re-rendered. This keeps the UI snappy even for large graphs with many downstream nodes.
- **Cache eviction.** Node values that haven't been demanded in recent evaluation cycles are evicted (age-based, following Typst's comemo pattern). This bounds memory usage for long-running live view sessions.

## Interaction Modes

| Mode | Description |
| --- | --- |
| **View** | Explore graph, see dependencies highlighted |
| **Edit** | Change param values in grid, instant recompute (ephemeral) |
| **Code** | Edit node logic, updates `.graph` file |
| **Commit** | Write edited param values back to `.graph` file |

## Multi-File Live View

Cross-file dependencies show their source module:

```
+-- EXTERNAL DEPS ------+-- Value ---+-- Source ----------------+
|  @total_dv             |  3935 m/s  |  orbit.transfer          |
|  @v_exhaust            |  3138 m/s  |  (local)                 |
+------------------------+------------+--------------------------+
```

## N-Dimensional Table Rendering

| Dimensionality | Visualization |
| --- | --- |
| 1D | Simple table |
| 2D | Matrix / heatmap |
| 3D | Matrix + slice selector |
| 4D+ | Multiple slice selectors |

Smallest cardinality axes are used for slicers.

## Open Questions

- **Graph visualization:** Should the DAG be rendered as a visual graph (nodes and edges) in addition to or instead of a table? Or both (switchable)?
- **Plot types:** Beyond time series for scans, what plot types are auto-rendered? Scatter? Bar? User-configurable?
- **Units in display:** Which unit is used for display? The declaration unit? User preference?
- **Sensitivity highlighting:** Should the view highlight which params have the most impact on a selected output node?
- **Diff view:** Can the live view show the difference between two scenarios side-by-side?
- **Custom layouts:** Should users ever be able to customize the layout, or is it always fully automatic?
- **Mobile / responsive:** How does the grid render on small screens?
- **Accessibility:** Keyboard navigation, screen reader support, color-blind friendly palettes.
- **Implementation order:** TUI (ratatui) first, then Web (WASM), then VS Code. Is the rendering model identical across all three, or adapted per platform?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): The DAG structure drives the layout.
- **Tables** ([10](./10-tables-and-autofill.md)): Table rendering is a major part of the view.
- **System Dynamics** ([11](./11-system-dynamics.md)): Time series plots.
- **Algebraic Data Types** ([05](./05-algebraic-data-types.md)): Struct expansion into sub-rows.
- **Spreadsheet Compatibility** ([14](./14-spreadsheet-compatibility.md)): The view must feel familiar to spreadsheet users.
