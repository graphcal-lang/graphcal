//! Dependency-ordered whole-project checking orchestration.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use graphcal_compiler::semantic_error::graph::GraphError;
use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::ir::resolve::ImportedValueNames;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::plugin::PluginError;
use graphcal_compiler::source_id::SourceId;

use super::{checking, imports, lowering};
use crate::compile_error::PipelineError;

use super::checked_project::CheckedProject;
use super::checked_project::CompiledFile;
use super::hir_project::HirProject;
use super::lowering::ProjectSemanticContext;
use super::model::{HirFile, ImportContext, ModuleArtifact, ModuleArtifactStore};
use super::template::ModuleTemplateStore;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::dependency_graph::Cycle;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;

/// G009 for templates that include each other in a circle, reported at the
/// declaration of the template the include expansion re-entered.
///
/// Each template is named by its path inside its file (`outer.inner`), or by
/// its module identity when it is a file root.
pub(super) fn recursive_dag_instantiation(
    project: &crate::loader::loaded_project::LoadedProject,
    cycle: &Cycle<DagId>,
) -> PipelineError {
    let templates = cycle
        .path()
        .chain(std::iter::once(cycle.entry()))
        .cloned()
        .collect::<Vec<_>>();
    let (src, span) = match project.module(cycle.entry()) {
        Some(crate::loader::loaded_file::LoadedModule::InlineDag { file, dag }) => {
            (file.source_id(), dag.declaration(file).span)
        }
        Some(crate::loader::loaded_file::LoadedModule::FileRoot(file)) => {
            (file.source_id(), file.source_id().whole_span())
        }
        None => {
            let src = project.root_file().source_id();
            (src, src.whole_span())
        }
    };
    PipelineError::Semantic(SemanticError::located(
        src,
        span,
        GraphError::RecursiveDagInstantiation { templates },
    ))
}

/// Lower one physical file after every dependency HIR interface is available.
fn lower_single_file_to_hir(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    loaded_file: &crate::loader::loaded_file::LoadedFile,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HirFile, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        module_map: HashMap::new(),
        include_instances: Vec::new(),
    };

    imports::process_file_body_declarations(
        semantic.project,
        loaded_file,
        semantic.module_resolver,
        &mut ctx,
        cancellation,
    )?;

    lowering::lower_file_to_hir(semantic, loaded_file, ctx, cancellation)
}

/// Store one pure compile-time module artifact for downstream imports, and
/// return its execution facts for the programs of downstream files.
fn store_module_artifact(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: SourceId,
    module_artifacts: &mut ModuleArtifactStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_eval::checked_program::ExecutionFacts, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let (tir, execution_facts) = compiled.program.into_parts();
    let local_owners = tir.local_dags().map(|(dag_id, _)| dag_id.clone()).collect();
    let override_dependencies =
        graphcal_compiler::tir::dim_check::collect_override_dependency_summary(&tir, cancellation)?;
    let extern_functions = tir.extern_functions().clone();
    // The checked file is no longer needed after publication. Consume its
    // mutable assembly registry so each local body becomes one immutable
    // handle; no DAG body is cloned for an importer.
    let dag_store = tir.freeze_local_dag_store().map_err(|error| {
        PipelineError::Semantic(SemanticError::internal_error(
            error.to_string(),
            file_src,
            DiagnosticAnchor::WholeFile,
        ))
    })?;

    module_artifacts
        .insert(
            file_dag_id.clone(),
            ModuleArtifact {
                local_owners,
                override_dependencies,
                dag_store: Arc::new(dag_store),
                extern_functions,
            },
        )
        .map_err(|error| {
            PipelineError::Semantic(SemanticError::internal_error(
                error.to_string(),
                file_src,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
    Ok(execution_facts)
}

/// Lower the complete loaded project into one authoritative HIR value.
///
/// Dependencies contribute HIR interfaces only. No TIR construction, static
/// body checking, constant evaluation, or host verification occurs here.
pub(super) fn lower_project_perfile<'project, Mode>(
    project: &'project crate::loader::loaded_project::LoadedProject,
    module_resolver: graphcal_compiler::resolve::ModuleResolver,
    mode: Mode,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HirProject<'project, Mode>, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let mut module_templates = ModuleTemplateStore::default();

    let files = {
        let mut definitions = graphcal_compiler::ir::lower::definition_evaluator(
            &module_resolver,
            project.files().iter().flat_map(|loaded_file| {
                let src = loaded_file.source_id();
                std::iter::once((
                    loaded_file.dag_id().clone(),
                    graphcal_compiler::ir::static_definitions::DefinitionSource {
                        declarations: &loaded_file.ast().declarations,
                        src,
                    },
                ))
                .chain(loaded_file.inline_dags().iter().map(move |inline| {
                    (
                        inline.dag_id().clone(),
                        graphcal_compiler::ir::static_definitions::DefinitionSource {
                            declarations: inline.body(loaded_file),
                            src,
                        },
                    )
                }))
            }),
            project.root_file().source_id(),
        )
        .map_err(PipelineError::from)?;
        let mut semantic = ProjectSemanticContext {
            project,
            module_resolver: &module_resolver,
            module_templates: &mut module_templates,
            definitions: &mut definitions,
        };
        // Dependency order guarantees every imported HIR interface is available
        // before its dependents are lowered.
        project.files().ordered().as_ref().try_map(|loaded_file| {
            cancellation.checkpoint()?;
            lower_single_file_to_hir(&mut semantic, loaded_file, cancellation)
        })?
    };

    let exported_runtime_units = project
        .files()
        .iter()
        .flat_map(|loaded_file| {
            std::iter::once(loaded_file.module()).chain(
                loaded_file
                    .inline_dags()
                    .iter()
                    .map(|inline| inline.module(loaded_file)),
            )
        })
        .map(|module| {
            (
                module.dag_id().clone(),
                module.interface().runtime_units().clone(),
            )
        })
        .collect();

    Ok(HirProject {
        files,
        plugins: project.plugins(),
        exported_runtime_units,
        module_resolver,
        sources: std::sync::Arc::clone(project.sources()),
        mode,
    })
}

fn build_project_type_store<Mode>(
    hir: &HirProject<'_, Mode>,
) -> Result<Arc<graphcal_compiler::tir::typed::ProjectTypeStore>, PipelineError> {
    let root_source = &hir.files.root().source;
    let mut project_types = graphcal_compiler::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|error| {
        SemanticError::internal_error(
            format!("failed to build prelude project type store: {error}"),
            *root_source,
            DiagnosticAnchor::Builtin,
        )
    })?;
    for file in &hir.files {
        let source = &file.source;
        std::iter::once(&file.root)
            .chain(&file.inline_dags)
            .try_for_each(|dag| {
                project_types
                    .insert_module(dag.definitions())
                    .map_err(|error| {
                        SemanticError::internal_error(
                            format!("cannot build project type store: {error}"),
                            *source,
                            DiagnosticAnchor::WholeFile,
                        )
                    })
            })?;
    }
    Ok(Arc::new(project_types))
}

/// Consume a complete HIR project and perform all mandatory static checks.
pub(super) fn check_hir_project<Mode>(
    hir: HirProject<'_, Mode>,
    host_metadata: &graphcal_eval::host_fns::HostFunctionMetadata,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProject, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let project_types = build_project_type_store(&hir)?;
    let HirProject {
        files,
        plugins,
        exported_runtime_units,
        module_resolver,
        sources,
        mode: _,
    } = hir;
    let (deps, root_file) = files.into_parts();
    let mut module_artifacts = ModuleArtifactStore::default();
    let mut inherited_execution_facts = graphcal_eval::checked_program::ExecutionFacts::default();
    // Check one HIR file against every dependency artifact published so far
    // and verify its declared host functions.
    let check_file =
        |hir_file: HirFile,
         module_artifacts: &ModuleArtifactStore,
         inherited_execution_facts: &graphcal_eval::checked_program::ExecutionFacts|
         -> Result<(CompiledFile, SourceId), Outcome<PipelineError>> {
            cancellation.checkpoint()?;
            let file_src = hir_file.source;
            let compiled = checking::check_hir_file(
                hir_file,
                module_artifacts,
                inherited_execution_facts,
                checking::FileCheckEnvironment {
                    exported_runtime_units: &exported_runtime_units,
                    module_resolver: &module_resolver,
                    project_types: &project_types,
                    sources: &sources,
                },
                cancellation,
            )?;
            verify_host_functions(
                plugins,
                compiled.program.tir(),
                file_src,
                host_metadata,
                cancellation,
            )?;
            Ok((compiled, file_src))
        };

    for hir_file in deps {
        let file_dag_id = hir_file.root.dag_id().clone();
        let (compiled, file_src) =
            check_file(hir_file, &module_artifacts, &inherited_execution_facts)?;
        inherited_execution_facts = store_module_artifact(
            compiled,
            &file_dag_id,
            file_src,
            &mut module_artifacts,
            cancellation,
        )?;
    }

    let (compiled, source) = check_file(root_file, &module_artifacts, &inherited_execution_facts)?;
    Ok(CheckedProject {
        compiled,
        source,
        sources,
        module_resolver,
    })
}

/// Load-time verification of every extern function declared by a file.
///
/// Checks run per declaration in source order, root cause first:
///
/// 1. For wasm plugins: the declaring file must belong to the root package,
///    the plugin file must have been read by the loader, and the plugin
///    host must have registered the module (its recorded failure is
///    reported otherwise).
/// 2. The registry must provide the function (`MissingHostFunction`, P003).
/// 3. When the registry entry carries a manifest-provided signature, the
///    declared signature must be structurally equivalent to it (P005) —
///    this is the "declaration verified against the embedded manifest"
///    guarantee of the plugin design (#25).
fn verify_host_functions(
    plugins: &HashMap<
        graphcal_compiler::plugin_identity::PluginIdentity,
        crate::loader::loaded_project::PluginFileEntry,
    >,
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    src: SourceId,
    host_metadata: &graphcal_eval::host_fns::HostFunctionMetadata,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), Outcome<PipelineError>> {
    // Deterministic reporting order: earliest declaration first.
    let mut declared: Vec<_> = tir.extern_functions().iter().collect();
    declared.sort_by_key(|(_, function)| function.name_span.offset());

    for (key, function) in declared {
        cancellation.checkpoint()?;
        if matches!(
            key.plugin,
            graphcal_compiler::plugin_identity::PluginIdentity::Wasm { .. }
        ) {
            verify_wasm_plugin(plugins, function, src, host_metadata)?;
        }
        if !host_metadata.contains(key) {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.name_span,
                PluginError::MissingHostFunction {
                    plugin: function.plugin.clone(),
                    name: function.name.clone(),
                },
            ))
            .into());
        }
        if let Some(provided) = host_metadata.provided_signature(key)
            && !function.signature.structurally_equivalent(provided)
        {
            let format_dim = |dim: &graphcal_compiler::dimension::Dimension| {
                tir.registry().dimensions.format_dimension(dim)
            };
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.decl_span,
                PluginError::ExternSignatureMismatch {
                    plugin: function.plugin.clone(),
                    name: function.name.clone(),
                    declared: function.signature.format_with(format_dim),
                    provided: provided.format_with(format_dim),
                },
            ))
            .into());
        }
    }
    Ok(())
}

/// The wasm-plugin-specific half of [`verify_host_functions`]: file-level
/// failures recorded by the loader and module-level failures recorded by
/// the embedder's plugin host, reported at the import path's span.
fn verify_wasm_plugin(
    plugins: &HashMap<
        graphcal_compiler::plugin_identity::PluginIdentity,
        crate::loader::loaded_project::PluginFileEntry,
    >,
    function: &graphcal_compiler::ir::extern_function::ExternFunctionEntry,
    src: SourceId,
    host_metadata: &graphcal_eval::host_fns::HostFunctionMetadata,
) -> Result<(), PipelineError> {
    match plugins.get(&function.plugin) {
        Some(Err(crate::loader::loaded_project::PluginFileError::NotPinned)) => {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.path_span,
                PluginError::PluginNotPinned {
                    plugin: function.plugin.clone(),
                },
            )));
        }
        Some(Err(crate::loader::loaded_project::PluginFileError::HashMismatch {
            expected,
            actual,
        })) => {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.path_span,
                PluginError::PluginHashMismatch {
                    plugin: function.plugin.clone(),
                    expected: expected.clone(),
                    actual: actual.clone(),
                },
            )));
        }
        Some(Err(file_error)) => {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.path_span,
                PluginError::PluginLoadFailed {
                    plugin: function.plugin.clone(),
                    reason: file_error.to_string(),
                },
            )));
        }
        // Defensive: the loader records an entry for every root-package wasm
        // import, so an absent entry means an embedder skipped `load_project`.
        None => {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.path_span,
                PluginError::PluginLoadFailed {
                    plugin: function.plugin.clone(),
                    reason: "the project loader provided no bytes for this plugin".to_string(),
                },
            )));
        }
        Some(Ok(_)) => {}
    }

    match host_metadata.plugin_failure(&function.plugin) {
        Some(graphcal_eval::host_fns::PluginRegistrationError::ForbiddenImport {
            module,
            name,
        }) => Err(PipelineError::Semantic(SemanticError::located(
            src,
            function.path_span,
            PluginError::PluginForbiddenImport {
                plugin: function.plugin.clone(),
                import_module: module.clone(),
                import_name: name.clone(),
            },
        ))),
        Some(graphcal_eval::host_fns::PluginRegistrationError::LoadFailed { reason }) => {
            Err(PipelineError::Semantic(SemanticError::located(
                src,
                function.path_span,
                PluginError::PluginLoadFailed {
                    plugin: function.plugin.clone(),
                    reason: reason.clone(),
                },
            )))
        }
        None => Ok(()),
    }
}
