//! Import processing functions for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// Process an instantiated include (one with param bindings), deferring it for
/// post-lowering IR merging.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and allow_defaults check form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to project, importer, dep, and context"
)]
pub(super) fn process_instantiated_include<'a>(
    project: &'a crate::loader::LoadedProject,
    importer_path: &Path,
    import_canonical: &PathBuf,
    include_decl: &graphcal_compiler::syntax::ast::IncludeDecl,
    decl: &graphcal_compiler::syntax::ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files[import_canonical];
    let importer_loaded = &project.files[importer_path];

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            if let Some(alias_ident) = alias {
                alias_ident.name.clone()
            } else {
                derive_module_name_from_import_path(&include_decl.path, file_src)?
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Selective(_) => {
            // For selective instantiated includes, we still need a prefix
            // for the merged declarations. Derive from filename.
            derive_module_name_from_import_path(&include_decl.path, file_src)?
        }
    };

    // Check for duplicate module names (instantiated includes occupy the same namespace).
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(gcl_err!(DuplicateModuleName {
            name: prefix,
            first: (*first_span).into(),
        } @ file_src, include_decl.path.span())));
    }
    ctx.module_map.insert(
        prefix.clone(),
        (import_canonical.clone(), include_decl.path.span()),
    );

    // Classify and validate bindings against the dependency's AST.
    // Each binding is either a param binding (name targets a `param`) or an
    // index binding (name targets a `cat`/`range` index).
    let mut bindings = HashMap::new();
    let mut index_bindings = HashMap::new();
    for binding in &include_decl.param_bindings {
        let binding_name = &binding.name.name;

        // Check if the binding name is a param in the dependency.
        let is_param = dep_loaded.ast.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == binding_name),
        );
        if is_param {
            bindings.insert(binding_name.clone(), binding.value.clone());
            continue;
        }

        // Check if it's an index in the dependency.
        let dep_index = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Index(idx) if idx.name.value.as_str() == binding_name => Some(idx),
                _ => None,
            });
        if let Some(dep_idx) = dep_index {
            // Index binding: extract the RHS index name from the expression.
            let rhs_name =
                lowering::extract_index_name_from_binding_expr(&binding.value, binding_name, file_src)?;

            // Validate the RHS resolves to an index in the importer's scope.
            // Check 1: importer's own AST.
            let importer_idx_ast =
                importer_loaded
                    .ast
                    .declarations
                    .iter()
                    .find_map(|d| match &d.kind {
                        DeclKind::Index(idx) if idx.name.value.as_str() == rhs_name => Some(idx),
                        _ => None,
                    });
            // Check 2: already-evaluated dependency registries.
            let importer_idx_from_registry = if importer_idx_ast.is_none() {
                ctx.extra_registry_builders
                    .iter()
                    .find_map(|(reg, _)| reg.indexes.get_index(&rhs_name))
                    .or_else(|| {
                        evaluated_files
                            .values()
                            .find_map(|ef| ef.registry.indexes.get_index(&rhs_name))
                    })
            } else {
                None
            };

            if importer_idx_ast.is_none() && importer_idx_from_registry.is_none() {
                return Err(CompileError::Eval(gcl_err!(IndexBindingNotAnIndex {
                    dep_index: binding_name.clone(),
                    value: rhs_name,
                } @ file_src, binding.value.span)));
            }

            // Validate kind matching (named-to-named, range-to-range).
            let dep_is_named = matches!(
                dep_idx.kind,
                graphcal_compiler::syntax::ast::IndexDeclKind::Named { .. }
                    | graphcal_compiler::syntax::ast::IndexDeclKind::RequiredNamed
            );
            let imp_is_named = importer_idx_ast.map_or_else(
                || {
                    importer_idx_from_registry
                        .map(graphcal_compiler::registry::types::IndexDef::is_named)
                },
                |imp_idx| {
                    Some(matches!(
                        imp_idx.kind,
                        graphcal_compiler::syntax::ast::IndexDeclKind::Named { .. }
                            | graphcal_compiler::syntax::ast::IndexDeclKind::RequiredNamed
                    ))
                },
            );
            if let Some(imp_named) = imp_is_named
                && dep_is_named != imp_named
            {
                return Err(CompileError::Eval(gcl_err!(IndexKindMismatch {
                    dep_index: binding_name.clone(),
                    dep_kind: if dep_is_named { "named" } else { "range" }.to_string(),
                    bound_index: rhs_name,
                    bound_kind: if imp_named { "named" } else { "range" }.to_string(),
                } @ file_src, binding.name.span)));
            }
            // Dimension matching for range indexes is deferred to
            // process_deferred_instantiated_imports() where registries are available.

            index_bindings.insert(binding_name.clone(), rhs_name);
            continue;
        }

        // Check if it's some other kind of declaration.
        let actual_kind = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::ConstNode(c) if c.name.value.as_str() == binding_name => {
                    Some("const node")
                }
                DeclKind::Node(n) if n.name.value.as_str() == binding_name => Some("node"),
                DeclKind::Assert(a) if a.name.value.as_str() == binding_name => Some("assert"),
                _ => None,
            });
        if let Some(kind) = actual_kind {
            return Err(CompileError::Eval(gcl_err!(BindingNotAParam {
                name: binding_name.clone(),
                actual_kind: kind.to_string(),
            } @ file_src, binding.name.span)));
        }
        return Err(CompileError::Eval(gcl_err!(UnknownParamBinding {
            name: binding_name.clone(),
            file_path: include_decl.path.display_path(),
        } @ file_src, binding.name.span)));
    }

    // Register the dependency's declaration names in the importer's scope
    // so that the resolver recognizes references to them.
    let mut import_item_attributes: HashMap<
        String,
        Vec<graphcal_compiler::syntax::ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &include_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the dependency.
                if !file_has_declaration(&dep_loaded.ast, orig_name) {
                    return Err(CompileError::Eval(gcl_err!(ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: include_decl.path.display_path(),
                    } @ file_src, import_item.name.span)));
                }

                // Collect import-item attributes for deferred processing.
                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(orig_name.clone(), import_item.attributes.clone());
                }

                // Register the local name in scope for the resolver.
                // Determine the category from the dep's AST.
                let is_const = dep_loaded.ast.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name)
                });
                let is_runtime = dep_loaded.ast.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name)
                });
                let scoped = ScopedName::Local(local_name.clone());
                let span = import_item.name.span;
                if is_const {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if is_runtime {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations (dim/unit/index/type) are not
                    // registered in imported_names; handled via registry merge.
                }
                // Type-system declarations from instantiated imports also need registration.
                let is_type_system = dep_loaded.ast.declarations.iter().any(|d| match &d.kind {
                    DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
                    DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
                    DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
                    DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
                    DeclKind::Type(t) => t.name.value.as_str() == orig_name,
                    _ => false,
                });
                if is_type_system {
                    ctx.imported_type_system_names
                        .entry(import_canonical.clone())
                        .or_default()
                        .insert(orig_name.clone());
                }

                selective.push((orig_name.clone(), local_name));
            }
            Some(selective)
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
            // Register all dep names under the prefix for scope checking.
            let import_span = include_decl.path.span();
            for dep_decl in &dep_loaded.ast.declarations {
                let (dep_name, is_const) = match &dep_decl.kind {
                    DeclKind::Param(p) => (Some(p.name.value.to_string()), false),
                    DeclKind::ConstNode(c) => (Some(c.name.value.to_string()), true),
                    DeclKind::Node(n) => (Some(n.name.value.to_string()), false),
                    _ => (None, false),
                };
                if let Some(name) = dep_name {
                    let scoped = ScopedName::Qualified {
                        module: prefix.clone(),
                        member: name,
                    };
                    if is_const {
                        ctx.imported_names.const_names.push((scoped, import_span));
                    } else {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            // Import type-system declarations (pub items only).
            if let Some(dep_eval) = evaluated_files.get(import_canonical) {
                ctx.extra_registry_builders
                    .push((&dep_eval.registry, &dep_eval.pub_names));
            }
            None
        }
    };

    // Required indexes must always be bound (regardless of allow_defaults).
    for dep_decl in &dep_loaded.ast.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(
                gcl_err!(RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                } @ file_src, include_decl.path.span()),
            ));
        }
    }

    // Strict check: when any binding is provided, ALL params and indexes
    // with defaults must be explicitly bound unless #[allow_defaults].
    let allow_defaults = decl
        .attributes
        .iter()
        .any(|a| a.name.name == "allow_defaults");
    if !allow_defaults {
        for dep_decl in &dep_loaded.ast.declarations {
            if let DeclKind::Param(p) = &dep_decl.kind
                && p.value.is_some()
                && !bindings.contains_key(p.name.value.as_str())
            {
                return Err(CompileError::Eval(gcl_err!(DefaultParamNotProvided {
                    name: p.name.value.to_string(),
                    help: format!(
                        "provide `{name} = <value>` in the include binding or add `#[allow_defaults]` to the include",
                        name = p.name.value,
                    ),
                } @ file_src, include_decl.path.span())));
            }
            // Indexes with defaults (Named/Range, not Required*) must also be bound.
            if let DeclKind::Index(idx) = &dep_decl.kind
                && !idx.kind.is_required()
                && !index_bindings.contains_key(idx.name.value.as_str())
            {
                return Err(CompileError::Eval(gcl_err!(DefaultIndexNotProvided {
                    name: idx.name.value.to_string(),
                    help: format!(
                        "provide `{name} = <IndexName>` in the include binding or add `#[allow_defaults]` to the include",
                        name = idx.name.value,
                    ),
                } @ file_src, include_decl.path.span())));
            }
        }
    }

    ctx.deferred_instantiated.push(DeferredInstantiatedImport {
        dep_path: import_canonical.clone(),
        prefix,
        bindings,
        index_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
    });
    Ok(())
}

/// Process an inline DAG include (`include dag_name(...) { ... }`).
///
/// Creates a virtual File from the DAG body, validates bindings against it,
/// and defers for IR merging.
///
/// When `is_cross_file` is `true`, const node declarations from the parent scope
/// (`import ..`) are included directly in the DAG body rather than being registered
/// as external imported names. This is necessary because the parent file's const
/// values are not available in the importing file's IR.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and deferred include setup form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to DAG def, include decl, parent AST, and cross-file flag"
)]
pub(super) fn process_inline_dag_include(
    dag_def: &graphcal_compiler::syntax::ast::DagDecl,
    dag_name: &str,
    include_decl: &graphcal_compiler::syntax::ast::IncludeDecl,
    decl: &graphcal_compiler::syntax::ast::Declaration,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    ctx: &mut ImportContext<'_>,
    is_cross_file: bool,
) -> Result<(), CompileError> {
    use graphcal_compiler::syntax::ast::ImportKind;

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        ImportKind::Module { alias } => alias.as_ref().map_or_else(
            || dag_name.to_string(),
            |alias_ident| alias_ident.name.clone(),
        ),
        ImportKind::Selective(_) => dag_name.to_string(),
    };

    // Check for duplicate module names.
    // We use a sentinel path for inline DAGs in the module_map.
    let sentinel_path = PathBuf::from(format!("<dag:{dag_name}>"));
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(gcl_err!(DuplicateModuleName {
            name: prefix,
            first: (*first_span).into(),
        } @ file_src, include_decl.path.span())));
    }
    ctx.module_map
        .insert(prefix.clone(), (sentinel_path, include_decl.path.span()));

    // Create a virtual File from the DAG body, filtering out `import ..` declarations.
    // Import .. declarations are processed separately to populate the DAG's imported names.
    let mut dag_body_decls = Vec::new();
    let mut dag_imported_names = ImportedValueNames::default();
    let mut dag_parent_type_decls = Vec::new();

    for body_decl in &dag_def.body {
        match &body_decl.kind {
            DeclKind::Import(import_decl) if import_decl.path.is_parent_scope() => {
                if is_cross_file {
                    // For cross-file DAGs, resolve parent scope items and include
                    // const declarations directly in the DAG body (since the parent
                    // file's values are not in the importing file's IR).
                    process_cross_file_parent_scope_import(
                        &import_decl.kind,
                        parent_ast,
                        file_src,
                        &mut dag_body_decls,
                        &mut dag_parent_type_decls,
                    )?;
                } else {
                    // For same-file DAGs, register names for resolution; the actual
                    // values are available in the parent file's IR.
                    process_parent_scope_import(
                        &import_decl.kind,
                        parent_ast,
                        file_src,
                        &mut dag_imported_names,
                        &mut dag_parent_type_decls,
                    )?;
                }
            }
            _ => {
                dag_body_decls.push(body_decl.clone());
            }
        }
    }

    let dag_body = graphcal_compiler::syntax::ast::File {
        declarations: dag_body_decls,
    };

    // Classify and validate bindings against the DAG body's declarations.
    let mut bindings = HashMap::new();
    let mut index_bindings = HashMap::new();
    for binding in &include_decl.param_bindings {
        let binding_name = &binding.name.name;

        // Check if the binding name is a param in the DAG body.
        let is_param = dag_body.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == binding_name),
        );
        if is_param {
            bindings.insert(binding_name.clone(), binding.value.clone());
            continue;
        }

        // Check if it's an index in the DAG body.
        let dep_index = dag_body.declarations.iter().find_map(|d| match &d.kind {
            DeclKind::Index(idx) if idx.name.value.as_str() == binding_name => Some(idx),
            _ => None,
        });
        if let Some(_dep_idx) = dep_index {
            let rhs_name =
                lowering::extract_index_name_from_binding_expr(&binding.value, binding_name, file_src)?;
            index_bindings.insert(binding_name.clone(), rhs_name);
            continue;
        }

        // Unknown binding target.
        let actual_kind = dag_body.declarations.iter().find_map(|d| match &d.kind {
            DeclKind::ConstNode(c) if c.name.value.as_str() == binding_name => Some("const node"),
            DeclKind::Node(n) if n.name.value.as_str() == binding_name => Some("node"),
            _ => None,
        });
        if let Some(kind) = actual_kind {
            return Err(CompileError::Eval(gcl_err!(BindingNotAParam {
                name: binding_name.clone(),
                actual_kind: kind.to_string(),
            } @ file_src, binding.name.span)));
        }
        return Err(CompileError::Eval(gcl_err!(UnknownParamBinding {
            name: binding_name.clone(),
            file_path: dag_name.to_string(),
        } @ file_src, binding.name.span)));
    }

    // Register imported names in the importer's scope.
    let mut import_item_attributes: HashMap<
        String,
        Vec<graphcal_compiler::syntax::ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &include_decl.kind {
        ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the DAG body.
                if !file_has_declaration(&dag_body, orig_name) {
                    return Err(CompileError::Eval(gcl_err!(ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: dag_name.to_string(),
                    } @ file_src, import_item.name.span)));
                }

                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(orig_name.clone(), import_item.attributes.clone());
                }

                // Register the local name in scope.
                let is_const = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name)
                });
                let is_runtime = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name)
                });
                let scoped = ScopedName::Local(local_name.clone());
                let span = import_item.name.span;
                if is_const {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if is_runtime {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations — handled via registry merge.
                }

                selective.push((orig_name.clone(), local_name));
            }
            Some(selective)
        }
        ImportKind::Module { .. } => {
            // Register all DAG body names under the prefix.
            let import_span = include_decl.path.span();
            for dep_decl in &dag_body.declarations {
                let (dep_name, is_const) = match &dep_decl.kind {
                    DeclKind::Param(p) => (Some(p.name.value.to_string()), false),
                    DeclKind::ConstNode(c) => (Some(c.name.value.to_string()), true),
                    DeclKind::Node(n) => (Some(n.name.value.to_string()), false),
                    _ => (None, false),
                };
                if let Some(name) = dep_name {
                    let scoped = ScopedName::Qualified {
                        module: prefix.clone(),
                        member: name,
                    };
                    if is_const {
                        ctx.imported_names.const_names.push((scoped, import_span));
                    } else {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            None
        }
    };

    // Strict binding check: all required params/indexes must be bound.
    for dep_decl in &dag_body.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(
                gcl_err!(RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                } @ file_src, include_decl.path.span()),
            ));
        }
    }

    let allow_defaults = decl
        .attributes
        .iter()
        .any(|a| a.name.name == "allow_defaults");
    if !allow_defaults {
        for dep_decl in &dag_body.declarations {
            if let DeclKind::Param(p) = &dep_decl.kind
                && p.value.is_some()
                && !bindings.contains_key(p.name.value.as_str())
            {
                return Err(CompileError::Eval(gcl_err!(DefaultParamNotProvided {
                    name: p.name.value.to_string(),
                    help: format!(
                        "provide `{name} = <value>` in the include binding or add `#[allow_defaults]` to the include",
                        name = p.name.value,
                    ),
                } @ file_src, include_decl.path.span())));
            }
            if let DeclKind::Index(idx) = &dep_decl.kind
                && !idx.kind.is_required()
                && !index_bindings.contains_key(idx.name.value.as_str())
            {
                return Err(CompileError::Eval(gcl_err!(DefaultIndexNotProvided {
                    name: idx.name.value.to_string(),
                    help: format!(
                        "provide `{name} = <IndexName>` in the include binding or add `#[allow_defaults]` to the include",
                        name = idx.name.value,
                    ),
                } @ file_src, include_decl.path.span())));
            }
        }
    }

    ctx.deferred_inline_dags.push(DeferredInlineDagInclude {
        dag_body,
        dag_imported_names,
        dag_parent_type_decls,
        prefix,
        bindings,
        index_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
    });
    Ok(())
}

/// Process `import .. { ... }` declarations inside a DAG body.
///
/// Resolves the imported items to compile-time declarations in the parent scope
/// and populates the DAG's imported names.
pub(super) fn process_parent_scope_import(
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    dag_imported_names: &mut ImportedValueNames,
    dag_parent_type_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
) -> Result<(), CompileError> {
    let names = match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => names,
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
            // `import .. as alias;` or `import ..;` — not supported (semantics unclear).
            // Only selective parent scope imports are supported.
            return Err(CompileError::Eval(gcl_err!(EvalError {
                message: "module-style `import ..` is not supported; use `import .. { name1, name2 }` to import specific items from the parent scope".to_string(),
            } @ file_src, 0..0)));
        }
    };

    for import_item in names {
        let orig_name = &import_item.name.name;
        let local_name = import_item.local_name().to_string();

        // Find the declaration in the parent scope.
        let parent_decl = parent_ast.declarations.iter().find(|d| match &d.kind {
            DeclKind::ConstNode(c) => c.name.value.as_str() == orig_name,
            DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
            DeclKind::Type(t) => t.name.value.as_str() == orig_name,
            DeclKind::UnionType(t) => t.name.value.as_str() == orig_name,
            DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
            DeclKind::Dag(dag) => dag.name.value.as_str() == orig_name,
            // Runtime items and other declarations are NOT importable via `import ..`.
            _ => false,
        });

        let parent_decl = parent_decl.ok_or_else(|| {
            CompileError::Eval(gcl_err!(ImportNameNotFound {
                name: orig_name.clone(),
                file_path: "..".to_string(),
            } @ file_src, import_item.name.span))
        })?;

        // Classify the imported item.
        match &parent_decl.kind {
            DeclKind::ConstNode(_) => {
                let scoped = ScopedName::Local(local_name);
                dag_imported_names
                    .const_names
                    .push((scoped, import_item.name.span));
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_) => {
                // Type-system declarations — need to be registered in the DAG's registry.
                dag_parent_type_decls.push(parent_decl.clone());
            }
            // DAG definitions and other items don't need registration in imported_names.
            _ => {}
        }
    }

    Ok(())
}

/// Process `import .. { ... }` declarations inside a cross-file DAG body.
///
/// Unlike same-file DAGs where parent scope const values are available in the
/// importing file's IR, cross-file DAGs must include the parent const declarations
/// directly in the DAG body. Type-system declarations are handled via the
/// `dag_parent_type_decls` mechanism as usual.
pub(super) fn process_cross_file_parent_scope_import(
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    dag_body_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
    dag_parent_type_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
) -> Result<(), CompileError> {
    let names = match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => names,
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
            return Err(CompileError::Eval(gcl_err!(EvalError {
                message: "module-style `import ..` is not supported; use `import .. { name1, name2 }` to import specific items from the parent scope".to_string(),
            } @ file_src, 0..0)));
        }
    };

    for import_item in names {
        let orig_name = &import_item.name.name;

        // Find the declaration in the parent scope.
        let parent_decl = parent_ast.declarations.iter().find(|d| match &d.kind {
            DeclKind::ConstNode(c) => c.name.value.as_str() == orig_name,
            DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
            DeclKind::Type(t) => t.name.value.as_str() == orig_name,
            DeclKind::UnionType(t) => t.name.value.as_str() == orig_name,
            DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
            DeclKind::Dag(dag) => dag.name.value.as_str() == orig_name,
            _ => false,
        });

        let parent_decl = parent_decl.ok_or_else(|| {
            CompileError::Eval(gcl_err!(ImportNameNotFound {
                name: orig_name.clone(),
                file_path: "..".to_string(),
            } @ file_src, import_item.name.span))
        })?;

        match &parent_decl.kind {
            DeclKind::ConstNode(_) => {
                // Include const declarations directly in the DAG body so they
                // become part of the DAG's IR and get merged with prefixing.
                dag_body_decls.push(parent_decl.clone());
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_) => {
                dag_parent_type_decls.push(parent_decl.clone());
            }
            _ => {}
        }
    }

    Ok(())
}

/// Check whether a multi-segment `ModulePath` include is a bare module path
/// DAG reference.  This is the case when:
/// 1. The path is a `ModulePath` with 2+ segments, AND
/// 2. The resolved target file's AST contains a `dag` definition whose name
///    matches the last segment of the module path.
///
/// For example, `include pkg/lib/double(...)` where `pkg/lib.gcl` defines
/// `dag double { ... }`.
pub(super) fn is_bare_module_dag_ref(
    import_path: &ImportPath,
    resolved_canonical: &PathBuf,
    project: &crate::loader::LoadedProject,
) -> bool {
    let segments = match import_path {
        ImportPath::ModulePath { segments, .. } if segments.len() >= 2 => segments,
        _ => return false,
    };

    // Safety: the match guard above ensures segments.len() >= 2
    let Some(last_seg) = segments.last() else {
        return false;
    };
    let last_segment = &last_seg.name;

    // Check if the resolved file contains a DAG with the matching name.
    let Some(target_loaded) = project.files.get(resolved_canonical) else {
        return false;
    };

    target_loaded
        .ast
        .declarations
        .iter()
        .any(|d| matches!(&d.kind, DeclKind::Dag(dag) if dag.name.value.as_str() == last_segment))
}

/// Process a non-instantiated import or include (no param bindings), importing values and
/// type-system declarations from the already-evaluated dependency.
///
/// When `is_import` is `true`, only compile-time items (consts, dims, units, types, indexes,
/// dags, assertions) are allowed. Runtime items (params, non-const nodes) trigger an error
/// advising the user to use `include` instead.
#[expect(
    clippy::too_many_arguments,
    reason = "import processing needs all these context parameters"
)]
pub(super) fn process_non_instantiated_import<'a>(
    project: &crate::loader::LoadedProject,
    import_canonical: &PathBuf,
    import_path: &graphcal_compiler::syntax::ast::ImportPath,
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
    is_import: bool,
) -> Result<(), CompileError> {
    let dep = evaluated_files.get(import_canonical).ok_or_else(|| {
        CompileError::Eval(gcl_err!(EvalError {
            message: format!(
                "internal: dependency {} not yet evaluated",
                import_canonical.display()
            ),
        } @ file_src, import_path.span()))
    })?;

    match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Visibility check: the item must be declared `pub` in the source file.
                if !dep.pub_names.contains(orig_name.as_str()) {
                    // Check if the name exists at all (value or type-system) before
                    // reporting "private" vs "not found".
                    let dep_loaded = &project.files[import_canonical];
                    let exists = dep.const_values.contains_key(orig_name)
                        || dep.values.contains_key(orig_name)
                        || dep.has_assert(orig_name)
                        || file_has_declaration(&dep_loaded.ast, orig_name);
                    if exists {
                        return Err(CompileError::Eval(gcl_err!(ImportPrivateItem {
                            name: orig_name.clone(),
                            file_path: import_path.display_path(),
                        } @ file_src, import_item.name.span)));
                    }
                    return Err(CompileError::Eval(gcl_err!(ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: import_path.display_path(),
                    } @ file_src, import_item.name.span)));
                }

                match import_selective_item(
                    dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    &mut ctx.imported_names,
                    &mut ctx.imported_values,
                    Some(&mut ctx.imported_source_order),
                ) {
                    SelectiveImportResult::Const => {}
                    SelectiveImportResult::Runtime => {
                        if is_import {
                            return Err(CompileError::Eval(gcl_err!(ImportRuntimeItem {
                                name: orig_name.clone(),
                            } @ file_src, import_item.name.span)));
                        }
                    }
                    SelectiveImportResult::Assert => {
                        // Assert is already evaluated in the dep file.
                        // We just need to make the name visible for #[assumes].
                        ctx.imported_names
                            .assert_names
                            .push((local_name, import_item.name.span));
                    }
                    SelectiveImportResult::NotFound => {
                        // Check if it's a type-system declaration in the dep's file.
                        let dep_loaded = &project.files[import_canonical];
                        if file_has_declaration(&dep_loaded.ast, orig_name) {
                            // Type-system declaration (dim/unit/index/type).
                            ctx.imported_type_system_names
                                .entry(import_canonical.clone())
                                .or_default()
                                .insert(orig_name.clone());
                        } else {
                            return Err(CompileError::Eval(gcl_err!(ImportNameNotFound {
                                name: orig_name.clone(),
                                file_path: import_path.display_path(),
                            } @ file_src, import_item.name.span)));
                        }
                    }
                }
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            let module_name = if let Some(alias_ident) = alias {
                alias_ident.name.clone()
            } else {
                derive_module_name_from_import_path(import_path, file_src)?
            };
            if let Some((_, first_span)) = ctx.module_map.get(&module_name) {
                return Err(CompileError::Eval(gcl_err!(DuplicateModuleName {
                    name: module_name,
                    first: (*first_span).into(),
                } @ file_src, import_path.span())));
            }
            ctx.module_map.insert(
                module_name.clone(),
                (import_canonical.clone(), import_path.span()),
            );

            // Import all values under module::name prefix.
            let import_span = import_path.span();
            import_module_values(
                dep,
                &module_name,
                import_span,
                &mut ctx.imported_names,
                &mut ctx.imported_values,
                Some(&mut ctx.imported_source_order),
                is_import,
            );
            // Import all public type-system declarations from dep's registry.
            ctx.extra_registry_builders
                .push((&dep.registry, &dep.pub_names));
        }
    }
    Ok(())
}

/// Check for recursive DAG instantiation.
///
/// Builds a dependency graph of inline DAGs and detects cycles.
/// Returns an error if a DAG directly or indirectly includes itself.
pub(super) fn check_dag_recursion(
    dag_definitions: &HashMap<String, &graphcal_compiler::syntax::ast::DagDecl>,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    fn dfs<'a>(
        node: &'a str,
        deps: &HashMap<&str, Vec<&'a str>>,
        visited: &mut HashSet<&'a str>,
        in_stack: &mut HashSet<&'a str>,
        path: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        if in_stack.contains(node) {
            let cycle_start = path.iter().position(|n| *n == node).unwrap_or(0);
            let mut cycle: Vec<String> = path[cycle_start..]
                .iter()
                .map(ToString::to_string)
                .collect();
            cycle.push(node.to_string());
            return Some(cycle);
        }
        if visited.contains(node) {
            return None;
        }
        visited.insert(node);
        in_stack.insert(node);
        path.push(node);

        if let Some(neighbors) = deps.get(node) {
            for &neighbor in neighbors {
                if let Some(cycle) = dfs(neighbor, deps, visited, in_stack, path) {
                    return Some(cycle);
                }
            }
        }

        in_stack.remove(node);
        path.pop();
        None
    }

    // Build adjacency list: dag_name -> set of dag names it includes.
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, dag) in dag_definitions {
        let mut includes = Vec::new();
        for decl in &dag.body {
            if let DeclKind::Include(inc) = &decl.kind
                && let graphcal_compiler::syntax::ast::ImportPath::ModulePath { segments, .. } =
                    &inc.path
                && segments.len() == 1
            {
                let target = segments[0].name.as_str();
                if dag_definitions.contains_key(target) {
                    includes.push(target);
                }
            }
        }
        deps.insert(name.as_str(), includes);
    }

    let mut visited: HashSet<&str> = HashSet::new();
    let mut in_stack: HashSet<&str> = HashSet::new();
    for name in dag_definitions.keys() {
        if let Some(cycle) = dfs(name, &deps, &mut visited, &mut in_stack, &mut Vec::new()) {
            let cycle_str = cycle.join(" -> ");
            return Err(CompileError::Eval(gcl_err!(EvalError {
                message: format!("recursive DAG instantiation: {cycle_str}"),
            } @ file_src, dag_definitions[name.as_str()].span)));
        }
    }
    Ok(())
}

/// Look up a single selective import item in an `EvaluatedFile` and register it.
///
/// Handles `const_values` and values (params/nodes).
/// Returns what was found so the caller can handle assert and type-system fallbacks.
pub(super) fn import_selective_item(
    dep: &EvaluatedFile,
    orig_name: &str,
    local_name: &str,
    span: Span,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> SelectiveImportResult {
    if let Some(rv) = dep.const_values.get(orig_name) {
        let scoped = ScopedName::Local(local_name.to_string());
        imported_names.const_names.push((scoped.clone(), span));
        let dt = dep
            .declared_types
            .get(orig_name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        SelectiveImportResult::Const
    } else if let Some(rv) = dep.values.get(orig_name) {
        let scoped = ScopedName::Local(local_name.to_string());
        imported_names.param_names.push((scoped.clone(), span));
        let dt = dep
            .declared_types
            .get(orig_name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        SelectiveImportResult::Runtime
    } else if dep.has_assert(orig_name) {
        SelectiveImportResult::Assert
    } else {
        SelectiveImportResult::NotFound
    }
}

/// Import all values from an `EvaluatedFile` under a module prefix.
///
/// Registers `const_values` and values (params/nodes) with qualified
/// `ScopedName::Qualified` names.
///
/// When `const_only` is `true`, only `const_values` are imported; runtime values
/// (params/nodes) are silently skipped. This is used for `import` statements which
/// only allow compile-time items.
pub(super) fn import_module_values(
    dep: &EvaluatedFile,
    module_name: &str,
    import_span: Span,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
    const_only: bool,
) {
    // Sort keys for deterministic ordering — HashMap iteration is arbitrary.
    let mut const_keys: Vec<&String> = dep.const_values.keys().collect();
    const_keys.sort();
    for name in const_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
        let rv = &dep.const_values[name];
        let scoped = ScopedName::Qualified {
            module: module_name.to_string(),
            member: name.clone(),
        };
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        let dt = dep
            .declared_types
            .get(name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }

    // Skip runtime values when const_only is true (import semantics).
    if const_only {
        return;
    }

    let mut value_keys: Vec<&String> = dep.values.keys().collect();
    value_keys.sort();
    for name in value_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
        let rv = &dep.values[name];
        let scoped = ScopedName::Qualified {
            module: module_name.to_string(),
            member: name.clone(),
        };
        imported_names
            .param_names
            .push((scoped.clone(), import_span));
        let dt = dep
            .declared_types
            .get(name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }
}
