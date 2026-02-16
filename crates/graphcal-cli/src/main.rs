use clap::{Parser, Subcommand, ValueEnum};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process;

use graphcal_eval::eval::{EvalResult, compile_and_eval_project};
use graphcal_syntax::names::DeclName;

#[derive(Parser)]
#[command(name = "graphcal", version, about = "Graphcal language evaluator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Evaluate a .gcl file
    Eval {
        /// Path to the .gcl file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
        /// Override a param value: --set 'name=expr'
        #[arg(long)]
        set: Vec<String>,
    },
    /// Start the Language Server Protocol (LSP) server
    Lsp,
}

#[derive(ValueEnum, Clone)]
enum OutputFormat {
    Text,
    Json,
}

#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
fn main() {
    // Install miette's fancy graphical error handler
    miette::set_hook(Box::new(|_| {
        Box::new(
            miette::MietteHandlerOpts::new()
                .terminal_links(true)
                .context_lines(2)
                .build(),
        )
    }))
    .ok();

    let cli = Cli::parse();
    match cli.command {
        Commands::Lsp => {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(graphcal_lsp::run());
        }
        Commands::Eval { file, format, set } => {
            // Parse --set overrides
            let mut overrides = std::collections::HashMap::new();
            for s in &set {
                let Some((name, value_str)) = s.split_once('=') else {
                    eprintln!("error: invalid --set format: {s:?} (expected 'name=expr')");
                    process::exit(1);
                };
                let name = name.trim();
                let value_str = value_str.trim();
                match graphcal_syntax::parser::Parser::new(value_str).parse_single_expr() {
                    Ok(expr) => {
                        overrides.insert(DeclName::new(name), expr);
                    }
                    Err(e) => {
                        eprintln!("error: failed to parse --set value for `{name}`: {e}");
                        process::exit(1);
                    }
                }
            }

            match compile_and_eval_project(&file, &overrides) {
                Ok(result) => {
                    match format {
                        OutputFormat::Text => print_text(&result),
                        OutputFormat::Json => print_json(&result),
                    }
                    if result.has_errors() {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("{:?}", miette::Report::new(e));
                    process::exit(1);
                }
            }
        }
    }
}

#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(clippy::print_stderr, reason = "CLI binary, stderr output for errors")]
fn print_text(result: &EvalResult) {
    use graphcal_eval::eval::{NodeError, Value};

    enum DisplayEntry<'a> {
        Value(String, &'a Value),
        Error(String, &'a NodeError),
    }

    // Flatten entries: scalars are one line, structs expand to `name.field` lines,
    // indexed values expand to `name[Variant]` lines
    fn flatten_value<'a>(prefix: &str, value: &'a Value, entries: &mut Vec<DisplayEntry<'a>>) {
        match value {
            Value::Scalar { .. } | Value::Bool(_) | Value::Int(_) => {
                entries.push(DisplayEntry::Value(prefix.to_string(), value));
            }
            Value::Struct {
                variant,
                type_name,
                fields,
            } => {
                if variant.as_str() == type_name.as_str() {
                    // Single-variant (struct sugar): show fields directly
                    for (field_name, field_val) in fields {
                        flatten_value(
                            &format!("{prefix}.{}", field_name.as_str()),
                            field_val,
                            entries,
                        );
                    }
                } else if fields.is_empty() {
                    // Bare variant: show as a label
                    entries.push(DisplayEntry::Value(prefix.to_string(), value));
                } else {
                    // Multi-variant with fields: show variant name as prefix
                    for (field_name, field_val) in fields {
                        flatten_value(
                            &format!("{prefix}::{}.{}", variant.as_str(), field_name.as_str()),
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

    let mut entries: Vec<DisplayEntry> = Vec::new();
    for (name, node_result, _) in &result.all {
        match node_result {
            Ok(value) => flatten_value(name.as_str(), value, &mut entries),
            Err(err) => {
                entries.push(DisplayEntry::Error(name.as_str().to_string(), err));
            }
        }
    }

    let max_name_len = entries
        .iter()
        .map(|e| match e {
            DisplayEntry::Value(n, _) | DisplayEntry::Error(n, _) => n.len(),
        })
        .max()
        .unwrap_or(0);

    for entry in &entries {
        let width = max_name_len;
        match entry {
            DisplayEntry::Error(name, err) => {
                eprintln!("{name:width$} = ERROR: {err}");
            }
            DisplayEntry::Value(name, value) => match value {
                Value::Bool(b) => println!("{name:width$} = {b}"),
                Value::Int(i) => println!("{name:width$} = {i}"),
                Value::Struct { variant, .. } => {
                    // Bare variant (no fields) — display the variant name
                    println!("{name:width$} = {}", variant.as_str());
                }
                _ => {
                    let formatted = format_number(value.display_value());
                    if let Some(label) = value.display_label(&result.base_dim_symbols) {
                        println!("{name:width$} = {formatted} {label}");
                    } else {
                        println!("{name:width$} = {formatted}");
                    }
                }
            },
        }
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "serde_json serialization cannot fail for these types"
)]
#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(
    clippy::too_many_lines,
    reason = "JSON output formatting is clearest as a single function"
)]
fn print_json(result: &EvalResult) {
    use graphcal_eval::eval::{NodeError, Value};

    fn value_to_json(
        v: &Value,
        symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    ) -> serde_json::Value {
        match v {
            Value::Scalar {
                si_value,
                display_unit,
                ..
            } => {
                let mut map = serde_json::Map::new();
                map.insert("si_value".to_string(), serde_json::json!(si_value));
                if let Some(du) = display_unit {
                    map.insert(
                        "display_value".to_string(),
                        serde_json::json!(v.display_value()),
                    );
                    map.insert("unit".to_string(), serde_json::json!(du.label));
                } else if let Some(si_unit) = v.display_label(symbols) {
                    map.insert("unit".to_string(), serde_json::json!(si_unit));
                } else {
                    // Dimensionless: no unit field
                }
                serde_json::Value::Object(map)
            }
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Int(i) => serde_json::Value::Number((*i).into()),
            Value::Struct {
                type_name,
                variant,
                fields,
            } => {
                let mut map = serde_json::Map::new();
                map.insert("type".to_string(), serde_json::json!(type_name.as_str()));
                // Include variant name only for multi-variant types (where variant != type name)
                if variant.as_str() != type_name.as_str() {
                    map.insert("variant".to_string(), serde_json::json!(variant.as_str()));
                }
                let fields_map: serde_json::Map<String, serde_json::Value> = fields
                    .iter()
                    .map(|(name, val)| (name.as_str().to_string(), value_to_json(val, symbols)))
                    .collect();
                map.insert("fields".to_string(), serde_json::Value::Object(fields_map));
                serde_json::Value::Object(map)
            }
            Value::Indexed {
                index_name,
                entries,
                ..
            } => {
                let mut map = serde_json::Map::new();
                map.insert("index".to_string(), serde_json::json!(index_name.as_str()));
                let entries_map: serde_json::Map<String, serde_json::Value> = entries
                    .iter()
                    .map(|(name, val)| (name.as_str().to_string(), value_to_json(val, symbols)))
                    .collect();
                map.insert(
                    "entries".to_string(),
                    serde_json::Value::Object(entries_map),
                );
                serde_json::Value::Object(map)
            }
        }
    }

    fn node_error_to_json(err: &NodeError) -> serde_json::Value {
        match err {
            NodeError::EvalFailed { message } => {
                serde_json::json!({
                    "error": {
                        "kind": "eval_failed",
                        "message": message,
                    }
                })
            }
            NodeError::DependencyFailed { failed_deps } => {
                let deps: Vec<&str> = failed_deps.iter().map(DeclName::as_str).collect();
                serde_json::json!({
                    "error": {
                        "kind": "dependency_failed",
                        "failed_deps": deps,
                    }
                })
            }
        }
    }

    fn result_to_json(
        r: &Result<Value, NodeError>,
        symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    ) -> serde_json::Value {
        match r {
            Ok(v) => value_to_json(v, symbols),
            Err(e) => node_error_to_json(e),
        }
    }

    let symbols = &result.base_dim_symbols;
    let mut output = serde_json::Map::new();

    let consts: BTreeMap<&str, serde_json::Value> = result
        .consts
        .iter()
        .map(|(n, v)| (n.as_str(), value_to_json(v, symbols)))
        .collect();
    let params: BTreeMap<&str, serde_json::Value> = result
        .params
        .iter()
        .map(|(n, r)| (n.as_str(), result_to_json(r, symbols)))
        .collect();
    let nodes: BTreeMap<&str, serde_json::Value> = result
        .nodes
        .iter()
        .map(|(n, r)| (n.as_str(), result_to_json(r, symbols)))
        .collect();

    output.insert("const".to_string(), serde_json::to_value(consts).unwrap());
    output.insert("param".to_string(), serde_json::to_value(params).unwrap());
    output.insert("node".to_string(), serde_json::to_value(nodes).unwrap());

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(output)).unwrap()
    );
}

/// Format a number for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
#[expect(
    clippy::cast_possible_truncation,
    reason = "guarded by abs() < 1e15 check"
)]
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        // Format with up to 6 decimal places, then strip trailing zeros
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;

    #[test]
    fn format_integer() {
        assert_eq!(format_number(1200.0), "1200");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(-42.0), "-42");
    }

    #[test]
    #[expect(
        clippy::approx_constant,
        reason = "testing exact format output of 3.14"
    )]
    fn format_decimal() {
        assert_eq!(format_number(9.80665), "9.80665");
        assert_eq!(format_number(3.14), "3.14");
    }

    #[test]
    fn format_large_decimal() {
        assert_eq!(format_number(3138.128), "3138.128");
    }
}
