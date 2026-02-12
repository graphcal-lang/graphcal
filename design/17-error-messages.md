# Error Messages and Diagnostics

> How the compiler communicates errors, warnings, and suggestions.

## Status

**Decision level:** Early. Error codes and formats are sketched in examples but not systematically designed.

## Summary

Cellgraph aims for Rust-quality error messages: specific error codes, source location, context, and actionable suggestions. This aspect covers error categorization, message format, and diagnostic philosophy.

## Error Code Prefixes (Observed in Design Docs)

| Prefix | Domain | Example |
| --- | --- | --- |
| `S0xx` | Space errors | `S001`: space mismatch |
| `N0xx` | Namespace errors | `N001`: ambiguous reference, `N002`: unknown reference |
| `F0xx` | Function errors | `F001`: `@` in function body |

## Example Error Format

```
error[N002]: unknown graph reference `@transfer`
  --> propulsion/fuel_budget.graph:7:42
   |
 7 |     node fuel_mass = ... @transfer.total_dv ...
   |                          ^^^^^^^^^
   |
   = help: did you mean `orbit.transfer.transfer`?
   = help: add `use orbit.transfer.{ transfer };`
```

## Error Categories to Design

| Category | Examples |
| --- | --- |
| **Parse errors** | Unexpected token, missing semicolon, unmatched brace |
| **Type errors** | Dimension mismatch (`Length + Mass`), wrong argument type |
| **Space errors** | Cross-space mixing without `.untagged` or `Transform` |
| **Graph errors** | Cyclic dependency, self-reference, unknown node |
| **Namespace errors** | Missing import, ambiguous reference, unused import |
| **Function errors** | `@` in fn body, wrong arity, type mismatch in generics |
| **Table errors** | Axis mismatch in aggregation, missing column |
| **Unit errors** | Incompatible unit conversion, ambiguous unit |
| **Warnings** | Unused node, shadowed import, zero-argument fn (suggest const) |

## Open Questions

- **Error code system:** Should there be a systematic error code scheme? What are all the prefixes?
- **Warning levels:** Are there warning levels (info, warning, error)? Can warnings be suppressed per-file or per-line?
- **LSP diagnostics:** How do errors map to Language Server Protocol diagnostics for IDE integration?
- **Error recovery:** Should the parser attempt error recovery to report multiple errors in one pass?
- **Suggestions:** How intelligent should suggestions be? "Did you mean X?" requires fuzzy matching.
- **Lints:** Beyond errors, should there be a lint system (like `clippy` for Rust)?
- **Internationalization:** Should error messages be localizable?
- **Machine-readable output:** Should the compiler support JSON error output for tool integration?

## Dependencies on Other Aspects

- **All other aspects:** Each language feature produces its own class of errors.
- **Live View** ([13](./13-live-view.md)): Errors may be displayed inline in the grid.
- **Syntax** ([02](./02-syntax-design.md)): Parse errors depend on grammar design.
