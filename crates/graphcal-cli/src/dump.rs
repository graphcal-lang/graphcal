//! Thin imperative shell for the experimental `graphcal dump` namespace.

use std::fmt::Debug;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Subcommand};
use miette::{Diagnostic, NamedSource};
use thiserror::Error;

use graphcal_compiler::diagnostic_render::RenderableDiagnostic;
use graphcal_compiler::syntax::ast::File;
use graphcal_compiler::syntax::lexer::tokenize;
use graphcal_compiler::syntax::parser::{ParseErrorKind, Parser};
use graphcal_io::{ByteLimit, FileSystemReadError, FileSystemReader, NeverCancel};
use graphcal_project::compile_error::CompileError;
use graphcal_project::loader::{build_rooted_filesystem, load_project};
use graphcal_project::prepare::{ParameterBindingRow, PreparedProject};
use graphcal_project::project_compiler::ProjectCompiler;

use crate::overrides::{
    OverrideParseError, ParameterArgs, ParsedOverrides, parse_overrides_with_sources,
};

const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// Nested `graphcal dump <STAGE>` commands.
#[derive(Subcommand)]
pub enum DumpCommands {
    /// Debug-print one physical source file
    Source(FileArgs),
    /// Debug-print lexer output for one physical source file
    Tokens(FileArgs),
    /// Debug-print the parser's surface-aware AST
    #[command(alias = "raw-ast")]
    Ast(FileArgs),
    /// Debug-print syntax after parser sugar has been removed
    #[command(alias = "desugared-ast")]
    Desugared(FileArgs),
    /// Debug-print the loaded project and canonical module resolver
    Modules(FileArgs),
    /// Debug-print the complete canonically resolved project
    Hir(FileArgs),
    /// Debug-print the fully checked typed program
    Tir(FileArgs),
    /// Debug-print the prepared execution plan without runtime evaluation
    Plan(FileArgs),
    /// Execute once and debug-print the evaluator's internal outcome
    Runtime(EvaluationArgs),
    /// Execute once and debug-print the normal public result
    Result(EvaluationArgs),
}

#[derive(Args)]
pub struct FileArgs {
    /// Path to the .gcl file
    file: PathBuf,
    /// Project root directory
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct EvaluationArgs {
    #[command(flatten)]
    project: FileArgs,
    #[command(flatten)]
    parameters: ParameterArgs,
}

/// Process status after a successful dump infrastructure run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpStatus {
    Success,
    ProgramErrors,
}

/// Failures that prevent construction or printing of the requested artifact.
#[derive(Debug, Error, Diagnostic)]
pub enum DumpError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(RenderableDiagnostic<ParseErrorKind>),
    #[error(transparent)]
    #[diagnostic(transparent)]
    Override(#[from] OverrideParseError),
    #[error("cannot read source file {}: {source}", path.display())]
    #[diagnostic(code(graphcal::dump::D001))]
    SourceRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "source file {} exceeds the dump safety limit of {limit} bytes",
        path.display()
    )]
    #[diagnostic(code(graphcal::dump::D002))]
    SourceTooLarge { path: PathBuf, limit: usize },
    #[error("source file {} is not valid UTF-8", path.display())]
    #[diagnostic(code(graphcal::dump::D003))]
    SourceEncoding { path: PathBuf },
    #[error("cannot construct the canonical module index: {source}")]
    #[diagnostic(code(graphcal::dump::D004))]
    ModuleIndex {
        #[source]
        source: graphcal_compiler::resolve::error::ModuleResolveError,
    },
    #[error("could not write dump output: {source}")]
    #[diagnostic(code(graphcal::dump::D005))]
    Output {
        #[source]
        source: std::io::Error,
    },
}

/// Run one nested dump command.
///
/// # Errors
///
/// Returns an input, compiler, evaluator, or output diagnostic.
pub fn run(command: DumpCommands) -> Result<DumpStatus, DumpError> {
    eprintln!(
        "warning: `graphcal dump` is experimental; its Rust Debug output may change without notice"
    );
    match command {
        DumpCommands::Source(args) => run_source(&args),
        DumpCommands::Tokens(args) => run_tokens(&args),
        DumpCommands::Ast(args) => run_ast(&args),
        DumpCommands::Desugared(args) => run_desugared(&args),
        DumpCommands::Modules(args) => run_modules(&args),
        DumpCommands::Hir(args) => run_hir(&args),
        DumpCommands::Tir(args) => run_tir(&args),
        DumpCommands::Plan(args) => run_plan(&args),
        DumpCommands::Runtime(args) => run_runtime(&args),
        DumpCommands::Result(args) => run_result(&args),
    }
}

fn run_source(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    write_debug(&read_source(&args.file, args.root.as_deref())?)?;
    Ok(DumpStatus::Success)
}

fn run_tokens(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let source = read_source(&args.file, args.root.as_deref())?;
    write_debug(&tokenize(&source.text))?;
    Ok(DumpStatus::Success)
}

fn run_ast(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let source = read_source(&args.file, args.root.as_deref())?;
    let artifact = parse_source(&source)?;
    write_debug(&artifact)?;
    Ok(DumpStatus::Success)
}

fn run_desugared(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let source = read_source(&args.file, args.root.as_deref())?;
    let raw = parse_source(&source)?;
    let artifact = graphcal_compiler::desugar::desugared_ast::File::from(raw);
    write_debug(&artifact)?;
    Ok(DumpStatus::Success)
}

fn run_modules(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let fs = build_rooted_filesystem(&args.file, args.root.as_deref())?;
    let project = load_project(&args.file, args.root.as_deref(), &fs)?;
    let resolver = project
        .build_module_resolver()
        .map_err(|source| DumpError::ModuleIndex { source })?;
    write_debug(&(project, resolver))?;
    Ok(DumpStatus::Success)
}

fn run_hir(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let fs = build_rooted_filesystem(&args.file, args.root.as_deref())?;
    let project = load_project(&args.file, args.root.as_deref(), &fs)?;
    let hir = ProjectCompiler::new(&project).lower()?;
    write_debug(&hir)?;
    Ok(DumpStatus::Success)
}

fn run_tir(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let fs = build_rooted_filesystem(&args.file, args.root.as_deref())?;
    let (project, host_fns) =
        crate::load_project_with_plugins(&args.file, args.root.as_deref(), &fs)?;
    let checked = ProjectCompiler::new(&project).host_fns(&host_fns).check()?;
    write_debug(checked.tir())?;
    Ok(DumpStatus::Success)
}

fn run_plan(args: &FileArgs) -> Result<DumpStatus, DumpError> {
    let prepared = prepare(&args.file, args.root.as_deref())?;
    write_debug(&prepared.debug_plan())?;
    Ok(DumpStatus::Success)
}

fn run_runtime(args: &EvaluationArgs) -> Result<DumpStatus, DumpError> {
    let prepared = prepare(&args.project.file, args.project.root.as_deref())?;
    let row = parse_bindings(&prepared, args)?;
    let outcome = prepared.evaluate_runtime(&row)?;
    let status = if outcome.has_errors() {
        DumpStatus::ProgramErrors
    } else {
        DumpStatus::Success
    };
    write_debug(&outcome)?;
    Ok(status)
}

fn run_result(args: &EvaluationArgs) -> Result<DumpStatus, DumpError> {
    let prepared = prepare(&args.project.file, args.project.root.as_deref())?;
    let row = parse_bindings(&prepared, args)?;
    let result = prepared.evaluate(&row)?;
    let status = if result.has_errors() {
        DumpStatus::ProgramErrors
    } else {
        DumpStatus::Success
    };
    write_debug(&result)?;
    Ok(status)
}

fn prepare(file: &Path, root: Option<&Path>) -> Result<PreparedProject, DumpError> {
    let fs = build_rooted_filesystem(file, root)?;
    let (project, host_fns) = crate::load_project_with_plugins(file, root, &fs)?;
    Ok(ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .prepare()?)
}

fn parse_bindings(
    prepared: &PreparedProject,
    args: &EvaluationArgs,
) -> Result<ParameterBindingRow, DumpError> {
    let overrides = parse_overrides_with_sources(&args.parameters)?;
    build_bindings(prepared, &overrides)
}

fn build_bindings(
    prepared: &PreparedProject,
    overrides: &ParsedOverrides,
) -> Result<ParameterBindingRow, DumpError> {
    let mut bindings = prepared.binding_builder();
    for (name, expression) in &overrides.values {
        match overrides.json_sources.get(name) {
            Some(source) => {
                bindings.bind_external_expression(name, expression, &source.source, source.span)?;
            }
            None => bindings.bind_expression(name, expression)?,
        }
    }
    Ok(bindings.finish()?)
}

#[derive(Debug)]
struct SourceUnit {
    path: PathBuf,
    text: String,
}

fn read_source(file: &Path, root: Option<&Path>) -> Result<SourceUnit, DumpError> {
    let fs = build_rooted_filesystem(file, root)?;
    let bytes = fs
        .read_bytes_bounded(file, ByteLimit::new(MAX_SOURCE_BYTES as u64), &NeverCancel)
        .map_err(|error| match error {
            FileSystemReadError::ByteLimitExceeded { .. } => DumpError::SourceTooLarge {
                path: file.to_path_buf(),
                limit: MAX_SOURCE_BYTES,
            },
            other => DumpError::SourceRead {
                path: file.to_path_buf(),
                source: std::io::Error::other(other),
            },
        })?;
    let text = String::from_utf8(bytes).map_err(|_| DumpError::SourceEncoding {
        path: file.to_path_buf(),
    })?;
    let path = fs.canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    Ok(SourceUnit { path, text })
}

/// Parse `source`, rendering a parse failure against the file it came from.
fn parse_source(source: &SourceUnit) -> Result<File, DumpError> {
    Parser::new(&source.text).parse_file().map_err(|error| {
        let named = NamedSource::new(source.path.to_string_lossy(), Arc::new(source.text.clone()));
        DumpError::Parse(RenderableDiagnostic::in_source(
            error.kind, error.span, named,
        ))
    })
}

fn write_debug<T: Debug + ?Sized>(value: &T) -> Result<(), DumpError> {
    writeln!(std::io::stdout().lock(), "{value:#?}").map_err(|source| DumpError::Output { source })
}
