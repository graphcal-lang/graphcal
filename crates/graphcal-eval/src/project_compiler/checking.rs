//! Static checking from authoritative project HIR to checked TIR.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project checking consumes the shared internal phase model"
)]
use super::*;

fn declared_type_for_target(
    target: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    local_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
) -> Option<DeclaredType> {
    let name = ScopedName::local(target.to_unowned_def_name());
    local_interfaces
        .get(target.owner())
        .and_then(|types| types.get(&name))
        .or_else(|| {
            module_artifacts
                .values()
                .find_map(|artifact| artifact.declared_types_by_dag.get(target.owner()))
                .and_then(|types| types.get(&name))
        })
        .cloned()
}

fn value_for_target(
    target: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
) -> Option<RuntimeValue> {
    module_artifacts
        .get(target.owner())
        .and_then(|artifact| artifact.const_values.get(&target.to_unowned_def_name()))
        .cloned()
}

fn resolve_imported_bindings(
    hir: &graphcal_compiler::ir::lower::HirDag,
    local_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ImportedBinding>, CompileError> {
    hir.imported_bindings()
        .iter()
        .map(|(lexical, hir_binding)| {
            let target = hir_binding.target();
            let declared_type = declared_type_for_target(target, local_interfaces, module_artifacts)
                .ok_or_else(|| {
                    CompileError::Eval(GraphcalError::InternalError {
                        message: format!(
                            "checked interface for HIR import `{lexical}` targeting `{target}` is unavailable"
                        ),
                        src: src.clone(),
                        span: Span::new(0, 0).into(),
                    })
                })?;
            let checked = match value_for_target(target, module_artifacts) {
                Some(value) =>
                    ImportedBinding::with_value(target.clone(), declared_type, value),
                None => ImportedBinding::deferred(target.clone(), declared_type),
            };
            Ok((lexical.clone(), checked))
        })
        .collect()
}

fn local_declared_interfaces(
    hir: &HirFile,
    file_src: &NamedSource<Arc<String>>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    project_types: &graphcal_compiler::tir::typed::ProjectTypeStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<
    HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    CompileError,
> {
    std::iter::once(&hir.root)
        .chain(&hir.inline_dags)
        .map(|dag| {
            graphcal_compiler::tir::typed::resolve_hir_declared_types_with_modules_and_cancellation(
                dag,
                file_src,
                module_resolver,
                project_types,
                cancellation,
            )
            .map(|types| (dag.dag_id().clone(), types))
            .map_err(CompileError::from)
        })
        .collect()
}

/// Check one physical file's root and inline-DAG HIR modules.
pub(super) fn check_hir_file(
    hir: HirFile,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    hir_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, HirModuleArtifact>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    project_types: &graphcal_compiler::tir::typed::ProjectTypeStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CompiledFile, CompileError> {
    cancellation.checkpoint()?;
    let file_src = &hir.source;
    let local_interfaces =
        local_declared_interfaces(&hir, file_src, module_resolver, project_types, cancellation)?;
    let root_bindings =
        resolve_imported_bindings(&hir.root, &local_interfaces, module_artifacts, file_src)?;
    let mut tir = graphcal_compiler::tir::typed::type_resolve_builder_with_imported_bindings_and_cancellation(
        hir.root,
        root_bindings,
        file_src,
        module_resolver,
        project_types,
        cancellation,
    )?;
    if let Some((name, span)) =
        lowering::imported_runtime_unit_reference(tir.root(), &hir.module_map, hir_interfaces)
    {
        return Err(GraphcalError::ImportRuntimeUnit {
            name,
            src: file_src.clone(),
            span: span.into(),
        }
        .into());
    }

    for dag in hir.inline_dags {
        cancellation.checkpoint()?;
        let imported_bindings =
            resolve_imported_bindings(&dag, &local_interfaces, module_artifacts, file_src)?;
        let checked = graphcal_compiler::tir::typed::type_resolve_single_with_imported_bindings_and_cancellation(
            dag,
            imported_bindings,
            file_src,
            module_resolver,
            project_types,
            cancellation,
        )?;
        tir.insert_dag(checked).map_err(|error| {
            CompileError::Eval(GraphcalError::InternalError {
                message: error.to_string(),
                src: file_src.clone(),
                span: Span::new(0, 0).into(),
            })
        })?;
    }

    tir.replace_project_type_store(project_types.clone());
    lowering::merge_dep_dag_tirs(&mut tir, &hir.module_map, module_artifacts, file_src)?;
    let mut tir = tir.finish();
    let override_dependencies = module_artifacts
        .values()
        .flat_map(|artifact| artifact.override_dependencies.iter())
        .map(|(declaration, dependencies)| (declaration.clone(), dependencies.clone()))
        .collect();
    let checked_owners: HashSet<graphcal_compiler::dag_id::DagId> = module_artifacts
        .values()
        .flat_map(|artifact| artifact.declared_types_by_dag.keys().cloned())
        .collect();
    graphcal_compiler::tir::dim_check::reconcile_external_override_dependencies(
        &mut tir,
        &override_dependencies,
        &checked_owners,
    );
    graphcal_compiler::tir::dim_check::check_dimensions_tir_with_cancellation(
        &tir,
        file_src,
        cancellation,
    )?;
    let checked_execution_facts =
        crate::exec_plan::check_execution_facts_with_cancellation(&tir, file_src, cancellation)?;
    let declared_types = tir.build_declared_types(file_src)?;
    let imported_values = tir
        .root()
        .imported_bindings()
        .iter()
        .filter_map(|(name, binding)| {
            binding.value().map(|value| {
                (
                    name.clone(),
                    (value.clone(), binding.declared_type().clone()),
                )
            })
        })
        .collect();

    Ok(CompiledFile {
        tir,
        checked_execution_facts,
        declared_types,
        imported_values,
        imported_source_order: hir.imported_source_order,
        output_surface: hir.output_surface,
        include_debug_names: hir.include_debug_names,
    })
}
