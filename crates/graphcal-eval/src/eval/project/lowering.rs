//! IR lowering, registry merging, and finalization for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

fn include_debug_name_map(ctx: &ImportContext<'_>) -> IncludeDebugNameMap {
    let mut leaf_counts: HashMap<ModuleAliasName, usize> = HashMap::new();
    ctx.deferred_dag_includes
        .iter()
        .filter(|include| matches!(include.instance_scope, IncludeInstanceScope::Anonymous(_)))
        .for_each(|include| {
            leaf_counts
                .entry(include.debug_scope.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        });

    ctx.deferred_dag_includes
        .iter()
        .filter(|include| matches!(include.instance_scope, IncludeInstanceScope::Anonymous(_)))
        .filter(|include| leaf_counts.get(&include.debug_scope) == Some(&1))
        .filter(|include| !ctx.module_map.contains_key(&include.debug_scope))
        .filter(|include| {
            !ctx.included_debug_values
                .iter()
                .any(|(name, _, _)| name.qualifier().first() == Some(&include.debug_scope))
        })
        .map(|include| {
            (
                include.instance_scope.merge_scope_name(),
                include.debug_scope.clone(),
            )
        })
        .collect()
}

/// Lower the AST to IR, process deferred instantiated imports, and type-resolve
/// the final `CompiledFile`.
#[expect(
    clippy::too_many_lines,
    reason = "project lowering coordinates resolver, TIR, and plan construction"
)]
pub(in crate::eval::project) fn lower_and_finalize(
    semantic_context: ProjectSemanticContext<'_>,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::desugar::desugared_ast::File,
    ctx: ImportContext<'_>,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<CompiledFile, CompileError> {
    cancellation.checkpoint()?;
    let ProjectSemanticContext {
        project,
        module_resolver,
    } = semantic_context;
    let include_debug_names = include_debug_name_map(&ctx);
    // Snapshot the source-facing output values before IR lowering consumes the
    // canonical imported-binding records.
    let saved_imported_values = ctx
        .imported_bindings
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

    // Imported type-system declarations (selective imports and module-import
    // registries) seed the registry builder before the file's own
    // declarations register, so local unit/dim definitions can reference
    // imported units and dimensions.
    let mut registry_seed = |builder: &mut RegistryBuilder| {
        seed_imported_type_system(
            builder,
            project,
            &ctx.imported_type_system_names,
            &ctx.extra_registry_builders,
            evaluated_files,
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
        )?;

    // Snapshot the consumer-facing output surface before include bodies are
    // merged. The file's own declarations and imported values are visible;
    // each deferred include contributes only its selected/projectable outputs.
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
            ctx.deferred_dag_includes
                .iter()
                .flat_map(|include| include.surface_outputs.iter().cloned()),
        )
        .collect();

    // Process every deferred DAG include (file-level instantiated, inline
    // DAG, qualified inline DAG) through one path: compile each source's
    // body to IR and merge into the importer.
    process_deferred_dag_includes(
        project,
        file_dag_id,
        file_dag_id,
        &ctx.deferred_dag_includes,
        evaluated_files,
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
    cancellation.checkpoint()?;
    let ir = unfrozen.freeze_with_cancellation(
        registry,
        file_dag_id,
        module_resolver,
        file_src,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    // Type-resolve top-level decls; then compile each inline dag body
    // explicitly (loader supplies the per-file self-import set and the
    // canonical parent `DagId`). Cross-file dep dag TIRs are merged in
    // afterward by `merge_dep_dag_tirs`.
    let mut module_types = graphcal_compiler::tir::typed::ModuleTypeRegistry::default();
    module_types
        .insert_graphcal_prelude()
        .map_err(|err| GraphcalError::InternalError {
            message: format!("failed to build prelude type registry: {err}"),
            src: file_src.clone(),
            span: Span::new(0, 0).into(),
        })?;
    module_types.insert_registry(file_dag_id, &ir.registry, file_src.clone());
    for (dep_dag_id, evaluated) in evaluated_files {
        cancellation.checkpoint()?;
        let dep_src = &project.files[dep_dag_id].named_source;
        module_types.insert_registry(dep_dag_id, &evaluated.registry, dep_src.clone());
    }
    // The aggregate root registry may expose dependency units under aliases or
    // selective-import spellings. Overlay those entries last so dynamic units
    // retain any dependency-evaluated static scale while their HIR identity
    // remains canonical.
    module_types.overlay_visible_units(file_dag_id, &ir.registry, module_resolver);

    let parent_external_surface = ir.external_surface.clone();
    let mut tir =
        graphcal_compiler::tir::typed::type_resolve_builder_with_modules_and_cancellation(
            ir,
            file_dag_id,
            file_src,
            module_resolver,
            &module_types,
            cancellation,
        )?;
    // File roots and inline `dag` blocks are both callable DAG modules. Keep
    // their externally projectable ports in the same compiled representation
    // so an imported file alias can be invoked directly as `@alias(...).out`.
    tir.root_mut()
        .populate_projectable_outputs(&file_ast.declarations);
    compile_inline_dag_modules(
        &mut tir,
        project,
        file_dag_id,
        file_src,
        &parent_external_surface,
        evaluated_files,
        module_resolver,
        &module_types,
        cancellation,
    )?;
    cancellation.checkpoint()?;
    merge_dep_dag_tirs(&mut tir, &ctx.module_map, evaluated_files, file_src)?;
    let tir = tir.finish();
    graphcal_compiler::tir::dim_check::check_dimensions_tir_with_cancellation(
        &tir,
        file_src,
        cancellation,
    )?;

    // Resolve domain constraints at compile time so malformed bounds (such as
    // C003 min > max) surface under `graphcal check` instead of only at `eval`.
    // Target-family, Int, and datetime-scale checks are pure type checks handled
    // in `dim_check`; ordering requires evaluated bound expressions and therefore
    // const values, so we evaluate consts here too. The resulting plan is
    // discarded — `exec_plan::compile` recomputes both as part of its run.
    {
        let const_values =
            crate::exec_plan::eval_consts_from_tir_with_cancellation(&tir, file_src, cancellation)?;
        let _ = crate::exec_plan::resolve_domain_constraints_with_cancellation(
            &tir,
            &const_values,
            file_src,
            cancellation,
        )?;
        let field_constraints =
            crate::exec_plan::resolve_struct_field_constraints_with_cancellation(
                &tir,
                &const_values,
                file_src,
                cancellation,
            )?;
        crate::exec_plan::check_const_struct_field_constraints_at_compile_time(
            &tir,
            &const_values,
            &field_constraints,
            file_src,
        )?;
    }

    let declared_types = tir.build_declared_types(file_src)?;

    Ok(CompiledFile {
        tir,
        declared_types,
        imported_values: saved_imported_values,
        imported_source_order: ctx.imported_source_order,
        output_surface,
        included_debug_values: ctx.included_debug_values,
        include_debug_names,
        included_plots: ctx.included_plot_specs,
    })
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
    reason = "inline DAG module compilation threads project, dependency artifacts, and module typing context"
)]
fn compile_inline_dag_modules<'a>(
    tir: &mut graphcal_compiler::tir::typed::TirBuilder,
    project: &'a crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    parent_external_surface: &ExternalDeclSurface,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    module_types: &graphcal_compiler::tir::typed::ModuleTypeRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let loaded_file = &project.files[file_dag_id];
    let parent_values = crate::inline_dag::classify_value_decls_in_dag(
        tir.root(),
        parent_external_surface,
        file_src,
    )?;

    for loaded_dag in &loaded_file.inline_dags {
        cancellation.checkpoint()?;
        let dag_ir = compile_loaded_dag_module_ir(
            tir,
            project,
            loaded_dag,
            file_src,
            &parent_values,
            evaluated_files,
            module_resolver,
            cancellation,
        )?;
        // The shared module registry contains file/dependency registries, but
        // an inline DAG owns type-system declarations in its body (including
        // required `pub(bind)` dimensions, types, and indexes). Add that local
        // registry under the child DAG identity before resolving its annotations.
        let mut dag_module_types = module_types.clone();
        dag_module_types.insert_registry(&loaded_dag.dag_id, &dag_ir.registry, file_src.clone());
        dag_module_types.overlay_visible_units(
            &loaded_dag.dag_id,
            &dag_ir.registry,
            module_resolver,
        );
        tir.insert_module_registry(
            &loaded_dag.dag_id,
            &dag_ir.registry,
            file_src.clone(),
            module_resolver,
        );
        let mut compiled_dag =
            graphcal_compiler::tir::typed::type_resolve_single_with_modules_and_cancellation(
                dag_ir,
                &loaded_dag.dag_id,
                file_src,
                module_resolver,
                &dag_module_types,
                cancellation,
            )?;
        compiled_dag.populate_projectable_outputs(&loaded_dag.body);
        tir.insert_dag(compiled_dag).map_err(|error| {
            CompileError::Eval(GraphcalError::InternalError {
                message: error.to_string(),
                src: file_src.clone(),
                span: Span::new(0, 0).into(),
            })
        })?;
    }

    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "DAG lowering threads project, module, dependency, resolver, and cancellation context"
)]
fn compile_loaded_dag_module_ir<'a>(
    tir: &graphcal_compiler::tir::typed::TirBuilder,
    project: &'a crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    file_src: &NamedSource<Arc<String>>,
    parent_values: &crate::inline_dag::ParentValueDecls,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::lower::IR, CompileError> {
    cancellation.checkpoint()?;
    let parent_loaded = &project.files[&loaded_dag.parent_dag_id];
    let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
        &loaded_dag.body,
        &loaded_dag.parent_dag_id,
        &parent_loaded.ast,
        parent_values,
        &loaded_dag.resolved_imports,
        file_src,
    )?;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        extra_registry_builders: Vec::new(),
        deferred_dag_includes: Vec::new(),
        included_debug_values: Vec::new(),
        included_plot_specs: Vec::new(),
    };

    process_dag_body_import_declarations(project, loaded_dag, file_src, evaluated_files, &mut ctx)?;
    process_dag_body_include_declarations(
        project,
        loaded_dag,
        file_src,
        evaluated_files,
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
        &mut ctx.deferred_dag_includes,
    );
    let mut registry_seed = |builder: &mut RegistryBuilder| {
        seed_imported_type_system(
            builder,
            project,
            &ctx.imported_type_system_names,
            &ctx.extra_registry_builders,
            evaluated_files,
            file_src,
        )
    };
    let (mut builder, mut unfrozen) =
        graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
            dag_ast.as_ref(),
            Some(tir.registry()),
            &ctx.imported_names,
            ctx.imported_bindings,
            file_src,
            &loaded_dag.dag_id,
            Some(&mut registry_seed),
            cancellation,
        )?;

    process_deferred_dag_includes(
        project,
        &loaded_dag.dag_id,
        &loaded_dag.parent_dag_id,
        &ctx.deferred_dag_includes,
        evaluated_files,
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
    Ok(unfrozen.freeze_with_cancellation(
        registry,
        &loaded_dag.dag_id,
        module_resolver,
        file_src,
        cancellation,
    )?)
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
    target: &mut HashMap<ScopedName, ImportedBinding>,
    source: HashMap<ScopedName, ImportedBinding>,
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
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    for decl in &loaded_dag.body {
        let DeclKind::Import(import_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::InlineBodyImportResolution::Resolved(import_dag_id)) = loaded_dag
            .resolved_imports
            .get(&crate::loader::ModulePathKey::from_path(&import_decl.path))
        else {
            continue;
        };
        if import_dag_id == &loaded_dag.parent_dag_id {
            continue;
        }
        imports::process_non_instantiated_import(
            project,
            import_dag_id,
            &import_decl.path,
            &import_decl.kind,
            file_src,
            evaluated_files,
            ctx,
            true,
        )?;
    }
    Ok(())
}

fn process_dag_body_include_declarations<'a>(
    project: &'a crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    for decl in &loaded_dag.body {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::InlineBodyImportResolution::Resolved(target_dag_id)) = loaded_dag
            .resolved_imports
            .get(&crate::loader::ModulePathKey::from_path(&include_decl.path))
        else {
            continue;
        };
        if project.files.contains_key(target_dag_id) {
            if include_decl.param_bindings.is_empty() {
                imports::process_non_instantiated_import(
                    project,
                    target_dag_id,
                    &include_decl.path,
                    &include_decl.kind,
                    file_src,
                    evaluated_files,
                    ctx,
                    false,
                )?;
            } else {
                imports::process_instantiated_include(
                    project,
                    target_dag_id,
                    include_decl,
                    decl,
                    file_src,
                    evaluated_files,
                    ctx,
                )?;
            }
            continue;
        }

        let Some((target_file_id, target_dag_decl)) = find_inline_dag_decl(project, target_dag_id)
        else {
            continue;
        };
        let boundary = if target_file_id == &loaded_dag.parent_dag_id {
            imports::IncludeVisibilityBoundary::Local
        } else {
            imports::IncludeVisibilityBoundary::CrossModule
        };
        imports::process_inline_dag_include(
            &imports::InlineDagIncludeTarget {
                dag_def: target_dag_decl,
                dag_id: target_dag_id,
                dag_name: target_dag_id.name(),
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

fn find_inline_dag_decl<'a>(
    project: &'a crate::loader::LoadedProject,
    target: &graphcal_compiler::dag_id::DagId,
) -> Option<(
    &'a graphcal_compiler::dag_id::DagId,
    &'a graphcal_compiler::desugar::desugared_ast::DagDecl,
)> {
    project.files.iter().find_map(|(file_id, loaded)| {
        find_inline_dag_decl_in_declarations(&loaded.ast.declarations, file_id, target)
            .map(|dag_decl| (file_id, dag_decl))
    })
}

fn find_inline_dag_decl_in_declarations<'a>(
    declarations: &'a [graphcal_compiler::desugar::desugared_ast::Declaration],
    lexical_parent_id: &graphcal_compiler::dag_id::DagId,
    target: &graphcal_compiler::dag_id::DagId,
) -> Option<&'a graphcal_compiler::desugar::desugared_ast::DagDecl> {
    declarations.iter().find_map(|decl| {
        let DeclKind::Dag(dag) = &decl.kind else {
            return None;
        };
        let dag_id = lexical_parent_id.child(dag.name.value.as_str());
        if &dag_id == target {
            return Some(dag);
        }
        find_inline_dag_decl_in_declarations(&dag.body, &dag_id, target)
    })
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
fn merge_dep_dag_tirs(
    tir: &mut graphcal_compiler::tir::typed::TirBuilder,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    for (alias, binding) in module_map {
        // Imported aliases name their exact reusable DAG target. Included
        // aliases are namespaces for an existing instance and are not callable.
        if binding.role == graphcal_compiler::syntax::module_resolve::ModuleAliasRole::ImportedDag {
            tir.insert_module_alias(alias.clone(), binding.target.clone());
        }
    }

    for (dep_dag_id, dep_eval) in evaluated_files {
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
            // Supply deferred values imported from this owning dependency.
            // Canonical target and declared type remain on the same binding
            // record; no independently keyed source/value maps can diverge.
            cloned.supply_imported_values(
                dep_dag_id,
                &dep_eval.const_values,
                &dep_eval.declared_types,
            );
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

fn check_semantic_override_reconciliation(
    dependencies: &graphcal_compiler::tir::dim_check::OverrideDependencySummary,
    source_dag_id: &graphcal_compiler::dag_id::DagId,
    deferred: &DeferredDagInclude,
    importer_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    use graphcal_compiler::syntax::index_name::ResolvedIndexName;
    use graphcal_compiler::syntax::type_name::ResolvedStructTypeName;
    use graphcal_compiler::tir::dim_check::NominalOverrideIdentity;

    let mut overridden_indexes = deferred.index_bindings.keys().cloned().collect::<Vec<_>>();
    overridden_indexes.sort();
    let mut overridden_types = deferred.type_bindings.keys().cloned().collect::<Vec<_>>();
    overridden_types.sort();
    let mut defaults = dependencies
        .iter()
        .filter(|(param, _)| param.owner() == source_dag_id)
        .collect::<Vec<_>>();
    defaults.sort_by_key(|(param, _)| (*param).clone());

    for (param, dependencies) in defaults {
        if deferred.bindings.contains_key(&param.to_unowned_def_name()) {
            continue;
        }
        for overridden in &overridden_indexes {
            let identity = NominalOverrideIdentity::Index(ResolvedIndexName::from_def(
                source_dag_id.clone(),
                overridden.clone(),
            ));
            if dependencies.contains(&identity) {
                return Err(CompileError::Eval(
                    GraphcalError::IncludeMustReconcileOverride {
                        overridden: overridden.to_string(),
                        overridden_kind: "index".to_string(),
                        orphan_decl: param.as_str().to_string(),
                        detail: format!("canonical dependency on index `{overridden}`"),
                        src: importer_src.clone(),
                        span: deferred.import_span.into(),
                    },
                ));
            }
        }
        for overridden in &overridden_types {
            let identity = NominalOverrideIdentity::Type(ResolvedStructTypeName::from_def(
                source_dag_id.clone(),
                overridden.clone(),
            ));
            if dependencies.contains(&identity) {
                return Err(CompileError::Eval(
                    GraphcalError::IncludeMustReconcileOverride {
                        overridden: overridden.to_string(),
                        overridden_kind: "type".to_string(),
                        orphan_decl: param.as_str().to_string(),
                        detail: format!("canonical dependency on type `{overridden}`"),
                        src: importer_src.clone(),
                        span: deferred.import_span.into(),
                    },
                ));
            }
        }
    }
    Ok(())
}

/// Process every deferred DAG include (file-level instantiated, same-file
/// inline DAG, cross-file qualified inline DAG) through one path:
///
/// 1. Resolve the include's source — file include reads the dep's full
///    AST; inline DAG include reads the dag block's body and pre-processes
///    `import <self>.{...}` against the dag's parent file (Concept 9: a
///    DAG's `<self>` is its file of definition, regardless of where the
///    include sits).
/// 2. Compile the body to IR with imported names/values set up.
/// 3. Capture importer-owned index binding candidates, merge the body's registry,
///    then validate every candidate against the body's effective typed contract.
/// 4. Preserve canonical A8/V005 reconciliation facts and run
///    `check_generics_leakage` (A9/V006).
/// 5. Merge the body's IR into the importer's IR with prefix/bindings.
/// 6. For selective includes, add `local_name = @prefix::orig_name` aliases.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline function threading project, importer, evaluated-deps, and IR-builder context"
)]
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive include pipeline: source resolution, registry merge, validation, IR merge"
)]
fn process_deferred_dag_includes(
    project: &crate::loader::LoadedProject,
    importer_dag_id: &graphcal_compiler::dag_id::DagId,
    importer_file_dag_id: &graphcal_compiler::dag_id::DagId,
    deferred_dag_includes: &[DeferredDagInclude],
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    importer_src: &NamedSource<Arc<String>>,
    importer_ast: &graphcal_compiler::desugar::desugared_ast::File,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let importer_external_surface = super::extract_external_decl_surface(importer_ast);
    let importer_local_type_names = collect_local_type_names(importer_ast);
    let empty_resolved: HashMap<
        crate::loader::ModulePathKey,
        crate::loader::InlineBodyImportResolution,
    > = HashMap::new();

    for deferred in deferred_dag_includes {
        cancellation.checkpoint()?;
        let merge_prefix = deferred.instance_scope.merge_scope_name();
        // ---- 1. Resolve source body + lower to IR ------------------------
        let (
            mut dep_unfrozen,
            dep_registry,
            dep_src,
            dep_resolution_owner,
            body_for_leakage_check,
            body_decls_for_aliases,
            source_override_dependencies,
        ) = match &deferred.source {
            DeferredDagSource::File { dep_dag_id } => {
                let dep_loaded = &project.files[dep_dag_id];
                let dep_src = &dep_loaded.named_source;
                let mut body_ctx = ImportContext {
                    imported_names: ImportedValueNames::default(),
                    imported_bindings: HashMap::new(),
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
                    dep_dag_id,
                    evaluated_files,
                    &mut body_ctx,
                    cancellation,
                )?;
                let rewritten_body = rewrite_qualified_refs_in_compilation_body(
                    &dep_loaded.ast,
                    &body_ctx.imported_names,
                    &mut body_ctx.deferred_dag_includes,
                );
                let instance_dag_id = importer_dag_id.child(merge_prefix.as_str());
                let mut registry_seed = |builder: &mut RegistryBuilder| {
                    seed_imported_type_system(
                        builder,
                        project,
                        &body_ctx.imported_type_system_names,
                        &body_ctx.extra_registry_builders,
                        evaluated_files,
                        dep_src,
                    )
                };
                let (mut dep_builder, mut dep_unfrozen) = graphcal_compiler::ir::lower::lower_to_builder_with_imported_bindings_and_cancellation(
                        rewritten_body.as_ref(),
                        dep_src,
                        &body_ctx.imported_names,
                        body_ctx.imported_bindings,
                        &instance_dag_id,
                        Some(&mut registry_seed),
                        cancellation,
                    )?;
                dep_unfrozen.retarget_existing_resolution_owners(dep_dag_id);
                process_deferred_dag_includes(
                    project,
                    &instance_dag_id,
                    importer_file_dag_id,
                    &body_ctx.deferred_dag_includes,
                    evaluated_files,
                    dep_src,
                    rewritten_body.as_ref(),
                    &mut dep_builder,
                    &mut dep_unfrozen,
                    cancellation,
                )?;
                let dep_registry = dep_builder
                    .try_build()
                    .map_err(|err| registry_build_compile_error(&err, dep_src))?;
                (
                    dep_unfrozen,
                    dep_registry,
                    dep_src.clone(),
                    dep_dag_id.clone(),
                    &dep_loaded.ast,
                    dep_loaded.ast.declarations.as_slice(),
                    evaluated_files
                        .get(dep_dag_id)
                        .map(|artifact| &artifact.override_dependencies),
                )
            }
            DeferredDagSource::InlineDag {
                dag_body,
                dag_imported_names,
                dag_id,
                parent_dag_id,
            } => {
                let parent_loaded = &project.files[parent_dag_id];
                let loaded_inline = parent_loaded
                    .inline_dags
                    .iter()
                    .find(|d| &d.dag_id == dag_id);
                let parent_values =
                    crate::inline_dag::classify_value_decls_in_ast(&parent_loaded.ast);
                let parent_resolved_imports =
                    loaded_inline.map_or(&empty_resolved, |d| &d.resolved_imports);
                let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
                    &dag_body.declarations,
                    parent_dag_id,
                    &parent_loaded.ast,
                    &parent_values,
                    parent_resolved_imports,
                    importer_src,
                )?;

                let mut body_ctx = ImportContext {
                    imported_names: dag_imported_names.clone(),
                    imported_bindings: HashMap::new(),
                    imported_source_order: Vec::new(),
                    imported_type_system_names: HashMap::new(),
                    module_map: HashMap::new(),
                    extra_registry_builders: Vec::new(),
                    deferred_dag_includes: Vec::new(),
                    included_debug_values: Vec::new(),
                    included_plot_specs: Vec::new(),
                };
                if let Some(loaded_inline) = loaded_inline {
                    process_dag_body_import_declarations(
                        project,
                        loaded_inline,
                        importer_src,
                        evaluated_files,
                        &mut body_ctx,
                    )?;
                    process_dag_body_include_declarations(
                        project,
                        loaded_inline,
                        importer_src,
                        evaluated_files,
                        &mut body_ctx,
                    )?;
                }
                extend_imported_value_names(&mut body_ctx.imported_names, self_imports.names);
                let mut imported_bindings = body_ctx.imported_bindings;
                extend_imported_bindings(
                    &mut imported_bindings,
                    self_imports.bindings,
                    &body_ctx.imported_names,
                    importer_src,
                )?;
                // For cross-file includes, supply concrete parent values to the
                // deferred canonical self-import bindings. Same-file includes
                // resolve them from the importer at runtime.
                if parent_dag_id != importer_file_dag_id
                    && let Some(parent_eval) = evaluated_files.get(parent_dag_id)
                {
                    for binding in imported_bindings.values_mut() {
                        if binding.target().owner() != parent_dag_id {
                            continue;
                        }
                        let source_name = binding.target().to_unowned_def_name();
                        let Some(value) = parent_eval.const_values.get(&source_name) else {
                            continue;
                        };
                        let Some(dt) = parent_eval
                            .declared_types
                            .get(&ScopedName::from(&source_name))
                        else {
                            continue;
                        };
                        binding.supply_value(value.clone());
                        binding.replace_declared_type(dt.clone());
                    }
                }

                let stripped_body = graphcal_compiler::desugar::desugared_ast::File {
                    declarations: self_imports.stripped_body,
                };
                let stripped_body = rewrite_qualified_refs_in_compilation_body(
                    &stripped_body,
                    &body_ctx.imported_names,
                    &mut body_ctx.deferred_dag_includes,
                );

                let dag_dag_id = importer_dag_id.child(merge_prefix.as_str());
                let mut registry_seed = |builder: &mut RegistryBuilder| {
                    seed_imported_type_system(
                        builder,
                        project,
                        &body_ctx.imported_type_system_names,
                        &body_ctx.extra_registry_builders,
                        evaluated_files,
                        importer_src,
                    )
                };
                let (mut dag_builder, mut dag_unfrozen) = graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_bindings_and_cancellation(
                        stripped_body.as_ref(),
                        None,
                        &body_ctx.imported_names,
                        imported_bindings,
                        importer_src,
                        &dag_dag_id,
                        Some(&mut registry_seed),
                        cancellation,
                    )?;
                dag_unfrozen.retarget_existing_resolution_owners(dag_id);
                process_deferred_dag_includes(
                    project,
                    &dag_dag_id,
                    importer_file_dag_id,
                    &body_ctx.deferred_dag_includes,
                    evaluated_files,
                    importer_src,
                    stripped_body.as_ref(),
                    &mut dag_builder,
                    &mut dag_unfrozen,
                    cancellation,
                )?;
                let dag_registry = dag_builder
                    .try_build()
                    .map_err(|err| registry_build_compile_error(&err, importer_src))?;
                (
                    dag_unfrozen,
                    dag_registry,
                    importer_src.clone(),
                    dag_id.clone(),
                    dag_body,
                    dag_body.declarations.as_slice(),
                    evaluated_files
                        .get(parent_dag_id)
                        .map(|artifact| &artifact.override_dependencies),
                )
            }
        };

        // Capture importer-owned candidates before the dependency registry is
        // merged, so a dependency declaration with the same leaf name cannot
        // accidentally satisfy its own binding.
        let index_binding_candidates = capture_index_binding_candidates(
            builder,
            &deferred.index_bindings,
            &deferred.index_binding_spans,
            importer_src,
            deferred.import_span,
        )?;

        // ---- 2. Merge dep registry into importer's --------------------
        merge_registry_into_builder(
            builder,
            &dep_registry,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
        )
        .map_err(|conflict| {
            CompileError::Eval(GraphcalError::ConflictingImportedUnit {
                name: conflict.name,
                src: importer_src.clone(),
                span: deferred.import_span.into(),
            })
        })?;

        // ---- 3. Validate typed index binding contracts -------------------
        validate_index_binding_contracts(
            body_decls_for_aliases,
            &dep_registry,
            &deferred.index_bindings,
            &deferred.index_binding_spans,
            &deferred.dim_bindings,
            &index_binding_candidates,
            builder,
            importer_src,
            deferred.import_span,
        )?;

        // ---- 4. Validation checks -----------------------------------------
        let mut dep_names: HashSet<DeclName> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.member().clone());
        }
        match source_override_dependencies {
            Some(dependencies) => check_semantic_override_reconciliation(
                dependencies,
                &dep_resolution_owner,
                deferred,
                importer_src,
            )?,
            None => dep_unfrozen.record_include_override_reconciliations(
                &deferred.bindings,
                &deferred.index_bindings,
                &deferred.type_bindings,
                &dep_registry,
                &dep_resolution_owner,
                importer_dag_id,
                importer_src,
                deferred.import_span,
            )?,
        }
        check_generics_leakage(
            body_for_leakage_check,
            &deferred.pub_reexport_items,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &importer_external_surface,
            &importer_local_type_names,
            importer_src,
            deferred.import_span,
        )?;

        // ---- 5. Merge dep IR into importer's IR ---------------------------
        unfrozen.merge_dependency(
            dep_unfrozen,
            &merge_prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &deferred.import_item_attributes,
            &deferred.requested_plots,
            importer_dag_id,
            importer_src,
            &dep_src,
        )?;

        // ---- 6. Add selective aliases -------------------------------------
        if let Some(selective) = &deferred.selective_names {
            add_selective_aliases_inner(
                body_decls_for_aliases,
                selective,
                &merge_prefix,
                &AliasSubstitutions {
                    index: &deferred.index_bindings,
                    r#type: &deferred.type_bindings,
                    dim: &deferred.dim_bindings,
                },
                &AliasResolutionOwners {
                    r#type: &dep_resolution_owner,
                    body: importer_dag_id,
                },
                deferred.import_span,
                unfrozen,
            );
        }
    }
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
/// it is registered in the importer's IR. Shared by both inline-DAG and
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
        // built — the qualification stays structural through the IR.
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

/// Merge type-system declarations from a dependency's frozen Registry into a builder.
///
/// This imports dimensions, units, indexes, and struct types so that the
/// importing file can reference them.
fn merge_registry_into_builder(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) -> Result<(), UnitMergeConflict> {
    merge_registry_into_builder_filtered(
        builder,
        dep_registry,
        index_bindings,
        type_bindings,
        dim_bindings,
        None,
        None,
    )
}

/// Merge imported type-system declarations into a registry builder: selective
/// imports register the selected declarations from each dependency's AST, and
/// module imports merge each dependency's `pub` registry entries under the
/// import alias.
///
/// Runs as the registry-seed hook of file lowering — before the file's own
/// declarations register — so local definitions (e.g. `const unit halfmile: Length
/// = 0.5 u.mile;`) resolve against the imported entries.
fn seed_imported_type_system(
    builder: &mut RegistryBuilder,
    project: &crate::loader::LoadedProject,
    imported_type_system_names: &HashMap<
        graphcal_compiler::dag_id::DagId,
        graphcal_compiler::ir::lower::SelectedDeclarations,
    >,
    extra_registry_builders: &[ModuleRegistryImport<'_>],
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (dep_dag_id, names) in imported_type_system_names {
        let dep_loaded = &project.files[dep_dag_id];
        let source_registered_names = if evaluated_files.contains_key(dep_dag_id) {
            names.without_resolved_dimensions_and_units()
        } else {
            names.clone()
        };
        if let Some(dep_eval) = evaluated_files.get(dep_dag_id) {
            register_selected_resolved_dimensions_and_units(
                builder,
                &dep_eval.registry,
                names,
                &dep_eval.resolved_dynamic_unit_scales,
                &dep_loaded.named_source,
            )?;
        }
        graphcal_compiler::ir::lower::register_selected_declarations(
            &dep_loaded.ast,
            builder,
            &dep_loaded.named_source,
            &source_registered_names,
            dep_dag_id,
        )?;
    }
    for import in extra_registry_builders {
        merge_registry_into_builder_export_filtered(builder, import).map_err(|conflict| {
            GraphcalError::ConflictingImportedUnit {
                name: conflict.name,
                src: file_src.clone(),
                span: import.import_span.into(),
            }
        })?;
    }
    Ok(())
}

/// Import selected dimensions and units from the dependency's resolved registry.
///
/// Re-registering only a selected declaration's source AST loses sibling
/// dimensions used by a dimension definition (`Weighted = Base * Mass`) or by
/// a unit annotation (`point: Score`). Both declarations are closed semantic
/// values after the dependency compiles, so copy them together with the base
/// dimension metadata needed to preserve canonical identities and formatting.
fn register_selected_resolved_dimensions_and_units(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    selected: &graphcal_compiler::ir::lower::SelectedDeclarations,
    resolved_unit_scales: &HashMap<
        graphcal_compiler::syntax::dimension::UnitRef,
        graphcal_compiler::registry::types::PositiveFiniteScale,
    >,
    dep_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    fn register_base_dimension_metadata(
        builder: &mut RegistryBuilder,
        dep_registry: &Registry,
        dimension: &graphcal_compiler::dimension::Dimension,
    ) {
        for (base_id, _) in dimension.iter() {
            if let Some(base_name) = dep_registry.dimensions.base_dim_names().get(base_id) {
                // Multiple package instances may use the same display leaf for
                // distinct nominal base IDs. Register every ID's metadata even
                // though the flat boundary registry keeps only one leaf value.
                builder.register_base_dimension(
                    graphcal_compiler::syntax::dimension::DimName::expect_valid(base_name),
                    base_id.clone(),
                );
            }
            if let Some(symbol) = dep_registry.dimensions.base_dim_symbols().get(base_id) {
                builder.set_base_dim_symbol(base_id.clone(), symbol.clone());
            }
        }
    }

    for name in selected.dimensions() {
        let dimension = dep_registry
            .dimensions
            .get_dimension(name.as_str())
            .cloned()
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "resolved dependency registry is missing selected dimension `{name}`"
                ),
                src: dep_src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        register_base_dimension_metadata(builder, dep_registry, &dimension);
        builder.register_dimension(name.clone(), dimension);
    }

    for name in selected.units() {
        use graphcal_compiler::registry::types::UnitScale;
        use graphcal_compiler::syntax::dimension::UnitRef;

        let unit_ref = UnitRef::local(name.clone());
        let info = dep_registry
            .units
            .get_unit(&unit_ref)
            .cloned()
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("resolved dependency registry is missing selected unit `{name}`"),
                src: dep_src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        register_base_dimension_metadata(builder, dep_registry, &info.dimension);
        let scale = match (&info.scale, resolved_unit_scales.get(&unit_ref)) {
            (UnitScale::Dynamic { .. }, Some(resolved)) => UnitScale::Static(*resolved),
            _ => info.scale,
        };
        builder.register_unit_with_scale(unit_ref, info.dimension, scale, info.constness);
    }

    Ok(())
}

/// Merge type-system declarations from a dependency's frozen registry into a
/// builder, restricted to its explicit export surface. Param input ports are
/// intentionally irrelevant here because they are not type-system exports.
fn merge_registry_into_builder_export_filtered(
    builder: &mut RegistryBuilder,
    import: &ModuleRegistryImport<'_>,
) -> Result<(), UnitMergeConflict> {
    merge_registry_into_builder_filtered(
        builder,
        import.registry,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        Some(import.external_surface),
        Some((&import.unit_alias, import.resolved_dynamic_scales)),
    )
}

/// A unit reference reached the importing file with two different definitions.
///
/// Includes inline the dependency's body into the importer, so the bodies of
/// two included modules share one unit scope; a silent last-write-wins merge
/// would make their references resolve to whichever include happened to land
/// last. The conflict is surfaced as a loud error instead.
#[derive(Debug)]
pub(in crate::eval::project) struct UnitMergeConflict {
    pub name: graphcal_compiler::syntax::dimension::UnitRef,
}

fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    external_surface: Option<&ExternalDeclSurface>,
    unit_alias: Option<(
        &ModuleAliasName,
        &HashMap<
            graphcal_compiler::syntax::dimension::UnitRef,
            graphcal_compiler::registry::types::PositiveFiniteScale,
        >,
    )>,
) -> Result<(), UnitMergeConflict> {
    // Import base-dimension metadata for display formatting and registry
    // invariants. This includes private transitive dependencies of exported
    // dimensions and units; module resolution still prevents those names from
    // becoming source-visible in the importer.
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        builder.register_base_dimension(
            graphcal_compiler::syntax::dimension::DimName::expect_valid(name),
            id.clone(),
        );
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        if external_surface.is_some_and(|surface| {
            !surface.is_explicit_export(&DeclName::from_atom(name.atom().clone()))
        }) {
            continue;
        }
        builder.register_dimension(name.clone(), dim.clone());
    }

    // Import base dimension symbols (for SI unit string display).
    for (id, symbol) in dep_registry.dimensions.base_dim_symbols() {
        builder.set_base_dim_symbol(id.clone(), symbol.clone());
    }

    // Import units.
    //
    // Module imports (`unit_alias` present) expose only the dependency's own
    // `pub` units, re-keyed under the import alias (`alias.unit`); the
    // dependency's alias-qualified imports and non-pub units stay internal to
    // it, and nothing lands in the importer's bare unit scope. Bare names in
    // the importer come only from its own declarations, selective imports,
    // and the prelude.
    //
    // Include merges (`unit_alias` absent) copy the dependency's full unit
    // scope unchanged because the dependency's body is inlined into the
    // importer and its unit references must keep resolving. Re-merging an
    // identical definition (diamond includes, prelude units present in every
    // dep registry) is idempotent; a *different* definition under the same
    // reference is a conflict.
    for (name, dim, scale) in dep_registry.units.all_units() {
        let target = if let Some((alias, _)) = unit_alias {
            if name.is_qualified() {
                continue;
            }
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(name.name().atom().clone()))
            }) {
                continue;
            }
            graphcal_compiler::syntax::dimension::UnitRef::qualified(
                alias.clone(),
                name.name().clone(),
            )
        } else {
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(name.name().atom().clone()))
            }) {
                continue;
            }
            name.clone()
        };
        // A dynamic unit's scale expression references the dependency's own
        // params and cannot be re-evaluated in the importer's context, so a
        // module import carries the scale resolved by the dependency's
        // evaluation instead. Without a resolved scale (the dependency is a
        // library or runs under `check`), the dynamic form is kept and use
        // surfaces the loud could-not-be-resolved error.
        let merged_scale = match (scale, unit_alias) {
            (
                graphcal_compiler::registry::types::UnitScale::Dynamic { .. },
                Some((_, resolved_scales)),
            ) => resolved_scales.get(name).map_or_else(
                || scale.clone(),
                |resolved| graphcal_compiler::registry::types::UnitScale::Static(*resolved),
            ),
            _ => scale.clone(),
        };
        let constness = dep_registry.units.get_unit(name).map_or(
            graphcal_compiler::syntax::ast::UnitConstness::Dynamic,
            |info| info.constness,
        );
        if let Some(existing) = builder.get_unit(&target) {
            if unit_definitions_compatible(existing, dim, &merged_scale, constness) {
                continue;
            }
            return Err(UnitMergeConflict { name: target });
        }
        builder.register_unit_with_scale(target, dim.clone(), merged_scale, constness);
    }

    // Import indexes — skip bound indexes (they are replaced by the importer's index).
    // Module imports use explicit-export filtering and keep required indexes in the
    // dependency's module registry only; pulling an unbound `pub(bind)` index
    // into the importer would incorrectly make the importer a library even if
    // it only needs a qualified type from the dependency.
    for idx_def in dep_registry.indexes.declared_indexes() {
        if external_surface.is_some() && idx_def.is_required() {
            continue;
        }
        if !index_bindings.contains_key(idx_def.name.as_str()) {
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(idx_def.name.atom().clone()))
            }) {
                continue;
            }
            builder.register_index(idx_def.clone());
        }
    }
    // Structural indexes have no module visibility or alias. Preserve their
    // typed identities so imported indexed types and values can resolve them.
    for finite_index in dep_registry.indexes.finite_indexes() {
        builder.ensure_finite_index(finite_index.cardinality());
    }

    // Import struct types — skip bound types (they are replaced by the importer's type).
    for type_def in dep_registry.types.all_types() {
        if type_bindings.contains_key(type_def.name().as_str()) {
            continue;
        }
        if external_surface.is_some_and(|surface| {
            !surface.is_explicit_export(&DeclName::from_atom(type_def.name().atom().clone()))
        }) {
            continue;
        }
        builder.register_type(type_def.clone());
    }
    Ok(())
}

/// Two unit definitions are compatible when they agree on dimension and
/// scale. Dynamic scales cannot be compared structurally; two dynamic
/// definitions are assumed to be the same declaration reached through a
/// diamond import (a genuinely different pair still differs in dimension or
/// base scale in practice).
fn unit_definitions_compatible(
    existing: &graphcal_compiler::registry::types::UnitInfo,
    dim: &graphcal_compiler::dimension::Dimension,
    scale: &graphcal_compiler::registry::types::UnitScale,
    constness: graphcal_compiler::syntax::ast::UnitConstness,
) -> bool {
    use graphcal_compiler::registry::types::UnitScale;
    if existing.dimension != *dim || existing.constness != constness {
        return false;
    }
    match (&existing.scale, scale) {
        (UnitScale::Static(a), UnitScale::Static(b))
        | (
            UnitScale::Dynamic {
                base_unit_scale: a, ..
            },
            UnitScale::Dynamic {
                base_unit_scale: b, ..
            },
        ) => a == b,
        (UnitScale::Static(_), UnitScale::Dynamic { .. })
        | (UnitScale::Dynamic { .. }, UnitScale::Static(_)) => false,
    }
}

/// Resolve the importer-side argument of an index-port binding.
///
/// The parser retains every include RHS as an expression until dependency
/// loading reveals the target namespace. [`Expr::index_binding_arg`] recovers
/// the source-level index shape here; this function then crosses into the
/// typed include core as either a declared index name or a validated structural
/// finite identity.
pub(in crate::eval::project) fn extract_index_binding_target(
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
pub(in crate::eval::project) fn extract_type_name_from_binding_expr(
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

/// Collect the set of type-system names declared locally in a file
/// (dims, units, indexes, types). Used to distinguish a private-local
/// symbol (V = private at the importer) from a builtin or cross-file
/// symbol for the V006 check.
fn collect_local_type_names(
    file: &graphcal_compiler::desugar::desugared_ast::File,
) -> HashMap<String, &'static str> {
    let mut names = HashMap::new();
    for decl in &file.declarations {
        let (name, kind) = match &decl.kind {
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Dimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Unit(u) => (u.name.value.to_string(), "unit"),
            DeclKind::Index(idx) => (idx.name.value.to_string(), "index"),
            DeclKind::Type(t) => (t.name.value.to_string(), "type"),
            _ => continue,
        };
        names.insert(name, kind);
    }
    names
}

/// Walk a `TypeExpr` collecting every type-system name reference
/// (dimension / type / index / type-application). Used by the V006
/// check to decide which names in a re-exported signature need a
/// visibility review at the importing site.
fn collect_type_expr_names(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    refs: &mut Vec<String>,
) {
    use graphcal_compiler::desugar::desugared_ast::{IndexExpr, TypeExprKind};
    match &type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => {
            for item in &dim_expr.terms {
                refs.push(item.term.name.value.display_path());
            }
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_names(base, refs);
            for idx in indexes {
                if let IndexExpr::Name(path) = idx {
                    refs.push(path.value.display_path());
                }
            }
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            refs.push(name.value.display_path());
            for arg in generic_args {
                match arg {
                    graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
                        collect_type_expr_names(type_expr, refs);
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push(path.value.display_path());
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | graphcal_compiler::syntax::ast::GenericArg::Nat(_) => {}
                    graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_names(ambiguous, refs);
                    }
                }
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            // `Complex` and `Key` are built in; collect only names in their
            // generic argument.
            for arg in generic_args {
                match arg {
                    graphcal_compiler::syntax::ast::GenericArg::Type(type_expr) => {
                        collect_type_expr_names(type_expr, refs);
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push(path.value.display_path());
                    }
                    graphcal_compiler::syntax::ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | graphcal_compiler::syntax::ast::GenericArg::Nat(_) => {}
                    graphcal_compiler::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_names(ambiguous, refs);
                    }
                }
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // `Datetime` is a built-in — no top-level name to push.
            for arg in type_args {
                collect_type_expr_names(arg, refs);
            }
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

fn collect_generic_arg_names(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    refs: &mut Vec<String>,
) {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::syntax::ast::GenericArg;
    match arg {
        GenericArg::Type(type_expr) => collect_type_expr_names(type_expr, refs),
        GenericArg::Index(IndexExpr::Name(path)) => refs.push(path.value.display_path()),
        GenericArg::Index(IndexExpr::Finite { .. } | IndexExpr::BareNat(_))
        | GenericArg::Nat(_) => {}
        GenericArg::Ambiguous(ambiguous) => collect_ambiguous_generic_names(ambiguous, refs),
    }
}

fn collect_ambiguous_generic_names(
    arg: &graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg,
    refs: &mut Vec<String>,
) {
    match arg {
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            refs.push(ident.name.to_string());
        }
        graphcal_compiler::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_names(operand, refs);
            }
        }
    }
}

/// A9 case 2 / V006 — re-exported decls must not name a private-at-importer
/// symbol in their effective signature.
///
/// For every dependency declaration that the importer explicitly re-exports
/// via `{ pub name }`, walk its signature, apply the include's substitution
/// map, and check each referenced type/dim/index name. If a name resolves to a
/// declaration that exists locally in the importer but is not explicitly
/// exported, the re-export leaks a private symbol.
#[expect(
    clippy::too_many_arguments,
    reason = "the check needs the dep AST, the three substitution maps, and the importer's visibility tables"
)]
fn check_generics_leakage(
    dep_ast: &graphcal_compiler::desugar::desugared_ast::File,
    pub_reexport_items: &HashSet<DeclName>,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    importer_external_surface: &ExternalDeclSurface,
    importer_local_type_names: &HashMap<String, &'static str>,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    if pub_reexport_items.is_empty() {
        return Ok(());
    }

    for decl in &dep_ast.declarations {
        // Is this decl part of the importer's re-exported surface?
        let (decl_name, decl_kind_str) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), "param"),
            DeclKind::Node(n) => (n.name.value.to_string(), "node"),
            DeclKind::ConstNode(c) => (c.name.value.to_string(), "const node"),
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Dimension(d) => (d.name.value.to_string(), "dim"),
            DeclKind::Unit(u) => (u.name.value.to_string(), "unit"),
            DeclKind::Index(idx) => (idx.name.value.to_string(), "index"),
            DeclKind::Type(t) => (t.name.value.to_string(), "type"),
            _ => continue,
        };
        if !pub_reexport_items.contains(decl_name.as_str()) {
            continue;
        }

        // Collect every type-system name the signature references.
        let mut refs: Vec<String> = Vec::new();
        match &decl.kind {
            DeclKind::Param(p) => collect_type_expr_names(&p.type_ann, &mut refs),
            DeclKind::Node(n) => collect_type_expr_names(&n.type_ann, &mut refs),
            DeclKind::ConstNode(c) => collect_type_expr_names(&c.type_ann, &mut refs),
            DeclKind::Unit(u) => {
                for item in &u.dim_type.terms {
                    refs.push(item.term.name.value.display_path());
                }
            }
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    for item in &def.terms {
                        refs.push(item.term.name.value.display_path());
                    }
                }
            }
            DeclKind::Type(t) => {
                if let graphcal_compiler::desugar::desugared_ast::TypeDeclBody::Constructors(
                    members,
                ) = &t.body
                {
                    for member in members {
                        if let Some(fields) = &member.payload {
                            for field in fields {
                                collect_type_expr_names(&field.type_ann, &mut refs);
                            }
                        }
                    }
                }
                for default in t
                    .generic_params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_generic_arg_names(default, &mut refs);
                }
                // Bare references to lexical generic parameters are not
                // module API dependencies, even when a local type-system
                // declaration has the same leaf spelling.
                let generic_params = t
                    .generic_params
                    .iter()
                    .map(|param| param.name.value.as_str())
                    .collect::<HashSet<_>>();
                refs.retain(|name| !generic_params.contains(name.as_str()));
            }
            _ => {}
        }

        // Apply substitutions, then check each substituted name against the
        // importer's visibility table.
        for raw_name in refs {
            let index_target = index_bindings.get(raw_name.as_str());
            // A structural target carries no importer-local declaration whose
            // visibility could leak through the re-exported signature.
            if matches!(index_target, Some(IndexBindingTarget::Finite(_))) {
                continue;
            }
            let substituted = index_target
                .and_then(IndexBindingTarget::declared_name)
                .map(IndexName::as_str)
                .or_else(|| {
                    type_bindings
                        .get(raw_name.as_str())
                        .map(StructTypeName::as_str)
                })
                .or_else(|| dim_bindings.get(raw_name.as_str()).map(DimName::as_str))
                .unwrap_or(raw_name.as_str());

            if let Some(kind) = importer_local_type_names.get(substituted)
                && !importer_external_surface
                    .is_explicit_export(&DeclName::expect_valid(substituted))
            {
                return Err(CompileError::Eval(GraphcalError::GenericsLeakage {
                    reexport_kind: decl_kind_str.to_string(),
                    reexport_name: decl_name,
                    leaked_kind: (*kind).to_string(),
                    leaked_name: substituted.to_string(),
                    src: importer_src.clone(),
                    span: include_span.into(),
                }));
            }
        }
    }
    Ok(())
}
