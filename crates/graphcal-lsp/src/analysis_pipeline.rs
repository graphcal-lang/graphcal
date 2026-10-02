//! The synchronous analysis pipeline: load, check, evaluate, and index one document.

use graphcal_project::load_error::LoadError;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::diagnostics::{compile_error_to_diagnostics_grouped, eval_result_to_diagnostics};
use crate::filesystem_events::TrackingFileSystem;
use crate::project_symbols::ProjectSymbols;
use crate::symbol_table::{self, SymbolTable};
use crate::workspace_revision::{AnalysisInputSnapshot, DocumentIdentity};
use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_project::compile_error::CompileError;
use graphcal_project::loader::LoadedProject;
use graphcal_project::project_compiler::{CheckedProject, ProjectCompiler};

use crate::analysis::{AnalysisResult, ResolvedImportLink};
use crate::file_identity::file_identity;
use crate::fn_signatures::{build_extern_fn_signatures, build_fn_signatures};
use crate::imported_definitions::{collect_import_surfaces, collect_imported_definitions};
use crate::value_format::format_eval_values;

/// One deliberate editor-feature degradation produced by synchronous analysis.
///
/// The functional analysis core records the typed reason; the async LSP shell
/// renders it through `window/logMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisDegradation {
    EmptySymbolTable { reason: String },
    FileLocalModuleResolver { reason: String },
}

impl AnalysisDegradation {
    pub fn message(&self, uri: &Url) -> String {
        match self {
            Self::EmptySymbolTable { reason } => format!(
                "analysis for {uri} could not build a fallback symbol table: {reason}; retaining previous symbol information when available"
            ),
            Self::FileLocalModuleResolver { reason } => format!(
                "analysis for {uri} is using a file-local module resolver after project resolver construction failed: {reason}; cross-module editor features may be unavailable"
            ),
        }
    }
}

/// Transient output of one analysis pass.
pub struct AnalysisRun {
    pub analysis: AnalysisResult,
    pub degradations: Vec<AnalysisDegradation>,
}

impl AnalysisRun {
    pub const fn complete(analysis: AnalysisResult) -> Self {
        Self {
            analysis,
            degradations: Vec::new(),
        }
    }
}

/// Snapshot of another open editor buffer, overlaid onto the filesystem
/// during analysis so cross-file diagnostics, goto-definition targets, and
/// hover reflect what the user actually sees instead of stale disk content.
pub struct OpenBuffer {
    pub path: std::path::PathBuf,
    pub text: Arc<String>,
}

/// Build a `LoadedProject` from a URI and in-memory text.
///
/// For file-backed URIs, loads the project from disk with the in-memory text
/// overlaid on the root file — and every other open document's latest text
/// overlaid on its path — via [`graphcal_io::OverlayFileSystem`]. The base
/// reader is sandboxed to the discovered project root (when a `graphcal.toml`
/// is reachable from the buffer's directory) — keeping the LSP's filesystem
/// access in lockstep with the CLI's. For untitled/non-file URIs, builds a
/// single-file project from the in-memory text alone.
pub struct ProjectBuild {
    pub project: std::result::Result<LoadedProject, Outcome<Box<CompileError>>>,
    pub filesystem_inputs: HashSet<DocumentIdentity>,
}

impl ProjectBuild {
    pub fn failed(error: CompileError) -> Self {
        Self {
            project: Err(Outcome::Failed(Box::new(error))),
            filesystem_inputs: HashSet::new(),
        }
    }
}

pub fn build_project(
    uri: &Url,
    text: &str,
    open_buffers: &[OpenBuffer],
    cancellation: &CancellationToken,
) -> ProjectBuild {
    let name = uri.as_str();
    let Ok(path) = uri.to_file_path() else {
        return ProjectBuild {
            project: LoadedProject::from_source_with_cancellation(text, name, cancellation)
                .map_err(|outcome| outcome.map_failed(Box::new)),
            filesystem_inputs: HashSet::new(),
        };
    };

    // OverlayFileSystem validates existing identities and proves unsaved
    // buffers are below a base-authorized canonical parent. The analyzed
    // snapshot comes first so it wins over any newer latest-text entry for the
    // same file.
    let base = match graphcal_project::loader::build_rooted_filesystem(&path, None) {
        Ok(base) => base,
        Err(error) => return ProjectBuild::failed(error),
    };
    let overlays = std::iter::once((path.clone(), text.to_string()))
        .chain(
            open_buffers
                .iter()
                .filter(|buffer| {
                    graphcal_io::OverlayFileSystem::new(
                        base.clone(),
                        buffer.path.clone(),
                        String::new(),
                    )
                    .is_ok()
                })
                .map(|buffer| (buffer.path.clone(), buffer.text.as_ref().clone())),
        )
        .collect::<Vec<_>>();
    let fs = match graphcal_io::OverlayFileSystem::with_overlays(base, overlays) {
        Ok(fs) => fs,
        Err(error) => {
            return ProjectBuild::failed(CompileError::Load(LoadError::InvalidSourcePath {
                path: error.path().to_path_buf(),
                reason: error.to_string(),
            }));
        }
    };
    let tracking_fs = TrackingFileSystem::new(fs);
    let project = graphcal_project::loader::load_project_with_cancellation(
        &path,
        None,
        &tracking_fs,
        cancellation,
    )
    .map_err(|outcome| outcome.map_failed(Box::new));
    let root_identity = file_identity(&path);
    let filesystem_inputs = tracking_fs
        .accessed_paths()
        .into_iter()
        .map(|input| file_identity(&input))
        .filter(|identity| *identity != root_identity)
        .collect();
    ProjectBuild {
        project,
        filesystem_inputs,
    }
}

pub fn project_dependency_identities(project: &LoadedProject) -> HashSet<DocumentIdentity> {
    project
        .files()
        .ordered()
        .deps()
        .iter()
        .map(|file| DocumentIdentity::file(file.path().to_path_buf()))
        .chain(
            project
                .package_closure()
                .into_iter()
                .flat_map(dependency_artifact_identities),
        )
        .collect()
}

pub fn dependency_artifact_identities(
    closure: &graphcal_project::loader::LoadedPackageClosure,
) -> impl Iterator<Item = DocumentIdentity> + '_ {
    closure.dependencies.values().flat_map(|dependency| {
        dependency
            .snapshot
            .files()
            .map(|(relative, _)| DocumentIdentity::file(dependency.root.join(relative)))
    })
}

/// Wrap a single-URI diagnostic vec into the per-URI map shape so the active
/// document's URI is always present (even when empty) and so eval diagnostics
/// — which always belong to the active file — sit alongside any cross-file
/// parse/TIR diagnostics.
pub fn diagnostics_for_active_uri(
    uri: &Url,
    diags: Vec<Diagnostic>,
) -> HashMap<Url, Vec<Diagnostic>> {
    let mut out = HashMap::new();
    out.insert(uri.clone(), diags);
    out
}

/// One plugin host shared by every analysis test, mirroring the Backend's
/// process-wide host (and keeping the module cache warm).
#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only helper shared with other modules' tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) fn test_plugin_host() -> &'static graphcal_plugin_host::PluginHost {
    static HOST: std::sync::OnceLock<graphcal_plugin_host::PluginHost> = std::sync::OnceLock::new();
    HOST.get_or_init(graphcal_plugin_host::PluginHost::new)
}

/// Run the analysis pipeline, producing an `AnalysisResult`.
///
/// The pipeline has two stages:
/// 1. Build a `LoadedProject` from the in-memory text (+ disk imports).
/// 2. Compile TIR from the project.
///
/// Both stages use the same source text, eliminating data provenance mismatches.
#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only helper shared with other modules' tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) fn run_analysis_for_test(uri: &tower_lsp::lsp_types::Url, text: &str) -> AnalysisResult {
    run_analysis(uri, text, &[], test_plugin_host())
}

#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only helper shared with other modules' tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) fn run_analysis_run(
    uri: &tower_lsp::lsp_types::Url,
    text: &str,
    open_buffers: &[OpenBuffer],
    plugin_host: &graphcal_plugin_host::PluginHost,
) -> AnalysisRun {
    let revision = crate::workspace_revision::RevisionClock::default()
        .next()
        .unwrap();
    let input_snapshot = AnalysisInputSnapshot::new(
        crate::file_identity::document_identity(uri),
        revision,
        std::collections::HashMap::new(),
    );
    run_analysis_with_cancellation(
        uri,
        text,
        open_buffers,
        plugin_host,
        &input_snapshot,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .expect("an unbounded analysis cannot be cancelled")
}

#[cfg(test)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "test-only helper shared with other modules' tests; cargo-hawk rejects `pub` for test-only items"
)]
pub(crate) fn run_analysis(
    uri: &tower_lsp::lsp_types::Url,
    text: &str,
    open_buffers: &[OpenBuffer],
    plugin_host: &graphcal_plugin_host::PluginHost,
) -> AnalysisResult {
    run_analysis_run(uri, text, open_buffers, plugin_host).analysis
}

#[expect(
    clippy::too_many_lines,
    reason = "analysis coordinates fallback diagnostics and successful semantic metadata"
)]
pub fn run_analysis_with_cancellation(
    uri: &Url,
    text: &str,
    open_buffers: &[OpenBuffer],
    plugin_host: &graphcal_plugin_host::PluginHost,
    input_snapshot: &AnalysisInputSnapshot,
    cancellation: &CancellationToken,
) -> std::result::Result<AnalysisRun, Cancelled> {
    cancellation.checkpoint()?;
    // Stage 1: Build project (parse + load imports).
    // If this fails, no AST is available for the multi-file pipeline. Fall
    // back to parsing just the active buffer so hover/goto-def on the active
    // file's own symbols still answer — the imported-file error remains
    // visible, but local LSP features degrade gracefully.
    let project_build = build_project(uri, text, open_buffers, cancellation);
    let mut filesystem_inputs = project_build.filesystem_inputs;
    let project = match project_build.project {
        Ok(project) => project,
        Err(Outcome::Cancelled) => return Err(Cancelled),
        Err(Outcome::Failed(error)) => {
            let mut diagnostics = compile_error_to_diagnostics_grouped(&error, uri);
            diagnostics.entry(uri.clone()).or_default();
            // `Some` when the failure was in an *import* (the buffer itself
            // parses): the buffer's own symbols are still fully usable.
            // `None` when the buffer doesn't parse — the result is then a
            // diagnostics-only fallback and `store_analysis` retains the
            // previous symbol state (#834).
            let (symbol_table, buffer_parsed, degradations) =
                match LoadedProject::from_source_with_cancellation(text, uri.as_str(), cancellation)
                {
                    Ok(single) => (
                        symbol_table::build_for_buffer(single.root_file().ast(), text),
                        true,
                        Vec::new(),
                    ),
                    Err(Outcome::Cancelled) => return Err(Cancelled),
                    Err(Outcome::Failed(error)) => (
                        SymbolTable::default(),
                        false,
                        vec![AnalysisDegradation::EmptySymbolTable {
                            reason: error.to_string(),
                        }],
                    ),
                };
            cancellation.checkpoint()?;
            return Ok(AnalysisRun {
                analysis: AnalysisResult {
                    inputs: input_snapshot.finish_partially_loaded_project(filesystem_inputs),
                    source: Arc::new(text.to_string()),
                    symbol_table,
                    project_symbols: ProjectSymbols::Incomplete,
                    imported_definitions: HashMap::new(),
                    imported_bindings: Vec::new(),
                    import_surfaces: HashMap::new(),
                    diagnostics: Arc::new(diagnostics),
                    eval_values: HashMap::new(),
                    fn_signatures: build_fn_signatures(),
                    extern_fn_signatures: HashMap::new(),
                    import_links: Vec::new(),
                    buffer_parsed,
                },
                degradations,
            });
        }
    };

    cancellation.checkpoint()?;
    filesystem_inputs.extend(project_dependency_identities(&project));
    let analysis_inputs = input_snapshot.finish_loaded_project(filesystem_inputs);
    let root_ast = project.root_file().ast();
    let import_links = collect_import_links(&project, cancellation)?;
    cancellation.checkpoint()?;
    // Extern (plugin) registry for this pass: the built-in demo plugin plus
    // the project's vendored wasm plugins. The plugin host outlives passes,
    // so unchanged modules come from its content-hash cache.
    cancellation.checkpoint()?;
    let mut host_fns = graphcal_eval::host_fns::demo_registry();
    graphcal_plugin_host::register_project_plugins(plugin_host, &project, &mut host_fns);
    cancellation.checkpoint()?;

    // Stage 2: Compile TIR from the project.
    match ProjectCompiler::new(&project)
        .host_fns(&host_fns)
        .cancellation(cancellation)
        .check()
    {
        Ok(checked) => {
            cancellation.checkpoint()?;
            let tir = checked.tir();
            let module_resolver = checked.module_resolver();
            // Full success: symbol table from AST + TIR enrichment.
            let mut symbol_table =
                symbol_table::build_from_ast(root_ast, text, project.root_id(), module_resolver);
            cancellation.checkpoint()?;
            symbol_table::enrich_from_tir(&mut symbol_table, tir, project.root_id());

            cancellation.checkpoint()?;
            let imported_symbols = collect_imported_definitions(
                uri,
                &project,
                Some(tir),
                module_resolver,
                cancellation,
            )?;
            cancellation.checkpoint()?;
            let fn_signatures = build_fn_signatures();
            let extern_fn_signatures = build_extern_fn_signatures(tir, cancellation)?;
            let import_surfaces = collect_import_surfaces(&project, module_resolver, cancellation)?;
            // Library files (required param/index not yet bound) cannot be evaluated
            // standalone. Skip the eval pipeline so editors don't surface false-positive
            // `RequiredStaticInputNotBound` / `RequiredParamNotProvided` diagnostics when the
            // user opens such a file for editing.
            let todos =
                crate::diagnostics::unfinished_node_diagnostics(&root_ast.declarations, text);
            let (mut diagnostics, eval_values) = if checked.is_library() {
                (HashMap::new(), HashMap::new())
            } else {
                run_eval_from_checked(checked, uri, text, &symbol_table, &host_fns, cancellation)?
            };
            cancellation.checkpoint()?;
            diagnostics.entry(uri.clone()).or_default().extend(todos);

            Ok(AnalysisRun::complete(AnalysisResult {
                inputs: analysis_inputs,
                source: Arc::new(text.to_string()),
                symbol_table,
                project_symbols: ProjectSymbols::Complete(imported_symbols.project_index),
                imported_definitions: imported_symbols.definitions,
                imported_bindings: imported_symbols.bindings,
                import_surfaces,
                diagnostics: Arc::new(diagnostics),
                eval_values,
                fn_signatures,
                extern_fn_signatures,
                import_links,
                buffer_parsed: true,
            }))
        }
        Err(Outcome::Cancelled) => Err(Cancelled),
        Err(Outcome::Failed(error)) => {
            cancellation.checkpoint()?;
            // A failed session has no checked resolver continuation. Rebuild a
            // best-effort resolver only for partial editor information.
            let (module_resolver, degradations) = match project.build_module_resolver() {
                Ok(module_resolver) => (module_resolver, Vec::new()),
                Err(resolver_error) => (
                    symbol_table::file_local_resolver(root_ast, project.root_id()),
                    vec![AnalysisDegradation::FileLocalModuleResolver {
                        reason: resolver_error.to_string(),
                    }],
                ),
            };
            let symbol_table =
                symbol_table::build_from_ast(root_ast, text, project.root_id(), &module_resolver);
            cancellation.checkpoint()?;
            let imported_symbols =
                collect_imported_definitions(uri, &project, None, &module_resolver, cancellation)?;
            let mut diagnostics = compile_error_to_diagnostics_grouped(&error, uri);
            diagnostics.entry(uri.clone()).or_default();

            Ok(AnalysisRun {
                analysis: AnalysisResult {
                    inputs: analysis_inputs,
                    source: Arc::new(text.to_string()),
                    symbol_table,
                    project_symbols: ProjectSymbols::Complete(imported_symbols.project_index),
                    imported_definitions: imported_symbols.definitions,
                    imported_bindings: imported_symbols.bindings,
                    import_surfaces: collect_import_surfaces(
                        &project,
                        &module_resolver,
                        cancellation,
                    )?,
                    diagnostics: Arc::new(diagnostics),
                    eval_values: HashMap::new(),
                    fn_signatures: build_fn_signatures(),
                    extern_fn_signatures: HashMap::new(),
                    import_links,
                    buffer_parsed: true,
                },
                degradations,
            })
        }
    }
}

pub type EvalAnalysisOutput = (HashMap<Url, Vec<Diagnostic>>, HashMap<ScopedName, String>);

/// Run evaluation from a loaded project and extract diagnostics and formatted values.
pub fn run_eval_from_checked(
    checked: CheckedProject,
    uri: &Url,
    text: &str,
    symbol_table: &SymbolTable,
    host_fns: &graphcal_eval::host_fns::HostFunctionRegistry,
    cancellation: &CancellationToken,
) -> std::result::Result<EvalAnalysisOutput, Cancelled> {
    let result = match checked.prepare_with_host_fns_and_cancellation(host_fns, cancellation) {
        Ok(prepared) => match prepared.binding_builder().finish() {
            Ok(row) => prepared.evaluate_with_cancellation(&row, cancellation),
            Err(error) => Err(Outcome::Failed(error)),
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(result) => {
            cancellation.checkpoint()?;
            let diagnostics = eval_result_to_diagnostics(&result, text, symbol_table);
            let values = format_eval_values(&result, cancellation)?;
            Ok((diagnostics_for_active_uri(uri, diagnostics), values))
        }
        Err(Outcome::Cancelled) => Err(Cancelled),
        Err(Outcome::Failed(error)) => {
            let mut diagnostics = compile_error_to_diagnostics_grouped(&error, uri);
            diagnostics.entry(uri.clone()).or_default();
            Ok((diagnostics, HashMap::new()))
        }
    }
}

/// Collect loader-resolved import links from the project for Document Links.
///
/// Uses `imports_with_paths()` and `includes_with_paths()` from the loader,
/// so document links agree with actual compilation behavior.
pub fn collect_import_links(
    project: &LoadedProject,
    cancellation: &CancellationToken,
) -> std::result::Result<Vec<ResolvedImportLink>, Cancelled> {
    cancellation.checkpoint()?;
    let root_file = project.root_file();

    let import_links = root_file
        .imports_with_targets()
        .map(|(_, import_decl, target)| (import_decl.path().span(), target));
    let include_links = root_file
        .includes_with_targets()
        .map(|(_, include_decl, target)| (include_decl.path.span(), target));

    import_links
        .chain(include_links)
        .map(|(span, target)| {
            cancellation.checkpoint()?;
            Ok(project.file(target.source_file()).and_then(|loaded| {
                Url::from_file_path(loaded.path())
                    .ok()
                    .map(|target_uri| ResolvedImportLink {
                        path_span: span,
                        target_uri,
                    })
            }))
        })
        .collect::<std::result::Result<Vec<_>, Cancelled>>()
        .map(|links| links.into_iter().flatten().collect())
}
