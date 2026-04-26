mod display;
mod json_input;
mod overrides;
mod plot;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process;

use graphcal_compiler::syntax::names::DeclName;
use graphcal_eval::eval::{
    EvalResult, compile_and_eval_project, compile_to_tir_project, format_number,
};
use graphcal_io::RealFileSystem;

use crate::display::{OutputBlock, build_output_blocks, format_indexed_table, max_flat_name_len};
use crate::overrides::{OverrideParseError, parse_overrides};

/// True when an assertion result represents a failure (a `Fail` outcome or
/// a runtime `Error` while evaluating the assertion). Used to decide both
/// the process exit code and stderr-vs-stdout routing in text output.
const fn is_assert_failure(r: &graphcal_eval::eval::AssertResult) -> bool {
    matches!(
        r,
        graphcal_eval::eval::AssertResult::Fail { .. }
            | graphcal_eval::eval::AssertResult::Error { .. }
    )
}

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
        /// Maximum size (in bytes) of the --input JSON file. Defaults to 1 MiB.
        #[arg(long)]
        input_max_bytes: Option<u64>,
        /// Project root directory (overrides automatic graphcal.toml detection)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Plot output mode: browser (default), json, or a file path for HTML output
        #[arg(long)]
        plot: Option<PlotOutput>,
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
    Check {
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

#[derive(ValueEnum, Clone)]
enum PlotOutput {
    /// Open interactive plot in the default browser
    Browser,
    /// Print Plotly JSON spec to stdout
    Json,
}

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
        Commands::Check { paths, root } => {
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
            input_max_bytes,
            root,
            plot: plot_output,
        } => {
            let overrides = match parse_overrides(&set, input.as_deref(), input_max_bytes) {
                Ok(o) => o,
                Err(e) => report_override_error(&e),
            };
            handle_eval(
                &file,
                &format,
                &overrides,
                root.as_deref(),
                plot_output.as_ref(),
            );
        }
    }
}

/// Print an override-parse error and exit with code 2.
///
/// Kept separate so both the return type (`!`) and the `print_stderr` lint
/// suppression stay out of the happy path in `main`.
#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
fn report_override_error(e: &OverrideParseError) -> ! {
    eprintln!("error: {e}");
    process::exit(2);
}

#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
#[expect(
    clippy::print_stdout,
    reason = "CLI binary, stdout output is expected for --plot json"
)]
fn handle_eval(
    file: &Path,
    format: &OutputFormat,
    overrides: &std::collections::HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    root: Option<&Path>,
    plot_output: Option<&PlotOutput>,
) {
    // Rooted sandbox: derive the project root from the loader's rules and
    // confine reads to it. `root` (the user's explicit --root) takes
    // precedence; otherwise walk up from `file`'s parent looking for
    // `graphcal.toml`, falling back to `file`'s parent.
    let fs = build_rooted_fs(file, root);
    match compile_and_eval_project(file, overrides, root, &fs) {
        Ok(result) => {
            match format {
                OutputFormat::Text => print_text(&result),
                OutputFormat::Json => {
                    if let Err(e) = print_json(&result) {
                        eprintln!("JSON serialization error: {e}");
                        process::exit(2);
                    }
                }
            }

            // Handle --plot output
            if let Some(plot_mode) = plot_output {
                let rendered = plot::build_figures(&result.plots, &result.figures, &result.layers);
                if rendered.is_empty() {
                    eprintln!("warning: no plot declarations found");
                } else {
                    match plot_mode {
                        PlotOutput::Browser => {
                            let html = plot::render_html(&rendered).unwrap_or_else(|e| {
                                eprintln!("error: could not render plots as HTML: {e}");
                                process::exit(2);
                            });
                            let mut tmp = tempfile::Builder::new()
                                .prefix("graphcal_plot_")
                                .suffix(".html")
                                .tempfile()
                                .unwrap_or_else(|e| {
                                    eprintln!("error: could not create temp file: {e}");
                                    process::exit(2);
                                });
                            std::io::Write::write_all(&mut tmp, html.as_bytes()).unwrap_or_else(
                                |e| {
                                    eprintln!("error: could not write HTML: {e}");
                                    process::exit(2);
                                },
                            );
                            // Keep the temp file so the browser has time to read it.
                            // The OS will clean it up on reboot.
                            let path = tmp.into_temp_path();
                            let kept = path.keep().unwrap_or_else(|e| {
                                eprintln!("error: could not persist temp file: {e}");
                                process::exit(2);
                            });
                            if let Err(e) = open::that(&kept) {
                                eprintln!("error: could not open browser: {e}");
                                process::exit(2);
                            }
                        }
                        PlotOutput::Json => {
                            let json = plot::render_json(&rendered).unwrap_or_else(|e| {
                                eprintln!("error: could not render plots as JSON: {e}");
                                process::exit(2);
                            });
                            println!("{json}");
                        }
                    }
                }
            }

            let has_eval_errors = result.params.iter().any(|(_, r)| r.is_err())
                || result.nodes.iter().any(|(_, r)| r.is_err());
            let has_assert_failures = result
                .assertions
                .iter()
                .any(|(_, r, _)| is_assert_failure(r));
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

/// Build a [`RealFileSystem`] sandboxed to the project root, if one can be
/// determined.
///
/// Resolution order mirrors `graphcal_eval::loader::resolve_project_root`:
/// 1. Explicit `--root` (canonicalized).
/// 2. Walk up from `file`'s parent looking for `graphcal.toml`.
/// 3. Fall back to the unrooted form so CLI one-shots of single loose files
///    keep working exactly as before.
fn build_rooted_fs(file: &Path, root_override: Option<&Path>) -> RealFileSystem {
    if let Some(explicit) = root_override
        && let Ok(canonical) = explicit.canonicalize()
    {
        return RealFileSystem::rooted(canonical);
    }

    let Ok(canonical_file) = file.canonicalize() else {
        return RealFileSystem::default();
    };
    let mut dir = canonical_file.parent().unwrap_or(&canonical_file);
    loop {
        if dir.join("graphcal.toml").is_file() {
            return RealFileSystem::rooted(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    RealFileSystem::default()
}

/// Resolve CLI path arguments to a list of `.gcl` files.
///
/// If `paths` is empty, collects all `.gcl` files from the current directory.
/// Otherwise, expands directories and passes through individual files.
fn resolve_target_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    if paths.is_empty() {
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
    }
}

#[expect(
    clippy::print_stderr,
    reason = "CLI binary, stderr output is expected for errors"
)]
#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
fn run_check(paths: &[PathBuf], project_root: Option<&Path>) {
    let targets = resolve_target_files(paths);

    if targets.is_empty() {
        eprintln!("No .gcl files found");
        process::exit(1);
    }

    let mut error_count = 0;
    for file in &targets {
        let fs = build_rooted_fs(file, project_root);
        match compile_to_tir_project(file, project_root, &fs) {
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
    let targets = resolve_target_files(paths);

    if targets.is_empty() {
        eprintln!("No .gcl files found");
        process::exit(1);
    }

    let mut unformatted_count = 0;
    let mut error_count = 0;
    for file in &targets {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                // Match `run_check`'s behavior: accumulate errors across the
                // batch instead of aborting on the first failure so users see
                // the full picture in one run.
                eprintln!("error: cannot read {}: {e}", file.display());
                error_count += 1;
                continue;
            }
        };

        let formatted = match graphcal_fmt::format_source(&source) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("warning: {}: {e}, skipping", file.display());
                continue;
            }
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
    if error_count > 0 {
        eprintln!("{error_count} file(s) could not be read");
        process::exit(1);
    }
}

/// Directories to skip during recursive `.gcl` file collection.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".build", "__pycache__"];

/// Recursively collect all `.gcl` files under a directory, sorted for deterministic output.
///
/// Uses `walkdir` for safe traversal: symlinks are not followed and common
/// generated directories (`.git`, `target`, `node_modules`, etc.) are skipped.
/// Traversal errors (permission denied, transient I/O) are logged to stderr
/// rather than silently dropped so users know when `format` only saw part of
/// the tree.
#[expect(clippy::print_stderr, reason = "CLI binary, stderr output for errors")]
fn collect_gcl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip well-known generated/vendored directories.
            // Non-UTF-8 directory names are not skipped — they won't match
            // any SKIP_DIRS entry anyway, so we intentionally let them
            // through rather than treating them as if they were the empty
            // string.
            if e.file_type().is_dir() {
                return !e
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIP_DIRS.contains(&name));
            }
            true
        })
    {
        match entry {
            Ok(e) if e.path().extension().is_some_and(|ext| ext == "gcl") => {
                files.push(e.into_path());
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!(
                    "warning: could not traverse {}: {err}",
                    err.path()
                        .map_or_else(|| dir.display().to_string(), |p| p.display().to_string())
                );
            }
        }
    }
    files.sort();
    files
}

#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(clippy::print_stderr, reason = "CLI binary, stderr output for errors")]
fn print_text(result: &EvalResult) {
    use graphcal_eval::eval::Value;

    // Build output blocks preserving source order.
    let items = result.all.iter().map(|(name, r, _)| (name.as_str(), r));
    let blocks = build_output_blocks(items);
    let max_name_len = max_flat_name_len(&blocks);

    // Print all blocks in order.
    for block in &blocks {
        match block {
            OutputBlock::Flat(entries) => {
                let width = max_name_len;
                for entry in entries {
                    match entry {
                        display::FlatEntry::Error(name, err) => {
                            eprintln!("{name:width$} = ERROR: {err}");
                        }
                        display::FlatEntry::Value(name, value) => {
                            if let Value::Scalar { .. } = value {
                                #[expect(
                                    clippy::expect_used,
                                    reason = "display_value always returns Ok for Scalar variant"
                                )]
                                let formatted =
                                    format_number(value.display_value().expect("value is Scalar"));
                                if let Some(label) = value.display_label(&result.base_dim_symbols) {
                                    println!("{name:width$} = {formatted} {label}");
                                } else {
                                    println!("{name:width$} = {formatted}");
                                }
                            } else {
                                let formatted =
                                    value.format_display(Some(&result.base_dim_symbols));
                                println!("{name:width$} = {formatted}");
                            }
                        }
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

    // Print assertion results
    if !result.assertions.is_empty() {
        println!();
        println!("Assertions:");
        let max_assert_len = result
            .assertions
            .iter()
            .map(|(n, _, _)| n.as_str().len())
            .max()
            .unwrap_or(0);
        for (name, assert_result, _) in &result.assertions {
            let line = format_assertion_line(
                name.as_str(),
                assert_result,
                max_assert_len,
                result.assumes_map.get(name.as_str()),
            );
            if is_assert_failure(assert_result) {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
    }
}

/// Format a single assertion result line for text output.
///
/// Returns the formatted string including the assertion name, status, and (for failures)
/// the failure message and affected nodes.
fn format_assertion_line(
    name: &str,
    result: &graphcal_eval::eval::AssertResult,
    name_width: usize,
    affected: Option<&Vec<graphcal_compiler::syntax::names::DeclName>>,
) -> String {
    use std::fmt::Write as _;

    use graphcal_eval::eval::AssertResult;

    let w = name_width;
    match result {
        AssertResult::Pass => {
            format!("  {name:w$}  PASS")
        }
        AssertResult::Fail { message } => {
            let mut line = format!("  {name:w$}  FAIL  ({message})");
            if let Some(affected) = affected {
                let _ = write!(
                    line,
                    "\n  {:w$}        affected: {}",
                    "",
                    affected.join(", ")
                );
            }
            line
        }
        AssertResult::Error { message } => {
            format!("  {name:w$}  ERROR ({message})")
        }
    }
}

#[expect(clippy::print_stdout, reason = "CLI binary, stdout output is expected")]
#[expect(
    clippy::too_many_lines,
    reason = "JSON output formatting is clearest as a single function"
)]
fn print_json(result: &EvalResult) -> Result<(), serde_json::Error> {
    use graphcal_eval::eval::{NodeError, Value};

    fn value_to_json(
        v: &Value,
        symbols: &std::collections::BTreeMap<
            graphcal_compiler::syntax::dimension::BaseDimId,
            String,
        >,
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
                    #[expect(
                        clippy::expect_used,
                        reason = "display_value always returns Ok for Scalar variant"
                    )]
                    let dv = v.display_value().expect("value is Scalar");
                    map.insert("display_value".to_string(), serde_json::json!(dv));
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
            Value::Struct { type_name, fields } => {
                let mut map = serde_json::Map::new();
                map.insert("type".to_string(), serde_json::json!(type_name.as_str()));
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
        symbols: &std::collections::BTreeMap<
            graphcal_compiler::syntax::dimension::BaseDimId,
            String,
        >,
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

    if !result.assertions.is_empty() {
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
                            let names: Vec<&str> = affected
                                .iter()
                                .map(graphcal_compiler::syntax::names::DeclName::as_str)
                                .collect();
                            obj["affected_nodes"] = serde_json::json!(names);
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
