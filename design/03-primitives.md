# Type System -- Primitives

> Layer 1: Base scalar types.

## Status

**Decision level:** Mostly settled. Core set defined.

## Summary

Primitives are the base scalar types that all other type layers build upon. They map directly to spreadsheet cell types.

## Primitive Types

| Type | Description | Example |
| --- | --- | --- |
| `f64` | 64-bit floating point (explicitly dimensionless) | `3.14` |
| `i64` | 64-bit integer | `42` |
| `bool` | Boolean | `true`, `false` |
| `Str` | Text string | `"hello"` |
| `Datetime` | Date and time | TBD |
| `Option<T>` | Nullable / optional value | `Some(42)`, `None` |

## Design Notes

- `f64` without a unit annotation is explicitly **dimensionless** (not "untyped"). This interacts with the dimension system: `exp()` and `ln()` require dimensionless arguments.
- `i64` is used for discrete quantities. The interaction between `i64` and the dimension system needs clarification (e.g., can you have `i64` with a dimension?).
- `Option<T>` maps to blank cells in spreadsheets. This is important for spreadsheet import/export.

## Open Questions

- **Integer dimensions:** Can `i64` carry physical dimensions (e.g., `i64<Length>` for pixel counts)? Or is `i64` always dimensionless?
- **Datetime representation:** What is the internal representation? Unix timestamp? Calendar type? How does it interact with the `Time` dimension?
- **Complex numbers:** Are complex numbers needed for engineering calculations (e.g., transfer functions, signal processing)?
- **Fixed-point / decimal:** Is `f64` sufficient for financial calculations, or should a `Decimal` type be provided?
- **Implicit conversions:** Is `i64` implicitly convertible to `f64`? Or must conversion be explicit?
- **Collection types:** Beyond `Option<T>`, are there built-in list/array types outside of tables?

## Dependencies on Other Aspects

- **Dimensions** ([04](./04-dimensions-and-units.md)): Primitives are the base that dimensions annotate.
- **Tables** ([10](./10-tables-and-autofill.md)): Table columns have primitive (or dimensioned) types.
- **Spreadsheet Compatibility** ([14](./14-spreadsheet-compatibility.md)): Primitives map to Excel cell types.
