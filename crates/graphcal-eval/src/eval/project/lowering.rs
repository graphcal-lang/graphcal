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
    file_path: &Path,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::syntax::ast::File,
    ctx: ImportContext<'_>,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    override_targets: &HashMap<DeclName, (PathBuf, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    let saved_imported_values = ctx.imported_values.clone();

    let (mut builder, mut unfrozen) = crate::ir::lower_to_builder_with_imported_values(
        file_ast,
        file_src,
        &ctx.imported_names,
        ctx.imported_values,
    )?;

    // Register type-system declarations from selectively imported files.
    for (dep_path, names) in &ctx.imported_type_system_names {
        let dep_loaded = &project.files[dep_path];
        crate::ir::register_selected_declarations(
            &dep_loaded.ast,
            &mut builder,
            &dep_loaded.named_source,
            names,
        )?;
    }

    // Merge type-system declarations from module-imported registries (pub items only).
    for (dep_registry, pub_names) in &ctx.extra_registry_builders {
        merge_registry_into_builder_filtered(
            &mut builder,
            dep_registry,
            &HashMap::new(),
            Some(pub_names),
        );
    }

    // Process deferred instantiated imports: compile dep to IR and merge.
    process_deferred_instantiated_imports(
        project,
        &ctx.deferred_instantiated,
        evaluated_files,
        &mut builder,
        &mut unfrozen,
    )?;

    // Process deferred inline DAG includes: compile DAG body to IR and merge.
    process_deferred_inline_dag_includes(
        &ctx.deferred_inline_dags,
        file_src,
        &mut builder,
        &mut unfrozen,
    )?;

    let ir = unfrozen.freeze(builder.build());

    // Apply overrides routed to this file (using original param names).
    let mut ir = ir;
    let file_overrides: HashMap<DeclName, graphcal_compiler::syntax::ast::Expr> = override_targets
        .iter()
        .filter(|(_, (target_path, _))| target_path == file_path)
        .map(|(name, (_, orig_name))| (orig_name.clone(), overrides[name].clone()))
        .collect();
    if !file_overrides.is_empty() {
        apply_overrides(&mut ir, &file_overrides)?;
    }

    // Type-resolve, check dimensions.
    let tir = crate::tir::type_resolve(ir, file_src)?;
    crate::dim_check::check_dimensions_tir(&tir, file_src)?;

    let declared_types = tir.build_declared_types(file_src)?;

    for (override_name, override_expr) in &file_overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
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

/// Process deferred instantiated imports by compiling each dependency to IR
/// and merging it into the importer's IR.
pub(super) fn process_deferred_instantiated_imports(
    project: &crate::loader::LoadedProject,
    deferred_imports: &[DeferredInstantiatedImport],
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    for deferred in deferred_imports {
        let dep_loaded = &project.files[&deferred.dep_path];
        let dep_src = &dep_loaded.named_source;

        // Build imported values for the dependency from its own transitive imports.
        let dep_imported = build_dep_imported_values(project, &deferred.dep_path, evaluated_files)?;

        // Compile the dependency to IR.
        let (dep_builder, dep_unfrozen) = crate::ir::lower_to_builder_with_imported_values(
            &dep_loaded.ast,
            dep_src,
            &dep_imported.names,
            dep_imported.values,
        )?;

        // Merge the dependency's type-system declarations into the importer's registry.
        let dep_registry = dep_builder.build();
        merge_registry_into_builder(builder, &dep_registry, &deferred.index_bindings);

        // Validate range index dimension matching (Phase B — requires compiled registries).
        for (dep_idx_name, importer_idx_name) in &deferred.index_bindings {
            if let Some(dep_idx_def) = dep_registry.indexes.get_index(dep_idx_name)
                && let crate::registry::IndexKind::RequiredRange { dimension: dep_dim } =
                    &dep_idx_def.kind
                && let Some(imp_idx_def) = builder.get_index(importer_idx_name)
                && let crate::registry::IndexKind::Range(
                    crate::registry::RangeIndexData { dimension: imp_dim, .. },
                )
                | crate::registry::IndexKind::RequiredRange { dimension: imp_dim } =
                    &imp_idx_def.kind
                && dep_dim != imp_dim
            {
                return Err(CompileError::Eval(
                    gcl_err!(IndexBindingDimensionMismatch {
                        dep_index: dep_idx_name.clone(),
                        expected_dim: dep_registry.dimensions.format_dimension(dep_dim),
                        bound_index: importer_idx_name.clone(),
                        found_dim: builder.format_dimension(imp_dim),
                    } @ dep_src, deferred.import_span),
                ));
            }
        }

        // Collect all declaration names in the dependency (for prefix_expr_refs).
        // These are un-prefixed member names used for containment checks.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }

        // Merge the dependency's IR into the importer's IR.
        unfrozen.merge_dependency(
            dep_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.import_item_attributes,
        );

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
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    for deferred in deferred_dags {
        // Compile the DAG body to IR.
        // The DAG body is lowered as if it were a standalone file, with only
        // prelude + explicitly imported items in scope.
        let (dag_builder, dag_unfrozen) = crate::ir::lower_to_builder_with_imported_values(
            &deferred.dag_body,
            file_src,
            &deferred.dag_imported_names,
            HashMap::new(), // No pre-evaluated values for inline DAGs
        )?;

        // Register parent scope type-system declarations in the DAG's registry.
        // These come from `import .. { DimName, UnitName }` in the DAG body.
        let mut dag_builder = dag_builder;
        for parent_decl in &deferred.dag_parent_type_decls {
            let parent_ast = graphcal_compiler::syntax::ast::File {
                declarations: vec![parent_decl.clone()],
            };
            let all_names: HashSet<String> = match &parent_decl.kind {
                DeclKind::BaseDimension(d) => std::iter::once(d.name.value.to_string()).collect(),
                DeclKind::Dimension(d) => std::iter::once(d.name.value.to_string()).collect(),
                DeclKind::Unit(u) => std::iter::once(u.name.value.to_string()).collect(),
                DeclKind::Type(t) => std::iter::once(t.name.value.to_string()).collect(),
                DeclKind::UnionType(t) => std::iter::once(t.name.value.to_string()).collect(),
                DeclKind::Index(idx) => std::iter::once(idx.name.value.to_string()).collect(),
                _ => HashSet::new(),
            };
            if !all_names.is_empty() {
                crate::ir::register_selected_declarations(
                    &parent_ast,
                    &mut dag_builder,
                    file_src,
                    &all_names,
                )?;
            }
        }

        // Merge the DAG's type-system declarations into the importer's registry.
        let dag_registry = dag_builder.build();
        merge_registry_into_builder(builder, &dag_registry, &deferred.index_bindings);

        // Collect all declaration names in the DAG body.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dag_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }

        // Merge the DAG's IR into the importer's IR.
        unfrozen.merge_dependency(
            dag_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.import_item_attributes,
        );

        // For selective imports, add alias nodes.
        if let Some(selective) = &deferred.selective_names {
            add_inline_dag_selective_aliases(&deferred.dag_body, selective, deferred, unfrozen);
        }
    }
    Ok(())
}

/// Add alias declarations for selective inline DAG includes.
pub(super) fn add_inline_dag_selective_aliases(
    dag_body: &graphcal_compiler::syntax::ast::File,
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

        // Substitute index names in the type annotation.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );

        let is_const = dag_body.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );
        let alias_expr = if is_const {
            Expr {
                kind: ExprKind::ConstRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                span: deferred.import_span,
            }
        } else {
            Expr {
                kind: ExprKind::GraphRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                span: deferred.import_span,
            }
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

        // Substitute index names in the type annotation.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );

        // Determine if this is a const or runtime declaration.
        let is_const = dep_loaded.ast.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );

        // Create an alias expression: `@prefix::orig_name` (or `PREFIX::CONST`)
        let alias_expr = if is_const {
            Expr {
                kind: ExprKind::ConstRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                span: deferred.import_span,
            }
        } else {
            Expr {
                kind: ExprKind::GraphRef(Spanned::new(
                    DeclName::new(&prefixed_name),
                    deferred.import_span,
                )),
                span: deferred.import_span,
            }
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
    index_bindings: &HashMap<String, String>,
) {
    merge_registry_into_builder_filtered(builder, dep_registry, index_bindings, None);
}

pub(super) fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<String, String>,
    pub_names: Option<&HashSet<String>>,
) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
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

    // Import struct types.
    for type_def in dep_registry.types.all_types() {
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
        _ => Err(CompileError::Eval(gcl_err!(BindingTargetsIndex {
            name: dep_index_name.to_string(),
        } @ file_src, expr.span))),
    }
}

/// Build imported value names and values for a dependency file from its own transitive imports.
///
/// This mirrors the import-processing logic in `compile_single_file_in_project` but
/// only for non-instantiated imports (the dependency's own transitive deps are already
/// evaluated and stored in `evaluated_files`).
pub(super) fn build_dep_imported_values(
    project: &crate::loader::LoadedProject,
    dep_path: &Path,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
) -> Result<DepImportedValues, CompileError> {
    let dep_loaded = &project.files[dep_path];
    let dep_src = &dep_loaded.named_source;

    let mut imported_names = ImportedValueNames::default();
    let mut imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)> = HashMap::new();

    // Process import declarations (non-instantiated).
    for (_decl, import_decl, trans_canonical) in dep_loaded.imports_with_paths() {
        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
            CompileError::Eval(gcl_err!(EvalError {
                message: format!(
                    "internal: transitive dependency {} not yet evaluated",
                    trans_canonical.display()
                ),
            } @ dep_src, import_decl.path.span()))
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
    for (_decl, include_decl, trans_canonical) in dep_loaded.includes_with_paths() {
        if !include_decl.param_bindings.is_empty() {
            // Nested instantiated includes are not supported in this initial implementation.
            return Err(CompileError::Eval(gcl_err!(EvalError {
                message: "nested instantiated includes are not yet supported".to_string(),
            } @ dep_src, include_decl.path.span())));
        }

        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
            CompileError::Eval(gcl_err!(EvalError {
                message: format!(
                    "internal: transitive dependency {} not yet evaluated",
                    trans_canonical.display()
                ),
            } @ dep_src, include_decl.path.span()))
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
    import_path: &ImportPath,
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    trans_dep: &EvaluatedFile,
    dep_src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    is_import: bool,
) {
    match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
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
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || {
                    derive_module_name_from_import_path(import_path, dep_src)
                        .unwrap_or_else(|_| "dep".to_string())
                },
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
