//! Text-output formatting helpers for the `eval` subcommand.
//!
//! The CLI text format is the human-readable flavour of evaluation results. It
//! prints:
//!
//! * Real/complex quantity, bool, int, struct, and datetime values one per line, aligned on
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

use graphcal_compiler::dimension::{BaseDimId, Dimension};
use graphcal_eval::eval::{DisplayUnit, NodeError, Value};

/// One line of flat output: either a successfully-evaluated value or an error.
///
/// The first field is the fully-qualified display name (e.g. `foo`, `foo.x`,
/// `foo[Departure]`). The renderer uses the max of these widths to align `=`
/// across a whole `Flat` block.
pub enum FlatEntry<'a> {
    /// A displayable leaf/composite value, or a 1D indexed value flattened to
    /// a single entry.
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
/// Scalar values / structs return `0`. A 1D indexed value returns `1`. A 2D
/// indexed-of-indexed returns `2`, and so on. The table renderer switches
/// modes at depth >= 2.
#[must_use]
pub fn index_depth(value: &Value) -> usize {
    match value {
        Value::Indexed { entries, .. } => entries.values().next().map_or(1, |v| 1 + index_depth(v)),
        _ => 0,
    }
}

/// Effective presentation attached to one quantity cell.
///
/// The physical dimension and scale participate in equality, so two cells are
/// never treated as uniform merely because their rendered labels happen to
/// match. Default SI presentation is normalized to scale `1.0`, making an
/// explicit `m` and the default `m` semantically identical.
#[derive(Debug, Clone, PartialEq)]
struct QuantityPresentation {
    dimension: Dimension,
    label: Option<String>,
    scale: f64,
}

/// Fold state while classifying every leaf cell in an indexed table.
#[derive(Debug, Clone, PartialEq)]
enum TableLeafPresentation {
    Empty,
    NonQuantity,
    Quantity(QuantityPresentation),
    Heterogeneous,
}

impl TableLeafPresentation {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Heterogeneous, _) | (_, Self::Heterogeneous) => Self::Heterogeneous,
            (Self::Empty, presentation) | (presentation, Self::Empty) => presentation,
            (Self::NonQuantity, Self::NonQuantity) => Self::NonQuantity,
            (Self::Quantity(left), Self::Quantity(right)) if left == right => Self::Quantity(left),
            (Self::NonQuantity | Self::Quantity(_), Self::Quantity(_))
            | (Self::Quantity(_), Self::NonQuantity) => Self::Heterogeneous,
        }
    }
}

/// Unit-rendering policy derived from all table leaves, never from a sentinel cell.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TableUnitPolicy {
    /// There are no quantity cells, so no unit text is needed.
    NoQuantity,
    /// Every quantity cell has one semantic presentation but no available label.
    UniformUnlabelled,
    /// Every cell has the same semantic quantity presentation and shared label.
    UniformLabelled(String),
    /// Cells differ in kind, dimension, display scale, or display label.
    PerCell,
}

impl TableUnitPolicy {
    fn for_value(value: &Value, symbols: &BTreeMap<BaseDimId, String>) -> Self {
        match table_leaf_presentation(value, symbols) {
            TableLeafPresentation::Empty | TableLeafPresentation::NonQuantity => Self::NoQuantity,
            TableLeafPresentation::Quantity(QuantityPresentation {
                label: Some(label), ..
            }) => Self::UniformLabelled(label),
            TableLeafPresentation::Quantity(QuantityPresentation { label: None, .. }) => {
                Self::UniformUnlabelled
            }
            TableLeafPresentation::Heterogeneous => Self::PerCell,
        }
    }

    const fn cell_symbols<'a>(
        &self,
        symbols: &'a BTreeMap<BaseDimId, String>,
    ) -> Option<&'a BTreeMap<BaseDimId, String>> {
        match self {
            Self::PerCell => Some(symbols),
            Self::NoQuantity | Self::UniformUnlabelled | Self::UniformLabelled(_) => None,
        }
    }
}

fn table_leaf_presentation(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> TableLeafPresentation {
    match value {
        Value::Indexed { entries, .. } => entries
            .values()
            .map(|entry| table_leaf_presentation(entry, symbols))
            .fold(TableLeafPresentation::Empty, TableLeafPresentation::combine),
        Value::Quantity {
            dimension,
            display_unit,
            ..
        }
        | Value::Complex {
            dimension,
            display_unit,
            ..
        } => TableLeafPresentation::Quantity(QuantityPresentation {
            dimension: dimension.clone(),
            label: value.display_label(symbols),
            scale: display_unit.as_ref().map_or(1.0, DisplayUnit::scale),
        }),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Label { .. }
        | Value::Struct { .. }
        | Value::Datetime { .. } => TableLeafPresentation::NonQuantity,
    }
}

/// Flatten a value into one or more [`FlatEntry::Value`] entries keyed by a
/// dotted-or-indexed display name.
///
/// - Leaves (quantities, bools, ints, labels, datetimes) become a single entry.
/// - Structs expand to `name.field` lines (empty structs stay as a single
///   entry so that unit-struct variants still show up).
/// - 1D indexed values expand to `name[Variant]` lines; higher-dimensional
///   values are NOT flattened here — the caller routes them to a table block.
pub fn flatten_value<'a>(prefix: &str, value: &'a Value, entries: &mut Vec<FlatEntry<'a>>) {
    match value {
        Value::Quantity { .. }
        | Value::Complex { .. }
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
                    &format!("{prefix}[{}]", value.indexed_entry_display_name(variant)),
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
/// Columns come from the union of row variant keys and become the top header.
/// Row variants become the leftmost column. Uniform quantity units are omitted
/// from cells because the caller places them in the caption; heterogeneous
/// cells carry their own labels.
fn format_table_grid_with_policy(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
    policy: &TableUnitPolicy,
) -> String {
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
    if !matches!(first_row, Value::Indexed { .. }) {
        return String::new();
    }
    // Union the column keys across all rows (preserving first-seen order):
    // deriving columns from the first row alone silently hid cells of any
    // row with extra columns and rendered phantom blanks for missing ones.
    // Capture each header label from the row where the column key is first
    // observed so coordinate-index display labels are not resolved through an
    // unrelated first row that may not contain that key.
    let mut columns: Vec<_> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row_val in row_entries.values() {
        if let Value::Indexed { entries: cells, .. } = row_val {
            for variant in cells.keys() {
                if seen.insert(variant.clone()) {
                    columns.push((variant.clone(), row_val.indexed_entry_display_name(variant)));
                }
            }
        }
    }

    let mut builder = Builder::default();

    // Header row: empty corner cell + column variant names
    let mut header_row = vec![String::new()];
    header_row.extend(columns.iter().map(|(_, display)| display.clone()));
    builder.push_record(header_row);

    // Data rows: row variant name + cell values
    for (row_variant, row_val) in row_entries {
        let mut row = vec![value.indexed_entry_display_name(row_variant)];
        if let Value::Indexed { entries: cells, .. } = row_val {
            for (col_variant, _) in &columns {
                let cell_val = cells.get(col_variant).map_or_else(String::new, |value| {
                    value
                        .format_display(policy.cell_symbols(symbols))
                        .unwrap_or_else(|error| format!("ERROR: {error}"))
                });
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
/// The top-level unit policy is reused by every slice, so a heterogeneous 3D
/// value cannot accidentally acquire a caption from one locally uniform slice.
fn format_table_slices(
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
    policy: &TableUnitPolicy,
    depth: usize,
    parts: &mut Vec<String>,
) {
    let Value::Indexed {
        index_name,
        entries,
        ..
    } = value
    else {
        return;
    };

    if depth == 2 {
        let grid = format_table_grid_with_policy(value, symbols, policy);
        parts.push(grid);
        return;
    }

    // depth >= 3: emit section headers and recurse
    for (variant, inner_val) in entries {
        parts.push(format!(
            "\n  [{}.{}]",
            index_name.display_name(),
            value.indexed_entry_display_name(variant)
        ));
        format_table_slices(inner_val, symbols, policy, depth - 1, parts);
    }
}

/// Render an N-dimensional indexed value (N >= 2) as a header + table(s).
///
/// - Depth 2: `name (unit):\n<grid>`.
/// - Depth >= 3: header + a list of `\n  [Outer::Variant]`-tagged 2D grids.
///
/// For dimensionless or non-quantity leaves, the `(unit)` part of the header is
/// omitted. If leaves have different semantic presentations, the shared caption
/// is omitted and each quantity/complex cell carries its own unit label.
#[must_use]
pub fn format_indexed_table(
    name: &str,
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> String {
    let policy = TableUnitPolicy::for_value(value, symbols);
    let header = match &policy {
        TableUnitPolicy::UniformLabelled(label) => format!("{name} ({label}):"),
        TableUnitPolicy::NoQuantity
        | TableUnitPolicy::UniformUnlabelled
        | TableUnitPolicy::PerCell => format!("{name}:"),
    };

    let depth = index_depth(value);
    if depth == 2 {
        let grid = format_table_grid_with_policy(value, symbols, &policy);
        return format!("{header}\n{grid}");
    }

    // depth >= 3: peel off outermost index levels until we reach 2D slices
    let mut parts = vec![header];
    format_table_slices(value, symbols, &policy, depth, &mut parts);
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::complex_value::ComplexValue;
    use graphcal_compiler::registry::declared_type::IndexTypeRef;
    use graphcal_compiler::registry::prelude::prelude_base_dimension;
    use graphcal_compiler::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
    use graphcal_compiler::syntax::type_name::{FieldName, StructTypeName};
    use indexmap::IndexMap;

    fn quantity(si: f64) -> Value {
        Value::Quantity {
            si_value: si,
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    fn displayed_length(si: f64, label: &str, scale: f64) -> Value {
        Value::Quantity {
            si_value: si,
            dimension: prelude_base_dimension("Length").unwrap(),
            display_unit: Some(DisplayUnit::try_new(label, scale).unwrap()),
        }
    }

    fn displayed_complex_length(re: f64, im: f64, label: &str, scale: f64) -> Value {
        Value::Complex {
            si_value: ComplexValue::new(re, im),
            dimension: prelude_base_dimension("Length").unwrap(),
            display_unit: Some(DisplayUnit::try_new(label, scale).unwrap()),
        }
    }

    fn test_owner() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::root_in_package("test", "<cli-display-test>")
    }

    fn indexed_1d(name: &str, pairs: &[(&str, Value)]) -> Value {
        let mut entries = IndexMap::new();
        for (k, v) in pairs {
            entries.insert(
                IndexEntryKey::named(IndexVariantName::expect_valid(*k)),
                v.clone(),
            );
        }
        Value::indexed_with_owner(test_owner(), IndexName::expect_valid(name), entries)
    }

    fn indexed_1d_with_display(
        name: &str,
        pairs: &[(&str, Value)],
        displays: &[(&str, &str)],
    ) -> Value {
        let mut entries = IndexMap::new();
        for (k, v) in pairs {
            entries.insert(
                IndexEntryKey::named(IndexVariantName::expect_valid(*k)),
                v.clone(),
            );
        }
        let mut entry_display_names = IndexMap::new();
        for (k, display) in displays {
            entry_display_names.insert(
                IndexEntryKey::named(IndexVariantName::expect_valid(*k)),
                (*display).to_string(),
            );
        }
        Value::Indexed {
            index_name: IndexTypeRef::with_owner(test_owner(), IndexName::expect_valid(name)),
            entries,
            entry_display_names: Some(entry_display_names),
        }
    }

    #[test]
    fn index_depth_quantity_is_zero() {
        assert_eq!(index_depth(&quantity(1.0)), 0);
    }

    #[test]
    fn index_depth_1d_is_one() {
        let v = indexed_1d("I", &[("A", quantity(1.0)), ("B", quantity(2.0))]);
        assert_eq!(index_depth(&v), 1);
    }

    #[test]
    fn index_depth_2d_is_two() {
        let inner = indexed_1d("Col", &[("X", quantity(1.0)), ("Y", quantity(2.0))]);
        let outer = indexed_1d("Row", &[("R1", inner.clone()), ("R2", inner)]);
        assert_eq!(index_depth(&outer), 2);
    }

    #[test]
    fn flatten_quantity_produces_single_entry() {
        let v = quantity(42.0);
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
        let v = indexed_1d("I", &[("A", quantity(1.0)), ("B", quantity(2.0))]);
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
        let inner = indexed_1d("Col", &[("X", quantity(1.0)), ("Y", quantity(2.0))]);
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
        fields.insert(FieldName::expect_valid("x"), quantity(1.0));
        fields.insert(FieldName::expect_valid("y"), quantity(2.0));
        let s =
            Value::struct_with_owner(test_owner(), StructTypeName::expect_valid("Pair"), fields);
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
        let s = Value::struct_with_owner(
            test_owner(),
            StructTypeName::expect_valid("Unit"),
            IndexMap::new(),
        );
        let mut out = Vec::new();
        flatten_value("u", &s, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn build_output_blocks_separates_tables() {
        // quantity -> flat; 2D -> table; quantity -> flat
        let a = Ok(quantity(1.0));
        let inner = indexed_1d("Col", &[("X", quantity(10.0))]);
        let b = Ok(indexed_1d("Row", &[("R1", inner)]));
        let c = Ok(quantity(3.0));
        let items: Vec<(&str, &Result<Value, NodeError>)> = vec![("a", &a), ("b", &b), ("c", &c)];
        let blocks = build_output_blocks(items);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], OutputBlock::Flat(_)));
        assert!(matches!(blocks[1], OutputBlock::Table("b", _)));
        assert!(matches!(blocks[2], OutputBlock::Flat(_)));
    }

    #[test]
    fn max_flat_name_len_ignores_tables() {
        let a = Ok(quantity(1.0));
        let inner = indexed_1d("Col", &[("X", quantity(10.0))]);
        let b = Ok(indexed_1d("Row", &[("R1", inner)]));
        let long = Ok(quantity(3.0));
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
        let inner_r1 = indexed_1d("Col", &[("X", quantity(1.0)), ("Y", quantity(2.0))]);
        let inner_r2 = indexed_1d("Col", &[("X", quantity(3.0)), ("Y", quantity(4.0))]);
        let v = indexed_1d("Row", &[("R1", inner_r1), ("R2", inner_r2)]);
        let symbols = BTreeMap::new();
        let policy = TableUnitPolicy::for_value(&v, &symbols);
        let grid = format_table_grid_with_policy(&v, &symbols, &policy);
        assert!(grid.contains("R1"), "grid missing R1 row: {grid}");
        assert!(grid.contains("R2"), "grid missing R2 row: {grid}");
        assert!(grid.contains('X'), "grid missing X col: {grid}");
        assert!(grid.contains('Y'), "grid missing Y col: {grid}");
    }

    #[test]
    fn format_table_grid_column_headers_use_row_containing_column() {
        let inner_r1 = indexed_1d_with_display("Col", &[("X", quantity(1.0))], &[("X", "first-X")]);
        let inner_r2 =
            indexed_1d_with_display("Col", &[("Y", quantity(2.0))], &[("Y", "second-Y")]);
        let v = indexed_1d("Row", &[("R1", inner_r1), ("R2", inner_r2)]);
        let symbols = BTreeMap::new();
        let policy = TableUnitPolicy::for_value(&v, &symbols);
        let grid = format_table_grid_with_policy(&v, &symbols, &policy);
        assert!(grid.contains("first-X"), "grid missing X display: {grid}");
        assert!(grid.contains("second-Y"), "grid missing Y display: {grid}");
    }

    #[test]
    fn heterogeneous_table_units_are_labelled_per_cell() {
        let kilometre = indexed_1d("Col", &[("X", displayed_length(1000.0, "km", 1000.0))]);
        let metre = indexed_1d(
            "Col",
            &[("X", displayed_complex_length(2000.0, 3.0, "m", 1.0))],
        );
        let value = indexed_1d("Row", &[("A", kilometre), ("B", metre)]);
        let output = format_indexed_table("grid", &value, &BTreeMap::new());

        assert!(output.starts_with("grid:\n"), "{output}");
        assert!(!output.contains("grid (km):"), "{output}");
        assert!(output.contains("1 [km]"), "{output}");
        assert!(output.contains("2000 + 3i [m]"), "{output}");
    }

    #[test]
    fn uniform_table_units_use_one_shared_caption() {
        let first = indexed_1d("Col", &[("X", displayed_length(1000.0, "km", 1000.0))]);
        let second = indexed_1d("Col", &[("X", displayed_length(2000.0, "km", 1000.0))]);
        let value = indexed_1d("Row", &[("A", first), ("B", second)]);
        let output = format_indexed_table("grid", &value, &BTreeMap::new());

        assert!(output.starts_with("grid (km):\n"), "{output}");
        assert!(!output.contains("[km]"), "{output}");
    }

    #[test]
    fn heterogeneous_units_across_slices_remain_per_cell() {
        let km_cell = indexed_1d("Col", &[("X", displayed_length(1000.0, "km", 1000.0))]);
        let m_cell = indexed_1d("Col", &[("X", displayed_length(2000.0, "m", 1.0))]);
        let km_slice = indexed_1d("Row", &[("A", km_cell)]);
        let m_slice = indexed_1d("Row", &[("A", m_cell)]);
        let value = indexed_1d("Slab", &[("One", km_slice), ("Two", m_slice)]);
        let output = format_indexed_table("cube", &value, &BTreeMap::new());

        assert!(output.starts_with("cube:\n"), "{output}");
        assert!(output.contains("1 [km]"), "{output}");
        assert!(output.contains("2000 [m]"), "{output}");
    }

    #[test]
    fn format_indexed_table_depth_2_has_name_header() {
        let inner = indexed_1d("Col", &[("X", quantity(1.0)), ("Y", quantity(2.0))]);
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
        let leaf = indexed_1d("Col", &[("X", quantity(1.0)), ("Y", quantity(2.0))]);
        let mid = indexed_1d("Row", &[("R1", leaf)]);
        let outer = indexed_1d("Slab", &[("S1", mid.clone()), ("S2", mid)]);
        let symbols = BTreeMap::new();
        let out = format_indexed_table("cube", &outer, &symbols);
        assert!(out.contains("cube:"), "missing top header: {out}");
        assert!(
            out.contains("[Slab.S1]"),
            "missing slice header for S1: {out}"
        );
        assert!(
            out.contains("[Slab.S2]"),
            "missing slice header for S2: {out}"
        );
    }
}
