//! Orchestration: top-level compilation and evaluation pipelines for multi-file projects.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// Compile a single file within a project, using dependency artifacts for imports.
///
/// Builds import bindings, lowers to IR, applies any compile-time include
/// bindings, and type-resolves to TIR. Both [`prepare_project_perfile`] and
/// [`compile_to_tir_project_perfile`] call this for each file in the project.
fn compile_single_file_in_project(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CompiledFile, CompileError> {
    cancellation.checkpoint()?;
    let loaded_file = &project.files[file_dag_id];
    let file_src = &loaded_file.named_source;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_values: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        extra_registry_builders: Vec::new(),
        deferred_dag_includes: Vec::new(),
        included_debug_values: Vec::new(),
        included_plot_specs: Vec::new(),
    };

    imports::process_file_body_declarations(
        project,
        file_dag_id,
        evaluated_files,
        &mut ctx,
        cancellation,
    )?;

    // For module imports/includes, resolve qualified references in body and
    // deferred binding expressions before lowering either representation.
    let file_ast = rewrite_qualified_refs_in_compilation_body(
        &loaded_file.ast,
        &ctx.imported_names,
        &mut ctx.deferred_dag_includes,
    );

    // Lower to IR and finalize compilation.
    lowering::lower_and_finalize(
        project,
        file_dag_id,
        file_src,
        &file_ast,
        ctx,
        evaluated_files,
        cancellation,
    )
}

fn tir_has_required_indexes(tir: &graphcal_compiler::tir::typed::TIR) -> bool {
    tir.registry
        .indexes
        .declared_indexes()
        .any(graphcal_compiler::registry::types::IndexDef::is_required)
        || tir
            .root()
            .semantic
            .collection_refs
            .index_defs
            .values()
            .any(graphcal_compiler::registry::types::IndexDef::is_required)
}

fn tir_requires_runtime_inputs(tir: &graphcal_compiler::tir::typed::TIR) -> bool {
    tir.root().params.iter().any(|p| p.default_expr.is_none()) || tir_has_required_indexes(tir)
}

fn first_required_index_diagnostic(
    tir: &graphcal_compiler::tir::typed::TIR,
    ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> Option<(String, miette::SourceSpan)> {
    for idx_def in tir.registry.indexes.declared_indexes() {
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
        .semantic
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
        .consts
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
    (name, result, decl_type): EvaluatedOutputValue,
    consts: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    params: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    nodes: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    all: &mut Vec<EvaluatedOutputValue>,
) {
    match decl_type {
        DeclType::Const => consts.push((name.clone(), result.clone())),
        DeclType::Param => params.push((name.clone(), result.clone())),
        DeclType::Node => nodes.push((name.clone(), result.clone())),
    }
    all.push((name, result, decl_type));
}

/// Assemble the complete evaluated state retained at a project-file boundary.
///
/// The direct result contains this file's own and merged values. Empty-argument
/// file includes remain pre-evaluated artifacts, so their complete debug values
/// and their selected imported aliases are added here without changing runtime
/// semantics.
fn assemble_file_output_values(
    compiled: &CompiledFile,
    eval_result: &EvalResult,
) -> Vec<EvaluatedOutputValue> {
    let mut values = eval_result.all.clone();
    let mut seen: HashSet<ScopedName> = values.iter().map(|(name, _, _)| name.clone()).collect();

    values.extend(
        compiled
            .included_debug_values
            .iter()
            .filter(|(name, _, _)| seen.insert(name.clone()))
            .cloned(),
    );
    values.extend(
        compiled
            .imported_source_order
            .iter()
            .filter_map(|(name, category)| {
                if !seen.insert(name.clone()) {
                    return None;
                }
                let decl_type = output_decl_type(*category)?;
                let (runtime, declared_type) = compiled.imported_values.get(name)?;
                let value = super::super::runtime::runtime_to_value(
                    runtime,
                    Some(declared_type),
                    &compiled.tir,
                );
                Some((name.clone(), Ok(value), decl_type))
            }),
    );
    values
}

/// Store a compiled-but-not-evaluated non-root file for downstream compile-time imports.
///
/// Library files with required params or indexes cannot produce runtime values
/// standalone, but their registry, declared types, consts, and DAG metadata are
/// still needed by importers.
fn store_compiled_file_artifact(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    external_surface: ExternalDeclSurface,
    evaluated_files: &mut HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    let const_values = crate::exec_plan::eval_consts_from_tir_with_cancellation(
        &compiled.tir,
        file_src,
        cancellation,
    )?;
    let top_level_consts = top_level_const_values(&compiled.tir, &const_values);
    let dag_tirs = compiled.tir.dags.clone();
    let extern_functions = compiled.tir.extern_functions.clone();

    evaluated_files.insert(
        file_dag_id.clone(),
        EvaluatedFile {
            runtime_available: false,
            plots: HashMap::new(),
            values: HashMap::new(),
            const_values: top_level_consts,
            evaluated_values: Vec::new(),
            declared_types: compiled.declared_types,
            assertions: HashMap::new(),
            registry: compiled.tir.registry,
            external_surface,
            resolved_dynamic_unit_scales: HashMap::new(),
            dag_tirs,
            extern_functions,
        },
    );
    Ok(())
}

/// Evaluate and store a non-root file, producing an [`EvaluatedFile`] for downstream imports.
fn evaluate_and_store_file(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    external_surface: ExternalDeclSurface,
    evaluated_files: &mut HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    cancellation.checkpoint()?;
    let plan = crate::exec_plan::compile_with_cancellation(&compiled.tir, file_src, cancellation)?;
    // One eval-loop run yields both the public result and the raw values
    // exported to downstream imports (this used to evaluate the whole file
    // twice).
    let (mut eval_result, runtime_values) =
        super::super::runtime::evaluate_plan_with_values_and_cancellation(
            &compiled.tir,
            &plan,
            &compiled.declared_types,
            file_src,
            host_fns,
            cancellation,
        )?;
    apply_include_debug_names(&mut eval_result, &compiled.include_debug_names);
    let file_runtime_values = filter_local_runtime_values(&compiled.tir, &runtime_values);
    let top_level_consts = top_level_const_values(&compiled.tir, &plan.const_values);
    // Dynamic-unit scales resolve against this file's final values here, so
    // module importers can carry the units across as static scales.
    let resolved_dynamic_unit_scales = super::super::runtime::export_dynamic_unit_scales(
        &compiled.tir,
        &plan,
        &runtime_values,
        file_src,
        cancellation,
    )?;

    // Capture dag TIRs so cross-file qualified inline calls can merge them
    // into the importer's TIR::dags under module-prefixed keys.
    let dag_tirs = compiled.tir.dags.clone();
    let extern_functions = compiled.tir.extern_functions.clone();

    // Evaluated plot specs, requestable by consumers through include brace
    // lists (#847). Includes this file's own plots and the ones it included
    // itself (for `pub`-item re-export chains); keys are local leaf names.
    let plots: HashMap<DeclName, crate::eval::PlotSpec> = eval_result
        .plots
        .iter()
        .chain(compiled.included_plots.iter())
        .filter(|spec| !spec.name.is_qualified())
        .map(|spec| (spec.name.member().clone(), spec.clone()))
        .collect();
    let evaluated_values = assemble_file_output_values(&compiled, &eval_result);

    evaluated_files.insert(
        file_dag_id.clone(),
        EvaluatedFile {
            runtime_available: true,
            plots,
            values: file_runtime_values,
            const_values: top_level_consts,
            evaluated_values,
            declared_types: compiled.declared_types,
            assertions: eval_result
                .assertions
                .into_iter()
                .map(|(name, result, span)| (name, (result, span)))
                .collect(),
            registry: compiled.tir.registry,
            external_surface,
            resolved_dynamic_unit_scales,
            dag_tirs,
            extern_functions,
        },
    );
    Ok(())
}

/// Compile a complete project into one reusable root prepared plan.
///
/// Dependencies are still compiled/evaluated in topological order so imported
/// constants and standalone library values retain their existing semantics.
/// The root file stops after plan construction and output-assembly capture.
pub(in crate::eval::project) fn prepare_project_perfile(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<super::prepared::PreparedProject, CompileError> {
    cancellation.checkpoint()?;
    let mut evaluated_files: HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile> =
        HashMap::new();

    for file_dag_id in &project.load_order {
        cancellation.checkpoint()?;
        let is_root = *file_dag_id == project.root;
        let compiled =
            compile_single_file_in_project(project, file_dag_id, &evaluated_files, cancellation)?;
        let file_src = &project.files[file_dag_id].named_source;
        verify_host_functions(
            project,
            file_dag_id,
            &compiled.tir,
            file_src,
            host_fns,
            cancellation,
        )?;

        if is_root {
            if tir_has_required_indexes(&compiled.tir)
                && let Some((name, span)) =
                    first_required_index_diagnostic(&compiled.tir, &project.files[file_dag_id].ast)
            {
                return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
                    name,
                    src: file_src.clone(),
                    span,
                }));
            }

            let plan =
                crate::exec_plan::compile_with_cancellation(&compiled.tir, file_src, cancellation)?;
            let dep_import_spans = build_dep_import_spans(project);
            let mut dependency_assertions = Vec::new();
            for dep_dag_id in &project.load_order {
                cancellation.checkpoint()?;
                if *dep_dag_id == project.root {
                    continue;
                }
                if let Some(dep_eval) = evaluated_files.get(dep_dag_id) {
                    let import_span = dep_import_spans
                        .get(dep_dag_id)
                        .copied()
                        .unwrap_or(Span::new(0, 0));
                    dependency_assertions.extend(
                        dep_eval
                            .assertions
                            .iter()
                            .map(|(name, (result, _))| (name.clone(), result.clone(), import_span)),
                    );
                }
            }
            let module_resolver = project.build_module_resolver().map_err(|error| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: error.to_string(),
                    src: file_src.clone(),
                    span: Span::new(0, 0).into(),
                })
            })?;
            return super::prepared::PreparedProject::from_compiled(
                compiled,
                plan,
                file_src.clone(),
                host_fns.clone(),
                module_resolver,
                &project.files[file_dag_id].ast,
                dependency_assertions,
            );
        }

        let external_surface = extract_external_decl_surface(&project.files[file_dag_id].ast);
        if tir_requires_runtime_inputs(&compiled.tir) {
            store_compiled_file_artifact(
                compiled,
                file_dag_id,
                file_src,
                external_surface,
                &mut evaluated_files,
                cancellation,
            )?;
        } else {
            evaluate_and_store_file(
                compiled,
                file_dag_id,
                file_src,
                external_surface,
                &mut evaluated_files,
                host_fns,
                cancellation,
            )?;
        }
    }

    Err(CompileError::Eval(GraphcalError::InternalError {
        message: "root file not found in project load order".to_string(),
        src: NamedSource::new("internal", Arc::new(String::new())),
        span: Span::new(0, 0).into(),
    }))
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
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    use graphcal_compiler::syntax::plugin::PluginSourceKind;

    // Deterministic reporting order: earliest declaration first.
    let mut declared: Vec<_> = tir.extern_functions.iter().collect();
    declared.sort_by_key(|(_, function)| function.name_span.offset());

    for (key, function) in declared {
        cancellation.checkpoint()?;
        if key.plugin.source_kind() == PluginSourceKind::WasmModule {
            verify_wasm_plugin(project, file_dag_id, function, src, host_fns)?;
        }
        if !host_fns.contains(key) {
            return Err(CompileError::Eval(GraphcalError::MissingHostFunction {
                plugin: function.plugin.clone(),
                name: function.name.clone(),
                src: src.clone(),
                span: function.name_span.into(),
            }));
        }
        if let Some(provided) = host_fns.provided_signature(key)
            && !function.signature.structurally_equivalent(provided)
        {
            let render = |signature: &graphcal_compiler::function_signature::FunctionSignature| {
                signature.format_with(|dim| tir.registry.dimensions.format_dimension(dim))
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
    host_fns: &crate::host_fns::HostFunctionRegistry,
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

    match host_fns.plugin_failure(&function.plugin) {
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

/// Map each dependency file to the root-level import statement span that brought it in.
///
/// Direct imports get the span of their own `import` declaration in the root file.
/// Transitive imports inherit the root-level import span of the direct import
/// that started the chain. When a transitive dependency is reachable from multiple
/// root imports, the first root import in source order wins.
fn build_dep_import_spans(
    project: &crate::loader::LoadedProject,
) -> HashMap<graphcal_compiler::dag_id::DagId, Span> {
    let root_file = &project.files[&project.root];
    let mut spans: HashMap<graphcal_compiler::dag_id::DagId, Span> = HashMap::new();

    // Process root's direct imports/includes in source order.
    // For each, DFS into its transitive dependencies, propagating the root span.
    // `entry().or_insert()` ensures the first root import/include (in source order) to reach
    // a transitive dep determines its attribution.
    let root_decl_dag_ids: Vec<(Span, graphcal_compiler::dag_id::DagId)> = root_file
        .imports_with_dag_ids()
        .map(|(d, _, c)| (d.span, c.clone()))
        .chain(
            root_file
                .includes_with_dag_ids()
                .map(|(d, _, c)| (d.span, c.clone())),
        )
        .collect();
    for (root_span, canonical) in root_decl_dag_ids {
        let mut stack = vec![canonical];
        while let Some(dag_id) = stack.pop() {
            if dag_id == project.root {
                continue;
            }
            // Only process if not already attributed.
            if let std::collections::hash_map::Entry::Vacant(entry) = spans.entry(dag_id.clone()) {
                entry.insert(root_span);
                // Push this file's own imports/includes for transitive propagation.
                if let Some(file) = project.files.get(&dag_id) {
                    for (_decl, _imp, c) in file.imports_with_dag_ids() {
                        if !spans.contains_key(c) {
                            stack.push(c.clone());
                        }
                    }
                    for (_decl, _inc, c) in file.includes_with_dag_ids() {
                        if !spans.contains_key(c) {
                            stack.push(c.clone());
                        }
                    }
                }
            }
        }
    }

    spans
}

/// Compile a project to TIR using per-file evaluation.
///
/// Non-root files are compiled to dependency artifacts. Files that can run
/// standalone are evaluated to produce `RuntimeValue`s for downstream imports;
/// library files with required runtime inputs keep compile-time artifacts only.
/// The root file stops at TIR and returns it.
pub(in crate::eval::project) fn compile_to_tir_project_perfile(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    cancellation.checkpoint()?;
    let mut evaluated_files: HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile> =
        HashMap::new();

    for file_dag_id in &project.load_order {
        cancellation.checkpoint()?;
        let is_root = *file_dag_id == project.root;
        let compiled =
            compile_single_file_in_project(project, file_dag_id, &evaluated_files, cancellation)?;

        // Compile-only consumers (`graphcal check`, LSP analysis) must
        // report the same load-time extern diagnostics evaluation does
        // (P003, P005–P010): a declaration whose plugin is missing or
        // whose manifest disagrees is a compile error, not something to
        // discover on the first evaluation.
        verify_host_functions(
            project,
            file_dag_id,
            &compiled.tir,
            &project.files[file_dag_id].named_source,
            host_fns,
            cancellation,
        )?;

        if is_root {
            return Ok(compiled.tir);
        }

        // Skip standalone evaluation for files with required params or indexes,
        // while still retaining compile-time artifacts for downstream imports.
        if tir_requires_runtime_inputs(&compiled.tir) {
            let file_src = &project.files[file_dag_id].named_source;
            let external_surface = extract_external_decl_surface(&project.files[file_dag_id].ast);
            store_compiled_file_artifact(
                compiled,
                file_dag_id,
                file_src,
                external_surface,
                &mut evaluated_files,
                cancellation,
            )?;
            continue;
        }

        let file_src = &project.files[file_dag_id].named_source;
        let external_surface = extract_external_decl_surface(&project.files[file_dag_id].ast);
        evaluate_and_store_file(
            compiled,
            file_dag_id,
            file_src,
            external_surface,
            &mut evaluated_files,
            host_fns,
            cancellation,
        )?;
    }

    let internal_src = NamedSource::new("internal", Arc::new(String::new()));
    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: internal_src,
        span: (0, 0).into(),
    }))
}

/// Filter an evaluated runtime-value map to only locally-defined param/node
/// values (not imported, not consts) for passing to downstream files.
///
/// Pure filter over an already-evaluated map: the eval loop runs once in
/// [`evaluate_plan_with_values`](super::super::runtime::evaluate_plan_with_values),
/// not again here.
fn filter_local_runtime_values(
    tir: &graphcal_compiler::tir::typed::TIR,
    values: &crate::eval_expr::RuntimeValueMap,
) -> HashMap<DeclName, RuntimeValue> {
    tir.root()
        .params
        .iter()
        .map(|e| &e.name)
        .chain(tir.root().nodes.iter().map(|e| &e.name))
        .filter_map(|name| {
            let key = crate::decl_key::RuntimeDeclKey::for_local_decl(tir.root(), name);
            values
                .get(&key)
                .cloned()
                .map(|value| (name.member().clone(), value))
        })
        .collect()
}
