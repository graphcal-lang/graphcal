//! `graphcal report` — build shareable auto-report artifacts (experimental).
//!
//! The auto-report needs no authoring: controls, value cards, grids, plots,
//! checks, and captions are all derived from what the model already declares
//! (#1410). This module is the imperative shell; document assembly and
//! rendering live in the `graphcal-report` crate.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use sha2::{Digest, Sha256};
use thiserror::Error;

use graphcal_eval::eval::{CompileError, EvalResult, ProjectCompiler};
use graphcal_eval::loader::{LoadedProject, build_rooted_filesystem, discover_project_root};
use graphcal_eval::project_bundle::{ArtifactContent, BundleArtifact, BundleError, ProjectBundle};
use graphcal_io::FileSystemReader as _;
use graphcal_report::plot_page::VegaScriptSource;
use graphcal_report::report_html::render_report_html;
use graphcal_report::report_hydrate::{EngineBundle, Hydration};
use graphcal_report::report_ir::{
    Provenance, ReportBuildError, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use graphcal_report::report_markdown::render_report_markdown;

use crate::overrides::{JsonOverrideSource, ParameterArgs, ParameterJsonSource, ParsedOverrides};

/// Environment variable overriding the browser engine bundled with the CLI.
const ENGINE_DIR_ENV: &str = "GRAPHCAL_REPORT_ENGINE_DIR";
/// Browser engine generated from this release's `graphcal-wasm` crate.
const EMBEDDED_ENGINE_GLUE_JS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/report-engine/graphcal_wasm.js"));
const EMBEDDED_ENGINE_WASM: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/report-engine/graphcal_wasm_bg.wasm"
));

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
    /// Override the bundled browser engine with `graphcal_wasm.js` and
    /// `graphcal_wasm_bg.wasm` from `wasm-pack --target no-modules`.
    /// Defaults to `$GRAPHCAL_REPORT_ENGINE_DIR`, then the engine embedded in
    /// this CLI release.
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
    Bundle(#[from] BundleError),
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
        "the browser engine bundle override was not found at {path}\n\
         ensure the directory contains `graphcal_wasm.js` and \
         `graphcal_wasm_bg.wasm`, or remove the --engine-dir / \
         ${ENGINE_DIR_ENV} override to use the engine bundled with the CLI"
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
    let metadata = if args.static_only {
        Vec::new()
    } else {
        capture_metadata(args, &fs)?
    };
    let snapshot = graphcal_io::OverlayFileSystem::with_overlays(
        fs.clone(),
        metadata
            .iter()
            .map(|(path, content)| (path.clone(), content.clone())),
    )
    .map_err(|error| ReportError::HydrationUnsupported {
        reason: error.to_string(),
    })?;
    let (project, host_fns) =
        crate::load_project_with_plugins(&args.file, args.root.as_deref(), &snapshot)?;
    let prepared = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .prepare()?;

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
        Some(engine) => Some(Hydration::new(
            EngineBundle {
                glue_js: engine.glue_js.as_ref(),
                wasm: engine.wasm.as_ref(),
            },
            &hydration_project(&project, args, &fs, &metadata)?,
            baseline_binding_strings(overrides),
        )?),
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
            error.name, error.reason
        );
    }

    for diagnostic in &result.presentation_diagnostics {
        eprintln!("presentation: {diagnostic}");
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
    let dependency_roots: std::collections::BTreeMap<_, _> = project
        .package_closure()
        .into_iter()
        .flat_map(|closure| &closure.dependencies)
        .map(|(id, dependency)| {
            (
                graphcal_compiler::dag_id::DagPackageId::new(id.as_str()),
                dependency.root.as_path(),
            )
        })
        .collect();
    let mut digests: Vec<SourceDigest> = project
        .files()
        .iter()
        .map(|(id, file)| {
            let path = file.path();
            let scope = dependency_roots
                .get(id.package())
                .copied()
                .or(root_dir.as_deref());
            let relative =
                relative_source_name(path, scope).unwrap_or_else(|| path.display().to_string());
            // Rendering boundary only: identities remain typed in the bundle.
            let name = if id.package() == project.root_id().package() {
                relative
            } else {
                format!("{} / {relative}", id.package())
            };
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

struct LoadedEngineBundle {
    glue_js: Cow<'static, str>,
    wasm: Cow<'static, [u8]>,
}

/// Use the release-matched embedded browser engine unless the caller
/// explicitly supplies a development bundle.
fn load_engine_bundle(args: &BuildArgs) -> Result<LoadedEngineBundle, ReportError> {
    let Some(dir) = args
        .engine_dir
        .clone()
        .or_else(|| std::env::var_os(ENGINE_DIR_ENV).map(PathBuf::from))
    else {
        return Ok(LoadedEngineBundle {
            glue_js: Cow::Borrowed(EMBEDDED_ENGINE_GLUE_JS),
            wasm: Cow::Borrowed(EMBEDDED_ENGINE_WASM),
        });
    };

    let glue_path = dir.join("graphcal_wasm.js");
    let wasm_path = dir.join("graphcal_wasm_bg.wasm");
    if !glue_path.is_file() || !wasm_path.is_file() {
        return Err(ReportError::EngineMissing { path: dir });
    }
    let glue_js =
        std::fs::read_to_string(&glue_path).map_err(|source| ReportError::EngineRead {
            path: glue_path,
            source,
        })?;
    let wasm = std::fs::read(&wasm_path).map_err(|source| ReportError::EngineRead {
        path: wasm_path,
        source,
    })?;
    Ok(LoadedEngineBundle {
        glue_js: Cow::Owned(glue_js),
        wasm: Cow::Owned(wasm),
    })
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

/// Freeze manifest and lockfile bytes before native loading. Plugin/source bytes
/// are already retained by `LoadedProject` and never reread during embedding.
fn capture_metadata(
    args: &BuildArgs,
    fs: &graphcal_io::RealFileSystem,
) -> Result<Vec<(PathBuf, String)>, ReportError> {
    let root =
        project_root_dir(&args.file, args.root.as_deref(), fs).ok_or(BundleError::UnsafePath)?;
    [
        ("graphcal.toml", 1024 * 1024),
        ("graphcal.lock", 4 * 1024 * 1024),
    ]
    .into_iter()
    .try_fold(Vec::new(), |mut files, (name, maximum)| {
        let path = root.join(name);
        match fs.read_to_string_bounded(
            &path,
            graphcal_io::ByteLimit::new(maximum),
            &graphcal_io::NeverCancel,
        ) {
            Ok(content) => files.push((path, content)),
            Err(graphcal_io::FileSystemReadError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ReportError::HydrationUnsupported {
                    reason: format!("could not capture {name}: {error}"),
                });
            }
        }
        Ok(files)
    })
}

/// Assemble the in-memory project shipped to the browser engine, rejecting
/// everything the engine cannot run — loudly, at build time.
fn hydration_project(
    project: &LoadedProject,
    args: &BuildArgs,
    fs: &graphcal_io::RealFileSystem,
    metadata: &[(PathBuf, String)],
) -> Result<ProjectBundle, ReportError> {
    let root_dir = project_root_dir(&args.file, args.root.as_deref(), fs);

    let dependencies = project
        .package_closure()
        .into_iter()
        .flat_map(|closure| &closure.dependencies)
        .map(|(id, dependency)| {
            graphcal_eval::project_bundle::BundlePackage::from_snapshot(
                id.clone(),
                &dependency.snapshot,
            )
        })
        .collect::<Result<_, _>>()?;

    let entry_name = args
        .file
        .canonicalize()
        .ok()
        .and_then(|entry| relative_source_name(&entry, root_dir.as_deref()));
    let mut files = Vec::new();
    for (_, file) in project
        .files()
        .iter()
        .filter(|(id, _)| id.package() == project.root_id().package())
    {
        let Some(name) = relative_source_name(file.path(), root_dir.as_deref()) else {
            return Err(ReportError::HydrationUnsupported {
                reason: format!(
                    "source {} lives outside the project root and cannot ship in the artifact",
                    file.path().display()
                ),
            });
        };
        files.push(BundleArtifact {
            path: name.try_into()?,
            content: ArtifactContent::Source(file.source().to_string()),
        });
    }
    for (path, content) in metadata {
        let name =
            relative_source_name(path, root_dir.as_deref()).ok_or(BundleError::UnsafePath)?;
        let content = match name.as_str() {
            "graphcal.toml" => ArtifactContent::Manifest(content.clone()),
            "graphcal.lock" => ArtifactContent::Lockfile(content.clone()),
            _ => return Err(BundleError::ArtifactKind.into()),
        };
        files.push(BundleArtifact {
            path: name.try_into()?,
            content,
        });
    }
    for (path, plugin) in project
        .plugins()
        .iter()
        .filter(|(identity, _)| identity.package() == Some(project.root_id().package()))
    {
        let plugin = plugin
            .as_ref()
            .map_err(|error| ReportError::HydrationUnsupported {
                reason: error.to_string(),
            })?;
        files.push(BundleArtifact {
            path: path.path().to_string().try_into()?,
            content: ArtifactContent::Plugin(plugin.bytes().to_vec()),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let entry = entry_name.ok_or(BundleError::MissingEntry)?.try_into()?;
    let bundle = ProjectBundle {
        entry,
        files,
        dependencies,
    };
    // Apply exactly the browser's artifact policy before writing an interactive report.
    bundle.mount(Path::new("/report"))?;
    Ok(bundle)
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
