//! Pure projection from evaluated runtime values to report display bodies.
//!
//! Follows the CLI's text conventions so a report and a terminal never
//! disagree: quantities render as `value unit` via the shared display-unit
//! projection, other scalars via [`Value::format_display`], flattened
//! entries as `name[Variant]` / `name.field`, and two-axis indexed values
//! as grids.

use std::collections::BTreeMap;

use graphcal_compiler::dimension::BaseDimId;
use graphcal_eval::eval::{DisplayProjectionError, Value, format_number, quantity_display_value};

/// Display body of one evaluated value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueBody {
    /// A single formatted scalar (`3138.128 m/s`).
    Scalar(String),
    /// Labelled scalar entries flattened from structs and one-axis maps
    /// (`name[Variant]`, `name.field`).
    Entries(Vec<(String, String)>),
    /// A two-axis grid.
    Grid(GridTable),
    /// Three or more axes: one labelled two-axis grid per outer slice.
    Slices(Vec<(String, GridTable)>),
}

/// A two-axis grid of formatted cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridTable {
    /// Inner-axis key labels, in first-seen order.
    pub(crate) columns: Vec<String>,
    /// Outer-axis rows: label plus one cell per column (empty string when a
    /// row has no value for that column).
    pub(crate) rows: Vec<(String, Vec<String>)>,
}

/// Format one non-indexed value the way the CLI's text output does.
///
/// # Errors
///
/// Returns the display-unit projection error for non-finite or underflowing
/// display conversions.
pub fn scalar_display(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Result<String, DisplayProjectionError> {
    match value {
        Value::Quantity {
            si_value,
            display_unit,
            ..
        } => {
            let displayed = quantity_display_value(*si_value, display_unit.as_ref())?;
            let mut out = format_number(displayed);
            if let Some(label) = value.display_label(symbols) {
                out.push(' ');
                out.push_str(&label);
            }
            Ok(out)
        }
        _ => value.format_display(Some(symbols)),
    }
}

/// Number of nested indexed axes (0 = scalar, 1 = one axis, ...).
fn index_depth(value: &Value) -> usize {
    match value {
        Value::Indexed { entries, .. } => 1 + entries.values().next().map_or(0, index_depth),
        _ => 0,
    }
}

/// Project one evaluated value into its report display body.
///
/// # Errors
///
/// Returns the first display-unit projection error encountered in any leaf.
pub(crate) fn project_value_body(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Result<ValueBody, DisplayProjectionError> {
    match index_depth(value) {
        0 => match value {
            Value::Struct { fields, .. } if !fields.is_empty() => {
                let mut entries = Vec::new();
                flatten_entries(String::new(), value, symbols, &mut entries)?;
                Ok(ValueBody::Entries(entries))
            }
            _ => Ok(ValueBody::Scalar(scalar_display(value, symbols)?)),
        },
        1 => {
            let mut entries = Vec::new();
            flatten_entries(String::new(), value, symbols, &mut entries)?;
            Ok(ValueBody::Entries(entries))
        }
        2 => Ok(ValueBody::Grid(project_grid(value, symbols)?)),
        _ => {
            let Value::Indexed { entries, .. } = value else {
                // index_depth >= 3 implies an Indexed value.
                return Ok(ValueBody::Scalar(scalar_display(value, symbols)?));
            };
            let mut slices = Vec::new();
            for key in entries.keys() {
                let label = value.indexed_entry_display_name(key);
                let inner = &entries[key];
                match project_value_body(inner, symbols)? {
                    ValueBody::Grid(grid) => slices.push((label, grid)),
                    ValueBody::Slices(nested) => {
                        for (nested_label, grid) in nested {
                            slices.push((format!("{label}, {nested_label}"), grid));
                        }
                    }
                    // Ragged nesting cannot occur: an indexed value has one
                    // uniform element type, so every entry at depth >= 3
                    // projects to grids or deeper slices.
                    ValueBody::Scalar(_) | ValueBody::Entries(_) => {}
                }
            }
            Ok(ValueBody::Slices(slices))
        }
    }
}

/// Flatten structs and one-axis maps into labelled scalar entries, mirroring
/// the CLI's `name[Variant]` / `name.field` conventions.
fn flatten_entries(
    prefix: String,
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
    out: &mut Vec<(String, String)>,
) -> Result<(), DisplayProjectionError> {
    match value {
        Value::Struct { fields, .. } if !fields.is_empty() => {
            for (field, inner) in fields {
                let label = if prefix.is_empty() {
                    format!(".{field}")
                } else {
                    format!("{prefix}.{field}")
                };
                flatten_entries(label, inner, symbols, out)?;
            }
            Ok(())
        }
        Value::Indexed { entries, .. } => {
            for key in entries.keys() {
                let display_key = value.indexed_entry_display_name(key);
                let label = format!("{prefix}[{display_key}]");
                flatten_entries(label, &entries[key], symbols, out)?;
            }
            Ok(())
        }
        _ => {
            out.push((prefix, scalar_display(value, symbols)?));
            Ok(())
        }
    }
}

/// Project a two-axis indexed value into a grid. Columns are the union of
/// inner keys in first-seen order; missing cells stay empty.
fn project_grid(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> Result<GridTable, DisplayProjectionError> {
    let Value::Indexed { entries, .. } = value else {
        return Ok(GridTable {
            columns: Vec::new(),
            rows: Vec::new(),
        });
    };
    let mut columns: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for key in entries.keys() {
        let row_label = value.indexed_entry_display_name(key);
        let inner = &entries[key];
        let mut cells: Vec<Option<String>> = vec![None; columns.len()];
        if let Value::Indexed {
            entries: inner_entries,
            ..
        } = inner
        {
            for inner_key in inner_entries.keys() {
                let column = inner.indexed_entry_display_name(inner_key);
                let cell = scalar_display(&inner_entries[inner_key], symbols)?;
                if let Some(index) = columns.iter().position(|c| *c == column) {
                    if let Some(slot) = cells.get_mut(index) {
                        *slot = Some(cell);
                    }
                } else {
                    columns.push(column);
                    cells.push(Some(cell));
                }
            }
        }
        rows.push((row_label, cells));
    }
    // Normalize row widths (earlier rows may predate later-added columns).
    let rows = rows
        .into_iter()
        .map(|(label, mut cells)| {
            cells.resize(columns.len(), None);
            (
                label,
                cells
                    .into_iter()
                    // A ragged map legally lacks some (row, column) pairs;
                    // those cells render empty.
                    .map(|cell| cell.unwrap_or_else(String::new))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    Ok(GridTable { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_eval::eval::compile_and_eval;

    fn body_of(source: &str, name: &str) -> ValueBody {
        let result = compile_and_eval(source).unwrap();
        let (_, value, _) = result
            .output_values(graphcal_eval::eval::EvalOutputView::Surface)
            .find(|(n, _, _)| n.to_string() == name)
            .unwrap();
        project_value_body(value.as_ref().unwrap(), &result.base_dim_symbols).unwrap()
    }

    #[test]
    fn quantity_scalar_matches_cli_convention() {
        let body = body_of("node speed: Velocity = 3.5 km/s;", "speed");
        assert_eq!(body, ValueBody::Scalar("3.5 km/s".to_string()));
    }

    #[test]
    fn one_axis_value_flattens_to_entries() {
        let body = body_of(
            "pub index Case = { A, B };\nnode xs: Dimensionless[Case] = { Case#A: 1.0, Case#B: 2.0 };",
            "xs",
        );
        assert_eq!(
            body,
            ValueBody::Entries(vec![
                ("[A]".to_string(), "1".to_string()),
                ("[B]".to_string(), "2".to_string()),
            ])
        );
    }

    #[test]
    fn two_axis_value_projects_to_grid() {
        let body = body_of(
            "pub index R = { R1, R2 };\npub index C = { C1, C2 };\n\
             node grid: Dimensionless[R, C] = for r: R { for c: C { 1.0 } };",
            "grid",
        );
        let ValueBody::Grid(grid) = body else {
            panic!("expected grid, got {body:?}");
        };
        assert_eq!(grid.columns, vec!["C1".to_string(), "C2".to_string()]);
        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.rows[0].0, "R1");
        assert_eq!(grid.rows[0].1, vec!["1".to_string(), "1".to_string()]);
    }

    #[test]
    fn three_axis_value_projects_to_slices() {
        let body = body_of(
            "pub index S = { S1, S2 };\npub index R = { R1 };\npub index C = { C1 };\n\
             node cube: Dimensionless[S, R, C] = for s: S { for r: R { for c: C { 2.0 } } };",
            "cube",
        );
        let ValueBody::Slices(slices) = body else {
            panic!("expected slices, got {body:?}");
        };
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].0, "S1");
        assert_eq!(slices[0].1.rows[0].1, vec!["2".to_string()]);
    }

    #[test]
    fn struct_value_flattens_fields() {
        let body = body_of(
            "pub type Point {\n    Point(x: Dimensionless, y: Dimensionless),\n}\n\
             node p: Point = Point(x: 1.0, y: 2.0);",
            "p",
        );
        assert_eq!(
            body,
            ValueBody::Entries(vec![
                (".x".to_string(), "1".to_string()),
                (".y".to_string(), "2".to_string()),
            ])
        );
    }
}
