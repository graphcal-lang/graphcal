mod json_input;

use clap::{Parser, Subcommand, ValueEnum};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process;

use graphcal_eval::eval::{
    EvalResult, compile_and_eval_project, compile_to_tir_project, format_number,
};
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
        /// JSON input file for param values
        #[arg(long)]
        input: Option<PathBuf>,
        /// Skip assertion checking
        #[arg(long)]
        no_assert: bool,
        /// Project root directory (overrides automatic graphcal.toml detection)
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Format .gcl files
    Format {
        /// Files or directories to format (default: current directory)
        paths: Vec<PathBuf>,
        /// Check formatting without modifying files (exit 1 if unformatted)
        #[arg(long)]
        check: bool,
    },
    /// Check .gcl files for type/dimension errors without evaluation
    Typecheck {
        /// Files or directories to check (default: current directory)
        paths: Vec<PathBuf>,
        /// Project root directory (overrides automatic graphcal.toml detection)
        #[arg(long)]
        root: Option<PathBuf>,
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
#[expect(
    clippy::too_many_lines,
    reason = "CLI main with subcommand dispatch and override parsing"
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
        Commands::Typecheck { paths, root } => {
            run_check(&paths, root.as_deref());
        }
        Commands::Format { paths, check } => {
            run_format(&paths, check);
        }
        Commands::Lsp => {
            #[expect(
                clippy::expect_used,
                reason = "fatal: cannot run LSP without a runtime"
            )]
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(graphcal_lsp::run());
        }
        Commands::Eval {
            file,
            format,
            set,
            input,
            no_assert,
            root,
        } => {
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

            // Parse --input JSON file
            if let Some(input_path) = &input {
                let source = std::fs::read_to_string(&file).unwrap_or_else(|e| {
                    eprintln!("error: cannot read {}: {e}", file.display());
                    process::exit(1);
                });
                let ast =
                    graphcal_syntax::parser::Parser::with_name(&source, &file.to_string_lossy())
                        .parse_file()
                        .unwrap_or_else(|e| {
                            eprintln!("error: failed to parse {}: {e}", file.display());
                            process::exit(1);
                        });

                let json_str = std::fs::read_to_string(input_path).unwrap_or_else(|e| {
                    eprintln!(
                        "error: cannot read input file {}: {e}",
                        input_path.display()
                    );
                    process::exit(1);
                });

                let json_overrides =
                    json_input::json_to_overrides(&json_str, &ast).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        process::exit(1);
                    });

                // Merge: --set takes precedence over --input
                for (name, expr) in json_overrides {
                    overrides.entry(name).or_insert(expr);
                }
            }

            match compile_and_eval_project(&file, &overrides, root.as_deref()) {
                Ok(result) => {
                    match format {
                        OutputFormat::Text => print_text(&result, no_assert),
                        OutputFormat::Json => {
                            if let Err(e) = print_json(&result, no_assert) {
                                eprintln!("JSON serialization error: {e}");
                                process::exit(2);
                            }
                        }
                    }
                    let has_eval_errors = result.params.iter().any(|(_, r)| r.is_err())
                        || result.nodes.iter().any(|(_, r)| r.is_err());
                    let has_assert_failures = !no_assert
                        && result.assertions.iter().any(|(_, r, _)| {
                            matches!(
                                r,
                                graphcal_eval::eval::AssertResult::Fail { .. }
                                    | graphcal_eval::eval::AssertResult::Error { .. }
                            )
                        });
                    if has_eval_errors || has_assert_failures {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("{:?}", miette::Report::new(e));
                    process::exit(2);
                }
            }
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
fn run_check(paths: &[PathBuf], project_root: Option<&Path>) {
    let targets = if paths.is_empty() {
        collect_gcl_files(&PathBuf::from("."))
    } else {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(collect_gcl_files(path));
            } else {
                files.push(path.clone());
            }
        }
        files
    };

    if targets.is_empty() {
        eprintln!("No .gcl files found");
        process::exit(1);
    }

    let mut error_count = 0;
    for file in &targets {
        match compile_to_tir_project(file, project_root) {
            Ok(_) => {
                println!("ok: {}", file.display());
            }
            Err(e) => {
                eprintln!("{:?}", miette::Report::new(e));
                error_count += 1;
            }
        }
    }

    if error_count > 0 {
        eprintln!("{error_count} file(s) had errors");
        process::exit(1);
    }
}

#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
fn run_format(paths: &[PathBuf], check: bool) {
    let targets = if paths.is_empty() {
        collect_gcl_files(&PathBuf::from("."))
    } else {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(collect_gcl_files(path));
            } else {
                files.push(path.clone());
            }
        }
        files
    };

    if targets.is_empty() {
        eprintln!("No .gcl files found");
        process::exit(1);
    }

    let mut unformatted_count = 0;
    for file in &targets {
        let source = std::fs::read_to_string(file).unwrap_or_else(|e| {
            eprintln!("error: cannot read {}: {e}", file.display());
            process::exit(1);
        });

        let Some(formatted) = graphcal_fmt::format_source(&source) else {
            eprintln!("warning: {} has parse errors, skipping", file.display());
            continue;
        };

        if source == formatted {
            continue;
        }

        if check {
            println!("Would reformat: {}", file.display());
            unformatted_count += 1;
        } else {
            std::fs::write(file, &formatted).unwrap_or_else(|e| {
                eprintln!("error: cannot write {}: {e}", file.display());
                process::exit(1);
            });
            println!("Formatted: {}", file.display());
        }
    }

    if check && unformatted_count > 0 {
        eprintln!("{unformatted_count} file(s) would be reformatted");
        process::exit(1);
    }
}

/// Recursively collect all `.gcl` files under a directory.
fn collect_gcl_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_gcl_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "gcl") {
            files.push(path);
        } else {
            // Skip non-.gcl files
        }
    }
    files.sort();
    files
}

/// Count how many levels of `Indexed` nesting a value has.
fn index_depth(value: &graphcal_eval::eval::Value) -> usize {
    match value {
        graphcal_eval::eval::Value::Indexed { entries, .. } => {
            entries.values().next().map_or(1, |v| 1 + index_depth(v))
        }
        _ => 0,
    }
}

/// Walk into nested `Indexed` to find the first leaf scalar's display label (unit).
fn extract_unit_label(
    value: &graphcal_eval::eval::Value,
    symbols: &BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
) -> Option<String> {
    match value {
        graphcal_eval::eval::Value::Scalar { .. } => value.display_label(symbols),
        graphcal_eval::eval::Value::Indexed { entries, .. } => entries
            .values()
            .next()
            .and_then(|v| extract_unit_label(v, symbols)),
        _ => None,
    }
}

/// Format a leaf value as a cell string without unit suffix.
fn format_leaf_cell(value: &graphcal_eval::eval::Value) -> String {
    use graphcal_eval::eval::Value;
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Label {
            index_name,
            variant,
        } => format!("{index_name}::{variant}"),
        Value::Scalar { .. } => format_number(value.display_value().unwrap_or_default()),
        Value::Struct { variant, .. } => variant.as_str().to_string(),
        Value::Indexed { .. } => "...".to_string(),
        Value::Datetime { .. } => value.format_datetime().unwrap_or_default(),
    }
}

/// Render a 2D `Indexed` value as a formatted table grid (without name/unit header).
fn format_table_grid(value: &graphcal_eval::eval::Value) -> String {
    use graphcal_eval::eval::Value;
    use tabled::builder::Builder;
    use tabled::settings::{Alignment, Style, object::Columns};

    let Value::Indexed {
        entries: row_entries,
        ..
    } = value
    else {
        return String::new();
    };

    // Extract column names from first row
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
        .map(graphcal_syntax::names::VariantName::as_str)
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
                    .map(|(_, v)| format_leaf_cell(v))
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

/// Render an N-dimensional indexed value (N >= 2) as formatted table(s).
fn format_indexed_table(
    name: &str,
    value: &graphcal_eval::eval::Value,
    symbols: &BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
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

/// Recursively peel outer index dimensions and render 2D table slices with section headers.
fn format_table_slices(
    value: &graphcal_eval::eval::Value,
    symbols: &BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    depth: usize,
    parts: &mut Vec<String>,
) {
    use graphcal_eval::eval::Value;

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
    let _ = symbols; // used only for recursive calls
    for (variant, inner_val) in entries {
        parts.push(format!("\n  [{index_name}::{variant}]"));
        format_table_slices(inner_val, symbols, depth - 1, parts);
    }
}

#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(clippy::print_stderr, reason = "CLI binary, stderr output for errors")]
#[expect(
    clippy::too_many_lines,
    reason = "text output formatting with assertion display"
)]
fn print_text(result: &EvalResult, no_assert: bool) {
    use graphcal_eval::eval::{NodeError, Value};

    /// A block of output: either a batch of flat lines or a table block.
    enum OutputBlock<'a> {
        Flat(Vec<FlatEntry<'a>>),
        Table(&'a str, &'a Value),
    }

    enum FlatEntry<'a> {
        Value(String, &'a Value),
        Error(String, &'a NodeError),
    }

    // Flatten entries: scalars are one line, structs expand to `name.field` lines,
    // indexed values (1D only) expand to `name[Variant]` lines
    fn flatten_value<'a>(prefix: &str, value: &'a Value, entries: &mut Vec<FlatEntry<'a>>) {
        match value {
            Value::Scalar { .. }
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Label { .. }
            | Value::Datetime { .. } => {
                entries.push(FlatEntry::Value(prefix.to_string(), value));
            }
            Value::Struct {
                variant,
                type_name,
                fields,
            } => {
                if variant.as_str() == type_name.as_str() {
                    for (field_name, field_val) in fields {
                        flatten_value(
                            &format!("{prefix}.{}", field_name.as_str()),
                            field_val,
                            entries,
                        );
                    }
                } else if fields.is_empty() {
                    entries.push(FlatEntry::Value(prefix.to_string(), value));
                } else {
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

    // Build output blocks preserving source order
    let mut blocks: Vec<OutputBlock> = Vec::new();
    let mut current_flat: Vec<FlatEntry> = Vec::new();

    for (name, node_result, _) in &result.all {
        match node_result {
            Ok(value) if index_depth(value) >= 2 => {
                // Flush accumulated flat entries before the table
                if !current_flat.is_empty() {
                    blocks.push(OutputBlock::Flat(std::mem::take(&mut current_flat)));
                }
                blocks.push(OutputBlock::Table(name.as_str(), value));
            }
            Ok(value) => {
                flatten_value(name.as_str(), value, &mut current_flat);
            }
            Err(err) => {
                current_flat.push(FlatEntry::Error(name.as_str().to_string(), err));
            }
        }
    }
    // Flush remaining flat entries
    if !current_flat.is_empty() {
        blocks.push(OutputBlock::Flat(current_flat));
    }

    // Compute max name width across all flat entries for alignment
    let max_name_len = blocks
        .iter()
        .filter_map(|b| match b {
            OutputBlock::Flat(entries) => Some(entries.iter().map(|e| match e {
                FlatEntry::Value(n, _) | FlatEntry::Error(n, _) => n.len(),
            })),
            OutputBlock::Table(..) => None,
        })
        .flatten()
        .max()
        .unwrap_or(0);

    // Print all blocks in order
    for block in &blocks {
        match block {
            OutputBlock::Flat(entries) => {
                let width = max_name_len;
                for entry in entries {
                    match entry {
                        FlatEntry::Error(name, err) => {
                            eprintln!("{name:width$} = ERROR: {err}");
                        }
                        FlatEntry::Value(name, value) => match value {
                            Value::Bool(b) => println!("{name:width$} = {b}"),
                            Value::Int(i) => println!("{name:width$} = {i}"),
                            Value::Label {
                                index_name,
                                variant,
                            } => {
                                println!("{name:width$} = {index_name}::{variant}");
                            }
                            Value::Struct { variant, .. } => {
                                println!("{name:width$} = {}", variant.as_str());
                            }
                            Value::Datetime { .. } => {
                                let formatted = value.format_datetime().unwrap_or_default();
                                println!("{name:width$} = {formatted}");
                            }
                            _ => {
                                let formatted =
                                    format_number(value.display_value().unwrap_or_default());
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
            OutputBlock::Table(name, value) => {
                println!();
                println!(
                    "{}",
                    format_indexed_table(name, value, &result.base_dim_symbols)
                );
            }
        }
    }

    // Print assertion results (unless --no-assert)
    if !no_assert && !result.assertions.is_empty() {
        use graphcal_eval::eval::AssertResult;

        println!();
        println!("Assertions:");
        let max_assert_len = result
            .assertions
            .iter()
            .map(|(n, _, _)| n.as_str().len())
            .max()
            .unwrap_or(0);
        for (name, assert_result, _) in &result.assertions {
            let n = name.as_str();
            let w = max_assert_len;
            match assert_result {
                AssertResult::Pass => {
                    println!("  {n:w$}  PASS");
                }
                AssertResult::Fail { message } => {
                    eprintln!("  {n:w$}  FAIL  ({message})");
                    if let Some(affected) = result.assumes_map.get(n) {
                        eprintln!(
                            "  {blank:w$}        affected: {nodes}",
                            blank = "",
                            nodes = affected.join(", ")
                        );
                    }
                }
                AssertResult::Error { message } => {
                    eprintln!("  {n:w$}  ERROR ({message})");
                }
            }
        }
    }
}

#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(
    clippy::too_many_lines,
    reason = "JSON output formatting is clearest as a single function"
)]
fn print_json(result: &EvalResult, no_assert: bool) -> Result<(), serde_json::Error> {
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
                        serde_json::json!(v.display_value().unwrap_or_default()),
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
            Value::Label {
                index_name,
                variant,
            } => {
                serde_json::json!({
                    "index": index_name.as_str(),
                    "variant": variant.as_str()
                })
            }
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
            Value::Datetime {
                epoch,
                time_scale,
                display_tz,
            } => {
                let mut map = serde_json::Map::new();
                let formatted =
                    graphcal_eval::eval::format_epoch_with_tz(epoch, display_tz.as_deref());
                map.insert("datetime".to_string(), serde_json::json!(formatted));
                map.insert(
                    "time_scale".to_string(),
                    serde_json::json!(time_scale.to_string()),
                );
                if let Some(tz) = display_tz {
                    map.insert("display_tz".to_string(), serde_json::json!(tz));
                }
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

    let consts: serde_json::Map<String, serde_json::Value> = result
        .consts
        .iter()
        .map(|(n, v)| (n.to_string(), value_to_json(v, symbols)))
        .collect();
    let params: serde_json::Map<String, serde_json::Value> = result
        .params
        .iter()
        .map(|(n, r)| (n.to_string(), result_to_json(r, symbols)))
        .collect();
    let nodes: serde_json::Map<String, serde_json::Value> = result
        .nodes
        .iter()
        .map(|(n, r)| (n.to_string(), result_to_json(r, symbols)))
        .collect();

    output.insert("const".to_string(), serde_json::Value::Object(consts));
    output.insert("param".to_string(), serde_json::Value::Object(params));
    output.insert("node".to_string(), serde_json::Value::Object(nodes));

    if !no_assert && !result.assertions.is_empty() {
        use graphcal_eval::eval::AssertResult;

        let assertions: serde_json::Map<String, serde_json::Value> = result
            .assertions
            .iter()
            .map(|(n, r, _)| {
                let val = match r {
                    AssertResult::Pass => serde_json::json!({"status": "pass"}),
                    AssertResult::Fail { message } => {
                        let mut obj = serde_json::json!({"status": "fail", "message": message});
                        if let Some(affected) = result.assumes_map.get(n.as_str()) {
                            obj["affected_nodes"] = serde_json::json!(affected);
                        }
                        obj
                    }
                    AssertResult::Error { message } => {
                        serde_json::json!({"status": "error", "message": message})
                    }
                };
                (n.to_string(), val)
            })
            .collect();
        output.insert("assert".to_string(), serde_json::Value::Object(assertions));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(output))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
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
