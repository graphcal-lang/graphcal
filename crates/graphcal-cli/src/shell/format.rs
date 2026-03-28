//! Value formatting utilities for the interactive shell.
//!
//! Provides per-value formatting for REPL output, extracted from the CLI's
//! `print_text` logic.

use std::collections::BTreeMap;

use graphcal_compiler::syntax::dimension::BaseDimId;
use graphcal_eval::eval::Value;

/// Format a single named value as a display string.
///
/// Returns a string like `"x: Dimensionless = 5"` or `"speed: Velocity = 7200 [km/hour]"`.
pub fn format_value_line(
    name: &str,
    value: &Value,
    symbols: &BTreeMap<BaseDimId, String>,
) -> String {
    match value {
        // Recursive cases: expand structs and indexed values into multiple lines.
        Value::Struct {
            type_name: _,
            fields,
        } if !fields.is_empty() => fields
            .iter()
            .map(|(field_name, field_val)| {
                format_value_line(
                    &format!("{name}.{}", field_name.as_str()),
                    field_val,
                    symbols,
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Indexed { entries, .. } => entries
            .iter()
            .map(|(variant, entry_val)| {
                format_value_line(&format!("{name}[{}]", variant.as_str()), entry_val, symbols)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        // Leaf cases: format as "  name = value".
        _ => format!("  {name} = {}", value.format_display(Some(symbols))),
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
    let new_str = new_value.format_display(Some(symbols));
    let old_str = old_value.format_display(Some(symbols));
    if new_str == old_str {
        None
    } else {
        let prefix = if is_primary { " " } else { "  -> " };
        Some(format!("{prefix}{name} = {new_str}  (was {old_str})"))
    }
}
