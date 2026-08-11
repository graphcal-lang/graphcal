//! Pure alignment of evaluated plot encoding channels.
//!
//! Encoding channels evaluate independently to (possibly nested) indexed
//! runtime values, but rendering needs row-oriented records, so the channels
//! must be flattened onto one shared row set. The rules, in keeping with the
//! project's explicitness principle (#840, #841):
//!
//! - The row set is the cross product of the axes of the channel with the
//!   *widest* axis set (e.g. `color: for p: P, t: T { ... }` drives a `P × T`
//!   row per cell).
//! - Every other channel must range over a subset of those axes; its values
//!   are broadcast across the axes it does not mention (`x: for p: P { ... }`
//!   repeats per `t`). A channel with no axes (an unindexed value) broadcasts
//!   to every row.
//! - Channels over unrelated axes have no meaningful row pairing and are
//!   rejected — never zipped up to the longest channel with rows silently
//!   missing fields.
//! - A value that cannot be represented in a plot (a struct, or a mix of
//!   numbers and labels in one channel) is an error — variant names are
//!   never substituted for data.

use graphcal_compiler::plot_shape::align_plot_channel_axes;
use graphcal_compiler::registry::declared_type::IndexTypeRef;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::ast::EncodingChannel;
use graphcal_compiler::syntax::index_name::IndexEntryKey;

use super::types::{DisplayUnit, PlotFieldValue, Value, epoch_to_rfc3339, quantity_display_value};

/// One leaf datum of an encoding channel.
#[derive(Debug, Clone, PartialEq)]
enum PlotDatum {
    Number(f64),
    Label(String),
    Datetime(String),
}

/// One index axis of an encoding channel's data.
#[derive(Debug, Clone)]
struct PlotAxis {
    index: IndexTypeRef,
    entry_keys: Vec<IndexEntryKey>,
}

impl PlotAxis {
    /// Two axes are the same when they are the same index with the same
    /// entry-key sequence.
    fn matches(&self, other: &Self) -> bool {
        self.index.matches_ref(&other.index) && self.entry_keys == other.entry_keys
    }
}

/// An encoding channel's evaluated data with its index axes.
#[derive(Debug, Clone)]
pub(super) struct ChannelData {
    /// Axes from outermost to innermost comprehension variable.
    axes: Vec<PlotAxis>,
    /// Leaf values in row-major order over `axes` (one value when `axes`
    /// is empty).
    values: Vec<PlotDatum>,
}

impl ChannelData {
    /// A single string value with no axes (from a string-literal channel).
    pub(super) fn unindexed_label(label: String) -> Self {
        Self {
            axes: Vec::new(),
            values: vec![PlotDatum::Label(label)],
        }
    }

    /// Format this channel's axes for error messages: `P × T`, or
    /// `no index` for an unindexed channel.
    fn describe_axes(&self) -> String {
        if self.axes.is_empty() {
            return "no index".to_string();
        }
        self.axes
            .iter()
            .map(|axis| {
                axis.index.declared_resolved().map_or_else(
                    || axis.index.display_name().to_string(),
                    ToString::to_string,
                )
            })
            .collect::<Vec<_>>()
            .join(" × ")
    }
}

/// Convert one leaf runtime value to a plot datum.
///
/// Booleans become the labels `"true"`/`"false"`, matching how a quantity
/// `Bool` channel encodes (#840). Structs cannot be plotted.
fn plot_datum_from_leaf(
    rv: &RuntimeValue,
    display_unit: Option<&DisplayUnit>,
) -> Result<PlotDatum, String> {
    match rv {
        RuntimeValue::Quantity(v) => quantity_display_value(*v, display_unit)
            .map(PlotDatum::Number)
            .map_err(|error| error.to_string()),
        RuntimeValue::Complex(_) => Err(
            "Complex values cannot be plotted directly; use re(), im(), abs(), or phase()"
                .to_string(),
        ),
        RuntimeValue::Int(i) => crate::eval_expr::numeric::exact_i64_to_f64(*i)
            .map(PlotDatum::Number)
            .map_err(|_| {
                format!(
                    "Int value {i} cannot be plotted exactly; convert it explicitly with to_float()"
                )
            }),
        // A coordinate-index loop variable surfacing as a value
        // (e.g. `x: for t: T { t }`) is numeric data (#839).
        RuntimeValue::CoordinateLabel { value, .. } => Ok(PlotDatum::Number(*value)),
        RuntimeValue::Bool(b) => Ok(PlotDatum::Label(b.to_string())),
        RuntimeValue::Label { variant, .. } => Ok(PlotDatum::Label(variant.to_string())),
        RuntimeValue::Datetime(epoch) => epoch_to_rfc3339(epoch)
            .map(PlotDatum::Datetime)
            .map_err(|error| error.to_string()),
        RuntimeValue::Struct { .. } | RuntimeValue::Indexed { .. } => {
            Err(format!("{} cannot be plotted", rv.kind()))
        }
    }
}

/// Flatten a (possibly nested) runtime value into axes plus row-major leaf
/// values.
pub(super) fn channel_data_from_runtime(rv: &RuntimeValue) -> Result<ChannelData, String> {
    channel_data_from_runtime_with_display_unit(rv, None)
}

/// Flatten a runtime value while converting quantity leaves to a requested
/// rendering unit. Canonical runtime values remain unchanged.
pub(super) fn channel_data_from_runtime_with_display_unit(
    rv: &RuntimeValue,
    display_unit: Option<&DisplayUnit>,
) -> Result<ChannelData, String> {
    let RuntimeValue::Indexed {
        index_name,
        entries,
    } = rv
    else {
        return Ok(ChannelData {
            axes: Vec::new(),
            values: vec![plot_datum_from_leaf(rv, display_unit)?],
        });
    };

    let entry_keys: Vec<IndexEntryKey> = entries.keys().cloned().collect();
    let mut inner_axes: Option<Vec<PlotAxis>> = None;
    let mut values = Vec::new();
    for entry in entries.values() {
        let entry_data = channel_data_from_runtime_with_display_unit(entry, display_unit)?;
        match &inner_axes {
            None => inner_axes = Some(entry_data.axes),
            Some(expected) => {
                if expected.len() != entry_data.axes.len()
                    || !expected
                        .iter()
                        .zip(&entry_data.axes)
                        .all(|(a, b)| a.matches(b))
                {
                    return Err(format!(
                        "entries of `{}` have inconsistent index axes",
                        index_name.display_name()
                    ));
                }
            }
        }
        values.extend(entry_data.values);
    }

    let mut axes = vec![PlotAxis {
        index: index_name.clone(),
        entry_keys,
    }];
    axes.extend(inner_axes.into_iter().flatten());
    Ok(ChannelData { axes, values })
}

/// Flatten runtime data using per-leaf display metadata from its checked public projection.
pub(super) fn channel_data_from_presented_value(
    runtime: &RuntimeValue,
    presented: &Value,
) -> Result<ChannelData, String> {
    let RuntimeValue::Indexed {
        index_name,
        entries,
    } = runtime
    else {
        let display_unit = match (runtime, presented) {
            (RuntimeValue::Quantity(_), Value::Quantity { display_unit, .. })
            | (RuntimeValue::Complex(_), Value::Complex { display_unit, .. }) => {
                display_unit.as_ref()
            }
            _ => None,
        };
        return Ok(ChannelData {
            axes: Vec::new(),
            values: vec![plot_datum_from_leaf(runtime, display_unit)?],
        });
    };
    let Value::Indexed {
        index_name: presented_index,
        entries: presented_entries,
        ..
    } = presented
    else {
        return Err("checked plot presentation does not mirror indexed runtime data".to_string());
    };
    if !index_name.matches_ref(presented_index) {
        return Err(format!(
            "checked plot presentation index `{presented_index}` does not match runtime index `{index_name}`"
        ));
    }

    let entry_keys = entries.keys().cloned().collect::<Vec<_>>();
    let mut inner_axes: Option<Vec<PlotAxis>> = None;
    let mut values = Vec::new();
    for (key, entry) in entries {
        let presented_entry = presented_entries
            .get(key)
            .ok_or_else(|| format!("checked plot presentation is missing indexed entry `{key}`"))?;
        let entry_data = channel_data_from_presented_value(entry, presented_entry)?;
        match &inner_axes {
            None => inner_axes = Some(entry_data.axes),
            Some(expected)
                if expected.len() == entry_data.axes.len()
                    && expected
                        .iter()
                        .zip(&entry_data.axes)
                        .all(|(left, right)| left.matches(right)) => {}
            Some(_) => {
                return Err(format!(
                    "entries of `{}` have inconsistent index axes",
                    index_name.display_name()
                ));
            }
        }
        values.extend(entry_data.values);
    }
    let mut axes = vec![PlotAxis {
        index: index_name.clone(),
        entry_keys,
    }];
    axes.extend(inner_axes.into_iter().flatten());
    Ok(ChannelData { axes, values })
}

/// Return one unit label shared by every quantity leaf in a presented channel.
pub(super) fn uniform_quantity_unit_label(value: &Value) -> Result<Option<String>, String> {
    enum UniformUnit {
        NoQuantity,
        Quantity(Option<String>),
    }

    fn visit(value: &Value, found: &mut UniformUnit) -> Result<(), String> {
        match value {
            Value::Quantity { display_unit, .. } | Value::Complex { display_unit, .. } => {
                let label = display_unit.as_ref().map(|unit| unit.label.clone());
                match found {
                    UniformUnit::NoQuantity => *found = UniformUnit::Quantity(label),
                    UniformUnit::Quantity(expected) if *expected == label => {}
                    UniformUnit::Quantity(_) => {
                        return Err("plot channel quantity leaves do not share one display unit"
                            .to_string());
                    }
                }
            }
            Value::Indexed { entries, .. } => {
                entries.values().try_for_each(|entry| visit(entry, found))?;
            }
            Value::Struct { fields, .. } => {
                fields.values().try_for_each(|field| visit(field, found))?;
            }
            Value::Bool(_) | Value::Int(_) | Value::Label { .. } | Value::Datetime { .. } => {}
        }
        Ok(())
    }

    let mut found = UniformUnit::NoQuantity;
    visit(value, &mut found)?;
    match found {
        UniformUnit::NoQuantity | UniformUnit::Quantity(None) => Ok(None),
        UniformUnit::Quantity(Some(label)) => Ok(Some(label)),
    }
}

/// Assemble a channel's projected data into a `PlotFieldValue`, rejecting
/// mixed value kinds within one channel.
fn plot_field_value_from_data(values: &[PlotDatum]) -> Result<PlotFieldValue, String> {
    let mut numbers = Vec::new();
    let mut labels = Vec::new();
    let mut datetimes = Vec::new();
    for datum in values {
        match datum {
            PlotDatum::Number(n) => numbers.push(*n),
            PlotDatum::Label(s) => labels.push(s.clone()),
            PlotDatum::Datetime(s) => datetimes.push(s.clone()),
        }
    }
    match (numbers.is_empty(), labels.is_empty(), datetimes.is_empty()) {
        // An empty channel (from an empty index) carries no values; the
        // numeric variant is an arbitrary but harmless representation.
        (_, true, true) => Ok(PlotFieldValue::Numbers(numbers)),
        (true, false, true) => Ok(PlotFieldValue::Labels(labels)),
        (true, true, false) => Ok(PlotFieldValue::Datetimes(datetimes)),
        _ => Err("mixed value kinds (numbers, labels, or datetimes) in one channel".to_string()),
    }
}

/// Flatten a runtime value into a `PlotFieldValue` without cross-channel
/// alignment, for property contexts (mark/plot/composition properties).
pub(super) fn flatten_to_field_value(rv: &RuntimeValue) -> Result<PlotFieldValue, String> {
    let data = channel_data_from_runtime(rv)?;
    plot_field_value_from_data(&data.values)
}

/// Align evaluated encoding channels onto one shared row set.
///
/// Returns the channels with their values expanded to one value per row, in
/// row-major order over the row axes. See the module docs for the rules.
pub(super) fn align_encoding_channels(
    channels: &[(EncodingChannel, ChannelData)],
) -> Result<Vec<(EncodingChannel, PlotFieldValue)>, String> {
    let channel_axis_refs = channels
        .iter()
        .map(|(_, data)| {
            data.axes
                .iter()
                .map(|axis| axis.index.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let channel_axis_slices = channel_axis_refs
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    let alignment = align_plot_channel_axes(&channel_axis_slices)
        .map_err(|_| incompatible_axes_message(channels))?;
    let Some(row_channel) = alignment.row_channel() else {
        return Ok(Vec::new());
    };
    let row_axes = &channels[row_channel].1.axes;

    // Static alignment matches canonical identities. Retain entry-key equality
    // as malformed-runtime-value defense in depth.
    let mapped = channels
        .iter()
        .zip(alignment.channel_positions())
        .map(|((channel, data), positions)| {
            let keys_match = data
                .axes
                .iter()
                .zip(positions)
                .all(|(axis, position)| axis.entry_keys == row_axes[*position].entry_keys);
            if keys_match {
                Ok((*channel, data, positions))
            } else {
                Err(incompatible_axes_message(channels))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Cross product of the row axes, row-major (last axis fastest).
    let row_count = row_axes.iter().try_fold(1usize, |count, axis| {
        count
            .checked_mul(axis.entry_keys.len())
            .ok_or_else(|| "plot row count overflowed usize".to_string())
    })?;
    let mut result = Vec::with_capacity(mapped.len());
    for (channel, data, positions) in mapped {
        let mut values = Vec::with_capacity(row_count);
        for row in 0..row_count {
            // Decompose the row number into per-axis digits.
            let mut digits = vec![0usize; row_axes.len()];
            let mut rest = row;
            for (i, axis) in row_axes.iter().enumerate().rev() {
                let axis_len = axis.entry_keys.len();
                digits[i] = rest
                    .checked_rem(axis_len)
                    .ok_or_else(|| "plot axis must contain at least one variant".to_string())?;
                rest = rest
                    .checked_div(axis_len)
                    .ok_or_else(|| "plot axis must contain at least one variant".to_string())?;
            }
            // Project the row onto this channel's own axes.
            let mut idx = 0usize;
            for (axis, pos) in data.axes.iter().zip(positions) {
                idx = idx
                    .checked_mul(axis.entry_keys.len())
                    .and_then(|base| base.checked_add(digits[*pos]))
                    .ok_or_else(|| "plot row index overflowed usize".to_string())?;
            }
            values.push(
                data.values
                    .get(idx)
                    .ok_or_else(|| "plot channel shape does not match its values".to_string())?
                    .clone(),
            );
        }
        let field_value = plot_field_value_from_data(&values)
            .map_err(|e| format!("encoding channel `{channel}`: {e}"))?;
        result.push((channel, field_value));
    }
    Ok(result)
}

fn incompatible_axes_message(channels: &[(EncodingChannel, ChannelData)]) -> String {
    let described = channels
        .iter()
        .map(|(channel, data)| format!("`{channel}` ranges over {}", data.describe_axes()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "encoding channels range over incompatible index axes: {described}; every channel must \
         range over (a subset of) one channel's axes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::complex_value::ComplexValue;
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName};
    use indexmap::IndexMap;

    fn indexed(index: &str, entries: Vec<(&str, RuntimeValue)>) -> RuntimeValue {
        RuntimeValue::indexed_with_owner(
            DagId::root_in_package("test", "main"),
            IndexName::expect_valid(index),
            entries
                .into_iter()
                .map(|(k, v)| (IndexEntryKey::named(IndexVariantName::expect_valid(k)), v))
                .collect::<IndexMap<_, _>>(),
        )
    }

    fn numbers(field: &PlotFieldValue) -> Vec<f64> {
        match field {
            PlotFieldValue::Numbers(ns) => ns.clone(),
            other => panic!("expected Numbers, got {other:?}"),
        }
    }

    #[test]
    fn rejects_complex_plot_data_with_projection_guidance() {
        let error = channel_data_from_runtime(&RuntimeValue::Complex(ComplexValue::new(1.0, 2.0)))
            .unwrap_err();
        assert!(error.contains("use re(), im(), abs(), or phase()"));
    }

    #[test]
    fn display_unit_scales_scalar_and_indexed_quantity_leaves() {
        let kilometres = DisplayUnit::try_new("km", 1000.0).unwrap();
        let scalar = channel_data_from_runtime_with_display_unit(
            &RuntimeValue::Quantity(3000.0),
            Some(&kilometres),
        )
        .unwrap();
        let indexed = channel_data_from_runtime_with_display_unit(
            &indexed(
                "Sample",
                vec![
                    ("A", RuntimeValue::Quantity(1000.0)),
                    ("B", RuntimeValue::Quantity(2000.0)),
                ],
            ),
            Some(&kilometres),
        )
        .unwrap();

        assert_eq!(scalar.values, vec![PlotDatum::Number(3.0)]);
        assert_eq!(
            indexed.values,
            vec![PlotDatum::Number(1.0), PlotDatum::Number(2.0)]
        );
    }

    #[test]
    fn broadcasts_1d_channels_over_2d_cross_product() {
        // The documented heat-map pattern: x over P, y over T, color over P×T.
        let x = channel_data_from_runtime(&indexed(
            "P",
            vec![
                ("P1", RuntimeValue::Quantity(1.0)),
                ("P2", RuntimeValue::Quantity(2.0)),
                ("P3", RuntimeValue::Quantity(3.0)),
            ],
        ))
        .unwrap();
        let y = channel_data_from_runtime(&indexed(
            "T",
            vec![
                ("T1", RuntimeValue::Quantity(10.0)),
                ("T2", RuntimeValue::Quantity(20.0)),
            ],
        ))
        .unwrap();
        let color = channel_data_from_runtime(&indexed(
            "P",
            vec![
                (
                    "P1",
                    indexed(
                        "T",
                        vec![
                            ("T1", RuntimeValue::Quantity(0.1)),
                            ("T2", RuntimeValue::Quantity(0.2)),
                        ],
                    ),
                ),
                (
                    "P2",
                    indexed(
                        "T",
                        vec![
                            ("T1", RuntimeValue::Quantity(0.3)),
                            ("T2", RuntimeValue::Quantity(0.4)),
                        ],
                    ),
                ),
                (
                    "P3",
                    indexed(
                        "T",
                        vec![
                            ("T1", RuntimeValue::Quantity(0.5)),
                            ("T2", RuntimeValue::Quantity(0.6)),
                        ],
                    ),
                ),
            ],
        ))
        .unwrap();

        let aligned = align_encoding_channels(&[
            (EncodingChannel::X, x),
            (EncodingChannel::Y, y),
            (EncodingChannel::Color, color),
        ])
        .unwrap();

        assert_eq!(
            numbers(&aligned[0].1),
            vec![1.0, 1.0, 2.0, 2.0, 3.0, 3.0],
            "x broadcasts over T"
        );
        assert_eq!(
            numbers(&aligned[1].1),
            vec![10.0, 20.0, 10.0, 20.0, 10.0, 20.0],
            "y broadcasts over P"
        );
        assert_eq!(
            numbers(&aligned[2].1),
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
            "color keeps all six cells"
        );
    }

    #[test]
    fn rejects_channels_over_unrelated_axes() {
        let x = channel_data_from_runtime(&indexed(
            "Step",
            vec![
                ("A", RuntimeValue::Quantity(1.0)),
                ("B", RuntimeValue::Quantity(2.0)),
            ],
        ))
        .unwrap();
        let y = channel_data_from_runtime(&indexed(
            "Pair",
            vec![
                ("L", RuntimeValue::Quantity(10.0)),
                ("R", RuntimeValue::Quantity(20.0)),
            ],
        ))
        .unwrap();

        let err = align_encoding_channels(&[(EncodingChannel::X, x), (EncodingChannel::Y, y)])
            .unwrap_err();
        assert!(
            err.contains("incompatible index axes"),
            "unexpected message: {err}"
        );
        assert!(err.contains("Step") && err.contains("Pair"));
    }

    #[test]
    fn unindexed_channel_broadcasts_to_all_rows() {
        let x = channel_data_from_runtime(&indexed(
            "Step",
            vec![
                ("A", RuntimeValue::Quantity(1.0)),
                ("B", RuntimeValue::Quantity(2.0)),
            ],
        ))
        .unwrap();
        let y = channel_data_from_runtime(&RuntimeValue::Quantity(7.0)).unwrap();

        let aligned =
            align_encoding_channels(&[(EncodingChannel::X, x), (EncodingChannel::Y, y)]).unwrap();
        assert_eq!(numbers(&aligned[1].1), vec![7.0, 7.0]);
    }

    #[test]
    fn indexed_bools_become_labels_like_unindexed_bools() {
        let flags = channel_data_from_runtime(&indexed(
            "Step",
            vec![
                ("A", RuntimeValue::Bool(false)),
                ("B", RuntimeValue::Bool(true)),
            ],
        ))
        .unwrap();
        let aligned = align_encoding_channels(&[(EncodingChannel::Y, flags)]).unwrap();
        match &aligned[0].1 {
            PlotFieldValue::Labels(labels) => assert_eq!(labels, &["false", "true"]),
            other => panic!("expected Labels, got {other:?}"),
        }
    }

    #[test]
    fn rejects_struct_values() {
        let err = channel_data_from_runtime(&RuntimeValue::struct_with_owner(
            DagId::root_in_package("test", "main"),
            graphcal_compiler::syntax::type_name::StructTypeName::expect_valid("Vec2"),
            IndexMap::new(),
        ))
        .unwrap_err();
        assert!(err.contains("cannot be plotted"), "unexpected: {err}");
    }

    #[test]
    fn rejects_mixed_value_kinds_in_one_channel() {
        let mixed = channel_data_from_runtime(&indexed(
            "Step",
            vec![
                ("A", RuntimeValue::Quantity(1.0)),
                ("B", RuntimeValue::Bool(true)),
            ],
        ))
        .unwrap();
        let err = align_encoding_channels(&[(EncodingChannel::X, mixed)]).unwrap_err();
        assert!(err.contains("mixed value kinds"), "unexpected: {err}");
    }

    #[test]
    fn duplicate_axes_map_to_distinct_row_positions() {
        // `for a: P, b: P { ... }` ranges over P × P; a 1D channel over P
        // maps onto the first (outer) occurrence.
        let grid = channel_data_from_runtime(&indexed(
            "P",
            vec![
                (
                    "P1",
                    indexed(
                        "P",
                        vec![
                            ("P1", RuntimeValue::Quantity(11.0)),
                            ("P2", RuntimeValue::Quantity(12.0)),
                        ],
                    ),
                ),
                (
                    "P2",
                    indexed(
                        "P",
                        vec![
                            ("P1", RuntimeValue::Quantity(21.0)),
                            ("P2", RuntimeValue::Quantity(22.0)),
                        ],
                    ),
                ),
            ],
        ))
        .unwrap();
        let line = channel_data_from_runtime(&indexed(
            "P",
            vec![
                ("P1", RuntimeValue::Quantity(1.0)),
                ("P2", RuntimeValue::Quantity(2.0)),
            ],
        ))
        .unwrap();

        let aligned =
            align_encoding_channels(&[(EncodingChannel::Color, grid), (EncodingChannel::X, line)])
                .unwrap();
        assert_eq!(numbers(&aligned[0].1), vec![11.0, 12.0, 21.0, 22.0]);
        assert_eq!(numbers(&aligned[1].1), vec![1.0, 1.0, 2.0, 2.0]);
    }
}
