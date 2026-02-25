//! Value formatting utilities for the interactive shell.
//!
//! Provides per-value formatting for REPL output, extracted from the CLI's
//! `print_text` logic.

use std::collections::BTreeMap;

use graphcal_eval::eval::{Value, format_number};
use graphcal_syntax::dimension::BaseDimId;

/// Format a single named value as a display string.
///
/// Returns a string like `"x: Dimensionless = 5"` or `"speed: Velocity = 7200 [km/hour]"`.
pub fn format_value_line(
    name: &str,
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> String {
    match value {
        Value::Bool(b) => format!("  {name} = {b}"),
        Value::Int(i) => format!("  {name} = {i}"),
        Value::Label {
            index_name,
            variant,
        } => format!("  {name} = {index_name}::{variant}"),
        Value::Struct {
            variant,
            type_name,
            fields,
        } => {
            if fields.is_empty() || variant.as_str() != type_name.as_str() {
                format!("  {name} = {}", variant.as_str())
            } else {
                // Expand struct fields
                let mut lines = Vec::new();
                for (field_name, field_val) in fields {
                    let field_line = format_value_line(
                        &format!("{name}.{}", field_name.as_str()),
                        field_val,
                        symbols,
                    );
                    lines.push(field_line);
                }
                lines.join("\n")
            }
        }
        Value::Indexed { entries, .. } => {
            let mut lines = Vec::new();
            for (variant, entry_val) in entries {
                let entry_line =
                    format_value_line(&format!("{name}[{}]", variant.as_str()), entry_val, symbols);
                lines.push(entry_line);
            }
            lines.join("\n")
        }
        Value::Datetime { .. } => {
            let formatted = value.format_datetime().unwrap_or_default();
            format!("  {name} = {formatted}")
        }
        Value::Scalar { .. } => {
            let formatted = format_number(value.display_value().unwrap_or_default());
            value.display_label(symbols).map_or_else(
                || format!("  {name} = {formatted}"),
                |label| format!("  {name} = {formatted} [{label}]"),
            )
        }
    }
}

/// Format a value with an old value for comparison (propagation display).
///
/// Returns a string like `"  name = 5  (was 3)"`.
pub fn format_value_changed(
    name: &str,
    new_value: &Value,
    old_value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
    is_primary: bool,
) -> Option<String> {
    let new_str = format_value_short(new_value, symbols);
    let old_str = format_value_short(old_value, symbols);
    if new_str == old_str {
        return None;
    }
    let prefix = if is_primary { " " } else { "  -> " };
    Some(format!("{prefix}{name} = {new_str}  (was {old_str})"))
}

/// Format a value as a short string (just the value + unit, no name).
fn format_value_short(value: &Value, symbols: &BTreeMap<BaseDimId, String>) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Label {
            index_name,
            variant,
        } => format!("{index_name}::{variant}"),
        Value::Struct { variant, .. } => variant.as_str().to_string(),
        Value::Datetime { .. } => value.format_datetime().unwrap_or_default(),
        Value::Scalar { .. } => {
            let formatted = format_number(value.display_value().unwrap_or_default());
            if let Some(label) = value.display_label(symbols) {
                format!("{formatted} [{label}]")
            } else {
                formatted
            }
        }
        Value::Indexed { .. } => "[...]".to_string(),
    }
}
