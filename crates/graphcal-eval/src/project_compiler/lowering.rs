//! HIR lowering, registry composition, and include elaboration for projects.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use super::*;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration, ExprKind};
use graphcal_compiler::syntax::span::Spanned;

use super::generic_leakage::{check_generics_leakage, collect_local_type_names};
use super::registry_merge::{merge_registry_into_builder, seed_imported_type_system};

fn reject_runtime_units_at_include_boundary(
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    registry
        .units
        .all_units()
        .filter(|(_, _, scale)| scale.is_dynamic())
        .map(|(unit, _, _)| unit)
        .min()
        .map_or(Ok(()), |unit| {
            Err(CompileError::Eval(GraphcalError::IncludeRuntimeUnit {
                name: unit.clone(),
                src: src.clone(),
                span: span.into(),
            }))
        })
}

fn imported_module_target<'map>(
    alias: &ModuleAliasName,
    module_map: &'map HashMap<ModuleAliasName, ProjectModuleBinding>,
) -> Option<&'map graphcal_compiler::dag_id::DagId> {
    module_map.get(alias).and_then(|binding| {
        (binding.role == graphcal_compiler::syntax::module_resolve::ModuleAliasRole::ImportedDag)
            .then_some(&binding.target)
    })
}

fn is_imported_dynamic_unit_during_lowering(
    alias: &ModuleAliasName,
    name: &graphcal_compiler::syntax::dimension::UnitName,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    module_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
) -> bool {
    imported_module_target(alias, module_map).is_some_and(|target| {
        module_interfaces
            .get(target)
            .is_some_and(|interface| interface.is_exported_dynamic_unit(name))
    })
}

fn remap_imported_dynamic_unit_error(
    error: GraphcalError,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    module_interfaces: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
) -> GraphcalError {
    match error {
        GraphcalError::UnknownUnit { name, src, span }
            if name.qualifier().is_some_and(|alias| {
                is_imported_dynamic_unit_during_lowering(
                    alias,
                    name.name(),
                    module_map,
                    module_interfaces,
                )
            }) =>
        {
            GraphcalError::ImportRuntimeUnit {
                name: name.to_string(),
                src,
                span,
            }
        }
        other => other,
    }
}

pub(super) fn imported_runtime_unit_reference(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    exported_dynamic_units: &HashMap<
        graphcal_compiler::dag_id::DagId,
        HashSet<graphcal_compiler::syntax::dimension::UnitName>,
    >,
) -> Option<(String, Span)> {
    let mut invalid = None;
    dag.visit_unit_references(&mut |unit, span| {
        if invalid.is_none()
            && unit.spelling().qualifier().is_some_and(|alias| {
                imported_module_target(alias, module_map).is_some_and(|target| {
                    exported_dynamic_units
                        .get(target)
                        .is_some_and(|names| names.contains(unit.spelling().name()))
                })
            })
        {
            invalid = Some((unit.to_string(), span));
        }
    });
    invalid
}

fn include_debug_name_map(ctx: &ImportContext<'_>) -> IncludeDebugNameMap {
    let mut leaf_counts: HashMap<ModuleAliasName, usize> = HashMap::new();
    ctx.include_instances
        .iter()
        .filter(|include| matches!(include.instance_scope, IncludeInstanceScope::Anonymous(_)))
        .for_each(|include| {
            leaf_counts
                .entry(include.debug_scope.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        });

    ctx.include_instances
        .iter()
        .filter(|include| matches!(include.instance_scope, IncludeInstanceScope::Anonymous(_)))
        .filter(|include| leaf_counts.get(&include.debug_scope) == Some(&1))
        .filter(|include| !ctx.module_map.contains_key(&include.debug_scope))
        .map(|include| {
            (
                include.instance_scope.merge_scope_name(),
                include.debug_scope.clone(),
            )
        })
        .collect()
}

/// Lower one physical file and every inline DAG it defines into authoritative HIR.
///
/// This phase performs elaboration and canonical reference resolution only.
/// It does not resolve checked declaration types, evaluate constants, verify
/// host signatures, or construct TIR.
pub(in crate::project_compiler) fn lower_file_to_hir(
    semantic_context: ProjectSemanticContext<'_>,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::desugar::desugared_ast::File,
    ctx: ImportContext<'_>,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(HirFile, LoweringModuleInterface), CompileError> {
    cancellation.checkpoint()?;
    let ProjectSemanticContext {
        project,
        module_resolver,
        module_templates,
    } = semantic_context;
    let include_debug_names = include_debug_name_map(&ctx);

    let mut registry_seed = |builder: &mut RegistryBuilder| {
        seed_imported_type_system(
            builder,
            project,
            &ctx.imported_type_system_names,
            &ctx.frontend_registry_imports,
            module_artifacts,
            file_src,
        )
    };
    let (mut builder, mut unfrozen) =
        graphcal_compiler::ir::lower::lower_to_builder_with_imported_bindings_and_cancellation(
            file_ast,
            file_src,
            &ctx.imported_names,
            ctx.imported_bindings,
            file_dag_id,
            Some(&mut registry_seed),
            cancellation,
        )
        .map_err(|error| {
            remap_imported_dynamic_unit_error(error, &ctx.module_map, module_artifacts)
        })?;

    let output_surface: HashSet<ScopedName> = unfrozen
        .source_order
        .iter()
        .chain(ctx.imported_source_order.iter())
        .filter(|(_, category)| {
            matches!(
                category,
                DeclCategory::Const | DeclCategory::Param | DeclCategory::Node
            )
        })
        .map(|(name, _)| name.clone())
        .chain(
            ctx.include_instances
                .iter()
                .flat_map(|include| include.surface_outputs.iter().cloned()),
        )
        .collect();

    elaborate_include_instances(
        project,
        file_dag_id,
        &ctx.include_instances,
        module_artifacts,
        module_templates,
        file_src,
        file_ast,
        &mut builder,
        &mut unfrozen,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let registry = builder
        .try_build()
        .map_err(|err| registry_build_compile_error(&err, file_src))?;
    let frontend_registry = registry.clone();
    let root = store_and_freeze_module_template(
        module_templates,
        file_dag_id,
        unfrozen,
        registry,
        module_resolver,
        file_src,
        cancellation,
    )?;
    let inline_dags = lower_inline_dag_modules(
        project,
        file_dag_id,
        file_src,
        &frontend_registry,
        module_artifacts,
        module_resolver,
        module_templates,
        cancellation,
    )?;

    let lowering_interface =
        LoweringModuleInterface::new(frontend_registry, root.external_surface.clone());
    Ok((
        HirFile {
            source: file_src.clone(),
            root,
            inline_dags,
            imported_source_order: ctx.imported_source_order,
            output_surface,
            include_debug_names,
            module_map: ctx.module_map,
        },
        lowering_interface,
    ))
}

pub(super) fn module_resolve_compile_error(
    err: graphcal_compiler::syntax::module_resolve::ModuleResolveError,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    match err {
        graphcal_compiler::syntax::module_resolve::ModuleResolveError::PrivateName {
            owner,
            name,
            ..
        } => CompileError::Eval(GraphcalError::ImportPrivateItem {
            name,
            file_path: owner.to_string(),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        }),
        graphcal_compiler::syntax::module_resolve::ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            span,
        } => CompileError::Eval(GraphcalError::ImportCategoryMismatch {
            file_path: owner.to_string(),
            mismatch,
            src: src.clone(),
            span: span.into(),
        }),
        graphcal_compiler::syntax::module_resolve::ModuleResolveError::DuplicateSymbol {
            name,
            first,
            duplicate,
            ..
        }
        | graphcal_compiler::syntax::module_resolve::ModuleResolveError::DuplicateImportName {
            name,
            first,
            duplicate,
            ..
        } => CompileError::Eval(GraphcalError::DuplicateName {
            name,
            src: src.clone(),
            duplicate: duplicate.into(),
            first: first.into(),
        }),
        other => CompileError::Eval(GraphcalError::EvalError {
            message: other.to_string(),
            src: src.clone(),
            span: Span::new(0, 0).into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "inline DAG HIR lowering threads project interfaces and template state"
)]
fn lower_inline_dag_modules<'a>(
    project: &'a crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    parent_registry: &Registry,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    module_templates: &mut ModuleTemplateStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<graphcal_compiler::ir::lower::HirDag>, CompileError> {
    let loaded_file = &project.files()[file_dag_id];

    loaded_file
        .inline_dags()
        .iter()
        .map(|loaded_dag| {
            cancellation.checkpoint()?;
            let dag_body = loaded_dag.body(loaded_file);
            compile_loaded_dag_module_ir(
                parent_registry,
                project,
                loaded_dag,
                dag_body,
                file_src,
                module_artifacts,
                module_resolver,
                module_templates,
                cancellation,
            )
        })
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one template-lowering transaction threads project semantic services and cancellation"
)]
fn compile_loaded_dag_module_ir<'a>(
    parent_registry: &Registry,
    project: &'a crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    dag_body: &[Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    module_templates: &mut ModuleTemplateStore,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::lower::HirDag, CompileError> {
    cancellation.checkpoint()?;
    if let Some(template) = module_templates.get(loaded_dag.dag_id()) {
        return freeze_inline_module_template(
            &template,
            loaded_dag.dag_id(),
            module_resolver,
            file_src,
            cancellation,
        );
    }
    let parent_loaded = &project.files()[loaded_dag.parent_dag_id()];
    let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
        dag_body,
        loaded_dag.parent_dag_id(),
        parent_loaded.ast(),
        loaded_dag.resolved_imports(),
        file_src,
    )?;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        frontend_registry_imports: Vec::new(),
        include_instances: Vec::new(),
    };

    process_dag_body_import_declarations(
        project,
        loaded_dag,
        dag_body,
        file_src,
        module_artifacts,
        &mut ctx,
    )?;
    process_dag_body_include_declarations(
        project,
        loaded_dag,
        dag_body,
        file_src,
        module_artifacts,
        &mut ctx,
    )?;

    extend_imported_value_names(&mut ctx.imported_names, self_imports.names);
    extend_imported_bindings(
        &mut ctx.imported_bindings,
        self_imports.bindings,
        &ctx.imported_names,
        file_src,
    )?;

    let dag_ast = graphcal_compiler::desugar::desugared_ast::File {
        declarations: self_imports.stripped_body,
    };
    let dag_ast = rewrite_qualified_refs_in_compilation_body(
        &dag_ast,
        &ctx.imported_names,
        &mut ctx.include_instances,
    );
    let mut registry_seed = |builder: &mut RegistryBuilder| {
        seed_imported_type_system(
            builder,
            project,
            &ctx.imported_type_system_names,
            &ctx.frontend_registry_imports,
            module_artifacts,
            file_src,
        )
    };
    let (mut builder, mut unfrozen) =
        graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
            dag_ast.as_ref(),
            Some(parent_registry),
            &ctx.imported_names,
            ctx.imported_bindings,
            file_src,
            loaded_dag.dag_id(),
            Some(&mut registry_seed),
            cancellation,
        )?;

    elaborate_include_instances(
        project,
        loaded_dag.dag_id(),
        &ctx.include_instances,
        module_artifacts,
        module_templates,
        file_src,
        dag_ast.as_ref(),
        &mut builder,
        &mut unfrozen,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let registry = builder
        .try_build()
        .map_err(|err| registry_build_compile_error(&err, file_src))?;
    store_and_freeze_module_template(
        module_templates,
        loaded_dag.dag_id(),
        unfrozen,
        registry,
        module_resolver,
        file_src,
        cancellation,
    )
}

fn freeze_inline_module_template(
    template: &ElaboratedModuleTemplate,
    dag_id: &graphcal_compiler::dag_id::DagId,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::lower::HirDag, CompileError> {
    Ok(template.unfrozen.clone().freeze_with_cancellation(
        template.frontend_registry.clone(),
        dag_id,
        module_resolver,
        src,
        cancellation,
    )?)
}

fn store_and_freeze_module_template(
    module_templates: &mut ModuleTemplateStore,
    dag_id: &graphcal_compiler::dag_id::DagId,
    unfrozen: graphcal_compiler::ir::lower::UnfrozenIR,
    registry: Registry,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::lower::HirDag, CompileError> {
    let template_unfrozen = unfrozen.clone();
    let template_registry = registry.clone();
    let frozen =
        unfrozen.freeze_with_cancellation(registry, dag_id, module_resolver, src, cancellation)?;
    module_templates.insert(
        dag_id.clone(),
        ElaboratedModuleTemplate {
            unfrozen: template_unfrozen,
            frontend_registry: template_registry,
            external_surface: frozen.external_surface.clone(),
        },
    );
    Ok(frozen)
}

fn registry_build_compile_error(
    err: &graphcal_compiler::registry::types::RegistryBuildError,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    CompileError::Eval(GraphcalError::InternalError {
        message: format!("registry build failed: {err}"),
        src: src.clone(),
        span: Span::new(0, 0).into(),
    })
}

fn extend_imported_value_names(target: &mut ImportedValueNames, source: ImportedValueNames) {
    target.const_names.extend(source.const_names);
    target.param_names.extend(source.param_names);
    target.node_names.extend(source.node_names);
    target.assert_names.extend(source.assert_names);
}

fn extend_imported_bindings(
    target: &mut HashMap<ScopedName, HirImportedBinding>,
    source: HashMap<ScopedName, HirImportedBinding>,
    imported_names: &ImportedValueNames,
    src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    for (name, binding) in source {
        if target.contains_key(&name) {
            let mut spans = imported_names
                .const_names
                .iter()
                .chain(&imported_names.param_names)
                .chain(&imported_names.node_names)
                .filter_map(|(candidate, span)| (candidate == &name).then_some(*span));
            let first = spans.next().unwrap_or(Span::new(0, 0));
            let duplicate = spans.next_back().unwrap_or(first);
            return Err(CompileError::Eval(GraphcalError::DuplicateName {
                name: name.to_string(),
                src: src.clone(),
                duplicate: duplicate.into(),
                first: first.into(),
            }));
        }
        target.insert(name, binding);
    }
    Ok(())
}

fn process_dag_body_import_declarations<'a>(
    project: &crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    dag_body: &[Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    for decl in dag_body {
        let DeclKind::Import(import_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::InlineBodyImportResolution::Resolved(target)) = loaded_dag
            .resolved_imports()
            .get(&crate::loader::ModulePathKey::from_path(&import_decl.path))
        else {
            continue;
        };
        if target.target() == loaded_dag.parent_dag_id() {
            continue;
        }
        imports::process_pure_import(
            project,
            target,
            &import_decl.path,
            &import_decl.kind,
            file_src,
            module_artifacts,
            ctx,
        )?;
    }
    Ok(())
}

fn process_dag_body_include_declarations<'a>(
    project: &'a crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    dag_body: &[Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    for decl in dag_body {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::InlineBodyImportResolution::Resolved(target)) = loaded_dag
            .resolved_imports()
            .get(&crate::loader::ModulePathKey::from_path(&include_decl.path))
        else {
            continue;
        };
        if target.target() == target.source_file() {
            imports::process_file_include(
                project,
                target.source_file(),
                include_decl,
                decl,
                file_src,
                module_artifacts,
                ctx,
            )?;
            continue;
        }

        let Some((target_file, target_dag)) = project.inline_dag(target.target()) else {
            continue;
        };
        let target_file_id = target.source_file();
        let boundary = if target_file_id == loaded_dag.parent_dag_id() {
            imports::IncludeVisibilityBoundary::Local
        } else {
            imports::IncludeVisibilityBoundary::CrossModule
        };
        imports::process_inline_dag_include(
            &imports::InlineDagIncludeTarget {
                dag_def: target_dag.declaration(target_file),
                dag_id: target.target(),
                dag_name: target.target().name(),
                parent_dag_id: target_file_id,
                boundary,
            },
            include_decl,
            decl,
            file_src,
            ctx,
        )?;
    }
    Ok(())
}

/// Merge every loaded dependency's compiled DAGs into the importer's flat
/// canonical registry and record this file root's source alias mappings.
///
/// Imports inside inline DAG bodies have their own lexical scopes and therefore
/// do not appear in the file root's `module_map`. Their HIR calls already carry
/// canonical targets, so all loaded dependency TIRs must be available here.
/// Visibility is enforced while HIR resolves each import edge; retaining private
/// dependency DAGs internally is also necessary when a public DAG calls one of
/// its private implementation children.
///
/// Each cloned DAG TIR also receives the dep-file values named by the DAG
/// body's explicit imports, so `import dep.{const as local}` resolves under the
/// local alias at inline-call eval time.
pub(super) fn merge_dep_dag_tirs(
    tir: &mut graphcal_compiler::tir::typed::TirBuilder,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, ModuleArtifact>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    for (alias, binding) in module_map {
        // Imported aliases name their exact reusable DAG target. Included
        // aliases are namespaces for an existing instance and are not callable.
        if binding.role == graphcal_compiler::syntax::module_resolve::ModuleAliasRole::ImportedDag {
            tir.insert_module_alias(alias.clone(), binding.target.clone());
        }
    }

    for (dep_dag_id, dep_eval) in module_artifacts {
        // Extern signatures travel with the dep's dag bodies: a qualified
        // inline call into a dep dag that uses extern functions resolves its
        // signature from the importer's merged TIR at eval time. First
        // insertion wins; conflicting cross-file redeclarations are rejected
        // when the declaring files themselves compile.
        for (key, function) in &dep_eval.extern_functions {
            tir.insert_extern_function_if_absent(key.clone(), function.clone());
        }
        for (dep_id, dag_tir) in &dep_eval.dag_tirs {
            if dep_id != dep_dag_id && !dep_id.is_descendant_of(dep_dag_id) {
                // `dep_eval` may itself contain canonical TIRs merged from its
                // dependencies. Their owning artifacts are visited separately.
                continue;
            }
            let mut cloned = dag_tir.clone();
            // Supply compile-time values imported from this owning dependency.
            // Canonical target and declared type remain on the same binding
            // record; no independently keyed source/value maps can diverge.
            for (owner, const_values) in &dep_eval.const_values_by_dag {
                if let Some(declared_types) = dep_eval.declared_types_by_dag.get(owner) {
                    cloned.supply_imported_values(owner, const_values, declared_types);
                }
            }
            tir.insert_dag(cloned).map_err(|error| {
                CompileError::Eval(GraphcalError::InternalError {
                    message: error.to_string(),
                    src: src.clone(),
                    span: Span::new(0, 0).into(),
                })
            })?;
        }
    }
    Ok(())
}

/// Elaborate every file-root, same-file inline, and cross-file qualified
/// include through one concrete-instance path:
///
/// 1. Resolve the include's source — file include reads the dep's full
///    AST; inline DAG include reads the dag block's body and pre-processes
///    `import <self>.{...}` against the dag's parent file (Concept 9: a
///    DAG's `<self>` is its file of definition, regardless of where the
///    include sits).
/// 2. Assemble the body with canonical imported targets set up.
/// 3. Capture importer-owned index binding candidates, merge the body's registry,
///    then validate every candidate against the body's effective typed contract.
/// 4. Preserve canonical A8/V005 reconciliation facts and run
///    `check_generics_leakage` (A9/V006).
/// 5. Merge the dependency assembly into the importer with prefix/bindings.
/// 6. For selective includes, add `local_name = @prefix::orig_name` aliases.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline function threads project, importer, module artifacts, and HIR builders"
)]
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive include pipeline: source resolution, registry merge, validation, HIR merge"
)]
fn elaborate_include_instances(
    project: &crate::loader::LoadedProject,
    importer_dag_id: &graphcal_compiler::dag_id::DagId,
    include_instances: &[IncludeInstanceRequest],
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_templates: &mut ModuleTemplateStore,
    importer_src: &NamedSource<Arc<String>>,
    importer_ast: &graphcal_compiler::desugar::desugared_ast::File,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let importer_external_surface = super::extract_external_decl_surface(importer_ast);
    let importer_local_type_names = collect_local_type_names(importer_ast);
    let mut source_order_blocks = Vec::new();

    for instance in include_instances {
        cancellation.checkpoint()?;
        let merge_prefix = instance.instance_scope.merge_scope_name();
        // ---- 1. Resolve and assemble source body -----------------------------
        let (mut dep_unfrozen, dep_registry, dep_src, dep_resolution_owner, body_decls_for_aliases) =
            if let Some(template) = module_templates.get(&instance.template.dag_id) {
                let source_file = &project.files()[&instance.template.source_file];
                let declarations = if instance.template.is_file_root() {
                    source_file.ast().declarations.as_slice()
                } else {
                    source_file
                        .inline_dags()
                        .iter()
                        .find(|loaded| loaded.dag_id() == &instance.template.dag_id)
                        .map(|loaded| loaded.body(source_file))
                        .ok_or_else(|| {
                            CompileError::Eval(GraphcalError::InternalError {
                                message: format!(
                                    "inline DAG template `{}` is unavailable",
                                    instance.template.dag_id
                                ),
                                src: importer_src.clone(),
                                span: instance.include_span.into(),
                            })
                        })?
                };
                (
                    template.unfrozen.clone(),
                    template.frontend_registry.clone(),
                    source_file.named_source().clone(),
                    instance.template.dag_id.clone(),
                    declarations,
                )
            } else if instance.template.is_file_root() {
                let dep_dag_id = &instance.template.dag_id;
                let dep_loaded = &project.files()[dep_dag_id];
                let dep_src = dep_loaded.named_source();
                let mut body_ctx = ImportContext {
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
                    dep_dag_id,
                    module_artifacts,
                    &mut body_ctx,
                    cancellation,
                )?;
                let rewritten_body = rewrite_qualified_refs_in_compilation_body(
                    dep_loaded.ast(),
                    &body_ctx.imported_names,
                    &mut body_ctx.include_instances,
                );
                let mut registry_seed = |builder: &mut RegistryBuilder| {
                    seed_imported_type_system(
                        builder,
                        project,
                        &body_ctx.imported_type_system_names,
                        &body_ctx.frontend_registry_imports,
                        module_artifacts,
                        dep_src,
                    )
                };
                let (mut dep_builder, mut dep_unfrozen) = graphcal_compiler::ir::lower::lower_to_builder_with_imported_bindings_and_cancellation(
                        rewritten_body.as_ref(),
                        dep_src,
                        &body_ctx.imported_names,
                        body_ctx.imported_bindings,
                        dep_dag_id,
                        Some(&mut registry_seed),
                        cancellation,
                    )?;
                dep_unfrozen.retarget_existing_resolution_owners(dep_dag_id);
                elaborate_include_instances(
                    project,
                    dep_dag_id,
                    &body_ctx.include_instances,
                    module_artifacts,
                    module_templates,
                    dep_src,
                    rewritten_body.as_ref(),
                    &mut dep_builder,
                    &mut dep_unfrozen,
                    cancellation,
                )?;
                let dep_registry = dep_builder
                    .try_build()
                    .map_err(|err| registry_build_compile_error(&err, dep_src))?;
                let template = module_templates.insert(
                    dep_dag_id.clone(),
                    ElaboratedModuleTemplate {
                        unfrozen: dep_unfrozen,
                        frontend_registry: dep_registry,
                        external_surface: super::extract_external_decl_surface(dep_loaded.ast()),
                    },
                );
                (
                    template.unfrozen.clone(),
                    template.frontend_registry.clone(),
                    dep_src.clone(),
                    dep_dag_id.clone(),
                    dep_loaded.ast().declarations.as_slice(),
                )
            } else {
                let dag_id = &instance.template.dag_id;
                let parent_dag_id = &instance.template.source_file;
                let parent_loaded = &project.files()[parent_dag_id];
                let loaded_inline = parent_loaded
                    .inline_dags()
                    .iter()
                    .find(|loaded| loaded.dag_id() == dag_id)
                    .ok_or_else(|| {
                        CompileError::Eval(GraphcalError::InternalError {
                            message: format!("inline DAG template `{dag_id}` is unavailable"),
                            src: importer_src.clone(),
                            span: instance.include_span.into(),
                        })
                    })?;
                let inline_body = loaded_inline.body(parent_loaded);
                let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
                    inline_body,
                    parent_dag_id,
                    parent_loaded.ast(),
                    loaded_inline.resolved_imports(),
                    importer_src,
                )?;

                let mut body_ctx = ImportContext {
                    imported_names: ImportedValueNames::default(),
                    imported_bindings: HashMap::new(),
                    imported_source_order: Vec::new(),
                    imported_type_system_names: HashMap::new(),
                    module_map: HashMap::new(),
                    frontend_registry_imports: Vec::new(),
                    include_instances: Vec::new(),
                };
                process_dag_body_import_declarations(
                    project,
                    loaded_inline,
                    inline_body,
                    importer_src,
                    module_artifacts,
                    &mut body_ctx,
                )?;
                process_dag_body_include_declarations(
                    project,
                    loaded_inline,
                    inline_body,
                    importer_src,
                    module_artifacts,
                    &mut body_ctx,
                )?;
                extend_imported_value_names(&mut body_ctx.imported_names, self_imports.names);
                let mut imported_bindings = body_ctx.imported_bindings;
                extend_imported_bindings(
                    &mut imported_bindings,
                    self_imports.bindings,
                    &body_ctx.imported_names,
                    importer_src,
                )?;
                let stripped_body = graphcal_compiler::desugar::desugared_ast::File {
                    declarations: self_imports.stripped_body,
                };
                let stripped_body = rewrite_qualified_refs_in_compilation_body(
                    &stripped_body,
                    &body_ctx.imported_names,
                    &mut body_ctx.include_instances,
                );

                let mut registry_seed = |builder: &mut RegistryBuilder| {
                    seed_imported_type_system(
                        builder,
                        project,
                        &body_ctx.imported_type_system_names,
                        &body_ctx.frontend_registry_imports,
                        module_artifacts,
                        importer_src,
                    )
                };
                let (mut dag_builder, mut dag_unfrozen) = graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
                        stripped_body.as_ref(),
                        None,
                        &body_ctx.imported_names,
                        imported_bindings,
                        importer_src,
                        dag_id,
                        Some(&mut registry_seed),
                        cancellation,
                    )?;
                dag_unfrozen.retarget_existing_resolution_owners(dag_id);
                elaborate_include_instances(
                    project,
                    dag_id,
                    &body_ctx.include_instances,
                    module_artifacts,
                    module_templates,
                    importer_src,
                    stripped_body.as_ref(),
                    &mut dag_builder,
                    &mut dag_unfrozen,
                    cancellation,
                )?;
                let dag_registry = dag_builder
                    .try_build()
                    .map_err(|err| registry_build_compile_error(&err, importer_src))?;
                let template = module_templates.insert(
                    dag_id.clone(),
                    ElaboratedModuleTemplate {
                        unfrozen: dag_unfrozen,
                        frontend_registry: dag_registry,
                        external_surface: super::extract_external_decl_surface(importer_ast),
                    },
                );
                (
                    template.unfrozen.clone(),
                    template.frontend_registry.clone(),
                    importer_src.clone(),
                    dag_id.clone(),
                    inline_body,
                )
            };

        reject_runtime_units_at_include_boundary(
            &dep_registry,
            importer_src,
            instance.include_span,
        )?;

        // Capture importer-owned candidates before the dependency registry is
        // merged, so a dependency declaration with the same leaf name cannot
        // accidentally satisfy its own binding.
        let index_binding_candidates = capture_index_binding_candidates(
            builder,
            &instance.index_bindings,
            &instance.index_binding_spans,
            importer_src,
            instance.include_span,
        )?;

        // ---- 2. Merge dep registry into importer's --------------------
        merge_registry_into_builder(
            builder,
            &dep_registry,
            &instance.index_bindings,
            &instance.type_bindings,
            &instance.dim_bindings,
        )
        .map_err(|conflict| {
            CompileError::Eval(GraphcalError::ConflictingImportedUnit {
                name: conflict.name,
                src: importer_src.clone(),
                span: instance.include_span.into(),
            })
        })?;

        // ---- 3. Validate typed index binding contracts -------------------
        validate_index_binding_contracts(
            body_decls_for_aliases,
            &dep_registry,
            &instance.index_bindings,
            &instance.index_binding_spans,
            &instance.dim_bindings,
            &index_binding_candidates,
            builder,
            importer_src,
            instance.include_span,
        )?;

        // ---- 4. Validation checks -----------------------------------------
        let mut dep_names: HashSet<DeclName> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.member().clone());
        }
        dep_unfrozen.record_include_override_reconciliations(
            &instance.bindings,
            &instance.index_bindings,
            &instance.type_bindings,
            &dep_registry,
            &dep_resolution_owner,
            importer_dag_id,
            importer_src,
            instance.include_span,
        )?;
        check_generics_leakage(
            body_decls_for_aliases,
            &instance.pub_reexport_items,
            &instance.index_bindings,
            &instance.type_bindings,
            &instance.dim_bindings,
            &importer_external_surface,
            &importer_local_type_names,
            importer_src,
            instance.include_span,
        )?;

        // ---- 5. Merge dependency assembly into importer ---------------------
        let source_order_start = unfrozen.source_order.len();
        unfrozen.merge_dependency(
            dep_unfrozen,
            &merge_prefix,
            &instance.bindings,
            &dep_names,
            &instance.index_bindings,
            &instance.type_bindings,
            &instance.dim_bindings,
            &instance.assertion_aliases,
            &instance.import_item_attributes,
            &instance.requested_plots,
            &dep_resolution_owner,
            importer_dag_id,
            importer_src,
            &dep_src,
        )?;

        // ---- 6. Add selective aliases -------------------------------------
        if let Some(selective) = &instance.selective_names {
            add_selective_aliases_inner(
                body_decls_for_aliases,
                selective,
                &merge_prefix,
                &AliasSubstitutions {
                    index: &instance.index_bindings,
                    r#type: &instance.type_bindings,
                    dim: &instance.dim_bindings,
                },
                &AliasResolutionOwners {
                    r#type: &dep_resolution_owner,
                    body: importer_dag_id,
                },
                instance.include_span,
                unfrozen,
            );
        }
        source_order_blocks.push((
            instance.include_span,
            unfrozen.source_order.split_off(source_order_start),
        ));
    }
    unfrozen.interleave_merged_source_order(source_order_blocks);
    Ok(())
}

fn index_binding_span(
    dep_index: &IndexName,
    spans: &HashMap<IndexName, Span>,
    include_span: Span,
) -> Span {
    spans.get(dep_index).copied().unwrap_or(include_span)
}

/// Capture candidates from the importer before dependency declarations are merged.
fn capture_index_binding_candidates(
    builder: &mut RegistryBuilder,
    bindings: &IndexBindings,
    spans: &HashMap<IndexName, Span>,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<HashMap<IndexName, graphcal_compiler::registry::types::IndexDef>, CompileError> {
    bindings
        .iter()
        .map(|(dep_index, target)| {
            let candidate = match target {
                IndexBindingTarget::Declared(importer_index) => builder
                    .get_index(importer_index.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        CompileError::Eval(GraphcalError::IndexBindingNotAnIndex {
                            dep_index: dep_index.to_string(),
                            value: target.to_string(),
                            src: importer_src.clone(),
                            span: index_binding_span(dep_index, spans, include_span).into(),
                        })
                    })?,
                IndexBindingTarget::Finite(finite) => {
                    builder.ensure_finite_index(finite.cardinality());
                    builder.get_finite_index(*finite).cloned().ok_or_else(|| {
                        CompileError::Eval(GraphcalError::InternalError {
                            message: format!(
                                "registered structural index `{target}` is unavailable"
                            ),
                            src: importer_src.clone(),
                            span: index_binding_span(dep_index, spans, include_span).into(),
                        })
                    })?
                }
            };
            Ok((dep_index.clone(), candidate))
        })
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "validation needs both module contracts, binding substitutions, candidates, and diagnostic provenance"
)]
fn validate_index_binding_contracts(
    dep_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dep_registry: &Registry,
    bindings: &IndexBindings,
    spans: &HashMap<IndexName, Span>,
    dim_bindings: &HashMap<DimName, DimName>,
    candidates: &HashMap<IndexName, graphcal_compiler::registry::types::IndexDef>,
    builder: &RegistryBuilder,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    use graphcal_compiler::registry::types::IndexBindingContractError;

    for (dep_index, target) in bindings {
        let span = index_binding_span(dep_index, spans, include_span);
        let contract = effective_index_binding_contract(
            dep_index,
            dep_declarations,
            dep_registry,
            dim_bindings,
            builder,
            importer_src,
            span,
        )?;
        let candidate = candidates.get(dep_index).ok_or_else(|| {
            CompileError::Eval(GraphcalError::InternalError {
                message: format!("captured index binding candidate for `{dep_index}` was lost"),
                src: importer_src.clone(),
                span: span.into(),
            })
        })?;

        match contract.validate(candidate) {
            Ok(()) => {}
            Err(IndexBindingContractError::KindMismatch { expected, found }) => {
                return Err(CompileError::Eval(GraphcalError::IndexKindMismatch {
                    dep_index: dep_index.to_string(),
                    dep_kind: expected.to_string(),
                    bound_index: target.to_string(),
                    bound_kind: found.to_string(),
                    src: importer_src.clone(),
                    span: span.into(),
                }));
            }
            Err(IndexBindingContractError::DimensionMismatch { expected, found }) => {
                return Err(CompileError::Eval(
                    GraphcalError::IndexBindingDimensionMismatch {
                        dep_index: dep_index.to_string(),
                        expected_dim: builder.format_dimension(&expected),
                        bound_index: target.to_string(),
                        found_dim: builder.format_dimension(&found),
                        src: importer_src.clone(),
                        span: span.into(),
                    },
                ));
            }
        }
    }
    Ok(())
}

fn effective_index_binding_contract(
    dep_index: &IndexName,
    dep_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dep_registry: &Registry,
    dim_bindings: &HashMap<DimName, DimName>,
    builder: &RegistryBuilder,
    importer_src: &NamedSource<Arc<String>>,
    binding_span: Span,
) -> Result<graphcal_compiler::registry::types::IndexBindingContract, CompileError> {
    use graphcal_compiler::desugar::desugared_ast::{DeclKind, IndexDeclKind};
    use graphcal_compiler::registry::dimension_registry::DimensionResolveError;
    use graphcal_compiler::registry::types::{IndexBindingContract, IndexKind};

    let definition = dep_registry
        .indexes
        .get_index(dep_index.as_str())
        .ok_or_else(|| {
            CompileError::Eval(GraphcalError::InternalError {
                message: format!(
                    "bound dependency index `{dep_index}` is missing from its registry"
                ),
                src: importer_src.clone(),
                span: binding_span.into(),
            })
        })?;

    match &definition.kind {
        IndexKind::Named { .. } => Ok(IndexBindingContract::Named),
        IndexKind::RequiredNamed => Ok(IndexBindingContract::Discrete),
        IndexKind::Coordinate(data) => Ok(IndexBindingContract::Coordinate {
            dimension: data.dimension.clone(),
        }),
        IndexKind::RequiredCoordinate { .. } => {
            let mut dimension_expr = dep_declarations
                .iter()
                .find_map(|declaration| match &declaration.kind {
                    DeclKind::Index(index) if index.name.value == *dep_index => match &index.kind {
                        IndexDeclKind::RequiredCoordinate { dimension } => Some(dimension.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .ok_or_else(|| {
                    CompileError::Eval(GraphcalError::InternalError {
                        message: format!(
                            "required coordinate index `{dep_index}` has no source declaration"
                        ),
                        src: importer_src.clone(),
                        span: binding_span.into(),
                    })
                })?;
            graphcal_compiler::ir::lower::substitute_dim_expr_names(
                &mut dimension_expr,
                dim_bindings,
            );
            let dimension =
                builder
                    .resolve_dim_expr_detailed(&dimension_expr)
                    .map_err(|error| {
                        CompileError::Eval(match error {
                            DimensionResolveError::UnknownDimension { name } => {
                                GraphcalError::UnknownDimension {
                                    name,
                                    src: importer_src.clone(),
                                    span: binding_span.into(),
                                }
                            }
                            DimensionResolveError::Overflow(_) => {
                                GraphcalError::DimensionOverflow {
                                    src: importer_src.clone(),
                                    span: binding_span.into(),
                                }
                            }
                        })
                    })?;
            Ok(IndexBindingContract::Coordinate { dimension })
        }
        IndexKind::Finite { .. } => Err(CompileError::Eval(GraphcalError::InternalError {
            message: format!("declared dependency index `{dep_index}` became structural"),
            src: importer_src.clone(),
            span: binding_span.into(),
        })),
    }
}

/// Bindings that an alias's type annotation must be rewritten through before
/// it is registered in the importer's HIR assembly. Shared by both inline-DAG and
/// file-include alias paths so their type-substitution stays in lock-step.
struct AliasSubstitutions<'a> {
    pub index: &'a IndexBindings,
    pub r#type: &'a HashMap<StructTypeName, StructTypeName>,
    pub dim: &'a HashMap<DimName, DimName>,
}

struct AliasResolutionOwners<'a> {
    r#type: &'a graphcal_compiler::dag_id::DagId,
    body: &'a graphcal_compiler::dag_id::DagId,
}

/// Add `local_name = @prefix::orig_name` aliases (const or graph) for each
/// selected item, rewriting the type annotation through `subs` so it lands
/// in the importer's merged registry.
///
/// `decls` is the dep's source list — the dag body's declarations for
/// inline-DAG includes, the file's top-level declarations for file
/// includes. Names not found in `decls` (e.g., type-system-only items) are
/// silently skipped.
fn add_selective_aliases_inner(
    decls: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    selective: &[ImportAlias],
    prefix: &ModuleAliasName,
    subs: &AliasSubstitutions<'_>,
    owners: &AliasResolutionOwners<'_>,
    import_span: Span,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    let alias_type_resolution_owner =
        if subs.index.is_empty() && subs.r#type.is_empty() && subs.dim.is_empty() {
            owners.r#type.clone()
        } else {
            owners.body.clone()
        };
    for alias in selective {
        let orig_name = &alias.original;
        let local_name = &alias.local;
        // The alias points at the dep's prefixed declaration: a typed
        // qualified `ScopedName`. No flat `prefix::orig_name` strings are
        // built — the qualification stays structural through HIR.
        let target = ScopedName::qualified(prefix.clone(), orig_name.clone());

        let type_ann = decls.iter().find_map(|d| match &d.kind {
            DeclKind::Param(p) if &p.name.value == orig_name => Some(p.type_ann.clone()),
            DeclKind::Node(n) if &n.name.value == orig_name => Some(n.type_ann.clone()),
            DeclKind::ConstNode(c) if &c.name.value == orig_name => Some(c.type_ann.clone()),
            _ => None,
        });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        graphcal_compiler::ir::lower::substitute_type_expr_indexes(&mut type_ann, subs.index);
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            subs.r#type,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(&mut type_ann, subs.dim);

        let is_const = decls
            .iter()
            .any(|d| matches!(&d.kind, DeclKind::ConstNode(c) if &c.name.value == orig_name));
        let alias_kind = if is_const {
            // A const alias body is a reference path to the prefixed target;
            // HIR lowering resolves it against the merged entries.
            ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(
                graphcal_compiler::syntax::ast::IdentPath::new(
                    graphcal_compiler::syntax::non_empty::NonEmpty::new(
                        graphcal_compiler::syntax::ast::Ident {
                            name: prefix.atom().clone(),
                            span: import_span,
                        },
                        vec![graphcal_compiler::syntax::ast::Ident {
                            name: orig_name.atom().clone(),
                            span: import_span,
                        }],
                    ),
                ),
            ))
        } else {
            ExprKind::GraphRef(Spanned::new(target.clone(), import_span))
        };
        let alias_expr = Expr::new(alias_kind, import_span);

        if is_const {
            unfrozen.add_const_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_type_resolution_owner.clone(),
                alias_expr,
                owners.body.clone(),
                import_span,
            );
        } else {
            unfrozen.add_node_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_type_resolution_owner.clone(),
                alias_expr,
                owners.body.clone(),
                import_span,
            );
        }
    }
}

/// Resolve the importer-side argument of an index-port binding.
///
/// The source expression crosses into the typed include core as either a
/// declared index name or a validated structural finite identity.
pub(in crate::project_compiler) fn extract_index_binding_target(
    expr: &Expr,
    dep_index_name: &IndexName,
    file_src: &NamedSource<Arc<String>>,
) -> Result<IndexBindingTarget, CompileError> {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::registry::types::FiniteIndex;

    let invalid_binding = || {
        CompileError::Eval(GraphcalError::InvalidTypeLevelBindingValue {
            name: dep_index_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })
    };
    match expr.index_binding_arg().ok_or_else(invalid_binding)? {
        IndexExpr::Name(path) => path
            .value
            .as_bare()
            .map(|name| IndexBindingTarget::Declared(IndexName::from_atom(name.clone())))
            .ok_or_else(invalid_binding),
        IndexExpr::Finite { cardinality, .. } => {
            let normalized =
                graphcal_compiler::tir::typed::normalize_nat_expr(&cardinality, &[], file_src)?;
            let cardinality = normalized.constant_value().ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: "Fin cardinality in an index binding must be concrete".to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            })?;
            let finite = FiniteIndex::try_from_u64(cardinality).map_err(|error| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: error.to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            })?;
            Ok(IndexBindingTarget::Finite(finite))
        }
        IndexExpr::BareNat(_) => Err(invalid_binding()),
    }
}

/// Extract a `PascalCase` type name from a binding expression.
///
/// Type bindings use the form `DepType: ImporterType` — the RHS is a bare
/// `PascalCase` identifier (an unresolved reference path in the desugared
/// AST) or a zero-arg `ConstructorCall` for constructor-shaped RHSs.
pub(in crate::project_compiler) fn extract_type_name_from_binding_expr(
    expr: &Expr,
    dep_type_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    let invalid_binding = || {
        CompileError::Eval(GraphcalError::InvalidTypeLevelBindingValue {
            name: dep_type_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })
    };
    match &expr.kind {
        ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(path)) => path
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(invalid_binding),
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } if generic_args.is_empty() && fields.is_empty() => callee
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(invalid_binding),
        _ => Err(invalid_binding()),
    }
}
