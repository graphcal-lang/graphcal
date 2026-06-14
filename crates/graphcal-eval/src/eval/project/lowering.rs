//! IR lowering, registry merging, and finalization for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// Lower the AST to IR, process deferred instantiated imports, apply overrides,
/// and type-resolve to produce the final `CompiledFile`.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline function threading project context through IR lowering stages"
)]
#[expect(
    clippy::too_many_lines,
    reason = "project lowering coordinates resolver, TIR, and plan construction"
)]
pub(in crate::eval::project) fn lower_and_finalize(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::desugar::desugared_ast::File,
    ctx: ImportContext<'_>,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    override_targets: &HashMap<DeclName, (graphcal_compiler::dag_id::DagId, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    // Snapshot before lower_to_builder_with_imported_values consumes
    // `ctx.imported_values`. The deferred-instantiated-include processing
    // below (and lower in this function) needs the original map back —
    // builder construction moves it.
    let saved_imported_values = ctx.imported_values.clone();

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
        graphcal_compiler::ir::lower::lower_to_builder_with_imported_values(
            file_ast,
            file_src,
            &ctx.imported_names,
            ctx.imported_values,
            file_dag_id,
            Some(&mut registry_seed),
        )?;

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
    )?;

    // Apply overrides routed to this file (using original param names)
    // before the freeze boundary lowers every body to HIR.
    let file_overrides: HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr> =
        override_targets
            .iter()
            .filter(|(_, (target_dag_id, _))| target_dag_id == file_dag_id)
            .map(|(name, (_, orig_name))| (orig_name.clone(), overrides[name].clone()))
            .collect();
    if !file_overrides.is_empty() {
        apply_overrides(&mut unfrozen, &file_overrides)?;
    }

    let module_resolver = project
        .build_module_resolver()
        .map_err(|err| module_resolve_compile_error(err, file_src))?;
    let ir = unfrozen.freeze(builder.build(), file_dag_id, &module_resolver, file_src)?;

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
    module_types.insert_registry(file_dag_id, &ir.registry);
    for (dep_dag_id, evaluated) in evaluated_files {
        module_types.insert_registry(dep_dag_id, &evaluated.registry);
    }

    let parent_pub_names = ir.pub_names.clone();
    let mut tir = graphcal_compiler::tir::typed::type_resolve_with_modules(
        ir,
        file_dag_id.clone(),
        file_src,
        &module_resolver,
        &module_types,
    )?;
    compile_inline_dag_modules(
        &mut tir,
        project,
        file_dag_id,
        file_src,
        &parent_pub_names,
        evaluated_files,
        &module_resolver,
        &module_types,
    )?;
    merge_dep_dag_tirs(&mut tir, &ctx.module_map, evaluated_files);
    graphcal_compiler::tir::dim_check::check_dimensions_tir(&tir, file_src)?;

    // Resolve domain constraints at compile time so malformed bounds (C003 min
    // > max, C004 wrong target) surface under `graphcal check` instead of only
    // at `eval`. C004 alone is a pure type check (handled in `dim_check`); C003
    // additionally requires evaluating the bound expressions, which depends on
    // const values, so we evaluate consts here too. The resulting plan is
    // discarded — `exec_plan::compile` recomputes both as part of its run.
    {
        let const_values = crate::exec_plan::eval_consts_from_tir(&tir, file_src)?;
        let _ = crate::exec_plan::resolve_domain_constraints(&tir, &const_values, file_src)?;
        let field_constraints =
            crate::exec_plan::resolve_struct_field_constraints(&tir, &const_values, file_src)?;
        crate::exec_plan::check_const_struct_field_constraints_at_compile_time(
            &tir,
            &const_values,
            &field_constraints,
            file_src,
        )?;
    }

    let declared_types = tir.build_declared_types(file_src)?;

    for override_name in file_overrides.keys() {
        graphcal_compiler::tir::dim_check::check_override_dimension(
            override_name.as_str(),
            &declared_types,
            &tir,
            &tir.registry,
            file_src,
        )?;
    }

    Ok(CompiledFile {
        tir,
        declared_types,
        imported_values: saved_imported_values,
        imported_source_order: ctx.imported_source_order,
        included_plots: ctx.included_plot_specs,
    })
}

fn module_resolve_compile_error(
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
        graphcal_compiler::syntax::module_resolve::ModuleResolveError::DuplicateSymbol {
            duplicate,
            ..
        }
        | graphcal_compiler::syntax::module_resolve::ModuleResolveError::DuplicateImportName {
            duplicate,
            ..
        } => CompileError::Eval(GraphcalError::EvalError {
            message: err.to_string(),
            src: src.clone(),
            span: duplicate.into(),
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
    tir: &mut graphcal_compiler::tir::typed::TIR,
    project: &'a crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    parent_pub_names: &HashSet<DeclName>,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    module_types: &graphcal_compiler::tir::typed::ModuleTypeRegistry,
) -> Result<(), CompileError> {
    let loaded_file = &project.files[file_dag_id];
    let parent_values =
        crate::inline_dag::classify_value_decls_in_tir(tir, parent_pub_names, file_src)?;

    for loaded_dag in &loaded_file.inline_dags {
        let dag_ir = compile_loaded_dag_module_ir(
            tir,
            project,
            loaded_dag,
            file_src,
            &parent_values,
            evaluated_files,
            module_resolver,
        )?;
        let mut compiled_dag = graphcal_compiler::tir::typed::type_resolve_single_with_modules(
            dag_ir,
            &loaded_dag.dag_id,
            file_src,
            module_resolver,
            module_types,
        )?;
        compiled_dag.populate_pub_nodes(&loaded_dag.body);
        tir.dags.insert(loaded_dag.dag_id.clone(), compiled_dag);
    }

    Ok(())
}

fn compile_loaded_dag_module_ir<'a>(
    tir: &graphcal_compiler::tir::typed::TIR,
    project: &'a crate::loader::LoadedProject,
    loaded_dag: &crate::loader::LoadedDag,
    file_src: &NamedSource<Arc<String>>,
    parent_values: &crate::inline_dag::ParentValueDecls,
    evaluated_files: &'a HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
) -> Result<graphcal_compiler::ir::lower::IR, CompileError> {
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
        imported_values: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        extra_registry_builders: Vec::new(),
        deferred_dag_includes: Vec::new(),
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
    let mut imported_decl_types: HashMap<ScopedName, DeclaredType> = ctx
        .imported_values
        .iter()
        .map(|(name, (_value, ty))| (name.clone(), ty.clone()))
        .collect();
    imported_decl_types.extend(self_imports.decl_types);

    let dag_ast = graphcal_compiler::desugar::desugared_ast::File {
        declarations: self_imports.stripped_body,
    };
    let dag_ast = rewrite_qualified_refs_in_ast(&dag_ast, &ctx.module_map, &ctx.imported_names);
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
        graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_value_decls(
            dag_ast.as_ref(),
            Some(&tir.registry),
            &ctx.imported_names,
            ctx.imported_values,
            imported_decl_types,
            self_imports.value_sources,
            file_src,
            &loaded_dag.dag_id,
            Some(&mut registry_seed),
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
    )?;

    Ok(unfrozen.freeze(
        builder.build(),
        &loaded_dag.dag_id,
        module_resolver,
        file_src,
    )?)
}

fn extend_imported_value_names(target: &mut ImportedValueNames, source: ImportedValueNames) {
    target.const_names.extend(source.const_names);
    target.param_names.extend(source.param_names);
    target.node_names.extend(source.node_names);
    target.assert_names.extend(source.assert_names);
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
                    &loaded_dag.parent_dag_id,
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

/// Merge compiled per-DAG TIRs from each module-imported dependency into
/// the importer's flat `tir.dags`, keyed by the dep's canonical
/// [`DagId`](graphcal_compiler::dag_id::DagId), and record the
/// alias→DagId mapping in `tir.module_aliases` so user-typed
/// `@alias.dag(args).out` references resolve through
/// [`graphcal_compiler::tir::typed::TIR::lookup_call_target`].
///
/// Every dep `DagTIR` is brought along — the dep file's own root entry as
/// well as its inline children — under their canonical id. Cross-file
/// qualified calls (`@alias.dag(...)`) and bare module-path calls
/// (`@alias.dag(...).out`) resolve through the same flat lookup as
/// same-file calls.
///
/// Only `pub` DAGs are exposed across the import boundary; private DAGs
/// in the dep stay local (the dep's `pub_names` filters them).
///
/// Each cloned DAG TIR also receives the dep-file values named by the
/// DAG body's explicit imports, so `import dep.{const as local}`
/// resolves under the local alias at inline-call eval time.
pub(in crate::eval::project) fn merge_dep_dag_tirs(
    tir: &mut graphcal_compiler::tir::typed::TIR,
    module_map: &HashMap<ModuleAliasName, (graphcal_compiler::dag_id::DagId, Span)>,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
) {
    for (alias, (dep_dag_id, _)) in module_map {
        // Record the alias → dep DagId mapping so call paths like
        // `@alias.dag(args)` translate to `dep_dag_id.child("dag")`.
        tir.module_aliases.insert(alias.clone(), dep_dag_id.clone());

        let Some(dep_eval) = evaluated_files.get(dep_dag_id) else {
            continue;
        };
        for (dep_id, dag_tir) in &dep_eval.dag_tirs {
            // Visibility check: only carry across `pub` dags. The dep's
            // own root is treated as accessible (it's the file itself).
            let is_root = dep_id == dep_dag_id;
            if !is_root {
                let leaf = dep_id.name();
                if !dep_eval.pub_names.contains(leaf) {
                    continue;
                }
            }
            let mut cloned = dag_tir.clone();
            // Inject only the values that the dag body imported from its
            // owning dep DAG. There are no synthetic placeholder values to
            // overwrite here; the source map carries the import alias.
            for (local_name, source) in &cloned.imported_value_sources {
                if &source.dag_id != dep_dag_id {
                    continue;
                }
                if let Some(value) = dep_eval.const_values.get(&source.source_name)
                    && let Some(dt) = dep_eval
                        .declared_types
                        .get(&ScopedName::local(source.source_name.as_str()))
                {
                    cloned
                        .imported_values
                        .entry(local_name.clone())
                        .or_insert_with(|| (value.clone(), dt.clone()));
                }
            }
            tir.dags.insert(dep_id.clone(), cloned);
        }
    }
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
/// 3. Merge the body's registry into the importer's; validate range-index
///    dimension matching (file include only — inline DAGs share the
///    file's registry).
/// 4. Run `check_include_reconciles_overrides` (A8/V005) and
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
pub(in crate::eval::project) fn process_deferred_dag_includes(
    project: &crate::loader::LoadedProject,
    importer_dag_id: &graphcal_compiler::dag_id::DagId,
    importer_file_dag_id: &graphcal_compiler::dag_id::DagId,
    deferred_dag_includes: &[DeferredDagInclude],
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
    importer_src: &NamedSource<Arc<String>>,
    importer_ast: &graphcal_compiler::desugar::desugared_ast::File,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    let importer_pub_names = super::extract_pub_names(importer_ast);
    let importer_local_type_names = collect_local_type_names(importer_ast);
    let empty_resolved: HashMap<
        crate::loader::ModulePathKey,
        crate::loader::InlineBodyImportResolution,
    > = HashMap::new();

    for deferred in deferred_dag_includes {
        // ---- 1. Resolve source body + lower to IR ------------------------
        let (dep_unfrozen, dep_registry, dep_src, body_for_leakage_check, body_decls_for_aliases) =
            match &deferred.source {
                DeferredDagSource::File { dep_dag_id } => {
                    let dep_loaded = &project.files[dep_dag_id];
                    let dep_src = &dep_loaded.named_source;
                    let dep_imported =
                        build_dep_imported_values(project, dep_dag_id, evaluated_files)?;
                    let (dep_builder, dep_unfrozen) =
                        graphcal_compiler::ir::lower::lower_to_builder_with_imported_values(
                            &dep_loaded.ast,
                            dep_src,
                            &dep_imported.names,
                            dep_imported.values,
                            dep_dag_id,
                            None,
                        )?;
                    let dep_registry = dep_builder.build();
                    (
                        dep_unfrozen,
                        dep_registry,
                        dep_src.clone(),
                        &dep_loaded.ast,
                        dep_loaded.ast.declarations.as_slice(),
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
                        imported_values: HashMap::new(),
                        imported_source_order: Vec::new(),
                        imported_type_system_names: HashMap::new(),
                        module_map: HashMap::new(),
                        extra_registry_builders: Vec::new(),
                        deferred_dag_includes: Vec::new(),
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

                    let mut imported_values = body_ctx.imported_values;
                    let mut imported_decl_types: HashMap<ScopedName, DeclaredType> =
                        imported_values
                            .iter()
                            .map(|(name, (_value, ty))| (name.clone(), ty.clone()))
                            .collect();
                    imported_decl_types.extend(self_imports.decl_types);
                    // For cross-file includes (parent != importer), fetch the
                    // parent's artifact values and declared types for each
                    // self-imported name. Same-file includes leave these
                    // empty — the parent isn't in `evaluated_files`
                    // yet, and the merged refs land on names already present
                    // in the importer's own decls.
                    if parent_dag_id != importer_file_dag_id
                        && let Some(parent_eval) = evaluated_files.get(parent_dag_id)
                    {
                        for (local_name, source) in &self_imports.value_sources {
                            if &source.dag_id != parent_dag_id {
                                continue;
                            }
                            let Some(value) = parent_eval.const_values.get(&source.source_name)
                            else {
                                continue;
                            };
                            let parent_key = ScopedName::local(source.source_name.as_str());
                            let Some(dt) = parent_eval.declared_types.get(&parent_key) else {
                                continue;
                            };
                            imported_values.insert(local_name.clone(), (value.clone(), dt.clone()));
                            // For cross-file the alias has no importer-side
                            // decl, so install the parent's actual declared
                            // type (overrides the placeholder from
                            // classify_value_decls_in_ast).
                            imported_decl_types.insert(local_name.clone(), dt.clone());
                        }
                    }

                    let stripped_body = graphcal_compiler::desugar::desugared_ast::File {
                        declarations: self_imports.stripped_body,
                    };
                    let stripped_body = rewrite_qualified_refs_in_ast(
                        &stripped_body,
                        &body_ctx.module_map,
                        &body_ctx.imported_names,
                    );

                    let dag_dag_id = importer_dag_id.child(deferred.prefix.as_str());
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
                    let (mut dag_builder, mut dag_unfrozen) = graphcal_compiler::ir::lower::lower_dag_module_to_builder_with_imported_value_decls(
                        stripped_body.as_ref(),
                        None,
                        &body_ctx.imported_names,
                        imported_values,
                        imported_decl_types,
                        self_imports.value_sources,
                        importer_src,
                        &dag_dag_id,
                        Some(&mut registry_seed),
                    )?;
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
                    )?;
                    let dag_registry = dag_builder.build();
                    (
                        dag_unfrozen,
                        dag_registry,
                        importer_src.clone(),
                        dag_body,
                        dag_body.declarations.as_slice(),
                    )
                }
            };

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

        // ---- 3. Validate range-index dimension matching (file include
        // only — inline DAGs share the file's registry, so there are no
        // separate dep-side range indexes to reconcile). --------------------
        if matches!(deferred.source, DeferredDagSource::File { .. }) {
            for (dep_idx_name, importer_idx_name) in &deferred.index_bindings {
                if let Some(dep_idx_def) = dep_registry.indexes.get_index(dep_idx_name.as_str())
                    && let graphcal_compiler::registry::types::IndexKind::RequiredRange {
                        dimension: dep_dim,
                    } = &dep_idx_def.kind
                    && let Some(imp_idx_def) = builder.get_index(importer_idx_name.as_str())
                    && let graphcal_compiler::registry::types::IndexKind::Range(
                        graphcal_compiler::registry::types::RangeIndexData {
                            dimension: imp_dim,
                            ..
                        },
                    )
                    | graphcal_compiler::registry::types::IndexKind::RequiredRange {
                        dimension: imp_dim,
                    } = &imp_idx_def.kind
                    && dep_dim != imp_dim
                {
                    return Err(CompileError::Eval(
                        GraphcalError::IndexBindingDimensionMismatch {
                            dep_index: dep_idx_name.as_str().to_string(),
                            expected_dim: dep_registry.dimensions.format_dimension(dep_dim),
                            bound_index: importer_idx_name.as_str().to_string(),
                            found_dim: builder.format_dimension(imp_dim),
                            src: dep_src,
                            span: deferred.import_span.into(),
                        },
                    ));
                }
            }
        }

        // ---- 4. Validation checks -----------------------------------------
        let mut dep_names: HashSet<DeclName> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(DeclName::new(name.member()));
        }
        dep_unfrozen.check_include_reconciles_overrides(
            &deferred.bindings,
            &deferred.index_bindings,
            &deferred.type_bindings,
            importer_src,
            deferred.import_span,
        )?;
        check_generics_leakage(
            body_for_leakage_check,
            deferred.pub_reexport_whole,
            &deferred.pub_reexport_items,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &importer_pub_names,
            &importer_local_type_names,
            importer_src,
            deferred.import_span,
        )?;

        // ---- 5. Merge dep IR into importer's IR ---------------------------
        unfrozen.merge_dependency(
            dep_unfrozen,
            deferred.prefix.as_str(),
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &deferred.import_item_attributes,
            &deferred.requested_plots,
            importer_src,
            &dep_src,
        )?;

        // ---- 6. Add selective aliases -------------------------------------
        if let Some(selective) = &deferred.selective_names {
            add_selective_aliases_inner(
                body_decls_for_aliases,
                selective,
                &deferred.prefix,
                &AliasSubstitutions {
                    index: &deferred.index_bindings,
                    r#type: &deferred.type_bindings,
                    dim: &deferred.dim_bindings,
                },
                deferred.import_span,
                unfrozen,
            );
        }
    }
    Ok(())
}

/// Bindings that an alias's type annotation must be rewritten through before
/// it is registered in the importer's IR. Shared by both inline-DAG and
/// file-include alias paths so their type-substitution stays in lock-step.
pub(in crate::eval::project) struct AliasSubstitutions<'a> {
    pub index: &'a HashMap<IndexName, IndexName>,
    pub r#type: &'a HashMap<StructTypeName, StructTypeName>,
    pub dim: &'a HashMap<DimName, DimName>,
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
    import_span: Span,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    for alias in selective {
        let orig_name = alias.original.as_str();
        let local_name = alias.local.as_str();
        // The alias points at the dep's prefixed declaration: a typed
        // qualified `ScopedName`. No flat `prefix::orig_name` strings are
        // built — the qualification stays structural through the IR.
        let target = ScopedName::qualified(prefix.as_str(), orig_name);

        let type_ann = decls.iter().find_map(|d| match &d.kind {
            DeclKind::Param(p) if p.name.value.as_str() == orig_name => Some(p.type_ann.clone()),
            DeclKind::Node(n) if n.name.value.as_str() == orig_name => Some(n.type_ann.clone()),
            DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name => {
                Some(c.type_ann.clone())
            }
            _ => None,
        });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        graphcal_compiler::ir::lower::substitute_type_expr_index_names(&mut type_ann, subs.index);
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            subs.r#type,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(&mut type_ann, subs.dim);

        let is_const = decls.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );
        let alias_kind = if is_const {
            // A const alias body is a reference path to the prefixed target;
            // HIR lowering resolves it against the merged entries.
            let (Ok(prefix_atom), Ok(member_atom)) = (
                graphcal_compiler::syntax::names::NameAtom::parse(prefix.as_str()),
                graphcal_compiler::syntax::names::NameAtom::parse(orig_name),
            ) else {
                // Alias components come from validated import items; a
                // non-identifier segment cannot resolve, so skip the alias.
                continue;
            };
            ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(
                graphcal_compiler::syntax::ast::IdentPath::new(
                    graphcal_compiler::syntax::non_empty::NonEmpty::new(
                        graphcal_compiler::syntax::ast::Ident {
                            name: prefix_atom,
                            span: import_span,
                        },
                        vec![graphcal_compiler::syntax::ast::Ident {
                            name: member_atom,
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
                ScopedName::local(local_name),
                type_ann,
                alias_expr,
                import_span,
            );
        } else {
            unfrozen.add_node_alias(
                ScopedName::local(local_name),
                type_ann,
                alias_expr,
                import_span,
            );
        }
    }
}

/// Merge type-system declarations from a dependency's frozen Registry into a builder.
///
/// This imports dimensions, units, indexes, and struct types so that the
/// importing file can reference them.
pub(in crate::eval::project) fn merge_registry_into_builder(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<IndexName, IndexName>,
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
        graphcal_compiler::ir::lower::register_selected_declarations(
            &dep_loaded.ast,
            builder,
            &dep_loaded.named_source,
            names,
            dep_dag_id,
        )?;
        // A selectively imported dynamic unit re-lowered from the dep's AST
        // carries a scale expression that references the dep's own params
        // and cannot be evaluated in this file's context. Substitute the
        // concrete scale from the dep's evaluation when available.
        if let Some(dep_eval) = evaluated_files.get(dep_dag_id) {
            replace_dynamic_units_with_resolved_scales(
                builder,
                names,
                &dep_eval.resolved_dynamic_unit_scales,
            );
        }
    }
    for import in extra_registry_builders {
        merge_registry_into_builder_pub_filtered(builder, import).map_err(|conflict| {
            GraphcalError::ConflictingImportedUnit {
                name: conflict.name,
                src: file_src.clone(),
                span: import.import_span.into(),
            }
        })?;
    }
    Ok(())
}

/// Overwrite selectively imported dynamic units with their dep-resolved
/// static scales. Units without a resolved scale keep the dynamic form and
/// surface the existing loud could-not-be-resolved error if actually used.
fn replace_dynamic_units_with_resolved_scales(
    builder: &mut RegistryBuilder,
    names: &graphcal_compiler::ir::lower::SelectedDeclarations,
    resolved_scales: &HashMap<
        graphcal_compiler::syntax::names::UnitRef,
        graphcal_compiler::registry::types::PositiveFiniteScale,
    >,
) {
    use graphcal_compiler::registry::types::UnitScale;
    for name in &names.default {
        let unit_ref = graphcal_compiler::syntax::names::UnitRef::local(name.clone());
        let Some(resolved) = resolved_scales.get(&unit_ref) else {
            continue;
        };
        let Some(info) = builder.get_unit(&unit_ref) else {
            continue;
        };
        if matches!(info.scale, UnitScale::Dynamic { .. }) {
            let dim = info.dimension.clone();
            let constness = info.constness;
            builder.register_unit_with_scale(
                unit_ref,
                dim,
                UnitScale::Static(*resolved),
                constness,
            );
        }
    }
}

/// Merge type-system declarations from a dependency's frozen Registry into a
/// builder, restricted to names listed in `pub_names`. Used for module imports
/// where only public items should cross the boundary.
pub(in crate::eval::project) fn merge_registry_into_builder_pub_filtered(
    builder: &mut RegistryBuilder,
    import: &ModuleRegistryImport<'_>,
) -> Result<(), UnitMergeConflict> {
    merge_registry_into_builder_filtered(
        builder,
        import.registry,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        Some(import.pub_names),
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
    pub name: graphcal_compiler::syntax::names::UnitRef,
}

pub(in crate::eval::project) fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<IndexName, IndexName>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    pub_names: Option<&HashSet<DeclName>>,
    unit_alias: Option<(
        &ModuleAliasName,
        &HashMap<
            graphcal_compiler::syntax::names::UnitRef,
            graphcal_compiler::registry::types::PositiveFiniteScale,
        >,
    )>,
) -> Result<(), UnitMergeConflict> {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        if pub_names.is_some_and(|visible| !visible.contains(name.as_str())) {
            continue;
        }
        builder.register_base_dimension(
            graphcal_compiler::syntax::names::DimName::new(name),
            id.clone(),
        );
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        if pub_names.is_some_and(|visible| !visible.contains(name.as_str())) {
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
            if pub_names.is_some_and(|visible| !visible.contains(name.name().as_str())) {
                continue;
            }
            graphcal_compiler::syntax::names::UnitRef::qualified(alias.clone(), name.name().clone())
        } else {
            if pub_names.is_some_and(|visible| !visible.contains(name.name().as_str())) {
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
    // Module imports use `pub_names` filtering and keep required indexes in the
    // dependency's module registry only; pulling an unbound `pub(bind)` index
    // into the importer would incorrectly make the importer a library even if
    // it only needs a qualified type from the dependency.
    for idx_def in dep_registry.indexes.all_indexes() {
        if pub_names.is_some() && idx_def.is_required() {
            continue;
        }
        if !index_bindings.contains_key(idx_def.name.as_str()) {
            if pub_names.is_some_and(|visible| !visible.contains(idx_def.name.as_str())) {
                continue;
            }
            builder.register_index(idx_def.clone());
        }
    }

    // Import struct types — skip bound types (they are replaced by the importer's type).
    for type_def in dep_registry.types.all_types() {
        if type_bindings.contains_key(type_def.name.as_str()) {
            continue;
        }
        if pub_names.is_some_and(|visible| !visible.contains(type_def.name.as_str())) {
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
    dim: &graphcal_compiler::syntax::dimension::Dimension,
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

/// Extract a `PascalCase` index name from a binding expression.
///
/// Index bindings use the form `DepIndex = ImporterIndex`, where both sides are
/// `PascalCase` identifiers. In the desugared AST a bare identifier is an
/// unresolved reference path; the parser can also produce a zero-arg
/// `ConstructorCall` for constructor-shaped binding RHSs.
pub(in crate::eval::project) fn extract_index_name_from_binding_expr(
    expr: &Expr,
    dep_index_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match &expr.kind {
        ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(path)) => path
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::BindingTargetsIndex {
                    name: dep_index_name.to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            }),
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } if generic_args.is_empty() && fields.is_empty() => callee
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::BindingTargetsIndex {
                    name: dep_index_name.to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            }),
        _ => Err(CompileError::Eval(GraphcalError::BindingTargetsIndex {
            name: dep_index_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })),
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
    match &expr.kind {
        ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(path)) => path
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::BindingTargetsIndex {
                    name: dep_type_name.to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            }),
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } if generic_args.is_empty() && fields.is_empty() => callee
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::BindingTargetsIndex {
                    name: dep_type_name.to_string(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
            }),
        _ => Err(CompileError::Eval(GraphcalError::BindingTargetsIndex {
            name: dep_type_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })),
    }
}

/// Build imported value names and values for a dependency file from its own transitive imports.
///
/// This mirrors the import-processing logic in `compile_single_file_in_project` but
/// only for non-instantiated imports (the dependency's own transitive deps already
/// have compiled artifacts in `evaluated_files`).
pub(in crate::eval::project) fn build_dep_imported_values(
    project: &crate::loader::LoadedProject,
    dep_dag_id: &graphcal_compiler::dag_id::DagId,
    evaluated_files: &HashMap<graphcal_compiler::dag_id::DagId, EvaluatedFile>,
) -> Result<DepImportedValues, CompileError> {
    let dep_loaded = &project.files[dep_dag_id];
    let dep_src = &dep_loaded.named_source;

    let mut imported_names = ImportedValueNames::default();
    let mut imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)> = HashMap::new();

    // Process import declarations (non-instantiated).
    for (_decl, import_decl, trans_canonical) in dep_loaded.imports_with_dag_ids() {
        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "transitive dependency `{trans_canonical}` is not available for imports",
                ),
                src: dep_src.clone(),
                span: import_decl.path.span().into(),
            })
        })?;

        build_dep_import_values_for_kind(
            &import_decl.path,
            &import_decl.kind,
            trans_dep,
            dep_src,
            &mut imported_names,
            &mut imported_values,
            true, // is_import: skip runtime items
        )?;
    }

    // Process include declarations.
    for (_decl, include_decl, trans_canonical) in dep_loaded.includes_with_dag_ids() {
        if !include_decl.param_bindings.is_empty() {
            // Nested instantiated includes are not supported in this initial implementation.
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "nested instantiated includes are not yet supported".to_string(),
                src: dep_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }

        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "transitive dependency `{trans_canonical}` is not available for imports",
                ),
                src: dep_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        build_dep_import_values_for_kind(
            &include_decl.path,
            &include_decl.kind,
            trans_dep,
            dep_src,
            &mut imported_names,
            &mut imported_values,
            false, // is_import: include allows runtime items
        )?;
    }

    Ok(DepImportedValues {
        names: imported_names,
        values: imported_values,
    })
}

/// Helper: import values from a dependency according to the import kind.
///
/// When `is_import` is `true`, runtime values are skipped (import semantics).
pub(in crate::eval::project) fn build_dep_import_values_for_kind(
    import_path: &ModulePath,
    import_kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    trans_dep: &EvaluatedFile,
    dep_src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    is_import: bool,
) -> Result<(), CompileError> {
    match import_kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();
                if is_import && trans_dep.values.contains_key(orig_name.as_str()) {
                    // The dep file's own import has already been validated.
                    // For transitive const-only imports, runtime values do not
                    // propagate through the compile-time import chain.
                    continue;
                }
                let _ = imports::import_selective_item(
                    trans_dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    dep_src,
                    imported_names,
                    imported_values,
                    None,
                )?;
            }
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || derive_module_name_from_import_path(import_path),
                |alias_ident| alias_ident.value.clone(),
            );
            let import_span = import_path.span();
            imports::import_module_values(
                trans_dep,
                &module_name,
                import_span,
                dep_src,
                imported_names,
                imported_values,
                None,
                is_import,
            )?;
        }
    }
    Ok(())
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
        TypeExprKind::TypeApplication { name, type_args } => {
            refs.push(name.value.display_path());
            for arg in type_args {
                collect_type_expr_names(arg, refs);
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

/// A9 case 2 / V006 — re-exported decls must not name a private-at-importer
/// symbol in their effective signature.
///
/// For every decl in the dep that the importer re-exports (whole-module
/// `pub include` / `pub import`, or selectively via `{ pub name }`),
/// walk its signature, apply the include's substitution map, and check
/// each referenced type/dim/index name: if the name resolves to a
/// declaration that exists locally in the importer but is not in the
/// importer's `pub_names`, the re-export leaks a private symbol.
#[expect(
    clippy::too_many_arguments,
    reason = "the check needs the dep AST, the three substitution maps, and the importer's visibility tables"
)]
fn check_generics_leakage(
    dep_ast: &graphcal_compiler::desugar::desugared_ast::File,
    pub_reexport_whole: bool,
    pub_reexport_items: &HashSet<DeclName>,
    index_bindings: &HashMap<IndexName, IndexName>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    importer_pub_names: &HashSet<DeclName>,
    importer_local_type_names: &HashMap<String, &'static str>,
    importer_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    if !pub_reexport_whole && pub_reexport_items.is_empty() {
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
        let reexported = if pub_reexport_whole {
            decl_is_public(decl)
        } else {
            pub_reexport_items.contains(decl_name.as_str())
        };
        if !reexported {
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
            }
            _ => {}
        }

        // Apply substitutions, then check each substituted name against the
        // importer's visibility table.
        for raw_name in refs {
            let substituted = index_bindings
                .get(raw_name.as_str())
                .map(IndexName::as_str)
                .or_else(|| {
                    type_bindings
                        .get(raw_name.as_str())
                        .map(StructTypeName::as_str)
                })
                .or_else(|| dim_bindings.get(raw_name.as_str()).map(DimName::as_str))
                .unwrap_or(raw_name.as_str());

            if let Some(kind) = importer_local_type_names.get(substituted)
                && !importer_pub_names.contains(substituted)
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
