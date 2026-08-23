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
use graphcal_report::report_hydrate::{EngineBundle, Hydration, HydrationProject};
use graphcal_report::report_ir::{
    Provenance, ReportBuildError, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use graphcal_report::report_markdown::render_report_markdown;

use crate::overrides::{JsonOverrideSource, ParameterArgs, ParameterJsonSource, ParsedOverrides};

/// Default engine bundle location, produced by `just wasm-report`.
const DEFAULT_ENGINE_DIR: &str = "target/wasm-report/pkg";
/// Environment variable overriding the engine bundle location.
const ENGINE_DIR_ENV: &str = "GRAPHCAL_REPORT_ENGINE_DIR";

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
    #[command(flatten)]
    pub parameters: ParameterArgs,
    /// Project root directory (overrides automatic graphcal.toml detection)
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Build a non-interactive report: no embedded engine, no controls
    #[arg(long = "static")]
    pub static_only: bool,
    /// Directory holding the browser engine bundle (`graphcal_wasm.js` and
    /// `graphcal_wasm_bg.wasm` from `wasm-pack --target no-modules`).
    /// Defaults to `$GRAPHCAL_REPORT_ENGINE_DIR`, then `target/wasm-report/pkg`
    #[arg(long, value_name = "DIR")]
    pub engine_dir: Option<PathBuf>,
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
    #[error(
        "this model cannot hydrate in the browser engine: {reason}\n\
         pass --static to build a non-interactive report of the same baseline"
    )]
    HydrationUnsupported { reason: String },
    #[error(
        "the browser engine bundle was not found at {path}\n\
         build it with `just wasm-report` (requires wasm-pack), point \
         --engine-dir (or ${ENGINE_DIR_ENV}) at a `wasm-pack --target \
         no-modules` build of crates/graphcal-wasm, or pass --static to \
         build a non-interactive report"
    )]
    EngineMissing { path: PathBuf },
    #[error("could not read the engine bundle file {path}: {source}")]
    EngineRead {
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
    validate_hydration_parameter_source(args, overrides)?;
    let fs = build_rooted_filesystem(&args.file, args.root.as_deref())?;
    let (project, host_fns) =
        crate::load_project_with_plugins(&args.file, args.root.as_deref(), &fs)?;
    let prepared = prepare_from_project_with_host_fns(&project, &host_fns)?;

    let mut bindings = prepared.binding_builder();
    for (name, expression) in &overrides.values {
        match overrides.json_sources.get(name) {
            Some(JsonOverrideSource { source, span }) => {
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
        repro_command: repro_command(args, overrides),
    };
    let document = build_report(ReportInputs {
        title: &title,
        result: &result,
        docs: &docs,
        provenance,
    })?;

    let engine = if args.static_only {
        None
    } else {
        Some(load_engine_bundle(args)?)
    };
    let hydration = match &engine {
        None => None,
        Some((glue_js, wasm)) => Some(Hydration {
            engine: EngineBundle { glue_js, wasm },
            project: hydration_project(&project, args, &fs)?,
            baseline_bindings: baseline_binding_strings(overrides),
        }),
    };

    let html = render_report_html(&document, VegaScriptSource::Inline, hydration.as_ref());
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

/// The canonicalized project root used to name sources in provenance and
/// hydration payloads: the explicit `--root`, else the discovered
/// `graphcal.toml` directory, else (for a loose file) the file's parent —
/// the same rule the loader's sandbox uses.
fn project_root_dir(
    entry: &Path,
    root_override: Option<&Path>,
    fs: &graphcal_io::RealFileSystem,
) -> Option<PathBuf> {
    root_override
        .map(PathBuf::from)
        .or_else(|| {
            let entry = entry.canonicalize().ok()?;
            let parent = entry.parent()?;
            discover_project_root(parent, fs).or_else(|| Some(parent.to_path_buf()))
        })
        .and_then(|root| root.canonicalize().ok())
}

/// A loaded file's project-relative name; `None` when it does not live under
/// the project root. Components join with `/` on every platform: the
/// hydration payload feeds the browser engine's virtual filesystem, which
/// requires forward-slash relative paths, and provenance names stay
/// OS-independent.
fn relative_source_name(path: &Path, root_dir: Option<&Path>) -> Option<String> {
    root_dir
        .and_then(|root| path.strip_prefix(root).ok())
        .map(|relative| {
            relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
}

/// SHA-256 digests of every loaded source, named relative to the project
/// root and sorted by name so the artifact is machine-independent.
fn source_digests(
    project: &LoadedProject,
    entry: &Path,
    root_override: Option<&Path>,
    fs: &graphcal_io::RealFileSystem,
) -> Vec<SourceDigest> {
    let root_dir = project_root_dir(entry, root_override, fs);
    let mut digests: Vec<SourceDigest> = project
        .files()
        .values()
        .map(|file| {
            let path = file.path();
            let name = relative_source_name(path, root_dir.as_deref()).unwrap_or_else(|| {
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            });
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

/// Locate and read the browser engine bundle (no-modules glue + wasm).
fn load_engine_bundle(args: &BuildArgs) -> Result<(String, Vec<u8>), ReportError> {
    let dir = args
        .engine_dir
        .clone()
        .or_else(|| std::env::var_os(ENGINE_DIR_ENV).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ENGINE_DIR));
    let glue_path = dir.join("graphcal_wasm.js");
    let wasm_path = dir.join("graphcal_wasm_bg.wasm");
    if !glue_path.is_file() || !wasm_path.is_file() {
        return Err(ReportError::EngineMissing { path: dir });
    }
    let glue = std::fs::read_to_string(&glue_path).map_err(|source| ReportError::EngineRead {
        path: glue_path,
        source,
    })?;
    let wasm = std::fs::read(&wasm_path).map_err(|source| ReportError::EngineRead {
        path: wasm_path,
        source,
    })?;
    Ok((glue, wasm))
}

fn validate_hydration_parameter_source(
    args: &BuildArgs,
    overrides: &ParsedOverrides,
) -> Result<(), ReportError> {
    if !args.static_only && overrides.json_source.is_some() {
        return Err(ReportError::HydrationUnsupported {
            reason: "JSON parameter baselines cannot be replayed in the browser; use --param"
                .to_string(),
        });
    }
    Ok(())
}

/// Assemble the in-memory project shipped to the browser engine, rejecting
/// everything the engine cannot run — loudly, at build time.
fn hydration_project(
    project: &LoadedProject,
    args: &BuildArgs,
    fs: &graphcal_io::RealFileSystem,
) -> Result<HydrationProject, ReportError> {
    if project
        .files()
        .values()
        .any(|file| file.ast().uses_plugins())
    {
        return Err(ReportError::HydrationUnsupported {
            reason: "the project imports plugins, which cannot run in the browser engine"
                .to_string(),
        });
    }

    let root_dir = project_root_dir(&args.file, args.root.as_deref(), fs);

    // The manifest drives module resolution in the browser exactly as on
    // disk, so ship it when the project has one — but dependency-carrying
    // projects are gated FIRST: locked-and-cached packages load from outside
    // the project root, and the out-of-root diagnostic below would otherwise
    // mask the actual reason hydration cannot work.
    let manifest = root_dir
        .as_deref()
        .and_then(|root| std::fs::read_to_string(root.join("graphcal.toml")).ok());
    if let Some(content) = &manifest {
        match graphcal_package::parse_manifest_str(content) {
            Ok(parsed) if !parsed.dependencies.is_empty() => {
                return Err(ReportError::HydrationUnsupported {
                    reason: "the project declares package dependencies, which the browser \
                             engine cannot fetch"
                        .to_string(),
                });
            }
            Ok(_) | Err(_) => {}
        }
    }

    let entry_name = args
        .file
        .canonicalize()
        .ok()
        .and_then(|entry| relative_source_name(&entry, root_dir.as_deref()));
    let mut files = Vec::new();
    for file in project.files().values() {
        let Some(name) = relative_source_name(file.path(), root_dir.as_deref()) else {
            return Err(ReportError::HydrationUnsupported {
                reason: format!(
                    "source {} lives outside the project root and cannot ship in the artifact",
                    file.path().display()
                ),
            });
        };
        files.push((name, file.source().to_string()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(content) = manifest {
        files.push(("graphcal.toml".to_string(), content));
    }

    let entry = entry_name
        .filter(|name| files.iter().any(|(candidate, _)| candidate == name))
        .ok_or_else(|| ReportError::HydrationUnsupported {
            reason: "could not resolve the entry file inside the project root".to_string(),
        })?;
    Ok(HydrationProject { entry, files })
}

/// Baseline `--param` bindings replayed as the hydrated report's initial
/// control values.
fn baseline_binding_strings(overrides: &ParsedOverrides) -> Vec<(String, String)> {
    overrides
        .direct_parameters
        .iter()
        .map(|binding| (binding.name.to_string(), binding.expression.clone()))
        .collect()
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
fn repro_command(args: &BuildArgs, overrides: &ParsedOverrides) -> String {
    let mut parts = vec![
        "graphcal".to_string(),
        "eval".to_string(),
        shell_quote(&args.file.display().to_string()),
    ];
    if let Some(root) = &args.root {
        parts.push("--root".to_string());
        parts.push(shell_quote(&root.display().to_string()));
    }
    for binding in &overrides.direct_parameters {
        parts.push("--param".to_string());
        parts.push(shell_quote(&format!(
            "{}={}",
            binding.name, binding.expression
        )));
    }
    match &overrides.json_source {
        Some(ParameterJsonSource::Inline(json)) => {
            parts.push("--params-json".to_string());
            parts.push(shell_quote(json));
        }
        Some(ParameterJsonSource::File(path)) => {
            parts.push("--params-json-file".to_string());
            parts.push(shell_quote(&path.display().to_string()));
        }
        Some(ParameterJsonSource::Stdin) => {
            parts.push("--params-json-file".to_string());
            parts.push("-".to_string());
        }
        None => {}
    }
    if let Some(limit) = overrides.json_max_bytes {
        parts.push("--params-json-max-bytes".to_string());
        parts.push(limit.to_string());
    }
    parts.join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quote_handles_plain_text_and_single_quotes() {
        assert_eq!(shell_quote("model.gcl"), "'model.gcl'");
        assert_eq!(shell_quote("scenario's.json"), "'scenario'\"'\"'s.json'");
    }
}
