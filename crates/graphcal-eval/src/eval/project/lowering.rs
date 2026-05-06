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
pub(super) fn lower_and_finalize(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::desugar::desugared_ast::File,
    ctx: ImportContext<'_>,
    evaluated_files: &HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    override_targets: &HashMap<DeclName, (graphcal_compiler::syntax::dag_id::DagId, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    // Snapshot before lower_to_builder_with_imported_values consumes
    // `ctx.imported_values`. The deferred-instantiated-include processing
    // below (and lower in this function) needs the original map back —
    // builder construction moves it.
    let saved_imported_values = ctx.imported_values.clone();

    let (mut builder, mut unfrozen) =
        graphcal_compiler::ir::lower::lower_to_builder_with_imported_values(
            file_ast,
            file_src,
            &ctx.imported_names,
            ctx.imported_values,
            file_dag_id,
        )?;

    // Register type-system declarations from selectively imported files.
    for (dep_dag_id, names) in &ctx.imported_type_system_names {
        let dep_loaded = &project.files[dep_dag_id];
        graphcal_compiler::ir::lower::register_selected_declarations(
            &dep_loaded.ast,
            &mut builder,
            &dep_loaded.named_source,
            names,
            dep_dag_id,
        )?;
    }

    // Merge type-system declarations from module-imported registries (pub items only).
    for (dep_registry, pub_names) in &ctx.extra_registry_builders {
        merge_registry_into_builder_filtered(
            &mut builder,
            dep_registry,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            Some(pub_names),
        );
    }

    // Process deferred instantiated imports: compile dep to IR and merge.
    process_deferred_instantiated_imports(
        project,
        file_dag_id,
        &ctx.deferred_instantiated,
        evaluated_files,
        file_src,
        &mut builder,
        &mut unfrozen,
    )?;

    // Process deferred inline DAG includes: compile DAG body to IR and merge.
    process_deferred_inline_dag_includes(
        &ctx.deferred_inline_dags,
        file_src,
        file_ast,
        &mut builder,
        &mut unfrozen,
    )?;

    let ir = unfrozen.freeze(builder.build());

    // Apply overrides routed to this file (using original param names).
    let mut ir = ir;
    let file_overrides: HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr> =
        override_targets
            .iter()
            .filter(|(_, (target_dag_id, _))| target_dag_id == file_dag_id)
            .map(|(name, (_, orig_name))| (orig_name.clone(), overrides[name].clone()))
            .collect();
    if !file_overrides.is_empty() {
        apply_overrides(&mut ir, &file_overrides)?;
    }

    // Type-resolve, merge dep dag TIRs from module imports, then check dimensions.
    let mut tir = graphcal_compiler::tir::typed::type_resolve(ir, file_src)?;
    merge_dep_dag_tirs(&mut tir, &ctx.module_map, evaluated_files);
    graphcal_compiler::tir::dim_check::check_dimensions_tir(&tir, file_src)?;

    let declared_types = tir.build_declared_types(file_src)?;

    for (override_name, override_expr) in &file_overrides {
        graphcal_compiler::tir::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
            &tir.dags,
            &tir.registry,
            file_src,
        )?;
    }

    Ok(CompiledFile {
        tir,
        declared_types,
        imported_values: saved_imported_values,
        imported_source_order: ctx.imported_source_order,
    })
}

/// Merge compiled dag TIRs from module-aliased dependencies into the
/// importer's `tir.dags`, keyed by a two-segment
/// [`DagKey::aliased(alias, dag_name)`](graphcal_compiler::tir::typed::DagKey::aliased).
///
/// Enables cross-file qualified inline calls `@alias.dag(args).out` to
/// resolve via the same `tir.dags` lookup used for same-file calls.
///
/// Only `pub` dags are exposed across the import boundary; private dags
/// in the dep stay local (the dep's `pub_names` already filters them).
///
/// Each cloned dag TIR also receives its dep file's `const_values` in
/// `imported_values`, so dag bodies that reference dep-file consts via
/// `import .. { name }` can resolve them at inline-call eval time.
pub(super) fn merge_dep_dag_tirs(
    tir: &mut graphcal_compiler::tir::typed::TIR,
    module_map: &HashMap<String, (graphcal_compiler::syntax::dag_id::DagId, Span)>,
    evaluated_files: &HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
) {
    for (alias, (dep_dag_id, _)) in module_map {
        let Some(dep_eval) = evaluated_files.get(dep_dag_id) else {
            continue;
        };
        for (dep_key, dag_tir) in &dep_eval.dag_tirs {
            // Pre-merge dep `dag_tirs` keys are always single-segment
            // (`DagKey::local`) — the dep file's own dag declarations.
            // Anything else would be a cross-file merge having happened
            // there too, which is not produced by the pipeline.
            if !dep_key.is_local() {
                continue;
            }
            let dag_name = dep_key.leaf();
            if !dep_eval.pub_names.contains(dag_name) {
                continue;
            }
            let mut cloned = dag_tir.clone();
            // Inject dep-file consts (and any other declared values) so a
            // dep dag body's `@r_earth` reference resolves even when the
            // importer has no such name in scope.
            for (name, value) in &dep_eval.const_values {
                if let Some(dt) = dep_eval.declared_types.get(name) {
                    cloned
                        .imported_values
                        .entry(ScopedName::local(name.clone()))
                        .or_insert_with(|| (value.clone(), dt.clone()));
                }
            }
            tir.dags.insert(
                graphcal_compiler::tir::typed::DagKey::aliased(alias.clone(), dag_name),
                cloned,
            );
        }
    }
}

/// Process deferred instantiated imports by compiling each dependency to IR
/// and merging it into the importer's IR.
pub(super) fn process_deferred_instantiated_imports(
    project: &crate::loader::LoadedProject,
    importer_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    deferred_imports: &[DeferredInstantiatedImport],
    evaluated_files: &HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
    importer_src: &NamedSource<Arc<String>>,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    let importer_ast = &project.files[importer_dag_id].ast;
    let importer_pub_names = super::extract_pub_names(importer_ast);
    let importer_local_type_names = collect_local_type_names(importer_ast);
    for deferred in deferred_imports {
        let dep_loaded = &project.files[&deferred.dep_dag_id];
        let dep_src = &dep_loaded.named_source;

        // Build imported values for the dependency from its own transitive imports.
        let dep_imported =
            build_dep_imported_values(project, &deferred.dep_dag_id, evaluated_files)?;

        // Compile the dependency to IR.
        let (dep_builder, dep_unfrozen) =
            graphcal_compiler::ir::lower::lower_to_builder_with_imported_values(
                &dep_loaded.ast,
                dep_src,
                &dep_imported.names,
                dep_imported.values,
                &deferred.dep_dag_id,
            )?;

        // Merge the dependency's type-system declarations into the importer's registry.
        let dep_registry = dep_builder.build();
        merge_registry_into_builder(
            builder,
            &dep_registry,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
        );

        // Validate range index dimension matching (Phase B — requires compiled registries).
        for (dep_idx_name, importer_idx_name) in &deferred.index_bindings {
            if let Some(dep_idx_def) = dep_registry.indexes.get_index(dep_idx_name.as_str())
                && let graphcal_compiler::registry::types::IndexKind::RequiredRange {
                    dimension: dep_dim,
                } = &dep_idx_def.kind
                && let Some(imp_idx_def) = builder.get_index(importer_idx_name.as_str())
                && let graphcal_compiler::registry::types::IndexKind::Range(
                    graphcal_compiler::registry::types::RangeIndexData {
                        dimension: imp_dim, ..
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
                        src: dep_src.clone(),
                        span: deferred.import_span.into(),
                    },
                ));
            }
        }

        // Collect all declaration names in the dependency (for prefix_expr_refs).
        // These are un-prefixed member names used for containment checks.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }

        // A8 / V005: the importer must re-bind every bindable symbol whose
        // default mentions a nominally-tied name of an overridden bindable.
        dep_unfrozen.check_include_reconciles_overrides(
            &deferred.bindings,
            &deferred.index_bindings,
            &deferred.type_bindings,
            importer_src,
            deferred.import_span,
        )?;

        // A9 case 2 / V006: a `pub`-re-exported decl must not leak a
        // private-at-importer symbol through its signature.
        check_generics_leakage(
            &dep_loaded.ast,
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

        // Merge the dependency's IR into the importer's IR.
        unfrozen.merge_dependency(
            dep_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &deferred.import_item_attributes,
            importer_src,
        )?;

        // For selective instantiated imports, add alias nodes that reference
        // the prefixed declarations. E.g., `delta_v` → `@prefix::delta_v`.
        if let Some(selective) = &deferred.selective_names {
            add_selective_aliases(dep_loaded, selective, deferred, unfrozen);
        }
    }
    Ok(())
}

/// Process deferred inline DAG includes by compiling each DAG body to IR
/// and merging it into the importer's IR.
pub(super) fn process_deferred_inline_dag_includes(
    deferred_dags: &[DeferredInlineDagInclude],
    file_src: &NamedSource<Arc<String>>,
    importer_ast: &graphcal_compiler::desugar::desugared_ast::File,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    let importer_pub_names = super::extract_pub_names(importer_ast);
    let importer_local_type_names = collect_local_type_names(importer_ast);
    for deferred in deferred_dags {
        // Compile the DAG body to IR.
        // The DAG body is lowered as if it were a standalone file, with only
        // prelude + explicitly imported items in scope.
        let dag_dag_id = graphcal_compiler::syntax::dag_id::DagId::from_relative_path(
            std::path::Path::new(file_src.name()),
        )
        .child(deferred.prefix.as_str());
        let (dag_builder, dag_unfrozen) =
            graphcal_compiler::ir::lower::lower_to_builder_with_imported_values(
                &deferred.dag_body,
                file_src,
                &deferred.dag_imported_names,
                HashMap::new(), // No pre-evaluated values for inline DAGs
                &dag_dag_id,
            )?;

        // Per Concept 9, inline DAGs are strictly isolated: there are no
        // parent-scope type-system declarations to register in the DAG's
        // registry. Drop straight to merging the DAG's own registry into the
        // importer's.
        // Merge the DAG's type-system declarations into the importer's registry.
        let dag_registry = dag_builder.build();
        merge_registry_into_builder(
            builder,
            &dag_registry,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
        );

        // Collect all declaration names in the DAG body.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dag_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }

        // A8 / V005: the importer must re-bind every bindable symbol whose
        // default mentions a nominally-tied name of an overridden bindable.
        dag_unfrozen.check_include_reconciles_overrides(
            &deferred.bindings,
            &deferred.index_bindings,
            &deferred.type_bindings,
            file_src,
            deferred.import_span,
        )?;

        // A9 case 2 / V006: a `pub`-re-exported decl must not leak a
        // private-at-importer symbol through its signature.
        check_generics_leakage(
            &deferred.dag_body,
            deferred.pub_reexport_whole,
            &deferred.pub_reexport_items,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &importer_pub_names,
            &importer_local_type_names,
            file_src,
            deferred.import_span,
        )?;

        // Merge the DAG's IR into the importer's IR.
        unfrozen.merge_dependency(
            dag_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.type_bindings,
            &deferred.dim_bindings,
            &deferred.import_item_attributes,
            file_src,
        )?;

        // For selective imports, add alias nodes.
        if let Some(selective) = &deferred.selective_names {
            add_inline_dag_selective_aliases(&deferred.dag_body, selective, deferred, unfrozen);
        }
    }
    Ok(())
}

/// Add alias declarations for selective inline DAG includes.
pub(super) fn add_inline_dag_selective_aliases(
    dag_body: &graphcal_compiler::desugar::desugared_ast::File,
    selective: &[(String, String)],
    deferred: &DeferredInlineDagInclude,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    for (orig_name, local_name) in selective {
        let prefixed_name = format!("{}::{}", deferred.prefix, orig_name);

        // Find the type annotation from the DAG body's declarations.
        let type_ann = dag_body.declarations.iter().find_map(|d| match &d.kind {
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

        // Substitute index/type/dim names in the type annotation.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            &deferred.type_bindings,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            &deferred.dim_bindings,
        );

        let is_const = dag_body.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );
        let alias_expr = if is_const {
            Expr::new(
                ExprKind::ConstRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                deferred.import_span,
            )
        } else {
            Expr::new(
                ExprKind::GraphRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                deferred.import_span,
            )
        };

        if is_const {
            unfrozen.add_const_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        } else {
            unfrozen.add_node_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        }
    }
}

/// Add alias declarations for selective instantiated imports.
///
/// For each selected name, creates either a const or node alias in the importer's IR
/// that references the prefixed declaration from the merged dependency.
pub(super) fn add_selective_aliases(
    dep_loaded: &crate::loader::LoadedFile,
    selective: &[(String, String)],
    deferred: &DeferredInstantiatedImport,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    for (orig_name, local_name) in selective {
        let prefixed_name = format!("{}::{}", deferred.prefix, orig_name);

        // Find the type annotation from the dependency's AST.
        let type_ann = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Param(p) if p.name.value.as_str() == orig_name => {
                    Some(p.type_ann.clone())
                }
                DeclKind::Node(n) if n.name.value.as_str() == orig_name => Some(n.type_ann.clone()),
                DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name => {
                    Some(c.type_ann.clone())
                }
                _ => None,
            });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        // Substitute index/type/dim names in the type annotation so the alias
        // resolves against the importer's merged registry.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            &deferred.type_bindings,
        );
        graphcal_compiler::ir::lower::substitute_type_expr_nominal_names(
            &mut type_ann,
            &deferred.dim_bindings,
        );

        // Determine if this is a const or runtime declaration.
        let is_const = dep_loaded.ast.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );

        // Create an alias expression: `@prefix::orig_name` (or `PREFIX::CONST`)
        let alias_expr = if is_const {
            Expr::new(
                ExprKind::ConstRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                deferred.import_span,
            )
        } else {
            Expr::new(
                ExprKind::GraphRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                deferred.import_span,
            )
        };

        // Add the alias as a declaration in the importer's IR.
        if is_const {
            unfrozen.add_const_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        } else {
            unfrozen.add_node_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        }
    }
}

/// Merge type-system declarations from a dependency's frozen Registry into a builder.
///
/// This imports dimensions, units, indexes, and struct types so that the
/// importing file can reference them.
pub(super) fn merge_registry_into_builder(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<IndexName, IndexName>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) {
    merge_registry_into_builder_filtered(
        builder,
        dep_registry,
        index_bindings,
        type_bindings,
        dim_bindings,
        None,
    );
}

pub(super) fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<IndexName, IndexName>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    pub_names: Option<&HashSet<String>>,
) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        if pub_names.is_some_and(|visible| !visible.contains(name)) {
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
    for (name, dim, scale) in dep_registry.units.all_units() {
        if pub_names.is_some_and(|visible| !visible.contains(name.as_str())) {
            continue;
        }
        builder.register_unit_dynamic((*name).clone(), dim.clone(), scale.clone());
    }

    // Import indexes — skip bound indexes (they are replaced by the importer's index).
    for idx_def in dep_registry.indexes.all_indexes() {
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
}

/// Extract a `PascalCase` index name from a binding expression.
///
/// Index bindings use the form `DepIndex = ImporterIndex`, where both sides are
/// `PascalCase` identifiers. The parser produces `ExprKind::StructConstruction`
/// (with empty `fields`/`type_args`) for bare `PascalCase` identifiers in
/// expression position, because it cannot distinguish a bare type name from an
/// index name at parse time.
pub(super) fn extract_index_name_from_binding_expr(
    expr: &Expr,
    dep_index_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match &expr.kind {
        ExprKind::ConstRef(name) => Ok(name.value.to_string()),
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } if type_args.is_empty() && fields.is_empty() => Ok(type_name.value.as_str().to_string()),
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
/// `PascalCase` identifier, which the parser produces as a zero-arg
/// `StructConstruction` (same shape as the index-binding RHS).
pub(super) fn extract_type_name_from_binding_expr(
    expr: &Expr,
    dep_type_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match &expr.kind {
        ExprKind::ConstRef(name) => Ok(name.value.to_string()),
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } if type_args.is_empty() && fields.is_empty() => Ok(type_name.value.as_str().to_string()),
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
/// only for non-instantiated imports (the dependency's own transitive deps are already
/// evaluated and stored in `evaluated_files`).
pub(super) fn build_dep_imported_values(
    project: &crate::loader::LoadedProject,
    dep_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    evaluated_files: &HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
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
                    "internal: transitive dependency {trans_canonical} not yet evaluated",
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
        );
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
                    "internal: transitive dependency {trans_canonical} not yet evaluated",
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
        );
    }

    Ok(DepImportedValues {
        names: imported_names,
        values: imported_values,
    })
}

/// Helper: import values from a dependency according to the import kind.
///
/// When `is_import` is `true`, runtime values are skipped (import semantics).
pub(super) fn build_dep_import_values_for_kind(
    import_path: &ModulePath,
    import_kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    trans_dep: &EvaluatedFile,
    _dep_src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    is_import: bool,
) {
    match import_kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();
                let result = imports::import_selective_item(
                    trans_dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    imported_names,
                    imported_values,
                    None,
                );
                // For transitive import dependencies, skip runtime items silently
                // (the dep file was already validated; we just don't propagate runtime values
                // through import chains).
                if is_import && matches!(result, SelectiveImportResult::Runtime) {
                    // Runtime item was registered by import_selective_item;
                    // remove it since import doesn't allow runtime items.
                    let scoped = ScopedName::Local(local_name);
                    imported_values.remove(&scoped);
                    imported_names.param_names.retain(|(s, _)| *s != scoped);
                }
            }
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || derive_module_name_from_import_path(import_path),
                |alias_ident| alias_ident.name.clone(),
            );
            let import_span = import_path.span();
            imports::import_module_values(
                trans_dep,
                &module_name,
                import_span,
                imported_names,
                imported_values,
                None,
                is_import,
            );
        }
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
            DeclKind::UnionType(u) => (u.name.value.to_string(), "type"),
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
                refs.push(item.term.name.name.clone());
            }
        }
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_expr_names(base, refs);
            for idx in indexes {
                if let IndexExpr::Name(ident) = idx {
                    refs.push(ident.name.clone());
                }
            }
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            refs.push(name.name.clone());
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
    pub_reexport_items: &HashSet<String>,
    index_bindings: &HashMap<IndexName, IndexName>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    importer_pub_names: &HashSet<String>,
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
            DeclKind::UnionType(u) => (u.name.value.to_string(), "type"),
            _ => continue,
        };
        let implicitly_visible = matches!(decl.kind, DeclKind::Param(_));
        let reexported = if pub_reexport_whole {
            decl.is_pub() || implicitly_visible
        } else {
            pub_reexport_items.contains(&decl_name)
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
                    refs.push(item.term.name.name.clone());
                }
            }
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    for item in &def.terms {
                        refs.push(item.term.name.name.clone());
                    }
                }
            }
            DeclKind::Type(t) => {
                if let Some(fields) = &t.fields {
                    for field in fields {
                        collect_type_expr_names(&field.type_ann, &mut refs);
                    }
                }
            }
            DeclKind::UnionType(u) => {
                for member in &u.members {
                    for arg in &member.type_args {
                        collect_type_expr_names(arg, &mut refs);
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
