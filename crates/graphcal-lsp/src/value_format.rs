//! Compact inline rendering of evaluated values for hover and inlay hints.

use std::collections::{HashMap, HashSet};

use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_eval::eval::{EvalResult, Value};

/// Format values and explicit incompleteness outcomes for inlay hints, keyed
/// by the runtime identity each entry reports.
///
/// Only identities in `shown` are formatted: an evaluation also reports, for
/// example, include instances that no inlay hint ever displays.
///
/// Keying by identity rather than by output name keeps an include instance's
/// values apart from the inline `dag` declarations it instantiates, even when
/// the instance's display name spells the DAG's own name.
pub fn format_eval_values(
    result: &EvalResult,
    shown: &HashSet<&ResolvedDeclName>,
    cancellation: &CancellationToken,
) -> std::result::Result<HashMap<ResolvedDeclName, String>, Cancelled> {
    let mut map = HashMap::new();
    for entry in &result.entries {
        cancellation.checkpoint()?;
        if !shown.contains(&entry.identity) {
            continue;
        }
        let formatted = match &entry.outcome {
            Ok(value) => format_value_inline(value, &result.render),
            Err(reason) if reason.is_incomplete() => reason.to_string(),
            Err(_) => continue,
        };
        map.insert(entry.identity.clone(), formatted);
    }
    Ok(map)
}

/// Maximum character length for inlay hint display strings.
/// When the formatted value exceeds this, entries are truncated with `...`.
pub const INLAY_HINT_MAX_LEN: usize = 80;

/// Format a single `Value` as a compact inline string for inlay hints.
///
/// - Quantity: `"9.81 [m/s^2]"` or `"3.14159"` (dimensionless)
/// - Bool: `"true"` / `"false"`
/// - Int: `"42"`
/// - Constructor value: `"LowThrust(thrust: 0.5 [N], duration: 3600 [s])"`
/// - Unit constructor value: `"Nominal"`
/// - Indexed: `"{ Departure: 4.92 [km/s], Correction: 0.24 [km/s], ... }"`
pub fn format_value_inline(value: &Value, render: &graphcal_eval::eval::RenderContext) -> String {
    format_value_inline_with_budget(value, render, INLAY_HINT_MAX_LEN)
}

/// Format a `Value` with a character budget. When the formatted entries would
/// exceed `max_len`, remaining entries are replaced with `...`.
fn format_value_inline_with_budget(
    value: &Value,
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    match value {
        // Leaf types: delegate to the shared `format_display` on `Value`.
        Value::Quantity { .. }
        | Value::Complex { .. }
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Key(_)
        | Value::Datetime { .. } => value
            .format_display(render, graphcal_eval::eval::UnitLabel::Inline)
            .unwrap_or_else(|error| format!("ERROR: {error}")),
        Value::Struct {
            constructor,
            fields,
            ..
        } => {
            if fields.is_empty() {
                return constructor.as_str().to_string();
            }
            format_parenthesized_entries(
                constructor.as_str(),
                fields.iter().map(|(k, v)| (k.as_str(), v)),
                render,
                max_len,
            )
        }
        Value::Indexed { entries, .. } => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            // For multi-indexed maps (nested Indexed values), flatten into
            // tuple-keyed form: `{ (A, X): 1, (A, Y): 2, (B, X): 3 }` instead
            // of nested braces: `{ A: { X: 1, Y: 2 }, B: { X: 3 } }`.
            //
            // Entries are produced lazily and formatting stops once the
            // budget is spent, so a huge grid costs O(budget), not O(entries).
            let mut leaves = IndexedLeaves::new(value).peekable();
            let is_multi = leaves.peek().is_some_and(|(keys, _)| keys.len() > 1);
            if is_multi {
                format_entries(
                    "",
                    leaves,
                    |keys: &Vec<String>| format!("({})", keys.join(", ")),
                    render,
                    max_len,
                )
            } else {
                format_entries(
                    "",
                    entries
                        .iter()
                        .map(|(k, v)| (value.indexed_entry_display_name(k), v)),
                    Clone::clone,
                    render,
                    max_len,
                )
            }
        }
    }
}

/// Format key-value pairs as `{prefix}{ k1: v1, k2: v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
///
/// `render_key` shapes each entry's key — e.g., the display name for
/// single-axis variants or `(k1, k2)` for tuple-keyed multi-axis entries.
fn format_entries<'v, K>(
    prefix: &str,
    entries: impl Iterator<Item = (K, &'v Value)>,
    render_key: impl Fn(&K) -> String,
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    format_delimited_entries(
        EntryListLayout {
            prefix,
            open: "{ ",
            close: " }",
            ellipsis: "... }",
        },
        entries,
        render_key,
        render,
        max_len,
    )
}

#[derive(Clone, Copy)]
struct EntryListLayout<'a> {
    prefix: &'a str,
    open: &'a str,
    close: &'a str,
    ellipsis: &'a str,
}

/// Consumes `entries` only until the budget is spent.
fn format_delimited_entries<'v, K>(
    layout: EntryListLayout<'_>,
    entries: impl Iterator<Item = (K, &'v Value)>,
    render_key: impl Fn(&K) -> String,
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    let mut result = format!("{}{}", layout.prefix, layout.open);
    let mut entries = entries.enumerate().peekable();

    while let Some((i, (key, val))) = entries.next() {
        let remaining_budget = max_len.saturating_sub(result.len() + layout.close.len());
        let entry_str = format!(
            "{}: {}",
            render_key(&key),
            format_value_inline_with_budget(val, render, remaining_budget)
        );

        let has_more = entries.peek().is_some();
        let separator = if has_more { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + layout.close.len() > max_len {
            result.push_str(layout.ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        result.push_str(separator);
    }

    result.push_str(layout.close);
    result
}

fn format_parenthesized_entries<'v>(
    prefix: &str,
    entries: impl Iterator<Item = (&'v str, &'v Value)>,
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    format_delimited_entries(
        EntryListLayout {
            prefix,
            open: "(",
            close: ")",
            ellipsis: "...)",
        },
        entries,
        |k| (*k).to_string(),
        render,
        max_len,
    )
}

/// The leaves of nested `Indexed` values with their key paths, in order,
/// produced lazily.
///
/// For a single-level `Indexed { A: 1, B: 2 }`, yields `([A], 1), ([B], 2)`.
/// For a nested `Indexed { A: Indexed { X: 1, Y: 2 }, B: Indexed { X: 3 } }`,
/// yields `([A, X], 1), ([A, Y], 2), ([B, X], 3)`.
struct IndexedLeaves<'a> {
    /// Each open `Indexed` value with its remaining entries.
    stack: Vec<(&'a Value, indexmap::map::Iter<'a, IndexEntryKey, Value>)>,
    /// Display names of the entries leading to the innermost open value.
    path: Vec<String>,
}

impl<'a> IndexedLeaves<'a> {
    fn new(value: &'a Value) -> Self {
        let stack = match value {
            Value::Indexed { entries, .. } => vec![(value, entries.iter())],
            _ => Vec::new(),
        };
        Self {
            stack,
            path: Vec::new(),
        }
    }
}

impl<'a> Iterator for IndexedLeaves<'a> {
    type Item = (Vec<String>, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (parent, entries) = self.stack.last_mut()?;
            let Some((key, child)) = entries.next() else {
                self.stack.pop();
                self.path.pop();
                continue;
            };
            let name = parent.indexed_entry_display_name(key);
            if let Value::Indexed { entries, .. } = child {
                self.path.push(name);
                self.stack.push((child, entries.iter()));
            } else {
                let mut keys = self.path.clone();
                keys.push(name);
                return Some((keys, child));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use graphcal_compiler::dimension::Dimension;
    use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName};
    use indexmap::IndexMap;

    use super::*;

    fn render() -> graphcal_eval::eval::RenderContext {
        graphcal_eval::eval::RenderContext::new(
            BTreeMap::new(),
            graphcal_compiler::semantic::time_zone::TimeZoneRegistry::bundled(),
        )
    }

    fn one() -> Value {
        Value::Quantity {
            si_value: graphcal_compiler::finite_value::FiniteQuantity::try_new(1.0).unwrap(),
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    fn indexed(index: &str, size: usize, value: impl Fn() -> Value) -> Value {
        let entries: IndexMap<_, _> = (0..size)
            .map(|position| {
                (
                    IndexEntryKey::named(IndexVariantName::expect_valid(format!(
                        "{index}{position}"
                    ))),
                    value(),
                )
            })
            .collect();
        Value::indexed_with_owner(
            graphcal_compiler::dag_id::DagId::root_in_package("test", "<value-format-test>"),
            IndexName::expect_valid(index),
            entries,
        )
    }

    /// LSP-10 (#2253): formatting stops pulling entries once the display
    /// budget is spent, so even an endless entry list terminates.
    #[test]
    fn entry_formatting_consumes_entries_only_within_the_budget() {
        let value = one();
        let consumed = Cell::new(0_usize);
        let formatted = format_entries(
            "",
            std::iter::repeat(("Key".to_string(), &value)).inspect(|_| {
                consumed.set(consumed.get() + 1);
            }),
            Clone::clone,
            &render(),
            INLAY_HINT_MAX_LEN,
        );
        assert!(formatted.ends_with("... }"), "{formatted}");
        assert!(consumed.get() <= INLAY_HINT_MAX_LEN, "{}", consumed.get());
    }

    #[test]
    fn nested_indexed_values_flatten_lazily_in_order() {
        let grid = indexed("Row", 2, || indexed("Col", 2, one));
        let leaves: Vec<_> = IndexedLeaves::new(&grid)
            .map(|(keys, _)| keys.join(","))
            .collect();
        assert_eq!(leaves, ["Row0,Col0", "Row0,Col1", "Row1,Col0", "Row1,Col1"]);
    }

    #[test]
    fn large_grids_format_as_a_truncated_tuple_keyed_prefix() {
        let grid = indexed("Row", 300, || indexed("Col", 100, one));
        let formatted = format_value_inline(&grid, &render());
        assert!(
            formatted.starts_with("{ (Row0, Col0): 1, (Row0, Col1): 1, "),
            "{formatted}"
        );
        assert!(formatted.ends_with("... }"), "{formatted}");
    }
}
