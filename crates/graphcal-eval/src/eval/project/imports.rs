//! Import processing functions for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// What kind of "other declaration" a binding name resolves to in the dep file
/// when it is not a param / type / dim / index.
#[derive(Clone, Copy)]
enum OtherDeclKind {
    ConstNode,
    Node,
    Assert,
}

impl OtherDeclKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ConstNode => "const node",
            Self::Node => "node",
            Self::Assert => "assert",
        }
    }
}

/// Classification of a name against a dependency's declarations.
///
/// Built once per dep file and reused for every binding rather than re-scanning
/// `declarations` four or five times per binding.
struct DepDeclIndex<'a> {
    params: HashSet<DeclName>,
    types: HashSet<StructTypeName>,
    dims: HashSet<DimName>,
    units: HashSet<graphcal_compiler::syntax::names::UnitName>,
    /// Maps index name to its declaration (needed for kind/required checks).
    indexes: HashMap<IndexName, &'a graphcal_compiler::desugar::resolved_ast::IndexDecl>,
    /// "Other" declarations (const node / node / assert) that are invalid as
    /// binding targets; used to produce precise "is actually a …" diagnostics.
    other: HashMap<DeclName, OtherDeclKind>,
}

impl DepDeclIndex<'_> {
    fn is_const(&self, name: &str) -> bool {
        matches!(self.other.get(name), Some(OtherDeclKind::ConstNode))
    }
    fn is_runtime(&self, name: &str) -> bool {
        self.params.contains(name) || matches!(self.other.get(name), Some(OtherDeclKind::Node))
    }
    fn is_type_system(&self, name: &str) -> bool {
        self.dims.contains(name)
            || self.units.contains(name)
            || self.indexes.contains_key(name)
            || self.types.contains(name)
    }
}

fn build_dep_decl_index(
    decls: &[graphcal_compiler::desugar::resolved_ast::Declaration],
) -> DepDeclIndex<'_> {
    let mut params = HashSet::new();
    let mut types = HashSet::new();
    let mut dims = HashSet::new();
    let mut units = HashSet::new();
    let mut indexes: HashMap<IndexName, &graphcal_compiler::desugar::resolved_ast::IndexDecl> =
        HashMap::new();
    let mut other: HashMap<DeclName, OtherDeclKind> = HashMap::new();
    for d in decls {
        match &d.kind {
            DeclKind::Param(p) => {
                params.insert(p.name.value.clone());
            }
            DeclKind::Type(t) => {
                types.insert(t.name.value.clone());
            }
            DeclKind::UnionType(t) => {
                types.insert(t.name.value.clone());
            }
            DeclKind::BaseDimension(dim) => {
                dims.insert(dim.name.value.clone());
            }
            DeclKind::Dimension(dim) => {
                dims.insert(dim.name.value.clone());
            }
            DeclKind::Unit(u) => {
                units.insert(u.name.value.clone());
            }
            DeclKind::Index(idx) => {
                indexes.insert(idx.name.value.clone(), idx);
            }
            DeclKind::ConstNode(c) => {
                other.insert(c.name.value.clone(), OtherDeclKind::ConstNode);
            }
            DeclKind::Node(n) => {
                other.insert(n.name.value.clone(), OtherDeclKind::Node);
            }
            DeclKind::Assert(a) => {
                other.insert(a.name.value.clone(), OtherDeclKind::Assert);
            }
            _ => {}
        }
    }
    DepDeclIndex {
        params,
        types,
        dims,
        units,
        indexes,
        other,
    }
}

/// Classified param bindings: each entry routes to one of the four binding
/// maps based on what the dependency declares the binding name as. The
/// `indexes` / `types` / `dims` maps use [`DepToImporter`] keying — the key
/// is the dep-side name and the value is the importer-side name it binds to.
pub(in crate::eval::project) struct ClassifiedBindings {
    pub(in crate::eval::project) params:
        HashMap<DeclName, graphcal_compiler::desugar::resolved_ast::Expr>,
    pub(in crate::eval::project) indexes: DepToImporter<IndexName>,
    pub(in crate::eval::project) types: DepToImporter<StructTypeName>,
    pub(in crate::eval::project) dims: DepToImporter<DimName>,
}

/// Classify each param binding against the dep's declaration index, returning
/// per-kind binding maps. Rejects bindings whose name targets a non-bindable
/// dep declaration (const/node/assert) or no dep declaration at all.
///
/// Caller-specific validation (e.g. index-kind matching, registry lookups for
/// already-evaluated dependencies) layers on top of this — this helper only
/// answers "is `binding_name` a param/type/dim/index of the dep, or invalid?".
fn classify_param_bindings(
    param_bindings: &[graphcal_compiler::desugar::resolved_ast::ParamBinding],
    dep_index: &DepDeclIndex<'_>,
    file_src: &NamedSource<Arc<String>>,
    dep_path_for_error: &str,
) -> Result<ClassifiedBindings, CompileError> {
    let mut out = ClassifiedBindings {
        params: HashMap::new(),
        indexes: HashMap::new(),
        types: HashMap::new(),
        dims: HashMap::new(),
    };
    for binding in param_bindings {
        let binding_name = &binding.name.name;
        if dep_index.params.contains(binding_name.as_str()) {
            out.params
                .insert(DeclName::new(binding_name), binding.value.clone());
            continue;
        }
        if dep_index.types.contains(binding_name.as_str()) {
            let rhs_name = lowering::extract_type_name_from_binding_expr(
                &binding.value,
                binding_name,
                file_src,
            )?;
            out.types.insert(
                StructTypeName::new(binding_name),
                StructTypeName::new(rhs_name),
            );
            continue;
        }
        if dep_index.dims.contains(binding_name.as_str()) {
            let rhs_name = lowering::extract_type_name_from_binding_expr(
                &binding.value,
                binding_name,
                file_src,
            )?;
            out.dims
                .insert(DimName::new(binding_name), DimName::new(rhs_name));
            continue;
        }
        if dep_index.indexes.contains_key(binding_name.as_str()) {
            let rhs_name = lowering::extract_index_name_from_binding_expr(
                &binding.value,
                binding_name,
                file_src,
            )?;
            out.indexes
                .insert(IndexName::new(binding_name), IndexName::new(rhs_name));
            continue;
        }
        if let Some(kind) = dep_index.other.get(binding_name.as_str()) {
            return Err(CompileError::Eval(GraphcalError::BindingNotAParam {
                name: binding_name.clone(),
                actual_kind: kind.as_str().to_string(),
                src: file_src.clone(),
                span: binding.name.span.into(),
            }));
        }
        return Err(CompileError::Eval(GraphcalError::UnknownParamBinding {
            name: binding_name.clone(),
            file_path: dep_path_for_error.to_string(),
            src: file_src.clone(),
            span: binding.name.span.into(),
        }));
    }
    Ok(out)
}

/// Process an instantiated include (one with param bindings), deferring it for
/// post-lowering IR merging.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation and scope registration form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to project, importer, dep, and context"
)]
pub(in crate::eval::project) fn process_instantiated_include<'a>(
    project: &'a crate::loader::LoadedProject,
    importer_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    import_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    include_decl: &graphcal_compiler::desugar::resolved_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::resolved_ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files[import_dag_id];
    let importer_loaded = &project.files[importer_dag_id];
    let dep_index = build_dep_decl_index(&dep_loaded.ast.declarations);

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        graphcal_compiler::desugar::resolved_ast::ImportKind::Module { alias } => {
            alias.as_ref().map_or_else(
                || derive_module_name_from_import_path(&include_decl.path),
                |alias_ident| alias_ident.value.to_string(),
            )
        }
        graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(_) => {
            // For selective instantiated includes, we still need a prefix for
            // the merged declarations. Derive from the path's leaf segment.
            derive_module_name_from_import_path(&include_decl.path)
        }
    };

    // Check for duplicate module names (instantiated includes occupy the same namespace).
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
            name: prefix,
            first: (*first_span).into(),
            src: file_src.clone(),
            span: include_decl.path.span().into(),
        }));
    }
    ctx.module_map.insert(
        prefix.clone(),
        (import_dag_id.clone(), include_decl.path.span()),
    );

    // Classify and validate bindings against the dependency's AST. Each
    // binding lands in one of params/types/dims/indexes, or is rejected as
    // an unknown / non-bindable name. Caller-specific cross-checks (importer
    // scope for index bindings) layer on top of the shared classification.
    let dep_path_display = include_decl.path.display_path();
    let ClassifiedBindings {
        params: bindings,
        indexes: index_bindings,
        types: type_bindings,
        dims: dim_bindings,
    } = classify_param_bindings(
        &include_decl.param_bindings,
        &dep_index,
        file_src,
        &dep_path_display,
    )?;

    // File includes additionally require each index binding's RHS to
    // resolve to an index already visible to the importer (in its own AST
    // or in a previously-evaluated dep's registry), and the dep/importer
    // kinds (named vs range) to agree. Inline-DAG includes share the
    // file's registry and skip this pass.
    for binding in &include_decl.param_bindings {
        let binding_name = &binding.name.name;
        let Some(dep_idx) = dep_index.indexes.get(binding_name.as_str()).copied() else {
            continue;
        };
        // classify_param_bindings inserts an entry for every accepted index
        // binding; the let-else is a no-panic safety net rather than a real
        // branch — a `continue` here is unreachable under that invariant.
        let Some(rhs_name) = index_bindings.get(binding_name.as_str()) else {
            continue;
        };
        let rhs_name = rhs_name.as_str().to_string();

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
            return Err(CompileError::Eval(GraphcalError::IndexBindingNotAnIndex {
                dep_index: binding_name.clone(),
                value: rhs_name,
                src: file_src.clone(),
                span: binding.value.span.into(),
            }));
        }

        // Validate kind matching (named-to-named, range-to-range).
        let dep_is_named = matches!(
            dep_idx.kind,
            graphcal_compiler::desugar::resolved_ast::IndexDeclKind::Named { .. }
                | graphcal_compiler::desugar::resolved_ast::IndexDeclKind::RequiredNamed
        );
        let imp_is_named = importer_idx_ast.map_or_else(
            || {
                importer_idx_from_registry
                    .map(graphcal_compiler::registry::types::IndexDef::is_named)
            },
            |imp_idx| {
                Some(matches!(
                    imp_idx.kind,
                    graphcal_compiler::desugar::resolved_ast::IndexDeclKind::Named { .. }
                        | graphcal_compiler::desugar::resolved_ast::IndexDeclKind::RequiredNamed
                ))
            },
        );
        if let Some(imp_named) = imp_is_named
            && dep_is_named != imp_named
        {
            return Err(CompileError::Eval(GraphcalError::IndexKindMismatch {
                dep_index: binding_name.clone(),
                dep_kind: if dep_is_named { "named" } else { "range" }.to_string(),
                bound_index: rhs_name,
                bound_kind: if imp_named { "named" } else { "range" }.to_string(),
                src: file_src.clone(),
                span: binding.name.span.into(),
            }));
        }
        // Dimension matching for range indexes is deferred to
        // process_deferred_dag_includes() where registries are available.
    }

    // Register the dependency's declaration names in the importer's scope
    // so that the resolver recognizes references to them.
    let mut import_item_attributes: HashMap<
        DeclName,
        Vec<graphcal_compiler::desugar::resolved_ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &include_decl.kind {
        graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the dependency.
                if !file_has_declaration(&dep_loaded.ast, orig_name) {
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: include_decl.path.display_path(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                // Collect import-item attributes for deferred processing.
                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(DeclName::new(orig_name), import_item.attributes.clone());
                }

                // Register the local name in scope for the resolver.
                // Determine the category from the dep's AST.
                let scoped = ScopedName::local(local_name.as_str());
                let span = import_item.name.span;
                if dep_index.is_const(orig_name) {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if dep_index.is_runtime(orig_name) {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations (dim/unit/index/type) are not
                    // registered in imported_names; handled via registry merge.
                }
                // Type-system declarations from instantiated imports also need registration.
                if dep_index.is_type_system(orig_name) {
                    let selected = ctx
                        .imported_type_system_names
                        .entry(import_dag_id.clone())
                        .or_default();
                    if dep_index.types.contains(orig_name.as_str()) {
                        selected.insert_type(orig_name.clone());
                    } else {
                        selected.insert_default(orig_name.clone());
                    }
                }

                selective.push(ImportAlias {
                    original: DeclName::new(orig_name),
                    local: DeclName::new(local_name),
                });
            }
            Some(selective)
        }
        graphcal_compiler::desugar::resolved_ast::ImportKind::Module { .. } => {
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
                    let scoped = ScopedName::qualified(prefix.as_str(), name);
                    if is_const {
                        ctx.imported_names.const_names.push((scoped, import_span));
                    } else {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            // Import type-system declarations (pub items only).
            if let Some(dep_eval) = evaluated_files.get(import_dag_id) {
                ctx.extra_registry_builders
                    .push((&dep_eval.registry, &dep_eval.pub_names));
            }
            None
        }
    };

    // Required indexes must always be bound.
    for dep_decl in &dep_loaded.ast.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(
                GraphcalError::RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                },
            ));
        }
    }

    let pub_reexport_whole =
        decl.visibility == graphcal_compiler::desugar::resolved_ast::Visibility::Public;
    let pub_reexport_items: HashSet<DeclName> = match &include_decl.kind {
        graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.is_pub)
            .map(|it| DeclName::new(&it.name.name))
            .collect(),
        graphcal_compiler::desugar::resolved_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.deferred_dag_includes.push(DeferredDagInclude {
        source: DeferredDagSource::File {
            dep_dag_id: import_dag_id.clone(),
        },
        prefix,
        bindings,
        index_bindings,
        type_bindings,
        dim_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
        pub_reexport_whole,
        pub_reexport_items,
    });
    Ok(())
}

/// Process an inline DAG include (`include dag_name(...) { ... }`).
///
/// Creates a virtual File from the DAG body, validates bindings against it,
/// and defers for IR merging.
///
/// Per Concept 9, inline DAGs are strictly isolated: every name a DAG uses
/// must come from its own declarations or its own `import` statements. There
/// is no parent-scope inheritance — same-file and cross-file inline DAGs are
/// handled identically.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and deferred include setup form a single cohesive pipeline"
)]
pub(in crate::eval::project) fn process_inline_dag_include(
    dag_def: &graphcal_compiler::desugar::resolved_ast::DagDecl,
    dag_name: &str,
    parent_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    include_decl: &graphcal_compiler::desugar::resolved_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::resolved_ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    ctx: &mut ImportContext<'_>,
) -> Result<(), CompileError> {
    use graphcal_compiler::desugar::resolved_ast::ImportKind;

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        ImportKind::Module { alias } => alias.as_ref().map_or_else(
            || dag_name.to_string(),
            |alias_ident| alias_ident.value.to_string(),
        ),
        ImportKind::Selective(_) => dag_name.to_string(),
    };

    // Check for duplicate module names.
    // We use a sentinel DagId for inline DAGs in the module_map.
    let sentinel_dag_id =
        graphcal_compiler::syntax::dag_id::DagId::root(format!("<dag:{dag_name}>"));
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
            name: prefix,
            first: (*first_span).into(),
            src: file_src.clone(),
            span: include_decl.path.span().into(),
        }));
    }
    ctx.module_map
        .insert(prefix.clone(), (sentinel_dag_id, include_decl.path.span()));

    let dag_body = graphcal_compiler::desugar::resolved_ast::File {
        declarations: dag_def.body.clone(),
    };
    let dag_imported_names = ImportedValueNames::default();

    // Classify and validate bindings against the DAG body's declarations.
    // Inline DAGs reuse the same DepDeclIndex as file-based instantiated
    // includes, but do not do additional cross-registry index-kind validation
    // (which file includes need to handle re-exported indexes).
    let dep_index = build_dep_decl_index(&dag_body.declarations);
    let ClassifiedBindings {
        params: bindings,
        indexes: index_bindings,
        types: type_bindings,
        dims: dim_bindings,
    } = classify_param_bindings(&include_decl.param_bindings, &dep_index, file_src, dag_name)?;

    // Register imported names in the importer's scope.
    let mut import_item_attributes: HashMap<
        DeclName,
        Vec<graphcal_compiler::desugar::resolved_ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &include_decl.kind {
        ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the DAG body.
                if !file_has_declaration(&dag_body, orig_name) {
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: dag_name.to_string(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(DeclName::new(orig_name), import_item.attributes.clone());
                }

                // Register the local name in scope.
                let is_const = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name)
                });
                let is_runtime = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name)
                });
                let scoped = ScopedName::local(local_name.as_str());
                let span = import_item.name.span;
                if is_const {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if is_runtime {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations — handled via registry merge.
                }

                selective.push(ImportAlias {
                    original: DeclName::new(orig_name),
                    local: DeclName::new(local_name),
                });
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
                    let scoped = ScopedName::qualified(prefix.as_str(), name);
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
                GraphcalError::RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                },
            ));
        }
    }

    let pub_reexport_whole =
        decl.visibility == graphcal_compiler::desugar::resolved_ast::Visibility::Public;
    let pub_reexport_items: HashSet<DeclName> = match &include_decl.kind {
        graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.is_pub)
            .map(|it| DeclName::new(&it.name.name))
            .collect(),
        graphcal_compiler::desugar::resolved_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.deferred_dag_includes.push(DeferredDagInclude {
        source: DeferredDagSource::InlineDag {
            dag_body,
            dag_imported_names,
            parent_dag_id: parent_dag_id.clone(),
            dag_name: dag_name.to_string(),
        },
        prefix,
        bindings,
        index_bindings,
        type_bindings,
        dim_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
        pub_reexport_whole,
        pub_reexport_items,
    });
    Ok(())
}

/// Check whether a multi-segment `ModulePath` include is a bare module path
/// DAG reference. This is the case when:
/// 1. The path has 2+ segments, AND
/// 2. The resolved target file's AST contains a `dag` definition whose name
///    matches the last segment of the module path.
///
/// For example, `include pkg.lib.double(...)` where `pkg/lib.gcl` defines
/// `dag double { ... }`.
pub(in crate::eval::project) fn is_bare_module_dag_ref(
    import_path: &ModulePath,
    resolved_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    project: &crate::loader::LoadedProject,
) -> bool {
    if import_path.segments.len() < 2 {
        return false;
    }
    let Some(last_seg) = import_path.segments.last() else {
        return false;
    };
    let last_segment = &last_seg.name;

    // Check if the resolved file contains a DAG with the matching name.
    let Some(target_loaded) = project.files.get(resolved_dag_id) else {
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
#[expect(
    clippy::too_many_lines,
    reason = "visibility check adds necessary logic to the import processing"
)]
pub(in crate::eval::project) fn process_non_instantiated_import<'a>(
    project: &crate::loader::LoadedProject,
    import_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    import_path: &graphcal_compiler::desugar::resolved_ast::ModulePath,
    import_kind: &graphcal_compiler::desugar::resolved_ast::ImportKind,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
    is_import: bool,
) -> Result<(), CompileError> {
    let dep = evaluated_files.get(import_dag_id).ok_or_else(|| {
        CompileError::Eval(GraphcalError::EvalError {
            message: format!("internal: dependency {import_dag_id} not yet evaluated"),
            src: file_src.clone(),
            span: import_path.span().into(),
        })
    })?;

    match import_kind {
        graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                let dep_loaded = &project.files[import_dag_id];

                // Visibility check: the item must be declared `pub` in the source file.
                if !file_exports_import_item(&dep_loaded.ast, orig_name, import_item.namespace) {
                    let exists =
                        file_has_import_item(&dep_loaded.ast, orig_name, import_item.namespace);
                    if exists {
                        return Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
                            name: orig_name.clone(),
                            file_path: import_path.display_path(),
                            src: file_src.clone(),
                            span: import_item.name.span.into(),
                        }));
                    }
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: import_path.display_path(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                if import_item.namespace
                    == graphcal_compiler::desugar::resolved_ast::ImportItemNamespace::Type
                {
                    ctx.imported_type_system_names
                        .entry(import_dag_id.clone())
                        .or_default()
                        .insert_type(orig_name.clone());
                    continue;
                }

                match import_selective_item(
                    dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    file_src,
                    &mut ctx.imported_names,
                    &mut ctx.imported_values,
                    Some(&mut ctx.imported_source_order),
                )? {
                    SelectiveImportResult::Const => {}
                    SelectiveImportResult::Runtime => {
                        if is_import {
                            return Err(CompileError::Eval(GraphcalError::ImportRuntimeItem {
                                name: orig_name.clone(),
                                src: file_src.clone(),
                                span: import_item.name.span.into(),
                            }));
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
                        if file_has_import_item(
                            &dep_loaded.ast,
                            orig_name,
                            graphcal_compiler::desugar::resolved_ast::ImportItemNamespace::Default,
                        ) {
                            // Default type-system declaration (dim/unit/index/dag).
                            ctx.imported_type_system_names
                                .entry(import_dag_id.clone())
                                .or_default()
                                .insert_default(orig_name.clone());
                        } else {
                            return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                                name: orig_name.clone(),
                                file_path: import_path.display_path(),
                                src: file_src.clone(),
                                span: import_item.name.span.into(),
                            }));
                        }
                    }
                }
            }
        }
        graphcal_compiler::desugar::resolved_ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || derive_module_name_from_import_path(import_path),
                |alias_ident| alias_ident.value.to_string(),
            );
            if let Some((_, first_span)) = ctx.module_map.get(&module_name) {
                return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                    name: module_name,
                    first: (*first_span).into(),
                    src: file_src.clone(),
                    span: import_path.span().into(),
                }));
            }
            ctx.module_map.insert(
                module_name.clone(),
                (import_dag_id.clone(), import_path.span()),
            );

            // Import all values under module::name prefix.
            let import_span = import_path.span();
            import_module_values(
                dep,
                &module_name,
                import_span,
                file_src,
                &mut ctx.imported_names,
                &mut ctx.imported_values,
                Some(&mut ctx.imported_source_order),
                is_import,
            )?;
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
pub(in crate::eval::project) fn check_dag_recursion(
    dag_definitions: &HashMap<String, &graphcal_compiler::desugar::resolved_ast::DagDecl>,
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
            #[expect(
                clippy::expect_used,
                reason = "DFS invariant: in_stack ⇒ node is on path"
            )]
            let cycle_start = path
                .iter()
                .position(|n| *n == node)
                .expect("DFS invariant: in_stack ⇒ node is on path");
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
                && inc.path.segments.len() == 1
            {
                let target = inc.path.segments[0].name.as_str();
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
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: format!("recursive DAG instantiation: {cycle_str}"),
                src: file_src.clone(),
                span: dag_definitions[name.as_str()].span.into(),
            }));
        }
    }
    Ok(())
}

/// Look up a single selective import item in an `EvaluatedFile` and register it.
///
/// Handles `const_values` and values (params/nodes).
/// Returns what was found so the caller can handle assert and type-system fallbacks.
#[expect(
    clippy::too_many_arguments,
    reason = "helper mutates imported name/value/source-order collections together"
)]
pub(in crate::eval::project) fn import_selective_item(
    dep: &EvaluatedFile,
    orig_name: &str,
    local_name: &str,
    span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<SelectiveImportResult, CompileError> {
    // The dep's `declared_types` is keyed by typed `ScopedName`. Its top-level
    // declarations are always bare locals, so wrap the bare member name.
    let orig_decl = DeclName::new(orig_name);
    if let Some(rv) = dep.const_values.get(&orig_decl) {
        let dt = imported_declared_type(dep, &orig_decl, src, span)?;
        let scoped = ScopedName::local(local_name);
        imported_names.const_names.push((scoped.clone(), span));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        Ok(SelectiveImportResult::Const)
    } else if let Some(rv) = dep.values.get(&orig_decl) {
        let dt = imported_declared_type(dep, &orig_decl, src, span)?;
        let scoped = ScopedName::local(local_name);
        imported_names.param_names.push((scoped.clone(), span));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        Ok(SelectiveImportResult::Runtime)
    } else if dep.has_assert(orig_name) {
        Ok(SelectiveImportResult::Assert)
    } else {
        Ok(SelectiveImportResult::NotFound)
    }
}

fn imported_declared_type(
    dep: &EvaluatedFile,
    name: &DeclName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<DeclaredType, CompileError> {
    dep.declared_types
        .get(&ScopedName::local(name.as_str()))
        .cloned()
        .ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!("internal: imported value `{name}` is missing its declared type"),
                src: src.clone(),
                span: span.into(),
            })
        })
}

/// Import all values from an `EvaluatedFile` under a module prefix.
///
/// Registers `const_values` and values (params/nodes) with qualified
/// `ScopedName` names.
///
/// When `const_only` is `true`, only `const_values` are imported; runtime values
/// (params/nodes) are silently skipped. This is used for `import` statements which
/// only allow compile-time items.
#[expect(
    clippy::too_many_arguments,
    reason = "helper mutates imported name/value/source-order collections together"
)]
pub(in crate::eval::project) fn import_module_values(
    dep: &EvaluatedFile,
    module_name: &str,
    import_span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
    const_only: bool,
) -> Result<(), CompileError> {
    // Sort keys for deterministic ordering — HashMap iteration is arbitrary.
    let mut const_keys: Vec<&DeclName> = dep.const_values.keys().collect();
    const_keys.sort();
    for name in const_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
        let rv = &dep.const_values[name];
        let scoped = ScopedName::qualified(module_name, name.as_str());
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        let dt = imported_declared_type(dep, name, src, import_span)?;
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }

    // Skip runtime values when const_only is true (import semantics).
    if const_only {
        return Ok(());
    }

    let mut value_keys: Vec<&DeclName> = dep.values.keys().collect();
    value_keys.sort();
    for name in value_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
        let rv = &dep.values[name];
        let scoped = ScopedName::qualified(module_name, name.as_str());
        imported_names
            .param_names
            .push((scoped.clone(), import_span));
        let dt = imported_declared_type(dep, name, src, import_span)?;
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_evaluated_file() -> EvaluatedFile {
        EvaluatedFile {
            values: HashMap::new(),
            const_values: HashMap::new(),
            declared_types: HashMap::new(),
            assertions: HashMap::new(),
            registry: graphcal_compiler::registry::types::RegistryBuilder::new().build(),
            pub_names: HashSet::new(),
            dag_tirs: HashMap::new(),
        }
    }

    #[test]
    fn import_selective_item_errors_when_declared_type_is_missing() {
        let mut dep = empty_evaluated_file();
        dep.const_values
            .insert(DeclName::new("g0"), RuntimeValue::Scalar(9.80665));
        dep.pub_names.insert(DeclName::new("g0"));

        let src = graphcal_compiler::syntax::named_source("test.gcl", String::new());
        let mut imported_names = ImportedValueNames::default();
        let mut imported_values = HashMap::new();

        let err = import_selective_item(
            &dep,
            "g0",
            "g0",
            Span::new(0, 2),
            &src,
            &mut imported_names,
            &mut imported_values,
            None,
        )
        .expect_err("missing declared type must be an internal compile error");

        let message = format!("{err:?}");
        assert!(
            message.contains("missing its declared type"),
            "unexpected error: {message}"
        );
        assert!(imported_names.const_names.is_empty());
        assert!(imported_values.is_empty());
    }

    #[test]
    fn transitive_const_only_import_skips_runtime_without_registering_it() {
        let mut dep = empty_evaluated_file();
        dep.values
            .insert(DeclName::new("runtime"), RuntimeValue::Scalar(1.0));
        dep.pub_names.insert(DeclName::new("runtime"));

        let import_item = graphcal_compiler::desugar::resolved_ast::ImportItem {
            name: graphcal_compiler::desugar::resolved_ast::Ident {
                name: "runtime".to_string(),
                span: Span::new(0, 7),
            },
            alias: None,
            is_pub: false,
            namespace: graphcal_compiler::desugar::resolved_ast::ImportItemNamespace::Default,
            attributes: Vec::new(),
        };
        let import_kind =
            graphcal_compiler::desugar::resolved_ast::ImportKind::Selective(vec![import_item]);
        let path = graphcal_compiler::desugar::resolved_ast::ModulePath {
            segments: vec![graphcal_compiler::desugar::resolved_ast::Ident {
                name: "dep".to_string(),
                span: Span::new(0, 3),
            }],
            span: Span::new(0, 3),
        };
        let src = graphcal_compiler::syntax::named_source("test.gcl", String::new());
        let mut imported_names = ImportedValueNames::default();
        let mut imported_values = HashMap::new();

        crate::eval::project::lowering::build_dep_import_values_for_kind(
            &path,
            &import_kind,
            &dep,
            &src,
            &mut imported_names,
            &mut imported_values,
            true,
        )
        .expect("runtime values are skipped for const-only transitive imports");

        assert!(imported_names.param_names.is_empty());
        assert!(imported_values.is_empty());
    }
}
