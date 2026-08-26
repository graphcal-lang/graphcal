//! Import processing functions for project-based compilation.

use super::recursion;
#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use super::*;
use crate::import_surface::{
    ProjectDeclIdentity, ProjectDeclKind, PureImportRejection, PureImportTermDisposition,
    decl_identity, pure_import_term_disposition,
};
use graphcal_compiler::desugar::desugared_ast::DeclKind;
use graphcal_compiler::registry::reserved_name::ReservedNameNamespace;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::names::NameAtom;

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
    /// Param declaration names whose value must be supplied at each instantiation.
    required_params: HashSet<DeclName>,
    /// Plot declaration names (requestable through include brace lists; #847).
    plots: HashSet<DeclName>,
    types: HashSet<StructTypeName>,
    dims: HashSet<DimName>,
    /// Maps index name to its declaration (needed for kind/required checks).
    indexes: HashMap<IndexName, &'a graphcal_compiler::desugar::desugared_ast::IndexDecl>,
    /// "Other" declarations (const node / node / assert) that are invalid as
    /// binding targets; used to produce precise "is actually a …" diagnostics.
    other: HashMap<DeclName, OtherDeclKind>,
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
    recursion::check_dag_recursion(&dag_definitions, file_src)?;

    for (_declaration, import, target) in loaded_file.imports_with_targets() {
        cancellation.checkpoint()?;
        process_pure_import(
            project,
            target,
            &import.path,
            &import.kind,
            file_src,
            module_artifacts,
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
            file_src,
            module_artifacts,
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
            file_src,
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
            file_src,
            ctx,
        )?;
    }
    Ok(())
}

impl DepDeclIndex<'_> {
    fn is_const(&self, name: &str) -> bool {
        matches!(self.other.get(name), Some(OtherDeclKind::ConstNode))
    }
    fn is_runtime(&self, name: &str) -> bool {
        self.params.contains(name) || matches!(self.other.get(name), Some(OtherDeclKind::Node))
    }
    fn is_assert(&self, name: &str) -> bool {
        matches!(self.other.get(name), Some(OtherDeclKind::Assert))
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

fn reject_runtime_unit_import(
    dep: &LoweringModuleInterface,
    name: &NameAtom,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let unit_name = graphcal_compiler::syntax::dimension::UnitName::from_atom(name.clone());
    if dep.is_exported_dynamic_unit(&unit_name) {
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
    let attributes =
        graphcal_compiler::ir::resolve::attribute_validation::validate_attributes(
            &import_item.attributes,
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
                if !is_plot {
                    return Err(CompileError::Eval(
                        GraphcalError::HiddenIncludeItemNotAPlot {
                            name: import_item.name.name.to_string(),
                            src: file_src.clone(),
                            span: attr.span.into(),
                        },
                    ));
                }
                if !attr.args.is_empty() {
                    return Err(CompileError::Eval(GraphcalError::EvalError {
                        message: "`#[hidden]` takes no arguments".to_string(),
                        src: file_src.clone(),
                        span: attr.span.into(),
                    }));
                }
                hidden = true;
            }
            AttributeName::ExpectedFail => {
                if !is_assert {
                    return Err(CompileError::Eval(
                        GraphcalError::InvalidExpectedFailTarget {
                            kind: "include/import item".to_string(),
                            src: file_src.clone(),
                            span: attr.span.into(),
                        },
                    ));
                }
            }
            AttributeName::Assumes => {
                return Err(CompileError::Eval(GraphcalError::InvalidAssumesTarget {
                    kind: "include/import item".to_string(),
                    src: file_src.clone(),
                    span: attr.span.into(),
                }));
            }
            AttributeName::Lazy => {
                // Recognized but semantics deferred, matching declaration-level validation.
            }
        }
    }
    Ok(hidden)
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

fn validate_reserved_alias(
    namespace: ReservedNameNamespace,
    import_item: &graphcal_compiler::desugar::desugared_ast::ImportItem,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    let local_name = import_item.local_name_atom();
    graphcal_compiler::registry::reserved_name::validate_reserved_name(namespace, local_name)
        .map_err(|_| {
            let kind = match namespace {
                ReservedNameNamespace::TypeSystem => match import_item.namespace {
                    ImportItemNamespace::Type => "type alias",
                    ImportItemNamespace::Dimension => "dimension alias",
                    ImportItemNamespace::Index => "index alias",
                    ImportItemNamespace::Term | ImportItemNamespace::Unit => "type-system alias",
                },
                ReservedNameNamespace::Unit => "unit alias",
                ReservedNameNamespace::GraphValue => "graph-value alias",
            };
            CompileError::Eval(GraphcalError::BuiltinNameShadowed {
                kind,
                name: local_name.to_string(),
                src: file_src.clone(),
                span: import_item.local_span().into(),
            })
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
) -> DepDeclIndex<'_> {
    let mut params = HashSet::new();
    let mut required_params = HashSet::new();
    let mut plots = HashSet::new();
    let mut types = HashSet::new();
    let mut dims = HashSet::new();
    let mut indexes: HashMap<IndexName, &graphcal_compiler::desugar::desugared_ast::IndexDecl> =
        HashMap::new();
    let mut other: HashMap<DeclName, OtherDeclKind> = HashMap::new();
    for d in decls {
        match &d.kind {
            DeclKind::Param(p) => {
                params.insert(p.name.value.clone());
                if p.value.is_none() {
                    required_params.insert(p.name.value.clone());
                }
            }
            DeclKind::Type(t) => {
                types.insert(t.name.value.clone());
            }
            DeclKind::BaseDimension(dim) => {
                dims.insert(dim.name.value.clone());
            }
            DeclKind::Dimension(dim) => {
                dims.insert(dim.name.value.clone());
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

/// Classify each param binding against the dep's declaration index, returning
/// per-kind binding maps. Rejects bindings whose name targets a non-bindable
/// dep declaration (const/node/assert) or no dep declaration at all.
///
/// Caller-specific validation (e.g. index-kind matching, registry lookups for
/// already-compiled dependency artifacts) layers on top of this — this helper only
/// answers "is `binding_name` a param/type/dim/index of the dep, or invalid?".
fn classify_param_bindings(
    param_bindings: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dep_index: &DepDeclIndex<'_>,
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
        let binding_name = &binding.name.name;
        if dep_index.params.contains(binding_name.as_str()) {
            out.params
                .insert(DeclName::expect_valid(binding_name), binding.value.clone());
            continue;
        }
        if dep_index.types.contains(binding_name.as_str()) {
            let rhs_name = lowering::extract_type_name_from_binding_expr(
                &binding.value,
                binding_name,
                file_src,
            )?;
            out.types.insert(
                StructTypeName::expect_valid(binding_name),
                StructTypeName::expect_valid(rhs_name),
            );
            continue;
        }
        if dep_index.dims.contains(binding_name.as_str()) {
            let rhs_name = lowering::extract_type_name_from_binding_expr(
                &binding.value,
                binding_name,
                file_src,
            )?;
            out.dims.insert(
                DimName::expect_valid(binding_name),
                DimName::expect_valid(rhs_name),
            );
            continue;
        }
        if dep_index.indexes.contains_key(binding_name.as_str()) {
            let dep_name = IndexName::expect_valid(binding_name);
            let target =
                lowering::extract_index_binding_target(&binding.value, &dep_name, file_src)?;
            out.index_spans.insert(dep_name.clone(), binding.value.span);
            out.indexes.insert(dep_name, target);
            continue;
        }
        if let Some(kind) = dep_index.other.get(binding_name.as_str()) {
            return Err(CompileError::Eval(GraphcalError::BindingNotAParam {
                name: binding_name.to_string(),
                actual_kind: kind.as_str().to_string(),
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
    Ok(out)
}

fn validate_required_param_bindings(
    dep_index: &DepDeclIndex<'_>,
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
    clippy::too_many_lines,
    reason = "binding validation and scope registration form a single cohesive pipeline"
)]
pub(in crate::project_compiler) fn process_file_include<'a>(
    project: &'a crate::loader::LoadedProject,
    import_dag_id: &graphcal_compiler::dag_id::DagId,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files()[import_dag_id];
    let dep_index = build_dep_decl_index(&dep_loaded.ast().declarations);

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

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
                let is_plot = is_term_namespace
                    && (dep_index.is_plot(orig_name)
                        || file_exports_plot(project, import_dag_id, orig_name));
                let is_assert = is_term_namespace && dep_index.is_assert(orig_name);
                let is_graph_value = is_term_namespace
                    && (dep_index.is_const(orig_name) || dep_index.is_runtime(orig_name));
                if is_graph_value {
                    validate_reserved_alias(
                        ReservedNameNamespace::GraphValue,
                        import_item,
                        file_src,
                    )?;
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
                    unit_alias: prefix.clone(),
                    dynamic_unit_boundary: DynamicUnitBoundary::ConcreteInstance,
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

    // Required indexes must always be bound.
    for dep_decl in &dep_loaded.ast().declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
                name: idx.name.value.to_string(),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }
    }
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
    file_src: &NamedSource<Arc<String>>,
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

    // Register imported names in the importer's scope.
    let mut import_item_attributes: HashMap<
        DeclName,
        Vec<graphcal_compiler::desugar::desugared_ast::Attribute>,
    > = HashMap::new();
    let mut requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot> =
        HashMap::new();
    let mut assertion_aliases = HashMap::new();
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

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
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
                    validate_reserved_alias(
                        ReservedNameNamespace::GraphValue,
                        import_item,
                        file_src,
                    )?;
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

    // Strict binding check: all required indexes must be bound.
    for dep_decl in &dag_body.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
                name: idx.name.value.to_string(),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }
    }
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
    clippy::too_many_lines,
    reason = "visibility check adds necessary logic to the import processing"
)]
pub(in crate::project_compiler) fn process_pure_import<'a>(
    project: &crate::loader::LoadedProject,
    resolved_module: &crate::loader::ResolvedModuleTarget,
    import_path: &graphcal_compiler::desugar::desugared_ast::ModulePath,
    import_kind: &graphcal_compiler::desugar::desugared_ast::ImportKind,
    file_src: &NamedSource<Arc<String>>,
    module_artifacts: &'a HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
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

    match import_kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = DeclName::from_atom(import_item.local_name_atom().clone());

                // Boundary check: an ordinary item must be exported; a param is
                // nameable through its distinct input-port role.
                if !declarations_expose_import_item(declarations, orig_name, import_item.namespace)
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

                if import_item.namespace != ImportItemNamespace::Term {
                    let namespace = match import_item.namespace {
                        ImportItemNamespace::Unit => ReservedNameNamespace::Unit,
                        ImportItemNamespace::Type
                        | ImportItemNamespace::Dimension
                        | ImportItemNamespace::Index => ReservedNameNamespace::TypeSystem,
                        ImportItemNamespace::Term => ReservedNameNamespace::GraphValue,
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

                let disposition = pure_import_term_disposition(declarations, orig_name)
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
                            ReservedNameNamespace::GraphValue,
                            import_item,
                            file_src,
                        )?;
                        import_selective_item(
                            module_target,
                            orig_name,
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
            import_module_values(
                declarations,
                dep.external_surface(),
                module_target,
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
                    unit_alias: module_name.clone(),
                    dynamic_unit_boundary: DynamicUnitBoundary::PureImport,
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
    let orig_decl = DeclName::from_atom(orig_name.clone());
    let scoped = ScopedName::local(local_name.clone());
    imported_names.const_names.push((scoped.clone(), span));
    if let Some(source_order) = imported_source_order {
        source_order.push((scoped.clone(), DeclCategory::Const));
    }
    insert_imported_binding(
        imported_bindings,
        imported_names,
        scoped,
        HirImportedBinding::new(
            graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
                source_owner.clone(),
                orig_decl,
            ),
        ),
        src,
        span,
    )
}

/// Import all exported constants under a module prefix.
#[expect(
    clippy::too_many_arguments,
    reason = "helper mutates imported name/binding/source-order collections together"
)]
pub(in crate::project_compiler) fn import_module_values(
    declarations: &[graphcal_compiler::desugar::desugared_ast::Declaration],
    external_surface: &ExternalDeclSurface,
    source_owner: &graphcal_compiler::dag_id::DagId,
    module_name: &ModuleAliasName,
    import_span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, HirImportedBinding>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    for declaration in declarations {
        let DeclKind::ConstNode(constant) = &declaration.kind else {
            continue;
        };
        let name = &constant.name.value;
        if !external_surface.is_explicit_export(name) {
            continue;
        }
        let scoped = ScopedName::qualified(module_name.clone(), name.clone());
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        insert_imported_binding(
            imported_bindings,
            imported_names,
            scoped,
            HirImportedBinding::new(
                graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
                    source_owner.clone(),
                    name.clone(),
                ),
            ),
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
