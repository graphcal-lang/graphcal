//! Dependency-ordered whole-project checking orchestration.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use super::*;
use graphcal_compiler::desugar::desugared_ast::DeclKind;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;

/// Validate inline-DAG recursion before constructing project-wide scopes.
///
/// This preserves source-level diagnostic ordering: a recursive instance graph
/// is rejected before resolver inheritance attempts to inspect that invalid
/// synthetic scope.
pub(in crate::project_compiler) fn validate_project_dag_recursion(
    project: &crate::loader::LoadedProject,
) -> Result<(), CompileError> {
    project.files().iter().try_for_each(|loaded_file| {
        let definitions = loaded_file
            .ast()
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.kind {
                DeclKind::Dag(dag) => Some((dag.name.value.clone(), dag)),
                _ => None,
            })
            .collect();
        recursion::check_dag_recursion(&definitions, loaded_file.named_source())
    })
}

/// Lower one physical file after every dependency HIR interface is available.
fn lower_single_file_to_hir(
    project: &crate::loader::LoadedProject,
    loaded_file: &crate::loader::LoadedFile,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    module_templates: &mut ModuleTemplateStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<
    (
        HirFile,
        Vec<(graphcal_compiler::dag_id::DagId, LoweringModuleInterface)>,
    ),
    CompileError,
> {
    cancellation.checkpoint()?;
    let file_dag_id = loaded_file.dag_id();
    let file_src = loaded_file.named_source();

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        projected_static_aliases: Vec::new(),
        module_map: HashMap::new(),
        frontend_registry_imports: Vec::new(),
        include_instances: Vec::new(),
    };

    imports::process_file_body_declarations(
        project,
        file_dag_id,
        module_artifacts,
        module_resolver,
        &mut ctx,
        cancellation,
    )?;

    let (hir, root_interface) = lowering::lower_file_to_hir(
        ProjectSemanticContext {
            project,
            module_resolver,
            module_templates,
        },
        file_dag_id,
        file_src,
        loaded_file.ast(),
        ctx,
        module_artifacts,
        cancellation,
    )?;
    let mut interfaces = vec![(file_dag_id.clone(), root_interface)];
    for inline in loaded_file.inline_dags() {
        let template = module_templates.get(inline.dag_id()).ok_or_else(|| {
            CompileError::Eval(GraphcalError::internal_error(
                format!(
                    "inline module template `{}` was not retained",
                    inline.dag_id()
                ),
                file_src,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
        interfaces.push((
            inline.dag_id().clone(),
            LoweringModuleInterface::new(
                template.frontend_registry.clone(),
                template.external_surface.clone(),
            ),
        ));
    }
    Ok((hir, interfaces))
}

fn validate_dag_constant_values(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    const_values: &crate::eval_expr::RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    dag.consts().iter().try_for_each(|entry| {
        let key = dag.require_bound_decl_identity(
            &entry.name,
            src,
            DiagnosticAnchor::Source(entry.span),
        )?;
        const_values.get(&key).map(|_| ()).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked constant `{key}` has no value at module publication"),
                src,
                DiagnosticAnchor::Source(entry.span),
            )
        })
    })
}

/// Store one pure compile-time module artifact for downstream imports.
fn store_module_artifact(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &mut ModuleArtifactStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    compiled.tir.local_dags().try_for_each(|(dag_id, dag)| {
        let scope = crate::execution_scope::CheckedExecutionScope::new(
            &compiled.tir,
            &compiled.checked_execution_facts,
            dag_id,
        )
        .map_err(|error| {
            GraphcalError::internal_error(error.to_string(), file_src, DiagnosticAnchor::WholeFile)
        })?;
        validate_dag_constant_values(dag, &scope.facts().const_values, file_src)
    })?;
    let declared_types_by_dag = compiled
        .tir
        .local_dags()
        .map(|(dag_id, dag)| {
            cancellation.checkpoint()?;
            dag.build_declared_types(file_src)
                .map(|types| (dag_id.clone(), types))
        })
        .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
    let override_dependencies =
        graphcal_compiler::tir::dim_check::collect_override_dependency_summary_with_cancellation(
            &compiled.tir,
            file_src,
            cancellation,
        )?;
    let extern_functions = compiled.tir.extern_functions().clone();
    // The checked file is no longer needed after publication. Consume its
    // mutable assembly registry so each local body becomes one immutable
    // handle; no DAG body is cloned for an importer.
    let dag_store = compiled.tir.freeze_local_dag_store().map_err(|error| {
        CompileError::Eval(GraphcalError::internal_error(
            error.to_string(),
            file_src,
            DiagnosticAnchor::WholeFile,
        ))
    })?;

    module_artifacts
        .insert(
            file_dag_id.clone(),
            ModuleArtifact {
                declared_types_by_dag,
                override_dependencies,
                dag_store: Arc::new(dag_store),
                extern_functions,
            },
        )
        .map_err(|error| {
            CompileError::Eval(GraphcalError::internal_error(
                error.to_string(),
                file_src,
                DiagnosticAnchor::WholeFile,
            ))
        })
}

/// Lower the complete loaded project into one authoritative HIR value.
///
/// Dependencies contribute HIR interfaces only. No TIR construction, static
/// body checking, constant evaluation, or host verification occurs here.
pub(in crate::project_compiler) fn lower_project_perfile<'project>(
    project: &'project crate::loader::LoadedProject,
    module_resolver: graphcal_compiler::syntax::module_resolve::ModuleResolver,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HirProject<'project>, CompileError> {
    cancellation.checkpoint()?;
    let mut module_interfaces = HashMap::new();
    let mut module_templates = ModuleTemplateStore::default();

    // Dependency order guarantees every imported HIR interface is available
    // before its dependents are lowered.
    let files = project.files().ordered().as_ref().try_map(|loaded_file| {
        cancellation.checkpoint()?;
        let (hir, lowering_interfaces) = lower_single_file_to_hir(
            project,
            loaded_file,
            &module_interfaces,
            &module_resolver,
            &mut module_templates,
            cancellation,
        )?;
        module_interfaces.extend(lowering_interfaces);
        Ok::<_, CompileError>(hir)
    })?;

    let exported_runtime_units = module_interfaces
        .iter()
        .map(|(owner, interface)| (owner.clone(), interface.exported_runtime_units().clone()))
        .collect();

    Ok(HirProject {
        files,
        plugins: project.plugins(),
        exported_runtime_units,
        module_resolver,
        cancellation: cancellation.clone(),
    })
}

fn build_project_type_store(
    hir: &HirProject<'_>,
) -> Result<Arc<graphcal_compiler::tir::typed::ProjectTypeStore>, CompileError> {
    let root_source = &hir.files.root().source;
    let mut project_types = graphcal_compiler::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|error| {
        GraphcalError::internal_error(
            format!("failed to build prelude project type store: {error}"),
            root_source,
            DiagnosticAnchor::Builtin,
        )
    })?;
    for file in &hir.files {
        let source = &file.source;
        std::iter::once(&file.root)
            .chain(&file.inline_dags)
            .try_for_each(|dag| {
                project_types
                    .insert_resolver_module(dag, &hir.module_resolver)
                    .map_err(|error| {
                        GraphcalError::internal_error(
                            format!("cannot build project type store: {error}"),
                            source,
                            DiagnosticAnchor::WholeFile,
                        )
                    })
            })?;
    }
    Ok(Arc::new(project_types))
}

/// Consume a complete HIR project and perform all mandatory static checks.
pub(in crate::project_compiler) fn check_hir_project(
    hir: HirProject<'_>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<CheckedProject, CompileError> {
    hir.cancellation.checkpoint()?;
    let project_types = build_project_type_store(&hir)?;
    let HirProject {
        files,
        plugins,
        exported_runtime_units,
        module_resolver,
        cancellation,
    } = hir;
    let (deps, root_file) = files.into_parts();
    let mut module_artifacts = ModuleArtifactStore::default();
    let mut inherited_execution_facts = crate::execution_facts::CheckedExecutionFacts::empty();
    // Check one HIR file against every dependency artifact published so far
    // and verify its declared host functions.
    let check_file = |hir_file: HirFile,
                      module_artifacts: &ModuleArtifactStore,
                      inherited_execution_facts: &crate::execution_facts::CheckedExecutionFacts|
     -> Result<(CompiledFile, NamedSource<Arc<String>>), CompileError> {
        cancellation.checkpoint()?;
        let file_src = hir_file.source.clone();
        let compiled = checking::check_hir_file(
            hir_file,
            module_artifacts,
            inherited_execution_facts,
            &exported_runtime_units,
            &module_resolver,
            &project_types,
            &cancellation,
        )?;
        verify_host_functions(
            plugins,
            &compiled.tir,
            &file_src,
            host_metadata,
            &cancellation,
        )?;
        Ok((compiled, file_src))
    };

    for hir_file in deps {
        let file_dag_id = hir_file.root.dag_id().clone();
        let (compiled, file_src) =
            check_file(hir_file, &module_artifacts, &inherited_execution_facts)?;
        inherited_execution_facts = compiled.checked_execution_facts.clone();
        store_module_artifact(
            compiled,
            &file_dag_id,
            &file_src,
            &mut module_artifacts,
            &cancellation,
        )?;
    }

    let (compiled, source) = check_file(root_file, &module_artifacts, &inherited_execution_facts)?;
    Ok(CheckedProject {
        compiled,
        source,
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
        crate::loader::PluginFileEntry,
    >,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
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
            return Err(CompileError::Eval(GraphcalError::MissingHostFunction {
                plugin: function.plugin.clone(),
                name: function.name.clone(),
                src: src.clone(),
                span: function.name_span.into(),
            }));
        }
        if let Some(provided) = host_metadata.provided_signature(key)
            && !function.signature.structurally_equivalent(provided)
        {
            let format_dim = |dim: &graphcal_compiler::dimension::Dimension| {
                tir.registry().dimensions.format_dimension(dim)
            };
            return Err(CompileError::Eval(GraphcalError::ExternSignatureMismatch {
                plugin: function.plugin.clone(),
                name: function.name.clone(),
                declared: function.signature.format_with(format_dim),
                provided: provided.format_with(format_dim),
                src: src.clone(),
                span: function.decl_span.into(),
            }));
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
        crate::loader::PluginFileEntry,
    >,
    function: &graphcal_compiler::ir::lower::ExternFunctionEntry,
    src: &NamedSource<Arc<String>>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<(), CompileError> {
    match plugins.get(&function.plugin) {
        Some(Err(crate::loader::PluginFileError::NotPinned)) => {
            return Err(CompileError::Eval(GraphcalError::PluginNotPinned {
                plugin: function.plugin.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            }));
        }
        Some(Err(crate::loader::PluginFileError::HashMismatch { expected, actual })) => {
            return Err(CompileError::Eval(GraphcalError::PluginHashMismatch {
                plugin: function.plugin.clone(),
                expected: expected.clone(),
                actual: actual.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            }));
        }
        Some(Err(file_error)) => {
            return Err(CompileError::Eval(GraphcalError::PluginLoadFailed {
                plugin: function.plugin.clone(),
                reason: file_error.to_string(),
                src: src.clone(),
                span: function.path_span.into(),
            }));
        }
        // Defensive: the loader records an entry for every root-package wasm
        // import, so an absent entry means an embedder skipped `load_project`.
        None => {
            return Err(CompileError::Eval(GraphcalError::PluginLoadFailed {
                plugin: function.plugin.clone(),
                reason: "the project loader provided no bytes for this plugin".to_string(),
                src: src.clone(),
                span: function.path_span.into(),
            }));
        }
        Some(Ok(_)) => {}
    }

    match host_metadata.plugin_failure(&function.plugin) {
        Some(crate::host_fns::PluginRegistrationError::ForbiddenImport { module, name }) => {
            Err(CompileError::Eval(GraphcalError::PluginForbiddenImport {
                plugin: function.plugin.clone(),
                import_module: module.clone(),
                import_name: name.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            }))
        }
        Some(crate::host_fns::PluginRegistrationError::LoadFailed { reason }) => {
            Err(CompileError::Eval(GraphcalError::PluginLoadFailed {
                plugin: function.plugin.clone(),
                reason: reason.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            }))
        }
        None => Ok(()),
    }
}
