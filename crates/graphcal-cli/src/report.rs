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

use graphcal_eval::eval::EvalResult;
use graphcal_io::FileSystemReader as _;
use graphcal_project::compile_error::CompileError;
use graphcal_project::loader::{
    LoadedProject, build_rooted_filesystem, resolve_entry_project_root,
};
use graphcal_project::project_bundle::{
    ArtifactContent, BundleArtifact, BundleError, ProjectBundle,
};
use graphcal_project::project_compiler::ProjectCompiler;
use graphcal_report::plot_page::VegaScriptSource;
use graphcal_report::report_html::render_report_html;
use graphcal_report::report_hydrate::{EngineBundle, Hydration};
use graphcal_report::report_ir::{
    Provenance, ReportBuildError, ReportInputs, SourceDigest, build_report, collect_doc_captions,
};
use graphcal_report::report_markdown::render_report_markdown;
use graphcal_report::report_provenance::{
    ParameterJsonArgument, ReproInvocation, build_provenance,
};

use crate::overrides::{ParameterArgs, ParameterJsonSource, ParsedOverrides};

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
        "the project loaded with root {} instead of the resolved root {}",
        loaded.as_ref().map_or_else(|| "(none)".to_string(), |root| root.display().to_string()),
        resolved.display()
    )]
    ProjectRootChanged {
        resolved: PathBuf,
        loaded: Option<PathBuf>,
    },
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
    // One resolved root names everything: the captured metadata, the loaded
    // project (checked below), provenance, and the hydration bundle.
    let project_root = resolve_entry_project_root(&args.file, args.root.as_deref(), &fs)?;
    let metadata = if args.static_only {
        Vec::new()
    } else {
        capture_metadata(&project_root, &fs)?
    };
    let snapshot = graphcal_io::OverlayFileSystem::with_overlays(
        fs,
        metadata
            .iter()
            .map(|captured| (captured.path.clone(), captured.content.clone())),
    )
    .map_err(|error| ReportError::HydrationUnsupported {
        reason: error.to_string(),
    })?;
    let (project, host_fns) =
        crate::load_project_with_plugins(&args.file, args.root.as_deref(), &snapshot)?;
    if project.project_root() != Some(project_root.as_path()) {
        return Err(ReportError::ProjectRootChanged {
            resolved: project_root,
            loaded: project.project_root().map(Path::to_path_buf),
        });
    }
    let prepared = ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .prepare()?;

    let mut bindings = prepared.binding_builder();
    for (name, value) in &overrides.values {
        value.bind(name, &mut bindings)?;
    }
    let result = prepared.evaluate(&bindings.finish()?)?;

    let docs = collect_doc_captions(prepared.tir().root());
    let title = args.file.file_stem().map_or_else(
        || "report".to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let provenance = report_provenance(
        compiler_version,
        source_digests(&project, &project_root),
        &result,
        args,
        overrides,
    );
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
            &hydration_project(&project, args, &project_root, &metadata)?,
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

/// A loaded file's project-relative name; `None` when it does not live under
/// the project root. Components join with `/` on every platform: the
/// hydration payload feeds the browser engine's virtual filesystem, which
/// requires forward-slash relative paths, and provenance names stay
/// OS-independent.
fn relative_source_name(path: &Path, root_dir: &Path) -> Option<String> {
    path.strip_prefix(root_dir).ok().map(|relative| {
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    })
}

/// SHA-256 digests of every loaded source, named relative to the project
/// root and sorted by name so the artifact is machine-independent.
fn source_digests(project: &LoadedProject, root_dir: &Path) -> Vec<SourceDigest> {
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
        .map(|file| {
            let id = file.dag_id();
            let path = file.path();
            let scope = dependency_roots
                .get(id.package())
                .copied()
                .unwrap_or(root_dir);
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

/// A project metadata file the hydrated report ships beside its sources.
#[derive(Debug, Clone, Copy)]
enum MetadataKind {
    Manifest,
    Lockfile,
}

impl MetadataKind {
    const ALL: [Self; 2] = [Self::Manifest, Self::Lockfile];

    /// The file name under the project root.
    const fn file_name(self) -> &'static str {
        match self {
            Self::Manifest => "graphcal.toml",
            Self::Lockfile => "graphcal.lock",
        }
    }

    /// The loader's size limit for this kind of file.
    const fn limit(self, policy: graphcal_io::ProjectIngestionPolicy) -> graphcal_io::ByteLimit {
        match self {
            Self::Manifest => policy.manifest(),
            Self::Lockfile => policy.lockfile(),
        }
    }

    /// The bundle artifact category for captured `text`.
    const fn artifact(self, text: String) -> ArtifactContent {
        match self {
            Self::Manifest => ArtifactContent::Manifest(text),
            Self::Lockfile => ArtifactContent::Lockfile(text),
        }
    }
}

/// Metadata bytes frozen before loading, tagged with their kind.
struct CapturedMetadata {
    kind: MetadataKind,
    path: PathBuf,
    content: String,
}

/// Freeze manifest and lockfile bytes under `project_root` before native
/// loading. Plugin/source bytes are already retained by `LoadedProject` and
/// never reread during embedding.
fn capture_metadata(
    project_root: &Path,
    fs: &graphcal_io::RealFileSystem,
) -> Result<Vec<CapturedMetadata>, ReportError> {
    let policy = graphcal_io::ProjectIngestionPolicy::default();
    MetadataKind::ALL
        .into_iter()
        .try_fold(Vec::new(), |mut files, kind| {
            let path = project_root.join(kind.file_name());
            // The overlay that freezes these bytes keys them by this path; a
            // symlink would give it a second, diverging identity (and move the
            // project root the loader sees), so only a regular file ships.
            let not_regular = |found: &str| ReportError::HydrationUnsupported {
                reason: format!(
                    "could not capture {}: it must be a regular file in the project root, not {found}",
                    kind.file_name()
                ),
            };
            match fs.entry_kind(&path) {
                Ok(graphcal_io::FileSystemEntryKind::File) => {}
                Ok(graphcal_io::FileSystemEntryKind::Symlink) => {
                    return Err(not_regular("a symlink"));
                }
                Ok(graphcal_io::FileSystemEntryKind::Directory) => {
                    return Err(not_regular("a directory"));
                }
                Ok(graphcal_io::FileSystemEntryKind::Other) => {
                    return Err(not_regular("a special file"));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
                Err(error) => {
                    return Err(ReportError::HydrationUnsupported {
                        reason: format!("could not capture {}: {error}", kind.file_name()),
                    });
                }
            }
            match graphcal_compiler::outcome::without_cancellation(|cancellation| {
                fs.read_to_string_bounded(&path, kind.limit(policy), cancellation)
            }) {
                Ok(content) => files.push(CapturedMetadata {
                    kind,
                    path,
                    content,
                }),
                Err(graphcal_io::FileSystemReadError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ReportError::HydrationUnsupported {
                        reason: format!("could not capture {}: {error}", kind.file_name()),
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
    root_dir: &Path,
    metadata: &[CapturedMetadata],
) -> Result<ProjectBundle, ReportError> {
    let dependencies = project
        .package_closure()
        .into_iter()
        .flat_map(|closure| &closure.dependencies)
        .map(|(id, dependency)| {
            graphcal_project::project_bundle::BundlePackage::from_snapshot(
                id.clone(),
                &dependency.snapshot,
            )
        })
        .collect::<Result<_, _>>()?;

    let entry_name = args
        .file
        .canonicalize()
        .ok()
        .and_then(|entry| relative_source_name(&entry, root_dir));
    let mut files = Vec::new();
    for file in project
        .files()
        .iter()
        .filter(|file| file.dag_id().package() == project.root_id().package())
    {
        let Some(name) = relative_source_name(file.path(), root_dir) else {
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
    for captured in metadata {
        let name = relative_source_name(&captured.path, root_dir).ok_or(BundleError::UnsafePath)?;
        files.push(BundleArtifact {
            path: name.try_into()?,
            content: captured.kind.artifact(captured.content.clone()),
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

/// The provenance of this baseline evaluation, with the `graphcal eval`
/// invocation that reproduces it.
fn report_provenance(
    compiler_version: &str,
    sources: Vec<SourceDigest>,
    result: &EvalResult,
    args: &BuildArgs,
    overrides: &ParsedOverrides,
) -> Provenance {
    let entry = args.file.display().to_string();
    let root = args.root.as_ref().map(|root| root.display().to_string());
    let json_file = match &overrides.json_source {
        Some(ParameterJsonSource::File(path)) => Some(path.display().to_string()),
        _ => None,
    };
    let parameter_json = match (&overrides.json_source, &json_file) {
        (Some(ParameterJsonSource::Inline(json)), _) => Some(ParameterJsonArgument::Inline(json)),
        (Some(ParameterJsonSource::File(_)), Some(path)) => Some(ParameterJsonArgument::File(path)),
        (Some(ParameterJsonSource::Stdin), _) => Some(ParameterJsonArgument::Stdin),
        _ => None,
    };
    build_provenance(
        compiler_version,
        sources,
        result,
        &ReproInvocation {
            entry: &entry,
            root: root.as_deref(),
            parameters: overrides
                .direct_parameters
                .iter()
                .map(|binding| (binding.name.as_str(), binding.expression.as_str()))
                .collect(),
            parameter_json,
            parameter_json_max_bytes: overrides.json_max_bytes,
        },
    )
}
