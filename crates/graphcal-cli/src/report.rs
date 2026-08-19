//! `graphcal report` — build shareable auto-report artifacts (experimental).
//!
//! The auto-report needs no authoring: controls, value cards, grids, plots,
//! checks, and captions are all derived from what the model already declares
//! (#1410). This module is the imperative shell; document assembly and
//! rendering live in the `graphcal-report` crate.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use sha2::{Digest, Sha256};
use thiserror::Error;

use graphcal_eval::eval::{CompileError, EvalResult, prepare_from_project_with_host_fns};
use graphcal_eval::loader::{LoadedProject, build_rooted_filesystem, discover_project_root};
use graphcal_report::plot_page::VegaScriptSource;
use graphcal_report::report_html::render_report_html;
use graphcal_report::report_ir::{
    Provenance, ReportBuildError, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use graphcal_report::report_markdown::render_report_markdown;

use crate::overrides::{InputOverrideSource, ParsedOverrides};

#[derive(Subcommand)]
pub enum ReportCommands {
    /// Build a self-contained HTML report derived from the model alone
    Build(BuildArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    /// Path to the entry .gcl file
    pub file: PathBuf,
    /// Output HTML path (default: the model path with a .report.html extension)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Also write a deterministic Markdown rendering for CI diffing
    #[arg(long, value_name = "FILE.md")]
    pub markdown: Option<PathBuf>,
    /// Bind a param to a closed value: --set 'name=value'
    #[arg(long)]
    pub set: Vec<String>,
    /// JSON input file for param values
    #[arg(long)]
    pub input: Option<PathBuf>,
    /// Maximum size (in bytes) of the --input JSON file. Defaults to 1 MiB.
    #[arg(long)]
    pub input_max_bytes: Option<u64>,
    /// Project root directory (overrides automatic graphcal.toml detection)
    #[arg(long)]
    pub root: Option<PathBuf>,
}

/// Outcome the shell maps to an exit code.
pub enum ReportStatus {
    /// Artifacts written; evaluation was clean.
    Success,
    /// Artifacts written, but the evaluation contains node/assertion errors.
    ProgramErrors,
}

#[derive(Debug, Error)]
pub enum ReportError {
    #[error(transparent)]
    Compile(#[from] Box<CompileError>),
    #[error(transparent)]
    Build(#[from] ReportBuildError),
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl From<CompileError> for ReportError {
    fn from(error: CompileError) -> Self {
        Self::Compile(Box::new(error))
    }
}

/// Build the report artifacts for one model.
///
/// The HTML (and optional Markdown) file is written even when the evaluation
/// contains per-node failures — error chips are part of the report — but the
/// returned status still signals them for a non-zero exit.
pub fn run_build(
    args: &BuildArgs,
    overrides: &ParsedOverrides,
    compiler_version: &str,
) -> Result<ReportStatus, ReportError> {
    let fs = build_rooted_filesystem(&args.file, args.root.as_deref())?;
    let (project, host_fns) =
        crate::load_project_with_plugins(&args.file, args.root.as_deref(), &fs)?;
    let prepared = prepare_from_project_with_host_fns(&project, &host_fns)?;

    let mut bindings = prepared.binding_builder();
    for (name, expression) in &overrides.values {
        match overrides.input_sources.get(name) {
            Some(InputOverrideSource { source, span }) => {
                bindings.bind_external_expression(name, expression, source, *span)?;
            }
            None => bindings.bind_expression(name, expression)?,
        }
    }
    let result = prepared.evaluate(&bindings.finish()?)?;

    let docs = collect_doc_captions(project.root_file().ast());
    let title = args.file.file_stem().map_or_else(
        || "report".to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let provenance = Provenance {
        compiler_version: compiler_version.to_string(),
        sources: source_digests(&project, &args.file, args.root.as_deref(), &fs),
        baseline_params: baseline_params(&result),
        repro_command: repro_command(args),
    };
    let document = build_report(ReportInputs {
        title: &title,
        result: &result,
        docs: &docs,
        provenance,
    })?;

    let html = render_report_html(&document, VegaScriptSource::Inline);
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.file.with_extension("report.html"));
    std::fs::write(&output, html).map_err(|source| ReportError::Write {
        path: output.clone(),
        source,
    })?;
    eprintln!("wrote report to {}", output.display());

    if let Some(markdown_path) = &args.markdown {
        let markdown = render_report_markdown(&document);
        std::fs::write(markdown_path, markdown).map_err(|source| ReportError::Write {
            path: markdown_path.clone(),
            source,
        })?;
        eprintln!("wrote markdown report to {}", markdown_path.display());
    }

    for error in &result.plot_errors {
        eprintln!(
            "error: plot `{}` not rendered: {}",
            error.name, error.message
        );
    }

    if result.has_errors() {
        Ok(ReportStatus::ProgramErrors)
    } else {
        Ok(ReportStatus::Success)
    }
}

/// SHA-256 digests of every loaded source, named relative to the project
/// root and sorted by name so the artifact is machine-independent.
fn source_digests(
    project: &LoadedProject,
    entry: &Path,
    root_override: Option<&Path>,
    fs: &graphcal_io::RealFileSystem,
) -> Vec<SourceDigest> {
    let root_dir = root_override
        .map(PathBuf::from)
        .or_else(|| {
            entry
                .canonicalize()
                .ok()
                .and_then(|entry| discover_project_root(entry.parent()?, fs))
        })
        .and_then(|root| root.canonicalize().ok());
    let mut digests: Vec<SourceDigest> = project
        .files()
        .values()
        .map(|file| {
            let path = file.path();
            let name = root_dir
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok())
                .map_or_else(
                    || {
                        path.file_name().map_or_else(
                            || path.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                    },
                    |relative| relative.display().to_string(),
                );
            let mut hasher = Sha256::new();
            hasher.update(file.source().as_bytes());
            SourceDigest {
                name,
                sha256: hex_string(&hasher.finalize()),
            }
        })
        .collect();
    digests.sort_by(|a, b| a.name.cmp(&b.name));
    digests
}

/// Lowercase hexadecimal rendering of a digest.
fn hex_string(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Baseline parameter values as displayed, in declaration order.
fn baseline_params(result: &EvalResult) -> Vec<(String, String)> {
    result
        .output_params(graphcal_eval::eval::EvalOutputView::Surface)
        .map(|(name, outcome)| {
            let display = match outcome {
                Ok(value) => {
                    graphcal_report::value_display::scalar_display(value, &result.base_dim_symbols)
                        .unwrap_or_else(|error| format!("ERROR: {error}"))
                }
                Err(error) => format!("ERROR: {error}"),
            };
            (name.to_string(), display)
        })
        .collect()
}

/// The copy-pasteable command line reproducing this baseline evaluation.
fn repro_command(args: &BuildArgs) -> String {
    let mut parts = vec![
        "graphcal".to_string(),
        "eval".to_string(),
        args.file.display().to_string(),
    ];
    if let Some(root) = &args.root {
        parts.push("--root".to_string());
        parts.push(root.display().to_string());
    }
    for set in &args.set {
        parts.push("--set".to_string());
        parts.push(format!("'{set}'"));
    }
    if let Some(input) = &args.input {
        parts.push("--input".to_string());
        parts.push(input.display().to_string());
    }
    parts.join(" ")
}
