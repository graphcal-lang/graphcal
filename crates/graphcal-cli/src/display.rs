//! Text-output formatting helpers for the `eval` subcommand.
//!
//! The CLI text format is the human-readable flavour of evaluation results. It
//! prints:
//!
//! * Scalar / bool / int / struct / datetime values one per line, aligned on
//!   the widest name.
//! * 1D indexed values flattened to `name[Variant]` lines.
//! * Higher-dimensional indexed values rendered as table grids (2D) or as a
//!   stack of 2D table slices with section headers (3D+).
//!
//! Everything in this module is pure: it takes `Value`s and returns strings.
//! `print_text` in `main.rs` owns the actual `println!`/`eprintln!` boundary.
//!
//! # Entry points
//!
//! * [`build_output_blocks`] groups consecutive flat entries and peels out
//!   table blocks while preserving source order.
//! * [`format_indexed_table`] renders an N-dimensional indexed value (N >= 2).
//! * [`FlatEntry`] / [`OutputBlock`] are the data types the renderer walks.

use std::collections::BTreeMap;

use graphcal_compiler::syntax::dimension::BaseDimId;
use graphcal_eval::eval::{NodeError, Value};

/// One line of flat output: either a successfully-evaluated value or an error.
///
/// The first field is the fully-qualified display name (e.g. `foo`, `foo.x`,
/// `foo[Departure]`). The renderer uses the max of these widths to align `=`
/// across a whole `Flat` block.
pub enum FlatEntry<'a> {
    /// A displayable value (scalar, bool, int, struct, datetime, or 1D
    /// indexed flattened to a single entry).
    Value(String, &'a Value),
    /// A node that failed to evaluate — rendered as `name = ERROR: <msg>`.
    Error(String, &'a NodeError),
}

/// A visual block of the text output.
///
/// Consecutive flat entries are coalesced into a single [`OutputBlock::Flat`]
/// so that name-column width is computed per visual group. Tables break up
/// flat blocks because they need their own vertical whitespace.
pub enum OutputBlock<'a> {
    /// A run of flat `name = value` lines that share a name column.
    Flat(Vec<FlatEntry<'a>>),
    /// A 2D-or-higher indexed value rendered as a table grid.
    Table(&'a str, &'a Value),
}

/// Count how many levels of `Indexed` nesting a value has.
///
/// Scalars / bools / structs return `0`. A 1D indexed value returns `1`. A 2D
/// indexed-of-indexed returns `2`, and so on. The table renderer switches
/// modes at depth >= 2.
#[must_use]
pub fn index_depth(value: &Value) -> usize {
    match value {
        Value::Indexed { entries, .. } => entries.values().next().map_or(1, |v| 1 + index_depth(v)),
        _ => 0,
    }
}

/// Walk into nested `Indexed` to find the first leaf scalar's display label (unit).
///
/// Used to annotate the table header (e.g. `delta_v (m/s):`). Returns `None`
/// if the value has no scalar leaves (e.g. an indexed of structs or labels).
#[must_use]
pub fn extract_unit_label(value: &Value, symbols: &BTreeMap<BaseDimId, String>) -> Option<String> {
    match value {
        Value::Scalar { .. } => value.display_label(symbols),
        Value::Indexed { entries, .. } => entries
            .values()
            .next()
            .and_then(|v| extract_unit_label(v, symbols)),
        _ => None,
    }
}

/// Flatten a value into one or more [`FlatEntry::Value`] entries keyed by a
/// dotted-or-indexed display name.
///
/// - Leaves (scalars, bools, ints, labels, datetimes) become a single entry.
/// - Structs expand to `name.field` lines (empty structs stay as a single
///   entry so that unit-struct variants still show up).
/// - 1D indexed values expand to `name[Variant]` lines; higher-dimensional
///   values are NOT flattened here — the caller routes them to a table block.
pub fn flatten_value<'a>(prefix: &str, value: &'a Value, entries: &mut Vec<FlatEntry<'a>>) {
    match value {
        Value::Scalar { .. }
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Label { .. }
        | Value::Datetime { .. } => {
            entries.push(FlatEntry::Value(prefix.to_string(), value));
        }
        Value::Struct {
            type_name: _,
            fields,
        } => {
            if fields.is_empty() {
                entries.push(FlatEntry::Value(prefix.to_string(), value));
            } else {
                for (field_name, field_val) in fields {
                    flatten_value(
                        &format!("{prefix}.{}", field_name.as_str()),
                        field_val,
                        entries,
                    );
                }
            }
        }
        Value::Indexed { entries: idx, .. } => {
            for (variant, entry_val) in idx {
                flatten_value(
                    &format!("{prefix}[{}]", variant.as_str()),
                    entry_val,
                    entries,
                );
            }
        }
    }
}

/// Group a sequence of `(name, Result<Value, NodeError>)` items into output
/// blocks in source order.
///
/// Each 2D-or-deeper indexed value flushes the current flat run and becomes
/// its own [`OutputBlock::Table`]. Everything else is flattened via
/// [`flatten_value`] into the current flat run.
#[must_use]
pub fn build_output_blocks<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a Result<Value, NodeError>)>,
) -> Vec<OutputBlock<'a>> {
    let mut blocks: Vec<OutputBlock<'a>> = Vec::new();
    let mut current_flat: Vec<FlatEntry<'a>> = Vec::new();

    for (name, node_result) in items {
        match node_result {
            Ok(value) if index_depth(value) >= 2 => {
                if !current_flat.is_empty() {
                    blocks.push(OutputBlock::Flat(std::mem::take(&mut current_flat)));
                }
                blocks.push(OutputBlock::Table(name, value));
            }
            Ok(value) => {
                flatten_value(name, value, &mut current_flat);
            }
            Err(err) => {
                current_flat.push(FlatEntry::Error(name.to_string(), err));
            }
        }
    }
    if !current_flat.is_empty() {
        blocks.push(OutputBlock::Flat(current_flat));
    }
    blocks
}

/// Compute the width needed to align the name column across all [`Flat`]
/// blocks (table blocks contribute nothing).
///
/// [`Flat`]: OutputBlock::Flat
#[must_use]
pub fn max_flat_name_len(blocks: &[OutputBlock<'_>]) -> usize {
    blocks
        .iter()
        .filter_map(|b| match b {
            OutputBlock::Flat(entries) => Some(entries.iter().map(|e| match e {
                FlatEntry::Value(n, _) | FlatEntry::Error(n, _) => n.len(),
            })),
            OutputBlock::Table(..) => None,
        })
        .flatten()
        .max()
        .unwrap_or(0)
}

/// Render a 2D `Indexed` value as a formatted table grid (without name/unit
/// header — the caller prepends that).
///
/// Columns come from the first row's variant keys and become the top header.
/// Row variants become the leftmost column. Cells use `format_display(None)`
/// (units are already in the table caption).
#[must_use]
pub fn format_table_grid(value: &Value) -> String {
    use tabled::builder::Builder;
    use tabled::settings::{Alignment, Style, object::Columns};

    let Value::Indexed {
        entries: row_entries,
        ..
    } = value
    else {
        return String::new();
    };

    let Some(first_row) = row_entries.values().next() else {
        return String::new();
    };
    let Value::Indexed {
        entries: col_entries,
        ..
    } = first_row
    else {
        return String::new();
    };
    let col_names: Vec<&str> = col_entries
        .keys()
        .map(graphcal_compiler::syntax::names::VariantName::as_str)
        .collect();

    let mut builder = Builder::default();

    // Header row: empty corner cell + column variant names
    let mut header_row = vec![String::new()];
    header_row.extend(col_names.iter().map(|s| (*s).to_string()));
    builder.push_record(header_row);

    // Data rows: row variant name + cell values
    for (row_variant, row_val) in row_entries {
        let mut row = vec![row_variant.as_str().to_string()];
        if let Value::Indexed { entries: cells, .. } = row_val {
            for col_name in &col_names {
                let cell_val = cells
                    .iter()
                    .find(|(k, _)| k.as_str() == *col_name)
                    .map(|(_, v)| v.format_display(None))
                    .unwrap_or_default();
                row.push(cell_val);
            }
        }
        builder.push_record(row);
    }

    let mut table = builder.build();
    table
        .with(Style::rounded())
        .modify(Columns::new(1..), Alignment::right());
    table.to_string()
}

/// Recursively peel outer index dimensions and render 2D table slices with
/// section headers.
///
/// `symbols` is threaded through for consistency with sibling formatters even
/// though 2D leaves ignore it; deeper calls forward it unchanged so that a
/// future "slice-scoped unit header" change stays local.
#[expect(
    clippy::only_used_in_recursion,
    reason = "symbols is threaded through for consistency with sibling formatters; \
              2D leaves ignore it but higher-depth calls forward it unchanged"
)]
pub fn format_table_slices(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
    depth: usize,
    parts: &mut Vec<String>,
) {
    let Value::Indexed {
        index_name,
        entries,
    } = value
    else {
        return;
    };

    if depth == 2 {
        let grid = format_table_grid(value);
        parts.push(grid);
        return;
    }

    // depth >= 3: emit section headers and recurse
    for (variant, inner_val) in entries {
        parts.push(format!("\n  [{index_name}::{variant}]"));
        format_table_slices(inner_val, symbols, depth - 1, parts);
    }
}

/// Render an N-dimensional indexed value (N >= 2) as a header + table(s).
///
/// - Depth 2: `name (unit):\n<grid>`.
/// - Depth >= 3: header + a list of `\n  [Outer::Variant]`-tagged 2D grids.
///
/// For dimensionless or non-scalar leaves, the `(unit)` part of the header is
/// omitted.
#[must_use]
pub fn format_indexed_table(
    name: &str,
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> String {
    let unit_label = extract_unit_label(value, symbols);
    let header = unit_label
        .as_ref()
        .map_or_else(|| format!("{name}:"), |label| format!("{name} ({label}):"));

    let depth = index_depth(value);
    if depth == 2 {
        let grid = format_table_grid(value);
        return format!("{header}\n{grid}");
    }

    // depth >= 3: peel off outermost index levels until we reach 2D slices
    let mut parts = vec![header];
    format_table_slices(value, symbols, depth, &mut parts);
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use super::*;
    use graphcal_compiler::syntax::dimension::Dimension;
    use graphcal_compiler::syntax::names::{FieldName, IndexName, StructTypeName, VariantName};
    use indexmap::IndexMap;

    fn scalar(si: f64) -> Value {
        Value::Scalar {
            si_value: si,
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    fn indexed_1d(name: &str, pairs: &[(&str, Value)]) -> Value {
        let mut entries = IndexMap::new();
        for (k, v) in pairs {
            entries.insert(VariantName::new(*k), v.clone());
        }
        Value::Indexed {
            index_name: IndexName::new(name),
            entries,
        }
    }

    #[test]
    fn index_depth_scalar_is_zero() {
        assert_eq!(index_depth(&scalar(1.0)), 0);
    }

    #[test]
    fn index_depth_1d_is_one() {
        let v = indexed_1d("I", &[("A", scalar(1.0)), ("B", scalar(2.0))]);
        assert_eq!(index_depth(&v), 1);
    }

    #[test]
    fn index_depth_2d_is_two() {
        let inner = indexed_1d("Col", &[("X", scalar(1.0)), ("Y", scalar(2.0))]);
        let outer = indexed_1d("Row", &[("R1", inner.clone()), ("R2", inner)]);
        assert_eq!(index_depth(&outer), 2);
    }

    #[test]
    fn flatten_scalar_produces_single_entry() {
        let v = scalar(42.0);
        let mut out = Vec::new();
        flatten_value("x", &v, &mut out);
        assert_eq!(out.len(), 1);
        match &out[0] {
            FlatEntry::Value(name, _) => assert_eq!(name, "x"),
            FlatEntry::Error(_, _) => panic!("expected Value entry"),
        }
    }

    #[test]
    fn flatten_1d_indexed_produces_bracketed_entries() {
        let v = indexed_1d("I", &[("A", scalar(1.0)), ("B", scalar(2.0))]);
        let mut out = Vec::new();
        flatten_value("dv", &v, &mut out);
        let names: Vec<&str> = out
            .iter()
            .map(|e| match e {
                FlatEntry::Value(n, _) | FlatEntry::Error(n, _) => n.as_str(),
            })
            .collect();
        assert_eq!(names, ["dv[A]", "dv[B]"]);
    }

    #[test]
    fn flatten_2d_indexed_fully_expands() {
        // The block-builder skips this case for tables, but flatten_value on
        // its own keeps peeling — verify that contract.
        let inner = indexed_1d("Col", &[("X", scalar(1.0)), ("Y", scalar(2.0))]);
        let outer = indexed_1d("Row", &[("R1", inner)]);
        let mut out = Vec::new();
        flatten_value("m", &outer, &mut out);
        let names: Vec<&str> = out
            .iter()
            .map(|e| match e {
                FlatEntry::Value(n, _) | FlatEntry::Error(n, _) => n.as_str(),
            })
            .collect();
        assert_eq!(names, ["m[R1][X]", "m[R1][Y]"]);
    }

    #[test]
    fn flatten_struct_expands_to_field_entries() {
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("x"), scalar(1.0));
        fields.insert(FieldName::new("y"), scalar(2.0));
        let s = Value::Struct {
            type_name: StructTypeName::new("Pair"),
            fields,
        };
        let mut out = Vec::new();
        flatten_value("p", &s, &mut out);
        let names: Vec<&str> = out
            .iter()
            .map(|e| match e {
                FlatEntry::Value(n, _) | FlatEntry::Error(n, _) => n.as_str(),
            })
            .collect();
        assert_eq!(names, ["p.x", "p.y"]);
    }

    #[test]
    fn flatten_empty_struct_keeps_single_entry() {
        let s = Value::Struct {
            type_name: StructTypeName::new("Unit"),
            fields: IndexMap::new(),
        };
        let mut out = Vec::new();
        flatten_value("u", &s, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn build_output_blocks_separates_tables() {
        // scalar -> flat; 2D -> table; scalar -> flat
        let a = Ok(scalar(1.0));
        let inner = indexed_1d("Col", &[("X", scalar(10.0))]);
        let b = Ok(indexed_1d("Row", &[("R1", inner)]));
        let c = Ok(scalar(3.0));
        let items: Vec<(&str, &Result<Value, NodeError>)> = vec![("a", &a), ("b", &b), ("c", &c)];
        let blocks = build_output_blocks(items);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], OutputBlock::Flat(_)));
        assert!(matches!(blocks[1], OutputBlock::Table("b", _)));
        assert!(matches!(blocks[2], OutputBlock::Flat(_)));
    }

    #[test]
    fn max_flat_name_len_ignores_tables() {
        let a = Ok(scalar(1.0));
        let inner = indexed_1d("Col", &[("X", scalar(10.0))]);
        let b = Ok(indexed_1d("Row", &[("R1", inner)]));
        let long = Ok(scalar(3.0));
        let items: Vec<(&str, &Result<Value, NodeError>)> = vec![
            ("a", &a),
            ("b_is_a_table_and_should_be_ignored", &b),
            ("cc", &long),
        ];
        let blocks = build_output_blocks(items);
        // Max flat name is "cc" (2) — the long name in the table is irrelevant.
        assert_eq!(max_flat_name_len(&blocks), 2);
    }

    #[test]
    fn format_table_grid_2d_has_header_and_rows() {
        let inner_r1 = indexed_1d("Col", &[("X", scalar(1.0)), ("Y", scalar(2.0))]);
        let inner_r2 = indexed_1d("Col", &[("X", scalar(3.0)), ("Y", scalar(4.0))]);
        let v = indexed_1d("Row", &[("R1", inner_r1), ("R2", inner_r2)]);
        let grid = format_table_grid(&v);
        assert!(grid.contains("R1"), "grid missing R1 row: {grid}");
        assert!(grid.contains("R2"), "grid missing R2 row: {grid}");
        assert!(grid.contains('X'), "grid missing X col: {grid}");
        assert!(grid.contains('Y'), "grid missing Y col: {grid}");
    }

    #[test]
    fn format_indexed_table_depth_2_has_name_header() {
        let inner = indexed_1d("Col", &[("X", scalar(1.0)), ("Y", scalar(2.0))]);
        let v = indexed_1d("Row", &[("R1", inner)]);
        let symbols = BTreeMap::new();
        let out = format_indexed_table("mymatrix", &v, &symbols);
        assert!(
            out.starts_with("mymatrix:"),
            "expected 'mymatrix:' header, got: {out}"
        );
    }

    #[test]
    fn format_indexed_table_depth_3_emits_slice_headers() {
        let leaf = indexed_1d("Col", &[("X", scalar(1.0)), ("Y", scalar(2.0))]);
        let mid = indexed_1d("Row", &[("R1", leaf)]);
        let outer = indexed_1d("Slab", &[("S1", mid.clone()), ("S2", mid)]);
        let symbols = BTreeMap::new();
        let out = format_indexed_table("cube", &outer, &symbols);
        assert!(out.contains("cube:"), "missing top header: {out}");
        assert!(
            out.contains("[Slab::S1]"),
            "missing slice header for S1: {out}"
        );
        assert!(
            out.contains("[Slab::S2]"),
            "missing slice header for S2: {out}"
        );
    }
}
