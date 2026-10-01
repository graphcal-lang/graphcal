//! Compact inline rendering of evaluated values for hover and inlay hints.

use std::collections::HashMap;

use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_eval::eval::{EvalResult, Value};

/// Format values and explicit incompleteness outcomes for hover and inlay hints.
pub fn format_eval_values(
    result: &EvalResult,
    cancellation: &CancellationToken,
) -> std::result::Result<HashMap<ScopedName, String>, Cancelled> {
    let mut map = HashMap::new();
    for (name, value_result, _decl_type) in &result.entries {
        cancellation.checkpoint()?;
        let formatted = match value_result {
            Ok(value) => format_value_inline(value, &result.render),
            Err(reason) if reason.is_incomplete() => reason.to_string(),
            Err(_) => continue,
        };
        map.insert(name.clone(), formatted);
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
pub fn format_value_inline_with_budget(
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
            let entries: Vec<(&str, &Value)> =
                fields.iter().map(|(k, v)| (k.as_str(), v)).collect();
            format_parenthesized_entries(constructor.as_str(), &entries, render, max_len)
        }
        Value::Indexed { entries, .. } => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            // For multi-indexed maps (nested Indexed values), flatten into
            // tuple-keyed form: `{ (A, X): 1, (A, Y): 2, (B, X): 3 }` instead
            // of nested braces: `{ A: { X: 1, Y: 2 }, B: { X: 3 } }`.
            let mut flat: Vec<(Vec<String>, &Value)> = Vec::new();
            flatten_indexed_entries(value, &mut Vec::new(), &mut flat);
            let is_multi = flat.first().is_some_and(|(keys, _)| keys.len() > 1);
            if is_multi {
                format_tuple_keyed_entries("", &flat, render, max_len)
            } else {
                let single: Vec<(String, &Value)> = entries
                    .iter()
                    .map(|(k, v)| (value.indexed_entry_display_name(k), v))
                    .collect();
                format_entries("", &single, Clone::clone, render, max_len)
            }
        }
    }
}

/// Format a list of key-value pairs as `{prefix}{ k1: v1, k2: v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
///
/// `render_key` shapes each entry's key — e.g., `|k| k.to_string()` for
/// single-axis variants or `|keys| format!("({})", keys.join(", "))` for
/// tuple-keyed multi-axis entries.
pub fn format_entries<K>(
    prefix: &str,
    entries: &[(K, &Value)],
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
pub struct EntryListLayout<'a> {
    pub prefix: &'a str,
    pub open: &'a str,
    pub close: &'a str,
    pub ellipsis: &'a str,
}

pub fn format_delimited_entries<K>(
    layout: EntryListLayout<'_>,
    entries: &[(K, &Value)],
    render_key: impl Fn(&K) -> String,
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    let mut result = format!("{}{}", layout.prefix, layout.open);
    let total = entries.len();

    for (i, (key, val)) in entries.iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + layout.close.len());
        let entry_str = format!(
            "{}: {}",
            render_key(key),
            format_value_inline_with_budget(val, render, remaining_budget)
        );

        let separator = if i + 1 < total { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + layout.close.len() > max_len {
            result.push_str(layout.ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        if i + 1 < total {
            result.push_str(", ");
        }
    }

    result.push_str(layout.close);
    result
}

pub fn format_parenthesized_entries(
    prefix: &str,
    entries: &[(&str, &Value)],
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

/// Recursively flatten nested `Indexed` values into a list of `(key_path, leaf_value)` pairs.
///
/// For a single-level `Indexed { A: 1, B: 2 }`, produces `[([A], 1), ([B], 2)]`.
/// For a nested `Indexed { A: Indexed { X: 1, Y: 2 }, B: Indexed { X: 3 } }`,
/// produces `[([A, X], 1), ([A, Y], 2), ([B, X], 3)]`.
pub fn flatten_indexed_entries<'a>(
    value: &'a Value,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'a Value)>,
) {
    let Value::Indexed { entries, .. } = value else {
        return;
    };
    for (key, val) in entries {
        prefix.push(value.indexed_entry_display_name(key));
        if matches!(val, Value::Indexed { .. }) {
            flatten_indexed_entries(val, prefix, out);
        } else {
            out.push((prefix.clone(), val));
        }
        prefix.pop();
    }
}

pub fn format_tuple_keyed_entries(
    prefix: &str,
    entries: &[(Vec<String>, &Value)],
    render: &graphcal_eval::eval::RenderContext,
    max_len: usize,
) -> String {
    format_entries(
        prefix,
        entries,
        |keys| format!("({})", keys.join(", ")),
        render,
        max_len,
    )
}
