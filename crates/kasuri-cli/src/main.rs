use clap::{Parser, Subcommand, ValueEnum};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process;

use kasuri_eval::eval::{EvalResult, compile_and_eval_named};

#[derive(Parser)]
#[command(name = "kasuri", version, about = "Kasuri language evaluator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Evaluate a .ksr file
    Eval {
        /// Path to the .ksr file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(ValueEnum, Clone)]
enum OutputFormat {
    Text,
    Json,
}

#[expect(clippy::print_stderr)] // CLI binary, stderr output is expected for errors
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
        Commands::Eval { file, format } => {
            let source = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: failed to read {}: {e}", file.display());
                    process::exit(1);
                }
            };

            let file_name = file.display().to_string();
            match compile_and_eval_named(&source, &file_name) {
                Ok(result) => match format {
                    OutputFormat::Text => print_text(&result),
                    OutputFormat::Json => print_json(&result),
                },
                Err(e) => {
                    eprintln!("{:?}", miette::Report::new(e));
                    process::exit(1);
                }
            }
        }
    }
}

#[expect(clippy::print_stdout)] // CLI binary, stdout output is expected
fn print_text(result: &EvalResult) {
    use kasuri_eval::eval::Value;

    // Flatten entries: scalars are one line, structs expand to `name.field` lines,
    // indexed values expand to `name[Variant]` lines
    fn flatten_value<'a>(prefix: &str, value: &'a Value, lines: &mut Vec<(String, &'a Value)>) {
        match value {
            Value::Scalar { .. } => {
                lines.push((prefix.to_string(), value));
            }
            Value::Struct { fields, .. } => {
                for (field_name, field_val) in fields {
                    flatten_value(&format!("{prefix}.{field_name}"), field_val, lines);
                }
            }
            Value::Indexed { entries, .. } => {
                for (variant, entry_val) in entries {
                    flatten_value(&format!("{prefix}[{variant}]"), entry_val, lines);
                }
            }
        }
    }

    let mut lines: Vec<(String, &Value)> = Vec::new();
    for (name, value, _) in &result.all {
        flatten_value(name, value, &mut lines);
    }

    let max_name_len = lines.iter().map(|(n, _)| n.len()).max().unwrap_or(0);

    for (name, value) in &lines {
        let formatted = format_number(value.display_value());
        let width = max_name_len;
        if let Some(label) = value.display_label() {
            println!("{name:width$} = {formatted} {label}");
        } else {
            println!("{name:width$} = {formatted}");
        }
    }
}

#[expect(clippy::unwrap_used)] // serde_json serialization cannot fail for these types
#[expect(clippy::print_stdout)] // CLI binary, stdout output is expected
fn print_json(result: &EvalResult) {
    use kasuri_eval::eval::Value;

    fn value_to_json(v: &Value) -> serde_json::Value {
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
                } else if let Some(si_unit) = v.display_label() {
                    map.insert("unit".to_string(), serde_json::json!(si_unit));
                } else {
                    // Dimensionless: no unit field
                }
                serde_json::Value::Object(map)
            }
            Value::Struct {
                type_name, fields, ..
            } => {
                let mut map = serde_json::Map::new();
                map.insert("type".to_string(), serde_json::json!(type_name));
                let fields_map: serde_json::Map<String, serde_json::Value> = fields
                    .iter()
                    .map(|(name, val)| (name.clone(), value_to_json(val)))
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
                map.insert("index".to_string(), serde_json::json!(index_name));
                let entries_map: serde_json::Map<String, serde_json::Value> = entries
                    .iter()
                    .map(|(name, val)| (name.clone(), value_to_json(val)))
                    .collect();
                map.insert(
                    "entries".to_string(),
                    serde_json::Value::Object(entries_map),
                );
                serde_json::Value::Object(map)
            }
        }
    }

    let mut output = serde_json::Map::new();

    let consts: BTreeMap<&str, serde_json::Value> = result
        .consts
        .iter()
        .map(|(n, v)| (n.as_str(), value_to_json(v)))
        .collect();
    let params: BTreeMap<&str, serde_json::Value> = result
        .params
        .iter()
        .map(|(n, v)| (n.as_str(), value_to_json(v)))
        .collect();
    let nodes: BTreeMap<&str, serde_json::Value> = result
        .nodes
        .iter()
        .map(|(n, v)| (n.as_str(), value_to_json(v)))
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
#[expect(clippy::cast_possible_truncation)] // Guarded by abs() < 1e15 check
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
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn format_integer() {
        assert_eq!(format_number(1200.0), "1200");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(-42.0), "-42");
    }

    #[test]
    #[expect(clippy::approx_constant)]
    fn format_decimal() {
        assert_eq!(format_number(9.80665), "9.80665");
        assert_eq!(format_number(3.14), "3.14");
    }

    #[test]
    fn format_large_decimal() {
        assert_eq!(format_number(3138.128), "3138.128");
    }
}
