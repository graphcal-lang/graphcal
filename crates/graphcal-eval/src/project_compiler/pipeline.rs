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
    project.files().values().try_for_each(|loaded_file| {
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
    file_dag_id: &graphcal_compiler::dag_id::DagId,
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
    let loaded_file = &project.files()[file_dag_id];
    let file_src = loaded_file.named_source();

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        frontend_registry_imports: Vec::new(),
        include_instances: Vec::new(),
    };

    imports::process_file_body_declarations(
        project,
        file_dag_id,
        module_artifacts,
        &mut ctx,
        cancellation,
    )?;

    // Resolve qualified references in both the body and pending include
    // bindings before lowering either representation.
    let file_ast = rewrite_qualified_refs_in_compilation_body(
        loaded_file.ast(),
        &ctx.imported_names,
        &mut ctx.include_instances,
    );

    let (hir, root_interface) = lowering::lower_file_to_hir(
        ProjectSemanticContext {
            project,
            module_resolver,
            module_templates,
        },
        file_dag_id,
        file_src,
        &file_ast,
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

fn dag_const_values(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    const_values: &crate::eval_expr::RuntimeValueMap,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<DeclName, RuntimeValue>, GraphcalError> {
    Ok(dag
        .consts()
        .iter()
        .map(|entry| {
            let identity = dag.require_bound_decl_identity(
                &entry.name,
                src,
                DiagnosticAnchor::Source(entry.span),
            )?;
            Ok(const_values
                .get(&crate::decl_key::RuntimeDeclKey::resolved(identity))
                .cloned()
                .map(|value| (entry.name.member().clone(), value)))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?
        .into_iter()
        .flatten()
        .collect())
}

/// Store one pure compile-time module artifact for downstream imports.
fn store_module_artifact(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &mut HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    let const_values_by_dag = compiled
        .tir
        .local_dags()
        .map(|(dag_id, dag)| {
            let facts = compiled
                .checked_execution_facts
                .for_dag(dag_id)
                .ok_or_else(|| {
                    CompileError::Eval(GraphcalError::internal_error(
                        format!("checked module artifact is missing DAG `{dag_id}`"),
                        file_src,
                        DiagnosticAnchor::WholeFile,
                    ))
                })?;
            Ok((
                dag_id.clone(),
                dag_const_values(dag, &facts.const_values, file_src)?,
            ))
        })
        .collect::<Result<HashMap<_, _>, CompileError>>()?;
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
    let dag_tirs = compiled.tir.dag_registry().clone();
    let extern_functions = compiled.tir.extern_functions().clone();

    module_artifacts.insert(
        file_dag_id.clone(),
        ModuleArtifact {
            checked_execution_facts: compiled.checked_execution_facts,
            const_values_by_dag,
            declared_types_by_dag,
            override_dependencies,
            dag_tirs,
            extern_functions,
        },
    );
    Ok(())
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
    let mut files = HashMap::new();
    let mut module_interfaces = HashMap::new();
    let mut module_templates = ModuleTemplateStore::default();

    for file_dag_id in project.load_order() {
        cancellation.checkpoint()?;
        let (hir, lowering_interfaces) = lower_single_file_to_hir(
            project,
            file_dag_id,
            &module_interfaces,
            &module_resolver,
            &mut module_templates,
            cancellation,
        )?;
        module_interfaces.extend(lowering_interfaces);
        files.insert(file_dag_id.clone(), hir);
    }

    if !files.contains_key(project.root_id()) {
        let internal_source = NamedSource::new("internal", Arc::new(String::new()));
        return Err(CompileError::Eval(GraphcalError::internal_error(
            "root file not found in project load order",
            &internal_source,
            DiagnosticAnchor::Builtin,
        )));
    }

    let exported_dynamic_units = module_interfaces
        .iter()
        .map(|(owner, interface)| (owner.clone(), interface.exported_dynamic_units().clone()))
        .collect();

    Ok(HirProject {
        root: project.root_id().clone(),
        load_order: project.load_order().to_vec(),
        files,
        plugins: project.plugins(),
        exported_dynamic_units,
        module_resolver,
        cancellation: cancellation.clone(),
    })
}

fn build_project_type_store(
    hir: &HirProject<'_>,
) -> Result<graphcal_compiler::tir::typed::ProjectTypeStore, CompileError> {
    let root_source = &hir.files[&hir.root].source;
    let mut project_types = graphcal_compiler::tir::typed::ProjectTypeStore::default();
    project_types.insert_graphcal_prelude().map_err(|error| {
        GraphcalError::internal_error(
            format!("failed to build prelude project type store: {error}"),
            root_source,
            DiagnosticAnchor::Builtin,
        )
    })?;
    for file_dag_id in &hir.load_order {
        let file = hir.files.get(file_dag_id).ok_or_else(|| {
            CompileError::Eval(GraphcalError::internal_error(
                format!("HIR module `{file_dag_id}` is unavailable"),
                root_source,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
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
    Ok(project_types)
}

/// Consume a complete HIR project and perform all mandatory static checks.
pub(in crate::project_compiler) fn check_hir_project(
    hir: HirProject<'_>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<CheckedProject, CompileError> {
    hir.cancellation.checkpoint()?;
    let project_types = build_project_type_store(&hir)?;
    let HirProject {
        root,
        load_order,
        mut files,
        plugins,
        exported_dynamic_units,
        module_resolver,
        cancellation,
    } = hir;
    let root_source = files
        .get(&root)
        .map(|file| file.source.clone())
        .ok_or_else(|| {
            let internal_source = NamedSource::new("internal", Arc::new(String::new()));
            CompileError::Eval(GraphcalError::internal_error(
                "root HIR module is unavailable before checking",
                &internal_source,
                DiagnosticAnchor::Builtin,
            ))
        })?;
    let mut module_artifacts = HashMap::new();

    for file_dag_id in &load_order {
        cancellation.checkpoint()?;
        let hir_file = files.remove(file_dag_id).ok_or_else(|| {
            CompileError::Eval(GraphcalError::internal_error(
                format!("HIR module `{file_dag_id}` was already consumed or is missing"),
                &root_source,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
        let file_src = hir_file.source.clone();
        let compiled = checking::check_hir_file(
            hir_file,
            &module_artifacts,
            &exported_dynamic_units,
            &module_resolver,
            &project_types,
            &cancellation,
        )?;
        verify_host_functions(
            root.package(),
            plugins,
            file_dag_id,
            &compiled.tir,
            &file_src,
            host_metadata,
            &cancellation,
        )?;

        if *file_dag_id == root {
            return Ok(CheckedProject {
                compiled,
                source: file_src,
                module_resolver,
            });
        }

        store_module_artifact(
            compiled,
            file_dag_id,
            &file_src,
            &mut module_artifacts,
            &cancellation,
        )?;
    }

    Err(CompileError::Eval(GraphcalError::internal_error(
        "root HIR module was not checked",
        &root_source,
        DiagnosticAnchor::WholeFile,
    )))
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
    root_package: &graphcal_compiler::dag_id::DagPackageId,
    plugins: &HashMap<
        graphcal_compiler::syntax::plugin::PluginPath,
        crate::loader::PluginFileEntry,
    >,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    tir: &graphcal_compiler::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    use graphcal_compiler::syntax::plugin::PluginSourceKind;

    // Deterministic reporting order: earliest declaration first.
    let mut declared: Vec<_> = tir.extern_functions().iter().collect();
    declared.sort_by_key(|(_, function)| function.name_span.offset());

    for (key, function) in declared {
        cancellation.checkpoint()?;
        if key.plugin.source_kind() == PluginSourceKind::WasmModule {
            verify_wasm_plugin(
                root_package,
                plugins,
                file_dag_id,
                function,
                src,
                host_metadata,
            )?;
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
            let render = |signature: &graphcal_compiler::function_signature::FunctionSignature| {
                signature.format_with(|dim| tir.registry().dimensions.format_dimension(dim))
            };
            return Err(CompileError::Eval(GraphcalError::ExternSignatureMismatch {
                plugin: function.plugin.clone(),
                name: function.name.clone(),
                declared: render(&function.signature),
                provided: render(provided),
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
    root_package: &graphcal_compiler::dag_id::DagPackageId,
    plugins: &HashMap<
        graphcal_compiler::syntax::plugin::PluginPath,
        crate::loader::PluginFileEntry,
    >,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    function: &graphcal_compiler::ir::lower::ExternFunctionEntry,
    src: &NamedSource<Arc<String>>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<(), CompileError> {
    if file_dag_id.package() != root_package {
        return Err(CompileError::Eval(
            GraphcalError::PluginInDependencyPackage {
                plugin: function.plugin.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            },
        ));
    }

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
