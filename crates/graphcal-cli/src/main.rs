// The CLI is the imperative shell — writing to stdout/stderr is the binary's
// whole purpose. Suppress both lints crate-wide so that library crates keep
// the workspace-level warning while this binary doesn't need per-call-site
// `#[expect]` blocks.
#![expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "CLI binary; stdout/stderr are the user-facing output channels"
)]
#![expect(
    clippy::result_large_err,
    reason = "GraphcalError is inherently large and only constructed on the error path"
)]

mod deps;
mod display;
mod json_input;
mod overrides;
mod plot;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process;

use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_eval::eval::{
    CompileError, EvalResult, compile_and_eval_from_project_with_host_fns,
    compile_to_tir_from_project_with_host_fns, format_number,
};
use graphcal_eval::host_fns::HostFunctionRegistry;
use graphcal_eval::loader::{LoadedProject, build_rooted_filesystem, load_project};

use graphcal::format::{FormatStatus, collect_gcl_files, format_status};

use crate::display::{OutputBlock, build_output_blocks, format_indexed_table, max_flat_name_len};
use crate::overrides::{OverrideParseError, parse_overrides};

const VERSION: &str = if env!("GIT_HASH").is_empty() {
    env!("CARGO_PKG_VERSION")
} else {
    concat!(
        env!("CARGO_PKG_VERSION"),
        " (commit: ",
        env!("GIT_HASH"),
        ")"
    )
};

/// True when an assertion result represents a failure (a `Fail` outcome or
/// a runtime `Error` while evaluating the assertion). Used to decide both
/// stderr-vs-stdout routing in text output.
const fn is_assert_failure(r: &graphcal_eval::eval::AssertResult) -> bool {
    matches!(
        r,
        graphcal_eval::eval::AssertResult::Fail { .. }
            | graphcal_eval::eval::AssertResult::Error { .. }
    )
}

/// Print `prefix: {err}` to stderr and exit with the given non-zero code.
/// Used to keep the plot pipeline's "could not X" branches one-line each.
fn bail_with(prefix: &str, err: impl std::fmt::Display, exit_code: i32) -> ! {
    eprintln!("error: {prefix}: {err}");
    process::exit(exit_code);
}

#[derive(Parser)]
#[command(name = "graphcal", version = VERSION, about = "Graphcal language evaluator")]
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
        /// Plot output mode: `browser` opens the rendered plots in the default
        /// browser, `json` prints the Vega-Lite spec array to stdout, and a
        /// path ending in `.html` writes a self-contained HTML page to that file
        #[arg(long, value_name = "browser|json|FILE.html", value_parser = parse_plot_output)]
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
    /// Export the dependency graph of a .gcl file (experimental)
    Graph {
        /// Path to the .gcl file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value = "dot")]
        format: GraphFormat,
        /// Project root directory (overrides automatic graphcal.toml detection)
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Manage package dependencies
    Deps {
        #[command(subcommand)]
        command: DepsCommands,
    },
    /// Start the Language Server Protocol (LSP) server
    Lsp,
}

#[derive(Subcommand)]
enum DepsCommands {
    /// Resolve Git dependencies and write graphcal.lock
    Lock {
        /// Project root directory (overrides automatic graphcal.toml detection)
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

#[derive(ValueEnum, Clone)]
enum GraphFormat {
    /// Graphviz DOT text (pipe to `dot -Tsvg` to render)
    Dot,
}

#[derive(ValueEnum, Clone)]
enum OutputFormat {
    Text,
    Json,
}

/// Plot output destination selected by `--plot`.
#[derive(Clone)]
enum PlotOutput {
    /// Open the rendered plots in the default browser.
    Browser,
    /// Print the Vega-Lite spec array to stdout.
    Json,
    /// Write the self-contained HTML page to the given path.
    HtmlFile(PathBuf),
}

/// Parse the `--plot` argument: `browser`, `json`, or a `.html` file path.
fn parse_plot_output(s: &str) -> Result<PlotOutput, String> {
    match s {
        "browser" => Ok(PlotOutput::Browser),
        "json" => Ok(PlotOutput::Json),
        path if Path::new(path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("html")) =>
        {
            Ok(PlotOutput::HtmlFile(PathBuf::from(path)))
        }
        other => Err(format!(
            "expected `browser`, `json`, or a path ending in `.html`, got `{other}`"
        )),
    }
}

/// Stack segment size for running a command.
///
/// Recursive walkers over user expressions grow the stack on demand
/// (`graphcal_compiler::stack::with_stack_growth`), but compiler-generated
/// *drop glue* for deep expression trees recurses without any hook we can
/// intercept. One large pre-grown segment covers teardown of trees from
/// pathologically long operator chains.
const COMMAND_STACK_SIZE: usize = 64 * 1024 * 1024;

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
    stacker::grow(COMMAND_STACK_SIZE, || run_command(cli));
}

fn run_command(cli: Cli) {
    match cli.command {
        Commands::Check { paths, root } => {
            run_check(&paths, root.as_deref());
        }
        Commands::Format { paths, check } => {
            run_format(&paths, check);
        }
        Commands::Graph { file, format, root } => {
            run_graph(&file, &format, root.as_deref());
        }
        Commands::Deps { command } => match command {
            DepsCommands::Lock { root } => {
                run_deps_lock(root.as_deref());
            }
        },
        Commands::Lsp => {
            #[expect(
                clippy::expect_used,
                reason = "fatal: cannot run LSP without a runtime"
            )]
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                // Worker and blocking threads analyze user buffers; like
                // `main`, they need headroom for deep-expression drop glue.
                .thread_stack_size(COMMAND_STACK_SIZE)
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

fn run_deps_lock(root: Option<&Path>) {
    match deps::lock(root) {
        Ok(outcome) => {
            if outcome.changed {
                println!("wrote {}", outcome.lockfile_path.display());
            } else {
                println!("up to date: {}", outcome.lockfile_path.display());
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(2);
        }
    }
}

/// Print an override-parse error and exit with code 2.
///
/// Kept separate so that the return type (`!`) stays out of the happy path
/// in `main`.
fn report_override_error(e: &OverrideParseError) -> ! {
    eprintln!("error: {e}");
    process::exit(2);
}

fn handle_eval(
    file: &Path,
    format: &OutputFormat,
    overrides: &std::collections::HashMap<
        DeclName,
        graphcal_compiler::desugar::desugared_ast::Expr,
    >,
    root: Option<&Path>,
    plot_output: Option<&PlotOutput>,
) {
    // Rooted sandbox: derive the project root from the loader's rules and
    // confine reads to it. `root` (the user's explicit --root) takes
    // precedence; otherwise walk up from `file`'s parent looking for
    // `graphcal.toml`, falling back to an unrooted FS for loose files.
    let fs = build_rooted_filesystem(file, root);
    let outcome = load_project_with_plugins(file, root, &fs).and_then(|(project, host_fns)| {
        compile_and_eval_from_project_with_host_fns(&project, overrides, &host_fns)
    });
    match outcome {
        Ok(result) => {
            let plot_json_only = matches!(plot_output, Some(PlotOutput::Json));
            if !plot_json_only {
                match format {
                    OutputFormat::Text => print_text(&result),
                    OutputFormat::Json => {
                        if let Err(e) = print_json(&result) {
                            eprintln!("JSON serialization error: {e}");
                            process::exit(2);
                        }
                    }
                }
            }

            // Plot evaluation failures are reported even without `--plot`, so
            // a normal eval run cannot hide broken plot declarations.
            for err in &result.plot_errors {
                eprintln!("error: plot `{}` not rendered: {}", err.name, err.message);
            }

            // Handle --plot output. JSON plot mode has a pipe-friendly stdout
            // contract: the entire stdout stream is the figure array, so normal
            // evaluation output is suppressed above.
            let mut plot_output_failed = false;
            if let Some(plot_mode) = plot_output {
                let rendered = plot::build_figures(&result.plots, &result.figures, &result.layers);
                let no_plot_decls = result.plots.is_empty()
                    && result.plot_errors.is_empty()
                    && result.figures.is_empty()
                    && result.layers.is_empty();
                if no_plot_decls {
                    eprintln!("warning: no plot declarations found");
                }
                let no_displayed_plots =
                    matches!(plot_mode, PlotOutput::Browser | PlotOutput::HtmlFile(_))
                        && rendered.is_empty()
                        && !no_plot_decls;
                if no_displayed_plots {
                    eprintln!(
                        "error: no displayed plots to render (all selected plots may be #[hidden])"
                    );
                    plot_output_failed = true;
                }
                match plot_mode {
                    PlotOutput::Browser | PlotOutput::HtmlFile(_) if rendered.is_empty() => {}
                    PlotOutput::HtmlFile(path) => {
                        let html = plot::render_html(&rendered)
                            .unwrap_or_else(|e| bail_with("could not render plots as HTML", e, 2));
                        std::fs::write(path, html).unwrap_or_else(|e| {
                            bail_with(&format!("could not write {}", path.display()), e, 2)
                        });
                        eprintln!("wrote plots to {}", path.display());
                    }
                    PlotOutput::Browser => {
                        let html = plot::render_html(&rendered)
                            .unwrap_or_else(|e| bail_with("could not render plots as HTML", e, 2));
                        let mut tmp = tempfile::Builder::new()
                            .prefix("graphcal_plot_")
                            .suffix(".html")
                            .tempfile()
                            .unwrap_or_else(|e| bail_with("could not create temp file", e, 2));
                        std::io::Write::write_all(&mut tmp, html.as_bytes())
                            .unwrap_or_else(|e| bail_with("could not write HTML", e, 2));
                        // Keep the temp file so the browser has time to read it.
                        // The OS will clean it up on reboot.
                        let path = tmp.into_temp_path();
                        let kept = path
                            .keep()
                            .unwrap_or_else(|e| bail_with("could not persist temp file", e, 2));
                        if let Err(e) = open::that(&kept) {
                            bail_with("could not open browser", e, 2);
                        }
                    }
                    PlotOutput::Json => {
                        let json = plot::render_json(&rendered)
                            .unwrap_or_else(|e| bail_with("could not render plots as JSON", e, 2));
                        println!("{json}");
                    }
                }
            }

            if result.has_errors() || plot_output_failed {
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{:?}", miette::Report::new(e));
            process::exit(2);
        }
    }
}

/// Resolve CLI path arguments to a list of `.gcl` files.
///
/// If `paths` is empty, collects all `.gcl` files from the current directory.
/// Otherwise, expands directories and passes through individual files.
fn resolve_target_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    if paths.is_empty() {
        collect_with_warnings(&PathBuf::from("."))
    } else {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(collect_with_warnings(path));
            } else {
                files.push(path.clone());
            }
        }
        files
    }
}

/// Thin shell over `format::collect_gcl_files`: surfaces traversal warnings on
/// stderr (the library returns them as data) so users know when `format` only
/// saw part of the tree, then yields just the file list.
fn collect_with_warnings(dir: &Path) -> Vec<PathBuf> {
    let (files, warnings) = collect_gcl_files(dir);
    for err in warnings {
        eprintln!(
            "warning: could not traverse {}: {err}",
            err.path()
                .map_or_else(|| dir.display().to_string(), |p| p.display().to_string())
        );
    }
    files
}

/// Load a project and build the host function registry backing its extern
/// (plugin) declarations: the built-in demo plugin plus every wasm plugin
/// the project vendors, loaded through one plugin host per invocation.
fn load_project_with_plugins<F: graphcal_io::FileSystemReader>(
    file: &Path,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<(LoadedProject, HostFunctionRegistry), CompileError> {
    let project = load_project(file, project_root, fs)?;
    let mut host_fns = graphcal_eval::host_fns::demo_registry();
    graphcal_plugin_host::register_project_plugins(
        &graphcal_plugin_host::PluginHost::new(),
        &project,
        &mut host_fns,
    );
    Ok((project, host_fns))
}

fn run_check(paths: &[PathBuf], project_root: Option<&Path>) {
    let targets = resolve_target_files(paths);

    if targets.is_empty() {
        eprintln!("No .gcl files found");
        process::exit(1);
    }

    let mut error_count = 0;
    for file in &targets {
        let fs = build_rooted_filesystem(file, project_root);
        let outcome =
            load_project_with_plugins(file, project_root, &fs).and_then(|(project, host_fns)| {
                compile_to_tir_from_project_with_host_fns(&project, &host_fns)
            });
        match outcome {
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

/// `graphcal graph`: compile to TIR, project the dependency graph IR, and
/// print it in the requested export format. The projection and rendering are
/// pure (`graphcal_eval::graph_ir`); this shell only does I/O.
fn run_graph(file: &Path, format: &GraphFormat, project_root: Option<&Path>) {
    // On stderr so stdout stays a clean pipe into `dot`.
    eprintln!(
        "warning: `graphcal graph` is experimental; its output and CLI surface may change in any release"
    );
    let fs = build_rooted_filesystem(file, project_root);
    let outcome =
        load_project_with_plugins(file, project_root, &fs).and_then(|(project, host_fns)| {
            compile_to_tir_from_project_with_host_fns(&project, &host_fns)
        });
    match outcome {
        Ok(tir) => {
            let ir = graphcal_eval::graph_ir::project_tir(&tir);
            match format {
                GraphFormat::Dot => print!("{}", graphcal_eval::graph_ir::dot::render(&ir)),
            }
        }
        Err(e) => {
            eprintln!("{:?}", miette::Report::new(e));
            process::exit(2);
        }
    }
}

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

        match format_status(&source) {
            FormatStatus::Unchanged => {}
            FormatStatus::Error(e) => {
                // A file that cannot be formatted (usually a parse error) is
                // a failure, not a skip: `format --check` in CI must not
                // pass on syntactically broken files.
                eprintln!("error: {}: {e}", file.display());
                error_count += 1;
            }
            FormatStatus::Changed(formatted) => {
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
        }
    }

    if check && unformatted_count > 0 {
        eprintln!("{unformatted_count} file(s) would be reformatted");
        process::exit(1);
    }
    if error_count > 0 {
        eprintln!("{error_count} file(s) could not be read or formatted");
        process::exit(1);
    }
}

fn print_text(result: &EvalResult) {
    use graphcal_eval::eval::Value;

    // Build output blocks preserving source order. Names render their full
    // alias-qualified path so multiple instantiations stay distinct (#813).
    let rendered_names: Vec<(String, &Result<Value, graphcal_eval::eval::NodeError>)> = result
        .all
        .iter()
        .map(|(name, r, _)| (name.to_string(), r))
        .collect();
    let items = rendered_names.iter().map(|(n, r)| (n.as_str(), *r));
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
                            if let Value::Scalar {
                                si_value,
                                display_unit,
                                ..
                            } = value
                            {
                                let formatted =
                                    format_number(graphcal_eval::eval::scalar_display_value(
                                        *si_value,
                                        display_unit.as_ref(),
                                    ));
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
        let assert_names: Vec<String> = result
            .assertions
            .iter()
            .map(|(n, _, _)| n.to_string())
            .collect();
        let max_assert_len = assert_names.iter().map(String::len).max().unwrap_or(0);
        for ((name, assert_result, _), name_str) in result.assertions.iter().zip(&assert_names) {
            let line = format_assertion_line(
                name_str,
                assert_result,
                max_assert_len,
                result.assumes_map.get(name),
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
    affected: Option<&Vec<graphcal_compiler::syntax::module_name::ScopedName>>,
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
                let affected: Vec<String> = affected.iter().map(ToString::to_string).collect();
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

#[expect(
    clippy::too_many_lines,
    reason = "JSON output formatting is clearest as a single function"
)]
fn print_json(result: &EvalResult) -> Result<(), serde_json::Error> {
    use graphcal_eval::eval::{NodeError, Value};

    fn value_to_json(
        v: &Value,
        symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
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
                    let dv = graphcal_eval::eval::scalar_display_value(*si_value, Some(du));
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
                    "index": index_name.display_name().as_str(),
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
                map.insert(
                    "index".to_string(),
                    serde_json::json!(index_name.display_name().as_str()),
                );
                let entries_map: serde_json::Map<String, serde_json::Value> = entries
                    .iter()
                    .map(|(name, val)| {
                        (
                            v.indexed_entry_display_name(name),
                            value_to_json(val, symbols),
                        )
                    })
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
        symbols: &std::collections::BTreeMap<graphcal_compiler::dimension::BaseDimId, String>,
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
        .map(|(n, r)| (n.to_string(), result_to_json(r, symbols)))
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
                        if let Some(affected) = result.assumes_map.get(n) {
                            let names: Vec<String> =
                                affected.iter().map(ToString::to_string).collect();
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

    #[test]
    fn format_small_nonzero_decimal() {
        assert_eq!(format_number(3.0e-9), "3e-9");
        assert_eq!(format_number(-3.0e-9), "-3e-9");
        assert_eq!(format_number(-0.0), "0");
    }
}
