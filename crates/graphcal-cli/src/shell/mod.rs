//! Interactive shell for Graphcal (`graphcal shell`).
//!
//! Provides a REPL that accumulates declarations, evaluates the graph after
//! each input, and displays values with propagation diffs.

mod commands;
mod format;
mod graph;
mod highlight;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use rustyline::Editor;
use rustyline::error::ReadlineError;

use graphcal_compiler::syntax::ast::DeclKind;
use graphcal_compiler::syntax::names::DeclName;
use graphcal_compiler::syntax::parser::Parser;
use graphcal_eval::builtins::builtin_constants;
use graphcal_eval::eval::{
    AssertResult, CompileError, EvalResult, NodeError, Value, compile_and_eval_from_project,
    compile_to_tir_from_project,
};
use graphcal_eval::loader::LoadedProject;
use graphcal_eval::registry::UnitScale;
use graphcal_eval::tir::TIR;
use graphcal_io::FileSystemReader as _;

use commands::{Command, HELP_TEXT, parse_command};
use format::{format_value_changed, format_value_line};

/// State of the interactive shell session.
struct ShellState {
    /// User-entered declarations: (name → source text) in entry order.
    user_decls: IndexMap<String, String>,
    /// Base file path (for overlay filesystem). `None` in standalone mode.
    base_path: Option<PathBuf>,
    /// Base file source (original content before user additions).
    base_source: Option<String>,
    /// Parameter overrides via `:set`.
    overrides: HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    /// Previous successful `EvalResult` (for propagation diff).
    prev_result: Option<EvalResult>,
    /// Previous successful TIR (for dependency info, `:graph`, `:type`).
    prev_tir: Option<TIR>,
}

impl ShellState {
    fn new() -> Self {
        Self {
            user_decls: IndexMap::new(),
            base_path: None,
            base_source: None,
            overrides: HashMap::new(),
            prev_result: None,
            prev_tir: None,
        }
    }

    /// Build the full source text from base source + user declarations.
    fn build_full_source(&self) -> String {
        let mut source = self.base_source.clone().unwrap_or_default();
        for decl_text in self.user_decls.values() {
            source.push('\n');
            source.push_str(decl_text);
        }
        source
    }

    /// Recompile and re-evaluate the full source.
    #[expect(
        clippy::result_large_err,
        reason = "CompileError size is fixed by the eval crate"
    )]
    fn recompile(&self) -> Result<(EvalResult, TIR), CompileError> {
        let full_source = self.build_full_source();
        if let Some(base_path) = &self.base_path {
            let base_fs = graphcal_io::RealFileSystem;
            let canonical = base_fs.canonicalize(base_path).map_err(|_| {
                CompileError::Eval(graphcal_eval::error::GraphcalError::FileNotFound {
                    path: base_path.display().to_string(),
                })
            })?;
            let fs = graphcal_io::OverlayFileSystem::new(base_fs, canonical, full_source);
            let project = graphcal_eval::loader::load_project(base_path, None, &fs)?;
            let tir = compile_to_tir_from_project(&project)?;
            let result = compile_and_eval_from_project(&project, &self.overrides, true)?;
            Ok((result, tir))
        } else {
            let project = LoadedProject::from_source(&full_source, "<repl>")?;
            let tir = compile_to_tir_from_project(&project)?;
            let result = compile_and_eval_from_project(&project, &self.overrides, true)?;
            Ok((result, tir))
        }
    }

    /// Get the set of all known declaration names (from file + user + prelude).
    fn all_known_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        if let Some(result) = &self.prev_result {
            for (name, _, _) in &result.all {
                names.insert(name.as_str().to_string());
            }
        }
        names
    }

    /// Check if a name is user-defined (entered in the REPL).
    fn is_user_defined(&self, name: &str) -> bool {
        self.user_decls.contains_key(name)
    }
}

/// Run the interactive shell.
///
/// If `file` is provided, it is loaded as the base project (like `python -i`).
/// `set_overrides` and `input_overrides` are applied to the loaded file.
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
pub fn run_shell(
    file: Option<&Path>,
    overrides: HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
) {
    let mut state = ShellState::new();
    state.overrides = overrides;

    // If a file was given, load it as the base.
    if let Some(file_path) = file {
        match load_base_file(&mut state, file_path) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("{:?}", miette::Report::new(e));
                return;
            }
        }
    }

    let mut editor = match Editor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: failed to initialize editor: {e}");
            return;
        }
    };
    editor.set_helper(Some(highlight::ShellHelper));

    let prompt = "graphcal> ";
    let continuation_prompt = "    ...> ";

    loop {
        let line = match editor.readline(prompt) {
            Ok(line) => line,
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("error: {e}");
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let _ = editor.add_history_entry(&line);

        // Classify the input.
        if let Some(cmd_str) = trimmed.strip_prefix(':') {
            handle_command(cmd_str, &mut state);
            continue;
        }

        // Check if it looks like a declaration.
        if is_declaration_keyword(trimmed) {
            // Try to parse. If it fails with unexpected EOF, try multi-line.
            let mut input = line.clone();
            loop {
                match try_add_declaration(&input, &mut state) {
                    DeclResult::Ok => break,
                    DeclResult::Error(e) => {
                        eprintln!("  error: {e}");
                        break;
                    }
                    DeclResult::Incomplete => {
                        // Multi-line: read continuation.
                        match editor.readline(continuation_prompt) {
                            Ok(cont_line) => {
                                if cont_line.trim().is_empty() {
                                    eprintln!("  (cancelled)");
                                    break;
                                }
                                input.push('\n');
                                input.push_str(&cont_line);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
            continue;
        }

        // Try as a name query (bare identifier) or a compound expression
        // (unit like `m/s` or dimension like `Length / Time`).
        let query = trimmed.trim_end_matches(';');
        if is_valid_identifier(query) {
            handle_name_query(query, &state);
        } else if looks_like_compound_expr(query) {
            handle_compound_query(query, &state);
        } else {
            eprintln!("  error: unrecognized input. Enter a declaration, a name, or :help");
        }
    }
}

/// Check if the input starts with a declaration keyword.
fn is_declaration_keyword(input: &str) -> bool {
    let first_word = input.split_whitespace().next().unwrap_or("");
    // Also handle attribute prefix
    if first_word.starts_with("#[") {
        return true;
    }
    matches!(
        first_word,
        "param"
            | "node"
            | "const"
            | "dimension"
            | "unit"
            | "index"
            | "type"
            | "fn"
            | "assert"
            | "import"
    )
}

/// Check if a string is a valid Graphcal identifier (for name queries).
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Allow qualified names like rocket::delta_v
    s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
        && s.starts_with(|c: char| c.is_alphabetic() || c == '_')
}

/// The result of trying to add a declaration.
enum DeclResult {
    Ok,
    Error(String),
    Incomplete,
}

/// Try to add a declaration to the state.
///
/// If parsing fails with "unexpected end of file" and the input doesn't end
/// with `;`, we auto-append `;` and retry — making the trailing semicolon
/// optional in the REPL. If that still fails, we report `Incomplete` so the
/// shell can prompt for continuation lines.
fn try_add_declaration(input: &str, state: &mut ShellState) -> DeclResult {
    // Parse as a file to validate syntax.
    // If the input is missing a trailing `;`, auto-append one and retry.
    let (ast, source) = match graphcal_compiler::syntax::parser::Parser::new(input).parse_file() {
        Ok(ast) => (ast, input.to_string()),
        Err(e) => {
            if matches!(
                e,
                graphcal_compiler::syntax::parser::ParseError::UnexpectedEof { .. }
            ) {
                // Auto-append `;` and retry — makes semicolons optional in the REPL.
                if input.trim_end().ends_with(';') {
                    return DeclResult::Incomplete;
                }
                let with_semi = format!("{input};");
                match graphcal_compiler::syntax::parser::Parser::new(&with_semi).parse_file() {
                    Ok(ast) => (ast, with_semi),
                    Err(_) => return DeclResult::Incomplete,
                }
            } else {
                return DeclResult::Error(format!("{e}"));
            }
        }
    };

    if ast.declarations.is_empty() {
        return DeclResult::Error("no declaration found".to_string());
    }

    // Extract the declared name.
    let decl = &ast.declarations[0];
    let Some(decl_name) = extract_decl_name(&decl.kind) else {
        // For declarations without a name we can extract (like import),
        // just use the raw input as the key.
        let key = format!("__import_{}", state.user_decls.len());
        return try_add_with_key(&key, &source, state);
    };

    // Check for duplicate names.
    let known = state.all_known_names();
    if known.contains(&decl_name) && !state.is_user_defined(&decl_name) {
        return DeclResult::Error(format!(
            "name `{decl_name}` already exists (from loaded file). Use :set to override params."
        ));
    }
    if state.is_user_defined(&decl_name) {
        return DeclResult::Error(format!(
            "name `{decl_name}` already defined. Use :remove first, then re-enter."
        ));
    }

    try_add_with_key(&decl_name, &source, state)
}

/// Add a declaration to the state with a given key.
fn try_add_with_key(key: &str, input: &str, state: &mut ShellState) -> DeclResult {
    // Save the old state for rollback.
    let old_decls = state.user_decls.clone();

    state.user_decls.insert(key.to_string(), input.to_string());

    match state.recompile() {
        Ok((result, tir)) => {
            // Show the new/changed values.
            print_propagation(key, &result, state.prev_result.as_ref());
            print_assertion_failures(&result, state.prev_result.as_ref());
            state.prev_result = Some(result);
            state.prev_tir = Some(tir);
            DeclResult::Ok
        }
        Err(e) => {
            // Rollback.
            state.user_decls = old_decls;
            DeclResult::Error(format!("{:?}", miette::Report::new(e)))
        }
    }
}

/// Extract the declaration name from a `DeclKind`.
fn extract_decl_name(kind: &DeclKind) -> Option<String> {
    match kind {
        DeclKind::Param(p) => Some(p.name.value.as_str().to_string()),
        DeclKind::Node(n) => Some(n.name.value.as_str().to_string()),
        DeclKind::ConstNode(c) => Some(c.name.value.as_str().to_string()),
        DeclKind::BaseDimension(d) => Some(d.name.value.as_str().to_string()),
        DeclKind::Dimension(d) => Some(d.name.value.as_str().to_string()),
        DeclKind::Unit(u) => Some(u.name.value.as_str().to_string()),
        DeclKind::Index(i) => Some(i.name.value.as_str().to_string()),
        DeclKind::Type(t) => Some(t.name.value.as_str().to_string()),
        DeclKind::UnionType(u) => Some(u.name.value.as_str().to_string()),
        DeclKind::Assert(a) => Some(a.name.value.as_str().to_string()),
        DeclKind::Plot(p) => Some(p.name.value.as_str().to_string()),
        DeclKind::Figure(f) => Some(f.name.value.as_str().to_string()),
        DeclKind::Layer(l) => Some(l.name.value.as_str().to_string()),
        DeclKind::Dag(d) => Some(d.name.value.as_str().to_string()),
        DeclKind::Import(_) | DeclKind::Include(_) => None,
    }
}

/// Print propagation: show new and changed values.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
fn print_propagation(primary_name: &str, new_result: &EvalResult, old_result: Option<&EvalResult>) {
    let symbols = &new_result.base_dim_symbols;

    // Build a map of old values for comparison.
    let old_values: HashMap<&str, &Result<Value, NodeError>> = old_result
        .map(|r| r.all.iter().map(|(n, v, _)| (n.as_str(), v)).collect())
        .unwrap_or_default();

    for (name, value_result, _) in &new_result.all {
        let name_str = name.as_str();
        match value_result {
            Ok(value) => {
                if name_str == primary_name {
                    // Primary declaration: always show.
                    if let Some(Ok(old_val)) = old_values.get(name_str) {
                        // It was redefined (shouldn't happen per our rules, but handle gracefully)
                        if let Some(line) =
                            format_value_changed(name_str, value, old_val, symbols, true)
                        {
                            println!("{line}");
                        } else {
                            println!("{}", format_value_line(name_str, value, symbols));
                        }
                    } else {
                        // New declaration.
                        println!("{}", format_value_line(name_str, value, symbols));
                    }
                } else if let Some(Ok(old_val)) = old_values.get(name_str) {
                    // Existing declaration: show only if value changed.
                    if let Some(line) =
                        format_value_changed(name_str, value, old_val, symbols, false)
                    {
                        println!("{line}");
                    }
                } else {
                    // New but not primary (e.g., const from import) — skip.
                }
            }
            Err(err) => {
                if name_str == primary_name {
                    println!("  {name_str} = ERROR: {err}");
                }
            }
        }
    }
}

/// Print assertion failures (only newly failing ones).
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn print_assertion_failures(new_result: &EvalResult, old_result: Option<&EvalResult>) {
    let old_failures: HashSet<&str> = old_result
        .map(|r| {
            r.assertions
                .iter()
                .filter(|(_, res, _)| matches!(res, AssertResult::Fail { .. }))
                .map(|(n, _, _)| n.as_str())
                .collect()
        })
        .unwrap_or_default();

    for (name, result, _) in &new_result.assertions {
        match result {
            AssertResult::Fail { message } if !old_failures.contains(name.as_str()) => {
                eprintln!("  ASSERTION FAILED: {} ({message})", name.as_str());
            }
            AssertResult::Error { message } => {
                eprintln!("  ASSERTION ERROR: {} ({message})", name.as_str());
            }
            _ => {}
        }
    }
}

/// Handle a `:command`.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_command(cmd_str: &str, state: &mut ShellState) {
    match parse_command(cmd_str) {
        Command::Help => {
            println!("{HELP_TEXT}");
        }
        Command::Quit => {
            std::process::exit(0);
        }
        Command::Clear => {
            state.user_decls.clear();
            state.overrides.clear();
            if state.base_source.is_some() {
                // Re-evaluate with just the base file.
                match state.recompile() {
                    Ok((result, tir)) => {
                        println!("  Cleared all user declarations and overrides.");
                        state.prev_result = Some(result);
                        state.prev_tir = Some(tir);
                    }
                    Err(e) => {
                        eprintln!("  error: {:?}", miette::Report::new(e));
                    }
                }
            } else {
                state.prev_result = None;
                state.prev_tir = None;
                println!("  Cleared all declarations.");
            }
        }
        Command::List => {
            handle_list(state);
        }
        Command::Set { name, expr_str } => {
            handle_set(name, expr_str, state);
        }
        Command::ClearSet { name } => {
            handle_clear_set(name, state);
        }
        Command::Remove { name, cascade } => {
            handle_remove(name, cascade, state);
        }
        Command::Graph => {
            if let Some(tir) = &state.prev_tir {
                println!("{}", graph::render_graph(tir));
            } else {
                println!("  (empty graph)");
            }
        }
        Command::Type { name } => {
            handle_type(name, state);
        }
        Command::Unknown(cmd) => {
            eprintln!("  unknown command: :{cmd}. Type :help for available commands.");
        }
    }
}

/// Handle `:list` command.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
fn handle_list(state: &ShellState) {
    let Some(result) = &state.prev_result else {
        println!("  (no declarations)");
        return;
    };
    let symbols = &result.base_dim_symbols;

    for (name, value_result, _) in &result.all {
        match value_result {
            Ok(value) => {
                println!("{}", format_value_line(name.as_str(), value, symbols));
            }
            Err(err) => {
                println!("  {} = ERROR: {err}", name.as_str());
            }
        }
    }

    // Show overrides.
    if !state.overrides.is_empty() {
        println!();
        println!("  Active overrides:");
        for name in state.overrides.keys() {
            println!("    :set {}", name.as_str());
        }
    }
}

/// Handle `:set param = expr` command.
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_set(name: &str, expr_str: &str, state: &mut ShellState) {
    // Parse the expression.
    let expr = match graphcal_compiler::syntax::parser::Parser::new(expr_str).parse_single_expr() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("  error: failed to parse expression: {e}");
            return;
        }
    };

    let decl_name = DeclName::new(name);
    let old_overrides = state.overrides.clone();
    state.overrides.insert(decl_name, expr);

    match state.recompile() {
        Ok((result, tir)) => {
            print_propagation(name, &result, state.prev_result.as_ref());
            print_assertion_failures(&result, state.prev_result.as_ref());
            state.prev_result = Some(result);
            state.prev_tir = Some(tir);
        }
        Err(e) => {
            state.overrides = old_overrides;
            eprintln!("  error: {:?}", miette::Report::new(e));
        }
    }
}

/// Handle `:clear-set` command.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_clear_set(name: Option<&str>, state: &mut ShellState) {
    if let Some(name) = name {
        let decl_name = DeclName::new(name);
        if state.overrides.remove(&decl_name).is_none() {
            eprintln!("  no override for `{name}`");
            return;
        }
    } else {
        if state.overrides.is_empty() {
            println!("  (no active overrides)");
            return;
        }
        state.overrides.clear();
    }

    match state.recompile() {
        Ok((result, tir)) => {
            let cleared = name.unwrap_or("all");
            println!("  Cleared override: {cleared}");
            print_propagation("", &result, state.prev_result.as_ref());
            state.prev_result = Some(result);
            state.prev_tir = Some(tir);
        }
        Err(e) => {
            eprintln!("  error: {:?}", miette::Report::new(e));
        }
    }
}

/// Handle `:remove` command.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_remove(name: &str, cascade: bool, state: &mut ShellState) {
    if !state.is_user_defined(name) {
        eprintln!(
            "  error: `{name}` is not a user-defined declaration. Only user-entered declarations can be removed."
        );
        return;
    }

    if let Some(tir) = &state.prev_tir {
        let dependents = graph::direct_dependents(tir, name);
        if !dependents.is_empty() && !cascade {
            let dep_list: Vec<&str> = dependents.iter().map(String::as_str).collect();
            eprintln!(
                "  error: `{name}` has dependents: {}. Use :remove {name}+ to remove with dependents.",
                dep_list.join(", ")
            );
            return;
        }

        if cascade {
            let all_deps = graph::transitive_dependents(tir, name);
            let mut removed = vec![name.to_string()];
            for dep in &all_deps {
                if state.is_user_defined(dep) {
                    state.user_decls.shift_remove(dep.as_str());
                    removed.push(dep.clone());
                }
            }
            state.user_decls.shift_remove(name);
            println!("  Removed: {}", removed.join(", "));
        } else {
            state.user_decls.shift_remove(name);
            println!("  Removed: {name}");
        }
    } else {
        state.user_decls.shift_remove(name);
        println!("  Removed: {name}");
    }

    // Recompile after removal.
    if state.user_decls.is_empty() && state.base_source.is_none() {
        state.prev_result = None;
        state.prev_tir = None;
    } else {
        match state.recompile() {
            Ok((result, tir)) => {
                state.prev_result = Some(result);
                state.prev_tir = Some(tir);
            }
            Err(e) => {
                eprintln!("  error after removal: {:?}", miette::Report::new(e));
            }
        }
    }
}

/// Handle `:type name` command.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_type(name: &str, state: &ShellState) {
    let Some(tir) = &state.prev_tir else {
        eprintln!("  (no declarations)");
        return;
    };

    // Look up in resolved_decl_types.
    let name_scoped = graphcal_eval::resolve::ScopedName::local(name);
    if let Some(resolved_type) = tir.resolved_decl_types.get(&name_scoped) {
        println!("  {name}: {resolved_type:?}");
    } else {
        eprintln!("  error: `{name}` not found");
    }
}

/// Handle a bare name query.
///
/// Lookup order: declared values → builtin constants → units → dimensions.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_name_query(name: &str, state: &ShellState) {
    // 1. Search in declared values.
    if let Some(result) = &state.prev_result {
        let found = result.all.iter().find(|(n, _, _)| n.as_str() == name);
        match found {
            Some((_, Ok(value), _)) => {
                println!(
                    "{}",
                    format_value_line(name, value, &result.base_dim_symbols)
                );
                return;
            }
            Some((_, Err(err), _)) => {
                println!("  {name} = ERROR: {err}");
                return;
            }
            None => {}
        }
    }

    // 2. Builtin constants (PI, E, TAU, etc.)
    let constants = builtin_constants();
    if let Some(&value) = constants.get(name) {
        println!("  {name} = {value} (builtin constant, Dimensionless)");
        return;
    }

    // 3. Units (from TIR registry).
    if let Some(tir) = &state.prev_tir
        && let Some(info) = tir.registry.units.get_unit(name)
    {
        let dim_str = tir.registry.dimensions.format_dimension(&info.dimension);
        match &info.scale {
            UnitScale::Static(s) => {
                println!("  {name}: unit of {dim_str} (scale: {s})");
            }
            UnitScale::Dynamic { .. } => {
                println!("  {name}: unit of {dim_str} (dynamic scale)");
            }
        }
        return;
    }

    // 4. Dimensions (from TIR registry).
    if let Some(tir) = &state.prev_tir
        && let Some(dim) = tir.registry.dimensions.get_dimension(name)
    {
        let formatted = tir.registry.dimensions.format_dimension(dim);
        if formatted == name {
            // Base dimension — just confirm it exists.
            println!("  {name}: dimension (base)");
        } else {
            // Derived dimension — show its expansion.
            println!("  {name} = {formatted} (dimension)");
        }
        return;
    }

    eprintln!("  error: `{name}` not found");
}

/// Check if input looks like a compound unit or dimension expression
/// (contains `/`, `*`, or `^` operators between identifiers).
fn looks_like_compound_expr(s: &str) -> bool {
    // Must contain at least one operator and consist of identifiers + operators + whitespace
    let has_operator = s.contains('/') || s.contains('*') || s.contains('^');
    if !has_operator {
        return false;
    }
    // All chars should be alphanumeric, underscore, whitespace, or operators
    s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '/' || c == '*' || c == '^' || c == ' ')
}

/// Handle a compound expression query (e.g., `m/s`, `Length / Time`).
///
/// Tries to parse as a unit expression first, then as a dimension expression.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(clippy::print_stderr, reason = "interactive shell error output")]
fn handle_compound_query(input: &str, state: &ShellState) {
    let Some(tir) = &state.prev_tir else {
        eprintln!("  error: no declarations loaded (cannot resolve units/dimensions)");
        return;
    };

    // Try as a unit expression (e.g., `m/s`, `kg * m / s^2`).
    if let Ok(unit_expr) = Parser::new(input).parse_standalone_unit_expr()
        && let Some(dim) = tir.registry.units.resolve_unit_dimension(&unit_expr)
    {
        let dim_str = tir.registry.dimensions.format_dimension(&dim);
        // Try to get a static scale; if any unit is dynamic, show "dynamic"
        if let Some((_dim, scale)) = tir.registry.units.resolve_unit_expr(&unit_expr) {
            println!("  {input}: unit of {dim_str} (scale: {scale})");
        } else {
            println!("  {input}: unit of {dim_str} (scale: dynamic)");
        }
        return;
    }

    // Try as a dimension expression (e.g., `Length / Time`).
    if let Ok(dim_expr) = Parser::new(input).parse_standalone_dim_expr()
        && let Some(dim) = tir.registry.dimensions.resolve_dim_expr(&dim_expr)
    {
        let formatted = tir.registry.dimensions.format_dimension(&dim);
        println!("  {input} = {formatted} (dimension)");
        return;
    }

    eprintln!("  error: `{input}` is not a recognized unit or dimension expression");
}

/// Load a base file and evaluate it.
#[expect(clippy::print_stdout, reason = "interactive shell output")]
#[expect(
    clippy::result_large_err,
    reason = "CompileError size is fixed by the eval crate"
)]
fn load_base_file(state: &mut ShellState, file_path: &Path) -> Result<(), CompileError> {
    let canonical = file_path.canonicalize().map_err(|_| {
        CompileError::Eval(graphcal_eval::error::GraphcalError::FileNotFound {
            path: file_path.display().to_string(),
        })
    })?;

    let source = std::fs::read_to_string(&canonical).map_err(|_| {
        CompileError::Eval(graphcal_eval::error::GraphcalError::FileNotFound {
            path: canonical.display().to_string(),
        })
    })?;

    state.base_path = Some(canonical);
    state.base_source = Some(source);

    let (result, tir) = state.recompile()?;

    // Display all values.
    let symbols = &result.base_dim_symbols;
    for (name, value_result, _) in &result.all {
        match value_result {
            Ok(value) => {
                println!("{}", format_value_line(name.as_str(), value, symbols));
            }
            Err(err) => {
                println!("  {} = ERROR: {err}", name.as_str());
            }
        }
    }

    // Show assertion results.
    for (name, assert_result, _) in &result.assertions {
        if let AssertResult::Fail { message } = assert_result {
            println!("  ASSERTION FAILED: {} ({message})", name.as_str());
        }
    }

    let param_count = result.params.len();
    let node_count = result.nodes.len();
    let const_count = result.consts.len();
    println!();
    println!("  Loaded: {param_count} param(s), {node_count} node(s), {const_count} const(s)");

    state.prev_result = Some(result);
    state.prev_tir = Some(tir);
    Ok(())
}
