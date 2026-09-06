//! Static checking from authoritative project HIR to checked TIR.

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::imported_binding::ImportedValueKind;
use graphcal_compiler::syntax::module_resolve::{DeclSymbolKind, ModuleResolver};

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project checking consumes the shared internal phase model"
)]
use super::*;

fn declared_type_for_target(
    target: &graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    local_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    module_artifacts: &ModuleArtifactStore,
) -> Option<DeclaredType> {
    let name = ScopedName::local(target.to_unowned_def_name());
    local_interfaces
        .get(target.owner())
        .and_then(|types| types.get(&name))
        .or_else(|| {
            module_artifacts
                .for_owner(target.owner())
                .and_then(|artifact| artifact.declared_types_by_dag.get(target.owner()))
                .and_then(|types| types.get(&name))
        })
        .cloned()
}

fn resolve_imported_bindings(
    hir: &graphcal_compiler::ir::lower::HirDag,
    local_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
    module_artifacts: &ModuleArtifactStore,
    module_resolver: &ModuleResolver,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, ImportedBinding>, CompileError> {
    hir.imported_bindings()
        .iter()
        .map(|(lexical, hir_binding)| {
            let target = hir_binding.target();
            let declared_type = declared_type_for_target(target, local_interfaces, module_artifacts)
                .ok_or_else(|| {
                    CompileError::Eval(GraphcalError::internal_error(
                        format!(
                            "checked interface for HIR import `{lexical}` targeting `{target}` is unavailable"
                        ),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ))
                })?;
            let kind = match module_resolver.decl_symbol_kind(target).map_err(|error| {
                CompileError::Eval(GraphcalError::internal_error(
                    error.to_string(), src, DiagnosticAnchor::WholeFile,
                ))
            })? {
                DeclSymbolKind::Const => ImportedValueKind::Constant,
                DeclSymbolKind::Param | DeclSymbolKind::Node => ImportedValueKind::Runtime,
                actual => return Err(CompileError::Eval(GraphcalError::internal_error(
                    format!("HIR imported value `{target}` has non-value category {actual:?}"),
                    src, DiagnosticAnchor::WholeFile,
                ))),
            };
            let checked = ImportedBinding::new(target.clone(), declared_type, kind);
            Ok((lexical.clone(), checked))
        })
        .collect()
}

fn checked_imported_values(
    tir: &graphcal_compiler::tir::typed::TIR,
    facts: &crate::execution_facts::CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<ScopedName, (RuntimeValue, DeclaredType)>, CompileError> {
    tir.root()
        .imported_bindings()
        .iter()
        .try_fold(HashMap::new(), |mut values, (name, binding)| {
            if let Some(value) = crate::execution_scope::checked_imported_constant(
                tir, facts, binding,
            )
            .map_err(|error| {
                CompileError::Eval(GraphcalError::internal_error(
                    error.to_string(),
                    src,
                    DiagnosticAnchor::WholeFile,
                ))
            })? {
                values.insert(
                    name.clone(),
                    (value.clone(), binding.declared_type().clone()),
                );
            }
            Ok(values)
        })
}

struct ResolvedFileSignatures {
    root: graphcal_compiler::tir::typed::SignatureResolvedHirDag,
    inline: Vec<graphcal_compiler::tir::typed::SignatureResolvedHirDag>,
    interfaces: HashMap<graphcal_compiler::dag_id::DagId, HashMap<ScopedName, DeclaredType>>,
}

/// Resolve every local declaration signature before any body is consumed.
fn resolve_file_signatures(
    root: graphcal_compiler::ir::lower::HirDag,
    inline: Vec<graphcal_compiler::ir::lower::HirDag>,
    file_src: &NamedSource<Arc<String>>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    project_types: &graphcal_compiler::tir::typed::ProjectTypeStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ResolvedFileSignatures, CompileError> {
    let root = graphcal_compiler::tir::typed::resolve_hir_signature_with_modules_and_cancellation(
        root,
        file_src,
        module_resolver,
        project_types,
        cancellation,
    )?;
    let inline = inline
        .into_iter()
        .map(|dag| {
            graphcal_compiler::tir::typed::resolve_hir_signature_with_modules_and_cancellation(
                dag,
                file_src,
                module_resolver,
                project_types,
                cancellation,
            )
            .map_err(CompileError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let interfaces = std::iter::once(&root)
        .chain(&inline)
        .map(|signed| (signed.dag_id().clone(), signed.declared_types().clone()))
        .collect();
    Ok(ResolvedFileSignatures {
        root,
        inline,
        interfaces,
    })
}

fn reconcile_checked_dependency_overrides(
    tir: &mut graphcal_compiler::tir::typed::TIR,
    module_artifacts: &ModuleArtifactStore,
) {
    let dependencies = module_artifacts
        .values()
        .flat_map(|artifact| artifact.override_dependencies.iter())
        .map(|(declaration, dependencies)| (declaration.clone(), dependencies.clone()))
        .collect();
    let checked_owners: HashSet<graphcal_compiler::dag_id::DagId> = module_artifacts
        .values()
        .flat_map(|artifact| artifact.declared_types_by_dag.keys().cloned())
        .collect();
    graphcal_compiler::tir::dim_check::reconcile_external_override_dependencies(
        tir,
        &dependencies,
        &checked_owners,
    );
}

/// Check one physical file's root and inline-DAG HIR modules.
pub(super) fn check_hir_file(
    hir: HirFile,
    module_artifacts: &ModuleArtifactStore,
    inherited_execution_facts: &crate::execution_facts::CheckedExecutionFacts,
    exported_runtime_units: &HashMap<
        graphcal_compiler::dag_id::DagId,
        HashSet<graphcal_compiler::syntax::dimension::UnitName>,
    >,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    project_types: &Arc<graphcal_compiler::tir::typed::ProjectTypeStore>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CompiledFile, CompileError> {
    cancellation.checkpoint()?;
    let file_src = &hir.source;
    let source_declarations = hir.root.source_declarations().to_vec();
    let entry_external_surface = hir.root.external_surface.clone();

    // Pass 1 gives same-file inline DAG imports complete checked interfaces.
    let ResolvedFileSignatures {
        root: signed_root,
        inline: signed_inline,
        interfaces: local_interfaces,
    } = resolve_file_signatures(
        hir.root,
        hir.inline_dags,
        file_src,
        module_resolver,
        project_types.as_ref(),
        cancellation,
    )?;

    // Pass 2 resolves bodies using exactly the signatures retained above.
    let root_bindings = resolve_imported_bindings(
        signed_root.hir(),
        &local_interfaces,
        module_artifacts,
        module_resolver,
        file_src,
    )?;
    let mut tir = graphcal_compiler::tir::typed::type_resolve_signed_builder_with_imported_bindings_and_cancellation(
        signed_root,
        root_bindings,
        file_src,
        module_resolver,
        Arc::clone(project_types),
        cancellation,
    )?;
    lowering::validate_imported_runtime_units(
        tir.root(),
        &hir.module_map,
        exported_runtime_units,
        file_src,
    )?;

    for signed in signed_inline {
        cancellation.checkpoint()?;
        let imported_bindings = resolve_imported_bindings(
            signed.hir(),
            &local_interfaces,
            module_artifacts,
            module_resolver,
            file_src,
        )?;
        let checked = graphcal_compiler::tir::typed::type_resolve_signed_single_with_imported_bindings_and_cancellation(
            signed,
            imported_bindings,
            file_src,
            module_resolver,
            project_types.as_ref(),
            cancellation,
        )?;
        tir.insert_dag(checked).map_err(|error| {
            CompileError::Eval(GraphcalError::internal_error(
                error.to_string(),
                file_src,
                DiagnosticAnchor::WholeFile,
            ))
        })?;
    }

    lowering::install_shared_module_artifacts(
        &mut tir,
        &hir.module_map,
        module_artifacts,
        file_src,
    )?;
    let tir = finish_module_assembly(tir, module_artifacts, file_src, cancellation)?;
    let checked_execution_facts = execution_check::check_execution_facts_with_inherited(
        &tir,
        inherited_execution_facts,
        file_src,
        cancellation,
    )?;
    let declared_types = tir.build_declared_types(file_src)?;
    let entry_interface = entry_interface::build_checked_entry_interface(
        &source_declarations,
        &tir,
        &declared_types,
        &entry_external_surface,
        file_src,
    )?;
    let imported_values = checked_imported_values(&tir, &checked_execution_facts, file_src)?;
    #[cfg(test)]
    observe_shared_artifacts(&tir, project_types, module_artifacts);

    Ok(CompiledFile {
        tir,
        checked_execution_facts,
        entry_interface,
        declared_types,
        imported_values,
        imported_source_order: hir.imported_source_order,
        output_surface: hir.output_surface,
        include_debug_names: hir.include_debug_names,
    })
}

/// Complete mutable local bodies before any execution facts are published.
fn finish_module_assembly(
    builder: graphcal_compiler::tir::typed::TirBuilder,
    module_artifacts: &ModuleArtifactStore,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    let mut tir = builder.finish();
    graphcal_compiler::tir::typed::instantiate_semantic_edges(&mut tir, src)?;
    reconcile_checked_dependency_overrides(&mut tir, module_artifacts);
    graphcal_compiler::tir::dim_check::check_dimensions_tir_with_cancellation(
        &mut tir,
        src,
        cancellation,
    )?;
    // Preparation only borrows these completed constructor targets.
    Ok(tir.with_external_value_constructors())
}

#[cfg(test)]
fn observe_shared_artifacts(
    tir: &graphcal_compiler::tir::typed::TIR,
    project_types: &graphcal_compiler::tir::typed::ProjectTypeStore,
    module_artifacts: &ModuleArtifactStore,
) {
    assert!(
        std::ptr::eq(project_types, tir.project_type_store()),
        "module checking must retain the canonical project type store"
    );
    module_artifacts
        .values()
        .flat_map(|artifact| artifact.dag_store.iter())
        .for_each(|(owner, canonical)| {
            let imported = tir
                .dag_registry()
                .get(owner)
                .expect("installed imported body");
            crate::pipeline_metrics::record_imported_body(canonical, imported);
        });
}
