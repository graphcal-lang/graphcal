# Spreadsheet Compatibility

> Import from and export to Excel; bidirectional sync via `.sheetmap` files.

## Status

**Decision level:** Conceptual. The strategy is outlined, but format details and mapping rules need specification.

## Summary

Spreadsheet import/export is a first-class workflow, critical for organizational adoption. Engineers maintain the `.graph` source; domain experts keep working in Excel.

## Import

```sh
cellgraph import mission_budget.xlsx --schema > mission_budget.schema
cellgraph import mission_budget.xlsx > mission_budget.graph
```

### Mapping

| Spreadsheet Concept | Cellgraph Concept |
| --- | --- |
| Named cell | `param` node |
| Named range / table | `table` type |
| Formula cell | `node` (reverse-engineered or user-mapped) |
| Sheet | Namespace / module |

## Export

```sh
cellgraph export mission_budget.graph --format xlsx
```

- Simple nodes become Excel formulas
- Complex nodes become frozen values with comments
- Params become highlighted input cells

## Bidirectional Sync

A `.sheetmap` file tracks `node_name <-> cell_reference`. On import, values flow in; on export, results flow out.

```
# mission_budget.sheetmap
parking_alt    -> Sheet1!B2
target_alt     -> Sheet1!B3
fuel_mass      <- Sheet1!B10
total_mass     <- Sheet1!B11
```

## Open Questions

- **Formula reverse-engineering:** How much of Excel formula semantics can be automatically translated to Cellgraph nodes? Where does the automation break down?
- **Formatting:** Should Cellgraph preserve or generate Excel formatting (colors, borders, conditional formatting)?
- **Charts:** Can Excel charts be generated from Cellgraph data? Or is chart generation out of scope?
- **Google Sheets:** Is Google Sheets support via API a priority, or only `.xlsx` files?
- **CSV/TSV:** Should simpler text-based tabular formats be supported alongside Excel?
- **`.sheetmap` specification:** What is the full syntax and semantics of `.sheetmap` files?
- **Conflict resolution:** What happens when both the `.graph` file and the `.xlsx` file have been modified since the last sync?
- **Array formulas:** How do multi-dimensional tables map to Excel array formulas or dynamic arrays?
- **VBA macros:** Should imported workbooks with VBA be supported, or is VBA ignored?

## Dependencies on Other Aspects

- **Tables** ([10](./10-tables-and-autofill.md)): Tables map to Excel tables/ranges.
- **Primitives** ([03](./03-primitives.md)): Primitive types map to Excel cell types.
- **Indexes** ([07](./07-indexes.md)): Indexes map to row/column headers.
- **Live View** ([13](./13-live-view.md)): The grid view should feel familiar to spreadsheet users.
- **Namespace** ([09](./09-namespace.md)): Sheets map to modules.
