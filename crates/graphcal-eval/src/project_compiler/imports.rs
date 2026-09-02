//! Import processing functions for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use super::*;
use crate::import_surface::{
    ProjectDeclIdentity, ProjectDeclKind, PureImportRejection, PureImportTermDisposition,
    decl_identity, pure_import_term_disposition, validate_constructor_alias,
    validate_reserved_alias,
};
use graphcal_compiler::desugar::desugared_ast::DeclKind;
use graphcal_compiler::ir::static_dependencies::{
    StaticImportRejection, StaticReferenceNamespaces, declaration_static_references,
    static_import_rejection,
};
use graphcal_compiler::ir::static_interface::{
    StaticInputKind, StaticInterface, StaticRole, static_binding_valid, static_interface,
};
use graphcal_compiler::registry::reserved_name::ReservedNameNamespace;
use graphcal_compiler::registry::resolve_types::{AttributeTarget, DeclarationKind};
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::dimension::UnitName;
use graphcal_compiler::syntax::module_resolve::{
    DeclSymbolKind, ExportedBindingTarget, ExportedImportItemKind,
};
use graphcal_compiler::syntax::names::NameAtom;

/// Classification of a name against a dependency's declarations.
///
/// Built once per dep file and reused for every binding rather than re-scanning
/// `declarations` four or five times per binding.
struct DepDeclIndex {
    params: HashSet<DeclName>,
    /// Param declaration names whose value must be supplied at each instantiation.
    required_params: HashSet<DeclName>,
    /// Plot declaration names (requestable through include brace lists; #847).
    plots: HashSet<DeclName>,
    types: HashMap<StructTypeName, StaticRole>,
    dims: HashMap<DimName, StaticRole>,
    /// Maps index name to its validated Static input role.
    indexes: HashMap<IndexName, StaticRole>,
    /// "Other" declarations (const node / node / assert) that are invalid as
    /// binding targets; used to produce precise "is actually a …" diagnostics.
    other: HashMap<DeclName, DeclarationKind>,
}

fn public_dynamic_units(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
) -> HashSet<UnitName> {
    declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            DeclKind::Unit(unit) if unit.visibility.is_public() && !unit.constness.is_const() => {
                Some(unit.name.value.clone())
            }
            _ => None,
        })
        .collect()
}

pub(in crate::project_compiler) struct InlineDagIncludeTarget<'a> {
    pub(in crate::project_compiler) dag_def: &'a graphcal_compiler::desugar::desugared_ast::DagDecl,
    pub(in crate::project_compiler) dag_id: &'a graphcal_compiler::dag_id::DagId,
    pub(in crate::project_compiler) dag_name: &'a str,
    pub(in crate::project_compiler) parent_dag_id: &'a graphcal_compiler::dag_id::DagId,
}

/// Populate one file body's pure imports and concrete include requests.
///
/// Both top-level file compilation and recursive file-DAG instantiation use
/// this path. Keeping import classification here prevents nested instances
/// from silently dropping their own include graph.
#[expect(
    clippy::too_many_lines,
    reason = "file imports plus file/inline/qualified include routing are one declaration pass"
)]
pub(in crate::project_compiler) fn process_file_body_declarations<'a>(
    project: &'a crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let loaded_file = &project.files()[file_dag_id];
    let file_src = loaded_file.named_source();
    let dag_definitions: HashMap<DeclName, &graphcal_compiler::desugar::desugared_ast::DagDecl> =
        loaded_file
            .ast()
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.kind {
                DeclKind::Dag(dag) => Some((dag.name.value.clone(), dag)),
                _ => None,
            })
            .collect();

    for (_declaration, import, target) in loaded_file.imports_with_targets() {
        cancellation.checkpoint()?;
        process_pure_import(
            project,
            target,
            &import.path,
            &import.kind,
            &loaded_file.ast().declarations,
            file_src,
            module_artifacts,
            module_resolver,
            ctx,
        )?;
    }

    for (declaration, include, target) in loaded_file.includes_with_targets() {
        cancellation.checkpoint()?;
        if target.target() != target.source_file() {
            continue;
        }
        process_file_include(
            project,
            target.source_file(),
            include,
            declaration,
            &loaded_file.ast().declarations,
            file_src,
            module_artifacts,
            module_resolver,
            ctx,
        )?;
    }

    for declaration in &loaded_file.ast().declarations {
        cancellation.checkpoint()?;
        let DeclKind::Include(include) = &declaration.kind else {
            continue;
        };
        if include.path.segments.len() != 1 {
            continue;
        }
        let dag_name = &include.path.segments[0].name;
        let Some(dag) = dag_definitions.get(dag_name.as_str()) else {
            continue;
        };
        let dag_id = file_dag_id.child(dag_name.as_str());
        process_inline_dag_include(
            &InlineDagIncludeTarget {
                dag_def: dag,
                dag_id: &dag_id,
                dag_name,
                parent_dag_id: file_dag_id,
            },
            include,
            declaration,
            &loaded_file.ast().declarations,
            file_src,
            module_resolver,
            ctx,
        )?;
    }

    for (declaration, include, target) in loaded_file.includes_with_targets() {
        cancellation.checkpoint()?;
        if target.target() == target.source_file() {
            continue;
        }
        let Some((target_loaded, target_dag)) = project.inline_dag(target.target()) else {
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "inline DAG target not found in project: {}",
                    target.target()
                ),
                src: file_src.clone(),
                span: include.path.span().into(),
            }));
        };
        if !target_dag.declaration(target_loaded).visibility.is_public()
            && target.source_file() != file_dag_id
        {
            return Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
                name: target.target().name().to_string(),
                file_path: include.path.display_path(),
                src: file_src.clone(),
                span: include.path.leaf().span.into(),
            }));
        }
        process_inline_dag_include(
            &InlineDagIncludeTarget {
                dag_def: target_dag.declaration(target_loaded),
                dag_id: target.target(),
                dag_name: target.target().name(),
                parent_dag_id: target.source_file(),
            },
            include,
            declaration,
            &loaded_file.ast().declarations,
            file_src,
            module_resolver,
            ctx,
        )?;
    }
    Ok(())
}

impl DepDeclIndex {
    fn is_const(&self, name: &str) -> bool {
        matches!(self.other.get(name), Some(DeclarationKind::ConstNode))
    }
    fn is_runtime(&self, name: &str) -> bool {
        self.params.contains(name) || matches!(self.other.get(name), Some(DeclarationKind::Node))
    }
    fn is_assert(&self, name: &str) -> bool {
        matches!(self.other.get(name), Some(DeclarationKind::Assert))
    }
    fn is_plot(&self, name: &str) -> bool {
        self.plots.contains(name)
    }
}

fn ensure_include_item_selectable(
    file: &graphcal_compiler::desugar::desugared_ast::File,
    name: &str,
    namespace: ImportItemNamespace,
    file_path: &str,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let presence = file_import_item_presence(file, name, namespace);
    if presence.can_select_output() {
        return Ok(());
    }
    match presence {
        ImportItemPresence::Private => Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src: file_src.clone(),
            span: span.into(),
        })),
        ImportItemPresence::Missing => Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src: file_src.clone(),
            span: span.into(),
        })),
        ImportItemPresence::ExplicitExport | ImportItemPresence::InputPort => Ok(()),
    }
}

fn validate_static_import_capability(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    name: &NameAtom,
    namespace: ImportItemNamespace,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    match static_import_rejection(declarations, name, namespace) {
        None => Ok(()),
        Some(StaticImportRejection::RequiredInput { kind, name }) => Err(CompileError::Eval(
            GraphcalError::ImportRequiredStaticInput {
                kind,
                name: name.to_string(),
                src: src.clone(),
                span: span.into(),
            },
        )),
        Some(StaticImportRejection::UnresolvedDependency(dependency)) => Err(CompileError::Eval(
            GraphcalError::ImportUnresolvedStaticDependency {
                name: name.to_string(),
                dependency_kind: dependency.kind(),
                dependency: dependency.name().to_string(),
                src: src.clone(),
                span: span.into(),
            },
        )),
    }
}

fn validate_qualified_static_import_references(
    consumer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dependency_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    module_name: &ModuleAliasName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    consumer_declarations
        .iter()
        .flat_map(|declaration| declaration_static_references(&declaration.kind))
        .filter_map(|reference| {
            let (owner, leaf) = reference.path().qualifier_and_leaf()?;
            (owner == [module_name.atom().clone()])
                .then_some((leaf.clone(), reference.namespaces()))
        })
        .try_for_each(|(name, namespaces)| {
            let candidate_namespaces: &[ImportItemNamespace] = match namespaces {
                StaticReferenceNamespaces::Exact(StaticInputKind::Type) => {
                    &[ImportItemNamespace::Type]
                }
                StaticReferenceNamespaces::Exact(StaticInputKind::Dimension) => {
                    &[ImportItemNamespace::Dimension]
                }
                StaticReferenceNamespaces::Exact(StaticInputKind::Index) => {
                    &[ImportItemNamespace::Index]
                }
                StaticReferenceNamespaces::TypeOrDimension => {
                    &[ImportItemNamespace::Type, ImportItemNamespace::Dimension]
                }
                StaticReferenceNamespaces::IndexTypeOrDimension => &[
                    ImportItemNamespace::Index,
                    ImportItemNamespace::Type,
                    ImportItemNamespace::Dimension,
                ],
            };
            candidate_namespaces
                .iter()
                .find(|namespace| {
                    declarations_have_import_item(dependency_declarations, &name, **namespace)
                })
                .map_or(Ok(()), |namespace| {
                    validate_static_import_capability(
                        dependency_declarations,
                        &name,
                        *namespace,
                        src,
                        span,
                    )
                })
        })
}

fn reject_runtime_unit_import(
    dep: &LoweringModuleInterface,
    name: &NameAtom,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let unit_name = graphcal_compiler::syntax::dimension::UnitName::from_atom(name.clone());
    if dep.is_exported_runtime_unit(&unit_name) {
        return Err(CompileError::Eval(GraphcalError::ImportRuntimeUnit {
            name: name.to_string(),
            src: src.clone(),
            span: span.into(),
        }));
    }
    Ok(())
}

fn include_value_decl(
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
) -> Option<(DeclName, bool)> {
    match &decl.kind {
        DeclKind::Param(p) => Some((p.name.value.clone(), false)),
        DeclKind::ConstNode(c) if c.visibility.is_public() => Some((c.name.value.clone(), true)),
        DeclKind::Node(n) if n.visibility.is_public() => Some((n.name.value.clone(), false)),
        _ => None,
    }
}

fn value_decl_identity(
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
) -> Option<ProjectDeclIdentity<'_>> {
    decl_identity(decl).filter(|identity| {
        matches!(
            identity.kind,
            ProjectDeclKind::Param | ProjectDeclKind::Node | ProjectDeclKind::Const
        )
    })
}

/// Classify the values an include intentionally exposes to its consumer.
///
/// A brace include exposes exactly its selected value aliases. A whole-instance
/// include exposes param ports and explicitly exported consts/nodes under the
/// instance prefix. Private merged declarations remain debug-only and cannot be
/// named by the including DAG.
fn include_surface_outputs(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    prefix: &ModuleAliasName,
    selective: Option<&[ImportAlias]>,
) -> Vec<ScopedName> {
    selective.map_or_else(
        || {
            declarations
                .iter()
                .filter(|decl| decl_has_external_role(decl))
                .filter_map(value_decl_identity)
                .map(|identity| {
                    ScopedName::qualified(
                        prefix.clone(),
                        DeclName::from_atom(identity.name.clone()),
                    )
                })
                .collect()
        },
        |aliases| {
            aliases
                .iter()
                .filter(|alias| {
                    declarations.iter().any(|decl| {
                        value_decl_identity(decl)
                            .is_some_and(|identity| identity.name == alias.original.as_str())
                    })
                })
                .map(|alias| ScopedName::local(alias.local.clone()))
                .collect()
        },
    )
}

/// Validate an include/import item's attributes and return whether it carries
/// `#[hidden]` (#847). Per-item attributes are intentionally limited:
/// `#[hidden]` is plot-only, and `#[expected_fail]` is assertion-only.
fn validate_include_item_attributes(
    import_item: &graphcal_compiler::desugar::desugared_ast::ImportItem,
    is_plot: bool,
    is_assert: bool,
    file_src: &NamedSource<Arc<String>>,
) -> Result<bool, CompileError> {
    let mut hidden = false;
    let producer = match (is_plot, is_assert) {
        (true, _) => Some(DeclarationKind::Plot),
        (false, true) => Some(DeclarationKind::Assert),
        (false, false) => None,
    };
    let target = AttributeTarget::include_item(producer, import_item.name.name.clone());
    let attributes =
        graphcal_compiler::ir::resolve::attribute_validation::validate_attributes(
            &import_item.attributes,
            &target,
        )
        .map_err(|error| {
            CompileError::Eval(
                graphcal_compiler::ir::resolve::attribute_validation::attribute_validation_error_to_graphcal(
                    error,
                    file_src,
                ),
            )
        })?;
    for validated in attributes {
        let attr = validated.attribute();
        match validated.name() {
            AttributeName::Hidden => {
                if !attr.args.is_empty() {
                    return Err(CompileError::Eval(GraphcalError::EvalError {
                        message: "`#[hidden]` takes no arguments".to_string(),
                        src: file_src.clone(),
                        span: attr.span.into(),
                    }));
                }
                hidden = true;
            }
            AttributeName::ExpectedFail => {}
            AttributeName::Assumes => {
                return Err(CompileError::Eval(GraphcalError::internal_error(
                    "attribute applicability accepted assumes on an include item",
                    file_src,
                    graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(attr.span),
                )));
            }
            AttributeName::Lazy => {
                return Err(CompileError::Eval(GraphcalError::LazyNotSupported {
                    src: file_src.clone(),
                    span: attr.span.into(),
                }));
            }
        }
    }
    Ok(hidden)
}

fn exported_bindings(
    resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    owner: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Vec<graphcal_compiler::syntax::module_resolve::ExportedBinding>, CompileError> {
    resolver.exported_bindings(owner).map_err(|error| {
        CompileError::Eval(GraphcalError::InternalError {
            message: format!("module resolver could not enumerate exports of `{owner}`: {error}"),
            src: file_src.clone(),
            span: span.into(),
        })
    })
}

fn validate_include_producers(
    items: &[graphcal_compiler::desugar::desugared_ast::ImportItem],
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    graphcal_compiler::ir::resolve::include_selection::validate_unique_include_producers(items)
        .map_err(|error| {
            CompileError::Eval(
                graphcal_compiler::ir::resolve::include_selection::duplicate_include_producer_to_graphcal(
                    error,
                    file_src,
                ),
            )
        })
}

fn file_exports_plot(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::dag_id::DagId,
    name: &NameAtom,
) -> bool {
    fn visit(
        project: &crate::loader::LoadedProject,
        file_dag_id: &graphcal_compiler::dag_id::DagId,
        name: &NameAtom,
        seen: &mut HashSet<(graphcal_compiler::dag_id::DagId, DeclName)>,
    ) -> bool {
        let Some(file) = project.files().get(file_dag_id) else {
            return false;
        };
        let typed_name = DeclName::from_atom(name.clone());
        if !seen.insert((file_dag_id.clone(), typed_name)) {
            return false;
        }
        if file.ast().declarations.iter().any(|declaration| {
            matches!(
                &declaration.kind,
                DeclKind::Plot(plot)
                    if plot.visibility.is_public() && plot.name.value.atom() == name
            )
        }) {
            return true;
        }
        file.includes_with_targets().any(|(_, include, target)| {
            let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                &include.kind
            else {
                return false;
            };
            items.iter().any(|item| {
                item.is_pub
                    && item.local_name_atom() == name
                    && visit(project, target.source_file(), &item.name.name, seen)
            })
        })
    }

    visit(project, file_dag_id, name, &mut HashSet::new())
}

fn build_dep_decl_index(
    decls: &[graphcal_compiler::desugar::desugared_ast::Declaration],
) -> DepDeclIndex {
    let mut params = HashSet::new();
    let mut required_params = HashSet::new();
    let mut plots = HashSet::new();
    let mut types = HashMap::new();
    let mut dims = HashMap::new();
    let mut indexes = HashMap::new();
    let mut other: HashMap<DeclName, DeclarationKind> = HashMap::new();
    for d in decls {
        match &d.kind {
            DeclKind::Param(p) => {
                params.insert(p.name.value.clone());
                if p.value.is_none() {
                    required_params.insert(p.name.value.clone());
                }
            }
            DeclKind::Type(type_decl) => {
                if let Some(interface) = static_interface(&d.kind) {
                    types.insert(type_decl.name.value.clone(), interface.role());
                }
            }
            DeclKind::BaseDimension(dimension) => {
                if let Some(interface) = static_interface(&d.kind) {
                    dims.insert(dimension.name.value.clone(), interface.role());
                }
            }
            DeclKind::Dimension(dimension) => {
                if let Some(interface) = static_interface(&d.kind) {
                    dims.insert(dimension.name.value.clone(), interface.role());
                }
            }
            DeclKind::Index(index) => {
                if let Some(interface) = static_interface(&d.kind) {
                    indexes.insert(index.name.value.clone(), interface.role());
                }
            }
            DeclKind::ConstNode(c) => {
                other.insert(c.name.value.clone(), DeclarationKind::ConstNode);
            }
            DeclKind::Node(n) => {
                other.insert(n.name.value.clone(), DeclarationKind::Node);
            }
            DeclKind::Assert(a) => {
                other.insert(a.name.value.clone(), DeclarationKind::Assert);
            }
            DeclKind::Plot(pl) => {
                plots.insert(pl.name.value.clone());
            }
            _ => {}
        }
    }
    DepDeclIndex {
        params,
        required_params,
        plots,
        types,
        dims,
        indexes,
        other,
    }
}

/// Classified param bindings: each entry routes to one of the four binding
/// maps based on what the dependency declares the binding name as. Index
/// values retain a typed declared-or-structural target; `types` / `dims` use
/// [`DepToImporter`] keying from the dep-side name to the importer-side name.
struct ClassifiedBindings {
    params: HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    indexes: IndexBindings,
    index_spans: HashMap<IndexName, Span>,
    types: DepToImporter<StructTypeName>,
    dims: DepToImporter<DimName>,
}

/// Route each binding through the namespace/category selected by its authored
/// marker. No branch retries another namespace when the selected target is
/// absent or has the wrong capability.
fn classify_param_bindings(
    param_bindings: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dep_index: &DepDeclIndex,
    file_src: &NamedSource<Arc<String>>,
    dep_path_for_error: &str,
) -> Result<ClassifiedBindings, CompileError> {
    let mut out = ClassifiedBindings {
        params: HashMap::new(),
        indexes: HashMap::new(),
        index_spans: HashMap::new(),
        types: HashMap::new(),
        dims: HashMap::new(),
    };
    for binding in param_bindings {
        use graphcal_compiler::syntax::ast::InputBindingCategory;

        let binding_name = &binding.name.name;
        match binding.category {
            InputBindingCategory::Unmarked if dep_index.params.contains(binding_name.as_str()) => {
                out.params
                    .insert(DeclName::expect_valid(binding_name), binding.value.clone());
            }
            InputBindingCategory::Type
                if dep_index
                    .types
                    .get(binding_name.as_str())
                    .is_some_and(|role| role.is_bindable()) =>
            {
                let rhs_name = lowering::extract_type_name_from_binding_expr(
                    &binding.value,
                    binding_name,
                    file_src,
                )?;
                out.types.insert(
                    StructTypeName::expect_valid(binding_name),
                    StructTypeName::expect_valid(rhs_name),
                );
            }
            InputBindingCategory::Dimension
                if dep_index
                    .dims
                    .get(binding_name.as_str())
                    .is_some_and(|role| role.is_bindable()) =>
            {
                let rhs_name = lowering::extract_type_name_from_binding_expr(
                    &binding.value,
                    binding_name,
                    file_src,
                )?;
                out.dims.insert(
                    DimName::expect_valid(binding_name),
                    DimName::expect_valid(rhs_name),
                );
            }
            InputBindingCategory::Index
                if dep_index
                    .indexes
                    .get(binding_name.as_str())
                    .is_some_and(|role| role.is_bindable()) =>
            {
                let dep_name = IndexName::expect_valid(binding_name);
                let target =
                    lowering::extract_index_binding_target(&binding.value, &dep_name, file_src)?;
                out.index_spans.insert(dep_name.clone(), binding.value.span);
                out.indexes.insert(dep_name, target);
            }
            InputBindingCategory::Unmarked => {
                if let Some(kind) = dep_index.other.get(binding_name.as_str()) {
                    return Err(CompileError::Eval(GraphcalError::BindingNotAParam {
                        name: binding_name.to_string(),
                        actual_kind: *kind,
                        src: file_src.clone(),
                        span: binding.name.span.into(),
                    }));
                }
                return Err(CompileError::Eval(GraphcalError::UnknownParamBinding {
                    name: binding_name.to_string(),
                    file_path: dep_path_for_error.to_string(),
                    src: file_src.clone(),
                    span: binding.name.span.into(),
                }));
            }
            category => {
                return Err(CompileError::Eval(
                    GraphcalError::DagInputCategoryMismatch {
                        name: binding_name.to_string(),
                        expected: match category {
                            InputBindingCategory::Unmarked => "param",
                            InputBindingCategory::Type => "type",
                            InputBindingCategory::Dimension => "dim",
                            InputBindingCategory::Index => "index",
                        },
                        src: file_src.clone(),
                        span: binding.name.span.into(),
                    },
                ));
            }
        }
    }
    Ok(out)
}

fn local_static_interface(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    target_kind: StaticInputKind,
    target_name: &NameAtom,
) -> Option<StaticInterface> {
    declarations.iter().find_map(|declaration| {
        let interface = static_interface(&declaration.kind)?;
        let name = match &declaration.kind {
            DeclKind::BaseDimension(dimension) => dimension.name.value.atom(),
            DeclKind::Dimension(dimension) => dimension.name.value.atom(),
            DeclKind::Type(type_decl) => type_decl.name.value.atom(),
            DeclKind::Index(index) => index.name.value.atom(),
            _ => return None,
        };
        (interface.kind() == target_kind && name == target_name).then_some(interface)
    })
}

/// Validate an include binding at blueprint-composition time.
///
/// The shared semantic predicate accepts only effective concrete targets. An
/// outer generic DAG may additionally forward a required index to a nested DAG;
/// that symbolic edge is resolved to a concrete target before instantiation.
fn static_binding_composition_valid(input: StaticInterface, target: StaticInterface) -> bool {
    static_binding_valid(input, target)
        || (input.kind() == StaticInputKind::Index
            && input.role().is_bindable()
            && target.kind() == StaticInputKind::Index
            && target.role() == StaticRole::RequiredInput)
}

fn validate_concrete_static_binding_targets(
    importer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dep_index: &DepDeclIndex,
    type_bindings: &DepToImporter<StructTypeName>,
    dim_bindings: &DepToImporter<DimName>,
    index_bindings: &IndexBindings,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let invalid_type = type_bindings.iter().find_map(|(input, target)| {
        let source = StaticInterface::new(StaticInputKind::Type, dep_index.types[input]);
        let target_interface =
            local_static_interface(importer_declarations, StaticInputKind::Type, target.atom())?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Type,
            input.to_string(),
            target.to_string(),
        ))
    });
    let invalid_dimension = dim_bindings.iter().find_map(|(input, target)| {
        let source = StaticInterface::new(StaticInputKind::Dimension, dep_index.dims[input]);
        let target_interface = local_static_interface(
            importer_declarations,
            StaticInputKind::Dimension,
            target.atom(),
        )?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Dimension,
            input.to_string(),
            target.to_string(),
        ))
    });
    let invalid_index = index_bindings.iter().find_map(|(input, target)| {
        let IndexBindingTarget::Declared(target) = target else {
            return None;
        };
        let source = StaticInterface::new(StaticInputKind::Index, dep_index.indexes[input]);
        let target_interface =
            local_static_interface(importer_declarations, StaticInputKind::Index, target.atom())?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Index,
            input.to_string(),
            target.to_string(),
        ))
    });
    match invalid_type.or(invalid_dimension).or(invalid_index) {
        None => Ok(()),
        Some((kind, name, target)) => Err(CompileError::Eval(
            GraphcalError::InvalidStaticBindingTarget {
                kind,
                name,
                target,
                src: file_src.clone(),
                span: include_span.into(),
            },
        )),
    }
}

fn projected_static_alias(
    item: &graphcal_compiler::syntax::ast::ImportItem,
    type_bindings: &DepToImporter<StructTypeName>,
    dim_bindings: &DepToImporter<DimName>,
    index_bindings: &IndexBindings,
) -> Option<ProjectedStaticAlias> {
    let source = item.name.name.clone();
    let alias = item.local_name_atom().clone();
    match item.namespace {
        ImportItemNamespace::Type => {
            let source = StructTypeName::from_atom(source);
            let target = type_bindings
                .get(&source)
                .cloned()
                .unwrap_or_else(|| source.clone());
            let specialized = target == source
                && (!index_bindings.is_empty()
                    || !type_bindings.is_empty()
                    || !dim_bindings.is_empty());
            Some(ProjectedStaticAlias::Type {
                alias: StructTypeName::from_atom(alias),
                target,
                specialized,
            })
        }
        ImportItemNamespace::Dimension => {
            let source = DimName::from_atom(source);
            Some(ProjectedStaticAlias::Dimension {
                alias: DimName::from_atom(alias),
                target: dim_bindings.get(&source).cloned().unwrap_or(source),
            })
        }
        ImportItemNamespace::Index => {
            let source = IndexName::from_atom(source);
            Some(ProjectedStaticAlias::Index {
                alias: IndexName::from_atom(alias),
                target: index_bindings
                    .get(&source)
                    .cloned()
                    .unwrap_or(IndexBindingTarget::Declared(source)),
            })
        }
        ImportItemNamespace::Unit => Some(ProjectedStaticAlias::Unit {
            alias: UnitName::from_atom(alias),
            target: UnitName::from_atom(source),
        }),
        ImportItemNamespace::Term => None,
    }
}

fn projection_uses_source_declaration(
    item: &graphcal_compiler::syntax::ast::ImportItem,
    projection: &ProjectedStaticAlias,
) -> bool {
    match projection {
        ProjectedStaticAlias::Type { target, .. } => target.atom() == &item.name.name,
        ProjectedStaticAlias::Dimension { target, .. } => target.atom() == &item.name.name,
        ProjectedStaticAlias::Index { target, .. } => matches!(
            target,
            IndexBindingTarget::Declared(target) if target.atom() == &item.name.name
        ),
        ProjectedStaticAlias::Unit { .. } => true,
    }
}

const fn projection_requires_source_registration(projection: &ProjectedStaticAlias) -> bool {
    !matches!(
        projection,
        ProjectedStaticAlias::Type {
            specialized: true,
            ..
        }
    )
}

fn validate_required_static_bindings(
    dep_index: &DepDeclIndex,
    type_bindings: &DepToImporter<StructTypeName>,
    dim_bindings: &DepToImporter<DimName>,
    index_bindings: &IndexBindings,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let mut missing = dep_index
        .types
        .iter()
        .filter(|(name, role)| role.is_required() && !type_bindings.contains_key(*name))
        .map(|(name, _)| (StaticInputKind::Type, name.to_string()))
        .chain(
            dep_index
                .dims
                .iter()
                .filter(|(name, role)| role.is_required() && !dim_bindings.contains_key(*name))
                .map(|(name, _)| (StaticInputKind::Dimension, name.to_string())),
        )
        .chain(
            dep_index
                .indexes
                .iter()
                .filter(|(name, role)| role.is_required() && !index_bindings.contains_key(*name))
                .map(|(name, _)| (StaticInputKind::Index, name.to_string())),
        )
        .collect::<Vec<_>>();
    missing.sort_by(|(first_kind, first_name), (second_kind, second_name)| {
        first_kind
            .marker()
            .cmp(second_kind.marker())
            .then_with(|| first_name.cmp(second_name))
    });
    let Some((kind, name)) = missing.into_iter().next() else {
        return Ok(());
    };
    Err(CompileError::Eval(
        GraphcalError::RequiredStaticInputNotBound {
            kind,
            name,
            src: file_src.clone(),
            span: include_span.into(),
        },
    ))
}

pub(super) fn validate_direct_dag_call_bindings(
    args: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dependency_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    importer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    dag_name: &str,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let dep_index = build_dep_decl_index(dependency_declarations);
    let collected = classify_param_bindings(args, &dep_index, file_src, dag_name)?;
    validate_concrete_static_binding_targets(
        importer_declarations,
        &dep_index,
        &collected.types,
        &collected.dims,
        &collected.indexes,
        file_src,
        span,
    )?;
    validate_required_static_bindings(
        &dep_index,
        &collected.types,
        &collected.dims,
        &collected.indexes,
        file_src,
        span,
    )?;
    validate_required_param_bindings(&dep_index, &collected.params, dag_name, file_src, span)
}

fn validate_required_param_bindings(
    dep_index: &DepDeclIndex,
    bindings: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    dag_name: &str,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let mut missing = dep_index
        .required_params
        .iter()
        .filter(|name| !bindings.contains_key(*name))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    missing.sort();
    Err(CompileError::Eval(GraphcalError::MissingDagBindings {
        missing,
        dag_name: dag_name.to_string(),
        src: file_src.clone(),
        span: include_span.into(),
    }))
}

/// Process every file-root DAG include, deferring its concrete instance for
/// post-lowering IR merging.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "binding validation and scope registration form a single cohesive pipeline over one include context"
)]
pub(in crate::project_compiler) fn process_file_include<'a>(
    project: &'a crate::loader::LoadedProject,
    import_dag_id: &graphcal_compiler::dag_id::DagId,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    importer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files()[import_dag_id];
    let dep_index = build_dep_decl_index(&dep_loaded.ast().declarations);
    let exported_bindings = exported_bindings(
        module_resolver,
        import_dag_id,
        file_src,
        include_decl.path.span(),
    )?;

    // A module-form include introduces a source-visible alias and therefore
    // participates in duplicate-alias checks. A selective include introduces
    // only its selected local declarations, so give its merged implementation
    // an opaque private instance scope instead (#1132).
    let instance_scope = include_decl.instance_scope();
    if let IncludeInstanceScope::Named(prefix) = &instance_scope {
        if let Some(first) = ctx.module_map.get(prefix) {
            return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                name: prefix.to_string(),
                first: first.span().into(),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }
        ctx.module_map.insert(
            prefix.clone(),
            ProjectModuleBinding {
                target: import_dag_id.clone(),
                span: include_decl.path.span(),
                role: graphcal_compiler::syntax::module_resolve::ModuleAliasRole::IncludedInstance,
            },
        );
    }
    let prefix = instance_scope.merge_scope_name();

    // Classify and validate bindings against the dependency's AST. Each
    // binding lands in one of params/types/dims/indexes, or is rejected as
    // an unknown / non-bindable name. Caller-specific cross-checks (importer
    // scope for index bindings) layer on top of the shared classification.
    let dep_path_display = include_decl.path.display_path();
    let ClassifiedBindings {
        params: bindings,
        indexes: index_bindings,
        index_spans: index_binding_spans,
        types: type_bindings,
        dims: dim_bindings,
    } = classify_param_bindings(
        &include_decl.param_bindings,
        &dep_index,
        file_src,
        &dep_path_display,
    )?;

    validate_concrete_static_binding_targets(
        importer_declarations,
        &dep_index,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;

    // Index existence, category, and effective coordinate dimension are checked
    // uniformly with inline-DAG bindings after both typed registries are available.

    // Register the dependency's declaration names in the importer's scope
    // so that the resolver recognizes references to them.
    let mut import_item_attributes: HashMap<
        DeclName,
        Vec<graphcal_compiler::desugar::desugared_ast::Attribute>,
    > = HashMap::new();
    let mut requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot> =
        HashMap::new();
    let mut assertion_aliases = HashMap::new();
    let mut unit_projection_aliases = Vec::new();
    let selective_names = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            validate_include_producers(names, file_src)?;
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let original = DeclName::from_atom(orig_name.clone());
                let local = DeclName::from_atom(import_item.local_name_atom().clone());

                ensure_include_item_selectable(
                    dep_loaded.ast(),
                    orig_name,
                    import_item.namespace,
                    &include_decl.path.display_path(),
                    file_src,
                    import_item.name.span,
                )?;
                if let Some(binding) = exported_bindings.iter().find(|binding| {
                    binding.name == *orig_name
                        && binding.target.kind().namespace() == import_item.namespace
                }) {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
                if !is_term_namespace
                    && let Some(projection) = projected_static_alias(
                        import_item,
                        &type_bindings,
                        &dim_bindings,
                        &index_bindings,
                    )
                {
                    if projection_uses_source_declaration(import_item, &projection)
                        && projection_requires_source_registration(&projection)
                    {
                        ctx.imported_type_system_names
                            .entry(import_dag_id.clone())
                            .or_default()
                            .insert(import_item.namespace, orig_name.clone());
                    }
                    if let ProjectedStaticAlias::Unit { alias, target } = &projection {
                        unit_projection_aliases.push(UnitProjectionAlias {
                            source: target.clone(),
                            alias: alias.clone(),
                        });
                    }
                    ctx.projected_static_aliases.push(projection);
                }
                let is_plot = is_term_namespace
                    && (dep_index.is_plot(orig_name)
                        || file_exports_plot(project, import_dag_id, orig_name));
                let is_assert = is_term_namespace && dep_index.is_assert(orig_name);
                let is_graph_value = is_term_namespace
                    && (dep_index.is_const(orig_name) || dep_index.is_runtime(orig_name));
                if is_graph_value {
                    validate_reserved_alias(ReservedNameNamespace::Term, import_item, file_src)?;
                }
                let hidden =
                    validate_include_item_attributes(import_item, is_plot, is_assert, file_src)?;
                if is_plot {
                    // The requested plot merges into the root namespace under
                    // its local alias, evaluating against this instance (#847).
                    requested_plots.insert(
                        original,
                        graphcal_compiler::ir::lower::RequestedPlot {
                            alias: local.clone(),
                            hidden,
                        },
                    );
                    ctx.imported_names
                        .plot_names
                        .push((ScopedName::local(local), import_item.local_span()));
                    continue;
                }

                // Collect import-item attributes for deferred processing.
                if !import_item.attributes.is_empty() {
                    import_item_attributes.insert(original.clone(), import_item.attributes.clone());
                }
                if is_assert {
                    ctx.imported_names
                        .assert_names
                        .push((local.clone(), import_item.local_span()));
                    assertion_aliases.insert(original.clone(), local.clone());
                }

                // Register the local name in scope for the resolver.
                // Determine the category from the dep's AST.
                let scoped = ScopedName::local(local.clone());
                let span = import_item.local_span();
                match (
                    dep_index.is_const(orig_name),
                    dep_index.is_runtime(orig_name),
                ) {
                    (true, _) => ctx.imported_names.const_names.push((scoped, span)),
                    (false, true) => ctx.imported_names.param_names.push((scoped, span)),
                    (false, false) => {}
                }

                selective.push(ImportAlias { original, local });
            }
            Some(selective)
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => {
            // Register all dep names under the prefix for scope checking.
            let import_span = include_decl.path.span();
            for dep_decl in &dep_loaded.ast().declarations {
                if let Some((name, is_const)) = include_value_decl(dep_decl) {
                    let scoped = ScopedName::qualified(prefix.clone(), name);
                    if is_const {
                        ctx.imported_names.const_names.push((scoped, import_span));
                    } else {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            // Import type-system declarations (pub items only). Dependency
            // interfaces are complete before any dependent HIR is lowered.
            let artifact = module_artifacts.get(import_dag_id).ok_or_else(|| {
                CompileError::Eval(GraphcalError::InternalError {
                    message: format!(
                        "HIR interface for included module `{import_dag_id}` is unavailable"
                    ),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                })
            })?;
            ctx.frontend_registry_imports
                .push(super::FrontendRegistryImport {
                    registry: artifact.frontend_registry(),
                    external_surface: artifact.external_surface(),
                    pure_import_declarations: None,
                    unit_alias: prefix.clone(),
                    runtime_unit_boundary: RuntimeUnitBoundary::ConcreteInstance,
                    import_span: include_decl.path.span(),
                });
            None
        }
    };
    let surface_outputs = include_surface_outputs(
        &dep_loaded.ast().declarations,
        &prefix,
        selective_names.as_deref(),
    );

    validate_required_static_bindings(
        &dep_index,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;
    validate_required_param_bindings(
        &dep_index,
        &bindings,
        &dep_path_display,
        file_src,
        decl.span,
    )?;

    let pub_reexport_items: HashSet<DeclName> = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.is_pub)
            .map(|it| DeclName::from_atom(it.name.name.clone()))
            .collect(),
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.include_instances.push(IncludeInstanceRequest {
        template: ModuleTemplateRef::file(import_dag_id.clone()),
        instance_scope,
        debug_scope: derive_module_name_from_import_path(&include_decl.path),
        bindings,
        index_bindings,
        index_binding_spans,
        type_bindings,
        dim_bindings,
        selective_names,
        unit_projection_aliases,
        runtime_unit_names: public_dynamic_units(&dep_loaded.ast().declarations),
        assertion_aliases,
        surface_outputs,
        requested_plots,
        include_span: decl.span,
        import_item_attributes,
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
    reason = "binding validation, scope registration, and instance request setup form one pipeline"
)]
pub(in crate::project_compiler) fn process_inline_dag_include(
    target: &InlineDagIncludeTarget<'_>,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    importer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    ctx: &mut ImportContext<'_>,
) -> Result<(), CompileError> {
    use graphcal_compiler::desugar::desugared_ast::ImportKind;

    let dag_def = target.dag_def;
    let dag_name = target.dag_name;
    let dag_id = target.dag_id;
    let parent_dag_id = target.parent_dag_id;

    // As for file-root includes, only the module form introduces an alias.
    // Selective inline-DAG includes receive an opaque private merge scope.
    let instance_scope = include_decl.instance_scope();
    if let IncludeInstanceScope::Named(prefix) = &instance_scope {
        if let Some(first) = ctx.module_map.get(prefix) {
            return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                name: prefix.to_string(),
                first: first.span().into(),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }
        ctx.module_map.insert(
            prefix.clone(),
            ProjectModuleBinding {
                target: dag_id.clone(),
                span: include_decl.path.span(),
                role: graphcal_compiler::syntax::module_resolve::ModuleAliasRole::IncludedInstance,
            },
        );
    }
    let prefix = instance_scope.merge_scope_name();

    let dag_body = graphcal_compiler::desugar::desugared_ast::File {
        declarations: dag_def.body.clone(),
    };
    let exported_bindings =
        exported_bindings(module_resolver, dag_id, file_src, include_decl.path.span())?;

    // Classify bindings against the DAG body's declarations. Typed index
    // compatibility is deferred to the same registry-backed path as file DAGs.
    let dep_index = build_dep_decl_index(&dag_body.declarations);
    let ClassifiedBindings {
        params: bindings,
        indexes: index_bindings,
        index_spans: index_binding_spans,
        types: type_bindings,
        dims: dim_bindings,
    } = classify_param_bindings(&include_decl.param_bindings, &dep_index, file_src, dag_name)?;
    validate_concrete_static_binding_targets(
        importer_declarations,
        &dep_index,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;

    // Register imported names in the importer's scope.
    let mut import_item_attributes: HashMap<
        DeclName,
        Vec<graphcal_compiler::desugar::desugared_ast::Attribute>,
    > = HashMap::new();
    let mut requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot> =
        HashMap::new();
    let mut assertion_aliases = HashMap::new();
    let mut unit_projection_aliases = Vec::new();
    let selective_names = match &include_decl.kind {
        ImportKind::Selective(names) => {
            validate_include_producers(names, file_src)?;
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let original = DeclName::from_atom(orig_name.clone());
                let local = DeclName::from_atom(import_item.local_name_atom().clone());

                ensure_include_item_selectable(
                    &dag_body,
                    orig_name,
                    import_item.namespace,
                    dag_name,
                    file_src,
                    import_item.name.span,
                )?;
                if let Some(binding) = exported_bindings.iter().find(|binding| {
                    binding.name == *orig_name
                        && binding.target.kind().namespace() == import_item.namespace
                }) {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
                if !is_term_namespace
                    && let Some(projection) = projected_static_alias(
                        import_item,
                        &type_bindings,
                        &dim_bindings,
                        &index_bindings,
                    )
                {
                    if projection_uses_source_declaration(import_item, &projection)
                        && projection_requires_source_registration(&projection)
                    {
                        ctx.imported_type_system_names
                            .entry(dag_id.clone())
                            .or_default()
                            .insert(import_item.namespace, orig_name.clone());
                    }
                    if let ProjectedStaticAlias::Unit { alias, target } = &projection {
                        unit_projection_aliases.push(UnitProjectionAlias {
                            source: target.clone(),
                            alias: alias.clone(),
                        });
                    }
                    ctx.projected_static_aliases.push(projection);
                }
                let is_plot = is_term_namespace
                    && dag_body.declarations.iter().any(|d| {
                        matches!(&d.kind, DeclKind::Plot(pl) if pl.name.value.as_str() == orig_name.as_str())
                    });
                let is_assert = is_term_namespace
                    && dag_body.declarations.iter().any(|d| {
                        matches!(&d.kind, DeclKind::Assert(assert) if assert.name.value.as_str() == orig_name.as_str())
                    });
                let is_graph_value = is_term_namespace
                    && dag_body.declarations.iter().any(|declaration| {
                        matches!(
                            &declaration.kind,
                            DeclKind::ConstNode(_) | DeclKind::Param(_) | DeclKind::Node(_)
                        ) && declaration
                            .kind
                            .name_and_span()
                            .is_some_and(|(name, _)| name == orig_name.as_str())
                    });
                if is_graph_value {
                    validate_reserved_alias(ReservedNameNamespace::Term, import_item, file_src)?;
                }
                let hidden =
                    validate_include_item_attributes(import_item, is_plot, is_assert, file_src)?;
                if is_plot {
                    requested_plots.insert(
                        original,
                        graphcal_compiler::ir::lower::RequestedPlot {
                            alias: local.clone(),
                            hidden,
                        },
                    );
                    ctx.imported_names
                        .plot_names
                        .push((ScopedName::local(local), import_item.local_span()));
                    continue;
                }

                if !import_item.attributes.is_empty() {
                    import_item_attributes.insert(original.clone(), import_item.attributes.clone());
                }
                if is_assert {
                    ctx.imported_names
                        .assert_names
                        .push((local.clone(), import_item.local_span()));
                    assertion_aliases.insert(original.clone(), local.clone());
                }

                // Register the local name in scope.
                let is_const = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name.as_str())
                });
                let is_runtime = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name.as_str())
                        || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name.as_str())
                });
                let scoped = ScopedName::local(local.clone());
                let span = import_item.local_span();
                if is_const {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if is_runtime {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations — handled via registry merge.
                }

                selective.push(ImportAlias { original, local });
            }
            Some(selective)
        }
        ImportKind::Module { .. } => {
            // Register all DAG body names under the prefix.
            let import_span = include_decl.path.span();
            for dep_decl in &dag_body.declarations {
                if let Some((name, is_const)) = include_value_decl(dep_decl) {
                    let scoped = ScopedName::qualified(prefix.clone(), name);
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
    let surface_outputs =
        include_surface_outputs(&dag_body.declarations, &prefix, selective_names.as_deref());

    validate_required_static_bindings(
        &dep_index,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;
    validate_required_param_bindings(&dep_index, &bindings, dag_name, file_src, decl.span)?;

    let pub_reexport_items: HashSet<DeclName> = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.is_pub)
            .map(|it| DeclName::from_atom(it.name.name.clone()))
            .collect(),
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.include_instances.push(IncludeInstanceRequest {
        template: ModuleTemplateRef {
            source_file: parent_dag_id.clone(),
            dag_id: dag_id.clone(),
        },
        instance_scope,
        debug_scope: ModuleAliasName::expect_valid(dag_name),
        bindings,
        index_bindings,
        index_binding_spans,
        type_bindings,
        dim_bindings,
        selective_names,
        unit_projection_aliases,
        runtime_unit_names: public_dynamic_units(&dag_body.declarations),
        assertion_aliases,
        surface_outputs,
        requested_plots,
        include_span: decl.span,
        import_item_attributes,
        pub_reexport_items,
    });
    Ok(())
}

/// Process one pure import from a dependency's compile-time artifact.
///
/// Imports expose only compile-time items (consts, dimensions, static units,
/// types, indexes, and DAG blueprints). Runtime items and assertion outcomes
/// require an explicit instance and are rejected with migration guidance.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "visibility and capability checks consume the complete import context in one boundary pass"
)]
pub(in crate::project_compiler) fn process_pure_import<'a>(
    project: &'a crate::loader::LoadedProject,
    resolved_module: &crate::loader::ResolvedModuleTarget,
    import_path: &graphcal_compiler::desugar::desugared_ast::ModulePath,
    import_kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    importer_declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    module_resolver: &graphcal_compiler::syntax::module_resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let source_file = resolved_module.source_file();
    let module_target = resolved_module.target();
    let dep = module_artifacts.get(module_target).ok_or_else(|| {
        CompileError::Eval(GraphcalError::EvalError {
            message: format!("module `{module_target}` is not available for imports"),
            src: file_src.clone(),
            span: import_path.span().into(),
        })
    })?;
    let dep_loaded = &project.files()[source_file];
    let declarations = if module_target == source_file {
        dep_loaded.ast().declarations.as_slice()
    } else {
        project
            .inline_dag(module_target)
            .map(|(file, dag)| dag.body(file))
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::InternalError {
                    message: format!("inline module `{module_target}` has no owning declaration"),
                    src: file_src.clone(),
                    span: import_path.span().into(),
                })
            })?
    };
    let exported_bindings = module_resolver
        .exported_bindings(module_target)
        .map_err(|error| {
            CompileError::Eval(GraphcalError::InternalError {
                message: format!(
                    "module resolver could not enumerate exports of `{module_target}`: {error}"
                ),
                src: file_src.clone(),
                span: import_path.span().into(),
            })
        })?;

    match import_kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = DeclName::from_atom(import_item.local_name_atom().clone());

                let resolved_export = exported_bindings.iter().find(|binding| {
                    binding.name == *orig_name
                        && binding.target.kind().namespace() == import_item.namespace
                });
                if let Some(binding) = resolved_export {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }
                // Boundary check: resolver exports include transitive public
                // re-exports, while the source scan preserves input-port
                // semantics for directly authored params.
                if resolved_export.is_none()
                    && !declarations_expose_import_item(
                        declarations,
                        orig_name,
                        import_item.namespace,
                    )
                {
                    let exists = declarations_have_import_item(
                        declarations,
                        orig_name,
                        import_item.namespace,
                    );
                    if exists {
                        return Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
                            name: orig_name.to_string(),
                            file_path: import_path.display_path(),
                            src: file_src.clone(),
                            span: import_item.name.span.into(),
                        }));
                    }
                    return Err(CompileError::Eval(
                        import_item_not_found_error_from_declarations(
                            declarations,
                            orig_name,
                            import_item.namespace,
                            &import_path.display_path(),
                            file_src,
                            import_item.name.span,
                        ),
                    ));
                }

                validate_static_import_capability(
                    declarations,
                    orig_name,
                    import_item.namespace,
                    file_src,
                    import_item.name.span,
                )?;

                if import_item.namespace != ImportItemNamespace::Term {
                    let namespace = match import_item.namespace {
                        ImportItemNamespace::Unit => ReservedNameNamespace::Unit,
                        ImportItemNamespace::Type
                        | ImportItemNamespace::Dimension
                        | ImportItemNamespace::Index => ReservedNameNamespace::Static,
                        ImportItemNamespace::Term => ReservedNameNamespace::Term,
                    };
                    validate_reserved_alias(namespace, import_item, file_src)?;
                    if import_item.namespace == ImportItemNamespace::Unit {
                        reject_runtime_unit_import(
                            dep,
                            orig_name,
                            file_src,
                            import_item.name.span,
                        )?;
                    }
                    ctx.imported_type_system_names
                        .entry(module_target.clone())
                        .or_default()
                        .insert(import_item.namespace, orig_name.clone());
                    continue;
                }

                let disposition = resolved_export
                    .and_then(|binding| match binding.target.kind() {
                        ExportedImportItemKind::Decl(kind) => Some(match kind {
                            DeclSymbolKind::Const => PureImportTermDisposition::BindConstant,
                            DeclSymbolKind::Param | DeclSymbolKind::Node => {
                                PureImportTermDisposition::Reject(PureImportRejection::Runtime)
                            }
                            DeclSymbolKind::Assert => {
                                PureImportTermDisposition::Reject(PureImportRejection::Assertion)
                            }
                            DeclSymbolKind::Plot
                            | DeclSymbolKind::Figure
                            | DeclSymbolKind::Layer => PureImportTermDisposition::Reject(
                                PureImportRejection::Visualization,
                            ),
                            DeclSymbolKind::Dag => PureImportTermDisposition::ResolverOnly,
                        }),
                        ExportedImportItemKind::Constructor => {
                            Some(PureImportTermDisposition::ResolverOnly)
                        }
                        _ => None,
                    })
                    .or_else(|| pure_import_term_disposition(declarations, orig_name))
                    .ok_or_else(|| {
                        CompileError::Eval(GraphcalError::ImportNameNotFound {
                            name: orig_name.to_string(),
                            file_path: import_path.display_path(),
                            src: file_src.clone(),
                            span: import_item.name.span.into(),
                        })
                    })?;
                let is_visualization = matches!(
                    disposition,
                    PureImportTermDisposition::Reject(PureImportRejection::Visualization)
                );
                let is_assertion = matches!(
                    disposition,
                    PureImportTermDisposition::Reject(PureImportRejection::Assertion)
                );
                validate_include_item_attributes(
                    import_item,
                    is_visualization,
                    is_assertion,
                    file_src,
                )?;

                match disposition {
                    PureImportTermDisposition::BindConstant => {
                        validate_reserved_alias(
                            ReservedNameNamespace::Term,
                            import_item,
                            file_src,
                        )?;
                        let canonical = resolved_export
                            .and_then(|binding| binding.target.declaration())
                            .cloned()
                            .ok_or_else(|| {
                                CompileError::Eval(GraphcalError::InternalError {
                                    message: format!(
                                        "exported constant `{orig_name}` has no canonical declaration target"
                                    ),
                                    src: file_src.clone(),
                                    span: import_item.name.span.into(),
                                })
                            })?;
                        import_selective_resolved_item(
                            canonical,
                            &local_name,
                            import_item.local_span(),
                            file_src,
                            &mut ctx.imported_names,
                            &mut ctx.imported_bindings,
                            Some(&mut ctx.imported_source_order),
                        )?;
                    }
                    PureImportTermDisposition::ResolverOnly => {
                        ctx.imported_type_system_names
                            .entry(module_target.clone())
                            .or_default()
                            .insert(import_item.namespace, orig_name.clone());
                    }
                    PureImportTermDisposition::Reject(reason) => {
                        return Err(CompileError::Eval(reason.diagnostic(
                            orig_name,
                            file_src,
                            import_item.name.span,
                        )));
                    }
                }
            }
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || derive_module_name_from_import_path(import_path),
                |alias_ident| alias_ident.value.clone(),
            );
            if let Some(first) = ctx.module_map.get(&module_name) {
                return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                    name: module_name.to_string(),
                    first: first.span().into(),
                    src: file_src.clone(),
                    span: import_path.span().into(),
                }));
            }
            validate_qualified_static_import_references(
                importer_declarations,
                declarations,
                &module_name,
                file_src,
                import_path.span(),
            )?;

            let role = graphcal_compiler::syntax::module_resolve::ModuleAliasRole::ImportedDag;
            ctx.module_map.insert(
                module_name.clone(),
                ProjectModuleBinding {
                    target: module_target.clone(),
                    span: import_path.span(),
                    role,
                },
            );

            let import_span = import_path.span();
            // Import compile-time constants under the module prefix.
            import_module_values_from_resolver(
                &exported_bindings,
                &module_name,
                import_span,
                file_src,
                &mut ctx.imported_names,
                &mut ctx.imported_bindings,
                Some(&mut ctx.imported_source_order),
            )?;
            // Import all public type-system declarations from dep's registry.
            // The module alias keys the dep's pub units in this file's scope.
            ctx.frontend_registry_imports
                .push(super::FrontendRegistryImport {
                    registry: dep.frontend_registry(),
                    external_surface: dep.external_surface(),
                    pure_import_declarations: Some(declarations),
                    unit_alias: module_name.clone(),
                    runtime_unit_boundary: RuntimeUnitBoundary::PureImport,
                    import_span,
                });
        }
    }

    Ok(())
}

fn insert_imported_binding(
    imported_bindings: &mut HashMap<ScopedName, HirImportedBinding>,
    imported_names: &ImportedValueNames,
    lexical_name: ScopedName,
    binding: HirImportedBinding,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    if imported_bindings.contains_key(&lexical_name) {
        let first = imported_names
            .const_names
            .iter()
            .chain(&imported_names.param_names)
            .chain(&imported_names.node_names)
            .find_map(|(name, first_span)| (name == &lexical_name).then_some(*first_span))
            .unwrap_or(span);
        return Err(CompileError::Eval(GraphcalError::DuplicateName {
            name: lexical_name.to_string(),
            src: src.clone(),
            duplicate: span.into(),
            first: first.into(),
        }));
    }
    imported_bindings.insert(lexical_name, binding);
    Ok(())
}

/// Register a selectively imported constant at the HIR boundary.
#[expect(
    clippy::too_many_arguments,
    reason = "helper mutates imported name/binding/source-order collections together"
)]
#[cfg(test)]
pub(in crate::project_compiler) fn import_selective_item(
    source_owner: &graphcal_compiler::dag_id::DagId,
    orig_name: &NameAtom,
    local_name: &DeclName,
    span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, HirImportedBinding>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    import_selective_resolved_item(
        graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
            source_owner.clone(),
            DeclName::from_atom(orig_name.clone()),
        ),
        local_name,
        span,
        src,
        imported_names,
        imported_bindings,
        imported_source_order,
    )
}

fn import_selective_resolved_item(
    canonical: graphcal_compiler::syntax::decl_name::ResolvedDeclName,
    local_name: &DeclName,
    span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, HirImportedBinding>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    let scoped = ScopedName::local(local_name.clone());
    imported_names.const_names.push((scoped.clone(), span));
    if let Some(source_order) = imported_source_order {
        source_order.push((scoped.clone(), DeclCategory::Const));
    }
    insert_imported_binding(
        imported_bindings,
        imported_names,
        scoped,
        HirImportedBinding::new(canonical),
        src,
        span,
    )
}

/// Import all resolver-visible exported constants under a module prefix.
fn import_module_values_from_resolver(
    exported_bindings: &[graphcal_compiler::syntax::module_resolve::ExportedBinding],
    module_name: &ModuleAliasName,
    import_span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, HirImportedBinding>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    for binding in exported_bindings {
        let ExportedBindingTarget::Decl {
            identity: canonical,
            kind: DeclSymbolKind::Const,
        } = &binding.target
        else {
            continue;
        };
        let local = DeclName::from_atom(binding.name.clone());
        let scoped = ScopedName::qualified(module_name.clone(), local);
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        if let Some(source_order) = imported_source_order.as_deref_mut() {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        insert_imported_binding(
            imported_bindings,
            imported_names,
            scoped,
            HirImportedBinding::new(canonical.clone()),
            src,
            import_span,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_index_preserves_typed_non_param_categories() {
        let raw = graphcal_compiler::syntax::parser::Parser::new(
            "const node fixed: Dimensionless = 1.0;\n\
             node computed: Dimensionless = 2.0;\n\
             assert check = true;",
        )
        .parse_file()
        .unwrap();
        let file = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw);
        let index = build_dep_decl_index(&file.declarations);

        assert_eq!(index.other["fixed"], DeclarationKind::ConstNode);
        assert_eq!(index.other["computed"], DeclarationKind::Node);
        assert_eq!(index.other["check"], DeclarationKind::Assert);
    }

    #[test]
    fn selective_import_records_only_the_canonical_hir_target() {
        let src = NamedSource::new("test.gcl", Arc::new(String::new()));
        let mut imported_names = ImportedValueNames::default();
        let mut imported_bindings = HashMap::new();
        let owner = graphcal_compiler::dag_id::DagId::root_in_package("test", "dep");

        import_selective_item(
            &owner,
            &NameAtom::parse("g0").unwrap(),
            &DeclName::expect_valid("local_g0"),
            Span::new(0, 2),
            &src,
            &mut imported_names,
            &mut imported_bindings,
            None,
        )
        .unwrap();

        let lexical = ScopedName::local(DeclName::expect_valid("local_g0"));
        assert_eq!(
            imported_bindings[&lexical].target(),
            &graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
                owner,
                DeclName::expect_valid("g0"),
            )
        );
    }
}
