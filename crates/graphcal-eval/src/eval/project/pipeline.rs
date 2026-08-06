//! Orchestration: top-level compilation and evaluation pipelines for multi-file projects.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// Validate inline-DAG recursion before constructing project-wide scopes.
///
/// This preserves source-level diagnostic ordering: a recursive instance graph
/// is rejected before resolver inheritance attempts to inspect that invalid
/// synthetic scope.
pub(in crate::eval::project) fn validate_project_dag_recursion(
    project: &crate::loader::LoadedProject,
) -> Result<(), CompileError> {
    project.files.values().try_for_each(|loaded_file| {
        let definitions = loaded_file
            .ast
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.kind {
                DeclKind::Dag(dag) => Some((dag.name.value.clone(), dag)),
                _ => None,
            })
            .collect();
        imports::check_dag_recursion(&definitions, &loaded_file.named_source)
    })
}

/// Compile a single file within a project, using dependency artifacts for imports.
///
/// Builds import bindings, lowers to IR, applies any compile-time include
/// bindings, and type-resolves to TIR. The project compilation session calls
/// this exactly once for each file in topological order.
fn compile_single_file_in_project(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    project_types: &mut graphcal_compiler::tir::typed::ProjectTypeStore,
    module_templates: &mut ModuleTemplateStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CompiledFile, CompileError> {
    cancellation.checkpoint()?;
    let loaded_file = &project.files[file_dag_id];
    let file_src = &loaded_file.named_source;

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
        &loaded_file.ast,
        &ctx.imported_names,
        &mut ctx.include_instances,
    );

    // Lower to IR and finalize compilation.
    lowering::lower_and_finalize(
        ProjectSemanticContext {
            project,
            module_resolver,
            project_types,
            module_templates,
        },
        file_dag_id,
        file_src,
        &file_ast,
        ctx,
        module_artifacts,
        cancellation,
    )
}

fn tir_has_required_indexes(tir: &graphcal_compiler::tir::typed::TIR) -> bool {
    tir.root_declared_indexes()
        .any(graphcal_compiler::registry::types::IndexDef::is_required)
        || tir
            .root()
            .semantic()
            .collection_refs
            .index_defs
            .values()
            .any(|definition| definition.is_required())
}

fn first_required_index_diagnostic(
    tir: &graphcal_compiler::tir::typed::TIR,
    ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> Option<(String, miette::SourceSpan)> {
    for idx_def in tir.root_declared_indexes() {
        if idx_def.is_required() {
            let span = ast
                .declarations
                .iter()
                .find_map(|d| {
                    if let DeclKind::Index(idx) = &d.kind
                        && idx.name.value.as_str() == idx_def.name.as_str()
                    {
                        Some(d.span.into())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| miette::SourceSpan::from((0, 0)));
            return Some((idx_def.name.to_string(), span));
        }
    }

    tir.root()
        .semantic()
        .collection_refs
        .index_defs
        .values()
        .find(|idx_def| idx_def.is_required())
        .map(|idx_def| {
            // Owner-qualified required indexes can enter through module type
            // resolution. The reference span is not retained in index_defs, so
            // point at the importing file as a whole rather than fabricating a
            // declaration span.
            (idx_def.name.to_string(), miette::SourceSpan::from((0, 0)))
        })
}

fn top_level_const_values(
    tir: &graphcal_compiler::tir::typed::TIR,
    const_values: &crate::eval_expr::RuntimeValueMap,
) -> HashMap<DeclName, RuntimeValue> {
    // Top-level consts are exposed by leaf name at the project import
    // boundary; internal eval routing uses canonical declaration keys.
    tir.root()
        .consts()
        .iter()
        .filter_map(|entry| {
            let key = crate::decl_key::RuntimeDeclKey::for_local_decl(tir.root(), &entry.name);
            const_values
                .get(&key)
                .cloned()
                .map(|value| (entry.name.member().clone(), value))
        })
        .collect()
}

pub(super) const fn output_decl_type(category: DeclCategory) -> Option<DeclType> {
    match category {
        DeclCategory::Const => Some(DeclType::Const),
        DeclCategory::Param => Some(DeclType::Param),
        DeclCategory::Node => Some(DeclType::Node),
        DeclCategory::Assert | DeclCategory::Plot | DeclCategory::Figure | DeclCategory::Layer => {
            None
        }
    }
}

pub(super) fn remap_include_debug_name(
    name: &ScopedName,
    aliases: &IncludeDebugNameMap,
) -> ScopedName {
    let Some((first, rest)) = name.qualifier().split_first() else {
        return name.clone();
    };
    let Some(display) = aliases.get(first) else {
        return name.clone();
    };
    ScopedName::qualified_path(
        std::iter::once(display.clone()).chain(rest.iter().cloned()),
        name.member().clone(),
    )
}

/// Replace private synthetic include scopes with unambiguous human-readable
/// target leaves at the presentation boundary.
pub(super) fn apply_include_debug_names(result: &mut EvalResult, aliases: &IncludeDebugNameMap) {
    if aliases.is_empty() {
        return;
    }

    result
        .consts
        .iter_mut()
        .chain(result.params.iter_mut())
        .chain(result.nodes.iter_mut())
        .for_each(|(name, _)| *name = remap_include_debug_name(name, aliases));
    result
        .all
        .iter_mut()
        .for_each(|(name, _, _)| *name = remap_include_debug_name(name, aliases));
    result.output_surface = std::mem::take(&mut result.output_surface)
        .into_iter()
        .map(|name| remap_include_debug_name(&name, aliases))
        .collect();
    result
        .assertions
        .iter_mut()
        .for_each(|(name, _, _)| *name = remap_include_debug_name(name, aliases));
    result
        .plots
        .iter_mut()
        .for_each(|plot| plot.name = remap_include_debug_name(&plot.name, aliases));
    result
        .plot_errors
        .iter_mut()
        .for_each(|plot| plot.name = remap_include_debug_name(&plot.name, aliases));
    result.figures.iter_mut().for_each(|figure| {
        figure.name = remap_include_debug_name(&figure.name, aliases);
        figure
            .plot_names
            .iter_mut()
            .for_each(|name| *name = remap_include_debug_name(name, aliases));
    });
    result.layers.iter_mut().for_each(|layer| {
        layer.name = remap_include_debug_name(&layer.name, aliases);
        layer
            .plot_names
            .iter_mut()
            .for_each(|name| *name = remap_include_debug_name(name, aliases));
    });
    result.assumes_map = std::mem::take(&mut result.assumes_map)
        .into_iter()
        .map(|(name, assumers)| {
            (
                remap_include_debug_name(&name, aliases),
                assumers
                    .into_iter()
                    .map(|assumer| remap_include_debug_name(&assumer, aliases))
                    .collect(),
            )
        })
        .collect();
    result.domain_constraints = std::mem::take(&mut result.domain_constraints)
        .into_iter()
        .map(|(name, constraint)| (remap_include_debug_name(&name, aliases), constraint))
        .collect();
}

pub(super) fn push_output_value(
    (name, result, decl_type): OutputValue,
    consts: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    params: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    nodes: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    all: &mut Vec<OutputValue>,
) {
    match decl_type {
        DeclType::Const => consts.push((name.clone(), result.clone())),
        DeclType::Param => params.push((name.clone(), result.clone())),
        DeclType::Node => nodes.push((name.clone(), result.clone())),
    }
    all.push((name, result, decl_type));
}

/// Store one pure compile-time module artifact for downstream imports.
fn store_module_artifact(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    external_surface: ExternalDeclSurface,
    module_artifacts: &mut HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    let top_level_consts = top_level_const_values(
        &compiled.tir,
        compiled.checked_execution_facts.const_values.as_ref(),
    );
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
            const_values: top_level_consts,
            declared_types: compiled.declared_types,
            frontend_registry: compiled.tir.registry().clone(),
            external_surface,
            override_dependencies,
            dag_tirs,
            extern_functions,
        },
    );
    Ok(())
}

/// Compile a complete project into one reusable checked continuation.
///
/// Dependencies produce compile-time artifacts only. No runtime schedule,
/// assertion, dynamic scale, plot, or host call is executed by this loop.
pub(in crate::eval::project) fn check_project_perfile(
    project: &crate::loader::LoadedProject,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
    module_resolver: graphcal_compiler::syntax::module_resolve::ModuleResolver,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CheckedProject, CompileError> {
    cancellation.checkpoint()?;
    let mut module_artifacts: HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact> =
        HashMap::new();
    let root_source = &project.files[&project.root].named_source;
    let mut project_types = graphcal_compiler::tir::typed::ProjectTypeStore::default();
    let mut module_templates = ModuleTemplateStore::default();
    project_types
        .insert_graphcal_prelude()
        .map_err(|error| GraphcalError::InternalError {
            message: format!("failed to build prelude project type store: {error}"),
            src: root_source.clone(),
            span: Span::new(0, 0).into(),
        })?;

    for file_dag_id in &project.load_order {
        cancellation.checkpoint()?;
        let compiled = compile_single_file_in_project(
            project,
            file_dag_id,
            &module_artifacts,
            &module_resolver,
            &mut project_types,
            &mut module_templates,
            cancellation,
        )?;
        let loaded_file = &project.files[file_dag_id];
        let file_src = &loaded_file.named_source;
        verify_host_functions(
            project,
            file_dag_id,
            &compiled.tir,
            file_src,
            host_metadata,
            cancellation,
        )?;

        if *file_dag_id == project.root {
            return Ok(CheckedProject {
                compiled,
                source: file_src.clone(),
                root_ast: loaded_file.ast.clone(),
                module_resolver,
            });
        }

        let external_surface = extract_external_decl_surface(&loaded_file.ast);
        store_module_artifact(
            compiled,
            file_dag_id,
            file_src,
            external_surface,
            &mut module_artifacts,
            cancellation,
        )?;
    }

    Err(CompileError::Eval(GraphcalError::InternalError {
        message: "root file not found in project load order".to_string(),
        src: NamedSource::new("internal", Arc::new(String::new())),
        span: Span::new(0, 0).into(),
    }))
}

/// Continue a checked project to one reusable execution plan.
pub(in crate::eval::project) fn prepare_checked_project(
    checked: CheckedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<super::prepared::PreparedProject, CompileError> {
    cancellation.checkpoint()?;
    let CheckedProject {
        compiled,
        source,
        root_ast,
        module_resolver,
    } = checked;
    if tir_has_required_indexes(&compiled.tir)
        && let Some((name, span)) = first_required_index_diagnostic(&compiled.tir, &root_ast)
    {
        return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
            name,
            src: source,
            span,
        }));
    }

    let plan = crate::exec_plan::compile_checked_with_cancellation(
        &compiled.tir,
        &compiled.checked_execution_facts,
        &source,
        cancellation,
    )?;
    super::prepared::PreparedProject::from_compiled(
        compiled,
        plan,
        source,
        host_fns.clone(),
        module_resolver,
        &root_ast,
    )
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
    project: &crate::loader::LoadedProject,
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
            verify_wasm_plugin(project, file_dag_id, function, src, host_metadata)?;
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
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    function: &graphcal_compiler::ir::lower::ExternFunctionEntry,
    src: &NamedSource<Arc<String>>,
    host_metadata: &crate::host_fns::HostFunctionMetadata,
) -> Result<(), CompileError> {
    if file_dag_id.package() != project.root.package() {
        return Err(CompileError::Eval(
            GraphcalError::PluginInDependencyPackage {
                plugin: function.plugin.clone(),
                src: src.clone(),
                span: function.path_span.into(),
            },
        ));
    }

    match project.plugins.get(&function.plugin) {
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
