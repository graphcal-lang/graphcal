//! Import processing functions for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::declaration_category::{DeclCategory, ValueDeclCategory};
use graphcal_compiler::desugar::desugared_ast::ModulePath;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::DimName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;

use super::binding_values::{extract_index_binding_target, extract_type_name_from_binding_expr};
use super::module_resolve_errors::module_resolve_compile_error;
use crate::eval::types::CompileError;

use super::model::{
    ImportAlias, ImportContext, IncludeInstanceRequest, IncludeStaticBindings, IndexBindingSite,
    ProjectModuleBinding, UnitProjectionAlias,
};
use crate::import_surface::{
    import_item_not_found_error, validate_constructor_alias, validate_reserved_alias,
};
use graphcal_compiler::declaration_kind::{AttributeTarget, DeclarationKind};
use graphcal_compiler::desugar::desugared_ast::DeclKind;
use graphcal_compiler::ir::module_interface::{
    ModuleInterface, PureImportRejection, PureImportTermDisposition,
};
use graphcal_compiler::ir::static_dependencies::{
    ModuleDeclarations, StaticImportRejection, StaticScope, declaration_static_references,
    static_import_rejection,
};
use graphcal_compiler::plot_visibility::PlotVisibility;
use graphcal_compiler::resolve::category::{DeclSymbolKind, ExportedImportItemKind};
use graphcal_compiler::resolve::exports::ExportedBindingTarget;
use graphcal_compiler::resolve::namespace::Namespace;
use graphcal_compiler::static_interface::{
    StaticInputKind, StaticInterface, StaticRole, static_binding_valid,
};
use graphcal_compiler::syntax::ast::{DeclExposure, ImportItemNamespace, IntroducedKind};
use graphcal_compiler::syntax::attribute::AttributeName;
use graphcal_compiler::syntax::dimension::UnitName;
use graphcal_compiler::syntax::names::NameAtom;

/// Whether `kind` declares a graph value (`param`, `node`, or `const node`).
const fn is_graph_value_kind(kind: IntroducedKind) -> bool {
    match kind {
        IntroducedKind::Param | IntroducedKind::Node | IntroducedKind::ConstNode => true,
        IntroducedKind::Assert
        | IntroducedKind::Plot
        | IntroducedKind::Figure
        | IntroducedKind::Layer
        | IntroducedKind::Dag
        | IntroducedKind::Constructor
        | IntroducedKind::BaseDimension
        | IntroducedKind::Dimension
        | IntroducedKind::Unit
        | IntroducedKind::Type
        | IntroducedKind::Index => false,
    }
}

/// The declaration kind of a Term that exists in a dependency but cannot be
/// bound as a `param`, for precise "is actually a …" diagnostics.
fn non_param_binding_kind(interface: &ModuleInterface, name: &NameAtom) -> Option<DeclarationKind> {
    interface
        .declared_kinds(name, ImportItemNamespace::Term)
        .find_map(|kind| match kind {
            IntroducedKind::ConstNode => Some(DeclarationKind::ConstNode),
            IntroducedKind::Node => Some(DeclarationKind::Node),
            IntroducedKind::Assert => Some(DeclarationKind::Assert),
            IntroducedKind::Param
            | IntroducedKind::Plot
            | IntroducedKind::Figure
            | IntroducedKind::Layer
            | IntroducedKind::Dag
            | IntroducedKind::Constructor
            | IntroducedKind::BaseDimension
            | IntroducedKind::Dimension
            | IntroducedKind::Unit
            | IntroducedKind::Type
            | IntroducedKind::Index => None,
        })
}

/// Whether the dependency declares `name` as a runtime value (`param` or `node`).
fn declares_runtime_value(interface: &ModuleInterface, name: &NameAtom) -> bool {
    interface.declares(name, IntroducedKind::Param)
        || interface.declares(name, IntroducedKind::Node)
}

/// Whether the static input `name` of `kind` accepts a typed binding.
fn static_input_is_bindable(
    interface: &ModuleInterface,
    kind: StaticInputKind,
    name: &NameAtom,
) -> bool {
    interface
        .static_interface(kind, name)
        .is_some_and(|input| input.role().is_bindable())
}

pub(super) struct InlineDagIncludeTarget<'a> {
    pub(super) module: crate::loader::LoadedModule<'a>,
    pub(super) dag_name: &'a str,
}

/// Populate one file body's pure imports and concrete include requests.
///
/// Both top-level file compilation and recursive file-DAG instantiation use
/// this path. Keeping import classification here prevents nested instances
/// from silently dropping their own include graph.
pub(super) fn process_file_body_declarations<'a>(
    project: &'a crate::loader::LoadedProject,
    loaded_file: &crate::loader::LoadedFile,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), CompileError> {
    let file_dag_id = loaded_file.dag_id();
    let file_src = loaded_file.named_source();

    for (_declaration, import, target) in loaded_file.imports_with_targets() {
        cancellation.checkpoint()?;
        process_pure_import(
            project,
            target,
            import,
            ModuleDeclarations::new(
                &loaded_file.ast().declarations,
                StaticScope::new(file_dag_id, module_resolver),
            ),
            file_src,
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
            target,
            include,
            declaration,
            loaded_file.interface(),
            file_src,
            StaticScope::new(file_dag_id, module_resolver),
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
        // A single-segment include names a top-level `dag` of this file.
        let dag_id = file_dag_id.inline_dag_child(DeclName::classify(dag_name.atom().clone()));
        let Some((dag_file, loaded_dag)) = project.inline_dag(&dag_id) else {
            continue;
        };
        process_inline_dag_include(
            &InlineDagIncludeTarget {
                module: loaded_dag.module(dag_file),
                dag_name: loaded_dag.declaration(dag_file).name.value.as_str(),
            },
            include,
            declaration,
            loaded_file.interface(),
            file_src,
            StaticScope::new(file_dag_id, module_resolver),
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
                name: target.target().leaf().to_string(),
                file_path: include.path.display_path(),
                src: file_src.clone(),
                span: include.path.leaf().span.into(),
            }));
        }
        process_inline_dag_include(
            &InlineDagIncludeTarget {
                module: target_dag.module(target_loaded),
                dag_name: target_dag.declaration(target_loaded).name.value.as_str(),
            },
            include,
            declaration,
            loaded_file.interface(),
            file_src,
            StaticScope::new(file_dag_id, module_resolver),
            ctx,
        )?;
    }
    Ok(())
}

fn ensure_include_item_selectable(
    interface: &ModuleInterface,
    name: &NameAtom,
    namespace: ImportItemNamespace,
    file_path: &str,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    match interface.exposure(name, namespace) {
        Some(DeclExposure::ExplicitExport | DeclExposure::InputPort) => Ok(()),
        Some(DeclExposure::Private) => Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src: file_src.clone(),
            span: span.into(),
        })),
        None => Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
            name: name.to_string(),
            file_path: file_path.to_string(),
            src: file_src.clone(),
            span: span.into(),
        })),
    }
}

fn validate_static_import_capability(
    dependency: ModuleDeclarations<'_>,
    name: &NameAtom,
    namespace: ImportItemNamespace,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    match static_import_rejection(dependency, name, namespace) {
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

/// The import category that selects a Static symbol of `kind`.
const fn static_import_namespace(kind: StaticInputKind) -> ImportItemNamespace {
    match kind {
        StaticInputKind::Type => ImportItemNamespace::Type,
        StaticInputKind::Dimension => ImportItemNamespace::Dimension,
        StaticInputKind::Index => ImportItemNamespace::Index,
    }
}

fn validate_qualified_static_import_references(
    consumer: ModuleDeclarations<'_>,
    dependency: ModuleDeclarations<'_>,
    dependency_interface: &ModuleInterface,
    module_name: &ModuleAliasName,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    consumer
        .declarations()
        .iter()
        .flat_map(|declaration| declaration_static_references(&declaration.kind, consumer.scope()))
        .filter_map(|reference| {
            let (owner, leaf) = reference.path().qualifier_and_leaf()?;
            (owner.as_slice() == [module_name.atom().clone()])
                .then(|| (leaf.clone(), static_import_namespace(reference.kind())))
        })
        .try_for_each(|(name, namespace)| {
            if dependency_interface.has_item(&name, namespace) {
                validate_static_import_capability(dependency, &name, namespace, src, span)
            } else {
                Ok(())
            }
        })
}

fn reject_runtime_unit_import(
    dep: &ModuleInterface,
    name: &NameAtom,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let unit_name = graphcal_compiler::syntax::dimension::UnitName::classify(name.clone());
    if dep.runtime_units().contains(&unit_name) {
        return Err(CompileError::Eval(GraphcalError::ImportRuntimeUnit {
            name: name.to_string(),
            src: src.clone(),
            span: span.into(),
        }));
    }
    Ok(())
}

/// Classify the values an include intentionally exposes to its consumer.
///
/// A brace include exposes exactly its selected value aliases. A whole-instance
/// include exposes param ports and explicitly exported consts/nodes under the
/// instance prefix. Private merged declarations remain debug-only and cannot be
/// named by the including DAG.
fn include_surface_outputs(
    interface: &ModuleInterface,
    prefix: &ScopeSegment,
    selective: Option<&[ImportAlias]>,
) -> Vec<ScopedName> {
    selective.map_or_else(
        || {
            interface
                .value_outputs()
                .iter()
                .map(|output| ScopedName::in_scope(prefix.clone(), output.name().clone()))
                .collect()
        },
        |aliases| {
            aliases
                .iter()
                .filter(|alias| {
                    interface
                        .declared_kinds(alias.original.atom(), ImportItemNamespace::Term)
                        .any(is_graph_value_kind)
                })
                .map(|alias| ScopedName::local(alias.local.clone()))
                .collect()
        },
    )
}

/// Validate an include/import item's attributes and return the plot visibility
/// implied by `#[hidden]` (#847). Per-item attributes are intentionally limited:
/// `#[hidden]` is plot-only, and `#[expected_fail]` is assertion-only.
fn validate_include_item_attributes(
    import_item: &graphcal_compiler::desugar::desugared_ast::ImportItem,
    is_plot: bool,
    is_assert: bool,
    file_src: &NamedSource<Arc<String>>,
) -> Result<PlotVisibility, CompileError> {
    let mut visibility = PlotVisibility::Standalone;
    let producer = match (is_plot, is_assert) {
        (true, _) => Some(DeclarationKind::Plot),
        (false, true) => Some(DeclarationKind::Assert),
        (false, false) => None,
    };
    let target = AttributeTarget::include_item(producer, import_item.name.name.atom().clone());
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
                visibility = PlotVisibility::CompositionOnly;
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
    Ok(visibility)
}

fn exported_bindings(
    resolver: &graphcal_compiler::resolve::ModuleResolver,
    owner: &graphcal_compiler::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Vec<graphcal_compiler::resolve::exports::ExportedBinding>, CompileError> {
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
        let typed_name = DeclName::classify(name.clone());
        if !seen.insert((file_dag_id.clone(), typed_name)) {
            return false;
        }
        if file
            .interface()
            .explicitly_exports(name, IntroducedKind::Plot)
        {
            return true;
        }
        file.includes_with_targets().any(|(_, include, target)| {
            let graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) =
                &include.kind
            else {
                return false;
            };
            items.iter().any(|item| {
                item.visibility.is_public()
                    && item.local_name_atom() == name
                    && visit(project, target.source_file(), item.name.name.atom(), seen)
            })
        })
    }

    visit(project, file_dag_id, name, &mut HashSet::new())
}

/// Classified param bindings as authored: each entry routes to one of the
/// four binding maps based on what the dependency declares the binding name
/// as, keyed by the dependency-side name. Index values retain a typed
/// declared-or-structural target. The Static maps serve interface-level
/// validation only; [`resolve_include_static_bindings`] turns them into the
/// canonical bindings an include instance carries.
struct ClassifiedBindings {
    params: HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    indexes: HashMap<IndexName, IndexBindingTarget>,
    index_spans: HashMap<IndexName, Span>,
    types: HashMap<StructTypeName, StructTypeName>,
    dims: HashMap<DimName, DimName>,
}

/// One include's authored Static bindings, keyed by dependency-side name.
struct AuthoredStaticBindings {
    indexes: HashMap<IndexName, IndexBindingTarget>,
    index_spans: HashMap<IndexName, Span>,
    types: HashMap<StructTypeName, StructTypeName>,
    dims: HashMap<DimName, DimName>,
}

/// Resolve one include's authored Static bindings canonically: each port in
/// the template's scope, each target in the importer's (`scope`).
///
/// A dimension target may also name a prelude dimension.
fn resolve_include_static_bindings(
    authored: AuthoredStaticBindings,
    template: &graphcal_compiler::dag_id::DagId,
    scope: StaticScope<'_>,
    src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<IncludeStaticBindings, CompileError> {
    use graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget;
    use graphcal_compiler::resolve::symbols::SymbolRef;
    use graphcal_compiler::syntax::names::NamePath;

    let AuthoredStaticBindings {
        indexes,
        index_spans,
        types,
        dims,
    } = authored;
    let resolver = scope.resolver();
    let missing_port = |kind: &str, port: &dyn std::fmt::Display| {
        CompileError::Eval(GraphcalError::internal_error(
            format!("template {kind} port `{port}` has no canonical identity"),
            src,
            graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::WholeFile,
        ))
    };
    let mut bindings = IncludeStaticBindings::default();
    for (port, authored) in indexes {
        let span = index_spans.get(&port).copied().unwrap_or(include_span);
        let identity = resolver
            .resolve_index_path(template, &NamePath::local(port.atom().clone()))
            .map(SymbolRef::into_resolved)
            .map_err(|_| missing_port("index", &port))?;
        let target = match &authored {
            IndexBindingTarget::Declared(target) => InstanceIndexBindingTarget::Declared(
                resolver
                    .resolve_index_path(scope.owner(), &NamePath::local(target.atom().clone()))
                    .map(SymbolRef::into_resolved)
                    .map_err(|_| {
                        CompileError::Eval(GraphcalError::IndexBindingNotAnIndex {
                            dep_index: port.to_string(),
                            value: authored.to_string(),
                            src: src.clone(),
                            span: span.into(),
                        })
                    })?,
            ),
            IndexBindingTarget::Finite(finite) => InstanceIndexBindingTarget::Finite(*finite),
        };
        bindings
            .substitution
            .indexes
            .insert(identity.clone(), target);
        bindings
            .index_sites
            .insert(identity, IndexBindingSite { authored, span });
    }
    for (port, target) in types {
        let identity = resolver
            .resolve_struct_type_path(template, &NamePath::local(port.atom().clone()))
            .map(SymbolRef::into_resolved)
            .map_err(|_| missing_port("type", &port))?;
        let target = resolver
            .resolve_struct_type_path(scope.owner(), &NamePath::local(target.atom().clone()))
            .map(SymbolRef::into_resolved)
            .map_err(|error| module_resolve_compile_error(error, src))?;
        bindings.substitution.types.insert(identity, target);
    }
    let prelude = graphcal_compiler::resolve::prelude::prelude_type_scope();
    for (port, target) in dims {
        let identity = resolver
            .resolve_dimension_path(template, &NamePath::local(port.atom().clone()))
            .map(SymbolRef::into_resolved)
            .map_err(|_| missing_port("dimension", &port))?;
        let path = NamePath::local(target.atom().clone());
        let target = match resolver
            .resolve_dimension_path(scope.owner(), &path)
            .map(SymbolRef::into_resolved)
        {
            Ok(identity) => identity,
            Err(error) => prelude
                .resolve_dimension_path(&path)
                .ok_or_else(|| module_resolve_compile_error(error, src))?,
        };
        bindings.substitution.dimensions.insert(identity, target);
    }
    Ok(bindings)
}

/// Route each binding through the namespace/category selected by its authored
/// marker. No branch retries another namespace when the selected target is
/// absent or has the wrong capability.
fn classify_param_bindings(
    param_bindings: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dep: &ModuleInterface,
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
        let binding_decl = DeclName::classify(binding_name.atom().clone());
        match binding.category {
            InputBindingCategory::Unmarked
                if dep.declares(binding_name.atom(), IntroducedKind::Param) =>
            {
                out.params.insert(binding_decl, binding.value.clone());
            }
            InputBindingCategory::Type
                if static_input_is_bindable(dep, StaticInputKind::Type, binding_name.atom()) =>
            {
                let rhs_name = extract_type_name_from_binding_expr(
                    &binding.value,
                    binding_name.as_str(),
                    file_src,
                )?;
                out.types.insert(
                    StructTypeName::classify(binding_name.atom().clone()),
                    StructTypeName::expect_valid(rhs_name),
                );
            }
            InputBindingCategory::Dimension
                if static_input_is_bindable(
                    dep,
                    StaticInputKind::Dimension,
                    binding_name.atom(),
                ) =>
            {
                let rhs_name = extract_type_name_from_binding_expr(
                    &binding.value,
                    binding_name.as_str(),
                    file_src,
                )?;
                out.dims.insert(
                    DimName::classify(binding_name.atom().clone()),
                    DimName::expect_valid(rhs_name),
                );
            }
            InputBindingCategory::Index
                if static_input_is_bindable(dep, StaticInputKind::Index, binding_name.atom()) =>
            {
                let dep_name = IndexName::classify(binding_name.atom().clone());
                let target = extract_index_binding_target(&binding.value, &dep_name, file_src)?;
                out.index_spans.insert(dep_name.clone(), binding.value.span);
                out.indexes.insert(dep_name, target);
            }
            InputBindingCategory::Unmarked => {
                if let Some(kind) = non_param_binding_kind(dep, binding_name.atom()) {
                    return Err(CompileError::Eval(GraphcalError::BindingNotAParam {
                        name: binding_name.to_string(),
                        actual_kind: kind,
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
    importer: &ModuleInterface,
    dep: &ModuleInterface,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    index_bindings: &HashMap<IndexName, IndexBindingTarget>,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let invalid_type = type_bindings.iter().find_map(|(input, target)| {
        let source = dep.static_interface(StaticInputKind::Type, input.atom())?;
        let target_interface = importer.static_interface(StaticInputKind::Type, target.atom())?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Type,
            input.to_string(),
            target.to_string(),
        ))
    });
    let invalid_dimension = dim_bindings.iter().find_map(|(input, target)| {
        let source = dep.static_interface(StaticInputKind::Dimension, input.atom())?;
        let target_interface =
            importer.static_interface(StaticInputKind::Dimension, target.atom())?;
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
        let source = dep.static_interface(StaticInputKind::Index, input.atom())?;
        let target_interface = importer.static_interface(StaticInputKind::Index, target.atom())?;
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

/// Record a unit include projection, which may alias a dynamic instance
/// unit. Every Static projection is a resolver binding with a canonical
/// definition and needs nothing here.
fn record_unit_projection(
    import_item: &graphcal_compiler::syntax::ast::ImportItem,
    unit_projection_aliases: &mut Vec<UnitProjectionAlias>,
) {
    if import_item.namespace == ImportItemNamespace::Unit {
        unit_projection_aliases.push(UnitProjectionAlias {
            source: UnitName::classify(import_item.name.name.atom().clone()),
            alias: UnitName::classify(import_item.local_name_atom().clone()),
        });
    }
}

fn validate_required_static_bindings(
    dep: &ModuleInterface,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    index_bindings: &HashMap<IndexName, IndexBindingTarget>,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let mut missing = dep
        .static_declarations()
        .filter(|(kind, name, role)| {
            role.is_required()
                && !match kind {
                    StaticInputKind::Type => {
                        type_bindings.contains_key(&StructTypeName::classify((*name).clone()))
                    }
                    StaticInputKind::Dimension => {
                        dim_bindings.contains_key(&DimName::classify((*name).clone()))
                    }
                    StaticInputKind::Index => {
                        index_bindings.contains_key(&IndexName::classify((*name).clone()))
                    }
                }
        })
        .map(|(kind, name, _)| (kind, name.to_string()))
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
    dependency: &ModuleInterface,
    importer: &ModuleInterface,
    dag_name: &str,
    file_src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), CompileError> {
    let collected = classify_param_bindings(args, dependency, file_src, dag_name)?;
    validate_concrete_static_binding_targets(
        importer,
        dependency,
        &collected.types,
        &collected.dims,
        &collected.indexes,
        file_src,
        span,
    )?;
    validate_required_static_bindings(
        dependency,
        &collected.types,
        &collected.dims,
        &collected.indexes,
        file_src,
        span,
    )?;
    validate_required_param_bindings(dependency, &collected.params, dag_name, file_src, span)
}

fn validate_required_param_bindings(
    dep: &ModuleInterface,
    bindings: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    dag_name: &str,
    file_src: &NamedSource<Arc<String>>,
    include_span: Span,
) -> Result<(), CompileError> {
    let mut missing = dep
        .required_params()
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
pub(super) fn process_file_include<'a>(
    project: &'a crate::loader::LoadedProject,
    target: &crate::loader::ResolvedModuleTarget,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    importer: &ModuleInterface,
    file_src: &NamedSource<Arc<String>>,
    importer_scope: StaticScope<'_>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let module_resolver = importer_scope.resolver();
    let dependency = project.module(target.target()).ok_or_else(|| {
        CompileError::Eval(GraphcalError::InternalError {
            message: format!("included module `{}` is not loaded", target.target()),
            src: file_src.clone(),
            span: include_decl.path.span().into(),
        })
    })?;
    let import_dag_id = dependency.dag_id();
    let dep = dependency.interface();
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
    if let ScopeSegment::Named(prefix) = &instance_scope {
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
                role: graphcal_compiler::resolve::scope::ModuleAliasRole::IncludedInstance,
            },
        );
    }

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
        dep,
        file_src,
        &dep_path_display,
    )?;

    validate_concrete_static_binding_targets(
        importer,
        dep,
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
    let mut requested_plots: HashMap<DeclName, graphcal_compiler::ir::model::RequestedPlot> =
        HashMap::new();
    let mut assertion_aliases = HashMap::new();
    let mut unit_projection_aliases = Vec::new();
    let selective_names = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(names) => {
            validate_include_producers(names, file_src)?;
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let original = DeclName::classify(orig_name.atom().clone());
                let local = DeclName::classify(import_item.local_name_atom().clone());

                ensure_include_item_selectable(
                    dep,
                    orig_name.atom(),
                    import_item.namespace,
                    &include_decl.path.display_path(),
                    file_src,
                    import_item.name.span,
                )?;
                if let Some(binding) = exported_bindings.iter().find(|binding| {
                    &binding.name == orig_name.atom()
                        && binding.target.kind().namespace() == import_item.namespace
                }) {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
                record_unit_projection(import_item, &mut unit_projection_aliases);
                let is_plot = is_term_namespace
                    && (dep.declares(orig_name.atom(), IntroducedKind::Plot)
                        || file_exports_plot(project, import_dag_id, orig_name.atom()));
                let is_assert =
                    is_term_namespace && dep.declares(orig_name.atom(), IntroducedKind::Assert);
                let is_graph_value = is_term_namespace
                    && (dep.declares(orig_name.atom(), IntroducedKind::ConstNode)
                        || declares_runtime_value(dep, orig_name.atom()));
                if is_graph_value {
                    validate_reserved_alias(Namespace::Term, import_item, file_src)?;
                }
                let visibility =
                    validate_include_item_attributes(import_item, is_plot, is_assert, file_src)?;
                if is_plot {
                    // The requested plot merges into the root namespace under
                    // its local alias, evaluating against this instance (#847).
                    requested_plots.insert(
                        original,
                        graphcal_compiler::ir::model::RequestedPlot {
                            alias: local.clone(),
                            visibility,
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
                    dep.declares(orig_name.atom(), IntroducedKind::ConstNode),
                    declares_runtime_value(dep, orig_name.atom()),
                ) {
                    (true, _) => ctx.imported_names.const_names.push((scoped, span)),
                    (false, true) => ctx.imported_names.param_names.push((scoped, span)),
                    (false, false) => {}
                }

                selective.push(ImportAlias { original, local });
            }
            Some(selective)
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
            let module_alias = include_decl.module_form_alias(alias.as_ref());
            // Register all dep names under the prefix for scope checking.
            let import_span = include_decl.path.span();
            for output in dep.value_outputs() {
                let scoped = ScopedName::in_scope(module_alias.clone(), output.name().clone());
                if output.is_const() {
                    ctx.imported_names.const_names.push((scoped, import_span));
                } else {
                    ctx.imported_names.param_names.push((scoped, import_span));
                }
            }
            None
        }
    };
    let surface_outputs = include_surface_outputs(dep, &instance_scope, selective_names.as_deref());

    validate_required_static_bindings(
        dep,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;
    validate_required_param_bindings(dep, &bindings, &dep_path_display, file_src, decl.span)?;
    let static_bindings = resolve_include_static_bindings(
        AuthoredStaticBindings {
            indexes: index_bindings,
            index_spans: index_binding_spans,
            types: type_bindings,
            dims: dim_bindings,
        },
        import_dag_id,
        importer_scope,
        file_src,
        decl.span,
    )?;

    let pub_reexport_items: HashSet<NameAtom> = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.visibility.is_public())
            .map(|it| it.name.name.atom().clone())
            .collect(),
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.include_instances.push(IncludeInstanceRequest {
        template: dependency,
        instance_scope,
        debug_scope: derive_module_name_from_import_path(&include_decl.path),
        bindings,
        static_bindings,
        selective_names,
        unit_projection_aliases,
        runtime_unit_names: dep.runtime_units().clone(),
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
pub(super) fn process_inline_dag_include<'a>(
    target: &InlineDagIncludeTarget<'a>,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    importer: &ModuleInterface,
    file_src: &NamedSource<Arc<String>>,
    importer_scope: StaticScope<'_>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    use graphcal_compiler::desugar::desugared_ast::ImportKind;

    let module_resolver = importer_scope.resolver();

    let dep = target.module.interface();
    let dag_name = target.dag_name;
    let dag_id = target.module.dag_id();

    // As for file-root includes, only the module form introduces an alias.
    // Selective inline-DAG includes receive an opaque private merge scope.
    let instance_scope = include_decl.instance_scope();
    if let ScopeSegment::Named(prefix) = &instance_scope {
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
                role: graphcal_compiler::resolve::scope::ModuleAliasRole::IncludedInstance,
            },
        );
    }

    let exported_bindings =
        exported_bindings(module_resolver, dag_id, file_src, include_decl.path.span())?;

    // Classify bindings against the DAG body's declarations. Typed index
    // compatibility is deferred to the same registry-backed path as file DAGs.
    let ClassifiedBindings {
        params: bindings,
        indexes: index_bindings,
        index_spans: index_binding_spans,
        types: type_bindings,
        dims: dim_bindings,
    } = classify_param_bindings(&include_decl.param_bindings, dep, file_src, dag_name)?;
    validate_concrete_static_binding_targets(
        importer,
        dep,
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
    let mut requested_plots: HashMap<DeclName, graphcal_compiler::ir::model::RequestedPlot> =
        HashMap::new();
    let mut assertion_aliases = HashMap::new();
    let mut unit_projection_aliases = Vec::new();
    let selective_names = match &include_decl.kind {
        ImportKind::Selective(names) => {
            validate_include_producers(names, file_src)?;
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let original = DeclName::classify(orig_name.atom().clone());
                let local = DeclName::classify(import_item.local_name_atom().clone());

                ensure_include_item_selectable(
                    dep,
                    orig_name.atom(),
                    import_item.namespace,
                    dag_name,
                    file_src,
                    import_item.name.span,
                )?;
                if let Some(binding) = exported_bindings.iter().find(|binding| {
                    &binding.name == orig_name.atom()
                        && binding.target.kind().namespace() == import_item.namespace
                }) {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }

                let is_term_namespace = import_item.namespace
                    == graphcal_compiler::syntax::ast::ImportItemNamespace::Term;
                record_unit_projection(import_item, &mut unit_projection_aliases);
                let is_plot =
                    is_term_namespace && dep.declares(orig_name.atom(), IntroducedKind::Plot);
                let is_assert =
                    is_term_namespace && dep.declares(orig_name.atom(), IntroducedKind::Assert);
                let is_graph_value = is_term_namespace
                    && dep
                        .declared_kinds(orig_name.atom(), ImportItemNamespace::Term)
                        .any(is_graph_value_kind);
                if is_graph_value {
                    validate_reserved_alias(Namespace::Term, import_item, file_src)?;
                }
                let visibility =
                    validate_include_item_attributes(import_item, is_plot, is_assert, file_src)?;
                if is_plot {
                    requested_plots.insert(
                        original,
                        graphcal_compiler::ir::model::RequestedPlot {
                            alias: local.clone(),
                            visibility,
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
                let is_const = dep.declares(orig_name.atom(), IntroducedKind::ConstNode);
                let is_runtime = declares_runtime_value(dep, orig_name.atom());
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
            for output in dep.value_outputs() {
                let scoped = ScopedName::in_scope(instance_scope.clone(), output.name().clone());
                if output.is_const() {
                    ctx.imported_names.const_names.push((scoped, import_span));
                } else {
                    ctx.imported_names.param_names.push((scoped, import_span));
                }
            }
            None
        }
    };
    let surface_outputs = include_surface_outputs(dep, &instance_scope, selective_names.as_deref());

    validate_required_static_bindings(
        dep,
        &type_bindings,
        &dim_bindings,
        &index_bindings,
        file_src,
        include_decl.path.span(),
    )?;
    validate_required_param_bindings(dep, &bindings, dag_name, file_src, decl.span)?;
    let static_bindings = resolve_include_static_bindings(
        AuthoredStaticBindings {
            indexes: index_bindings,
            index_spans: index_binding_spans,
            types: type_bindings,
            dims: dim_bindings,
        },
        dag_id,
        importer_scope,
        file_src,
        decl.span,
    )?;

    let pub_reexport_items: HashSet<NameAtom> = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|it| it.visibility.is_public())
            .map(|it| it.name.name.atom().clone())
            .collect(),
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => HashSet::new(),
    };

    ctx.include_instances.push(IncludeInstanceRequest {
        template: target.module,
        instance_scope,
        debug_scope: ModuleAliasName::expect_valid(dag_name),
        bindings,
        static_bindings,
        selective_names,
        unit_projection_aliases,
        runtime_unit_names: dep.runtime_units().clone(),
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
    reason = "visibility and capability checks consume the complete import context in one boundary pass"
)]
pub(super) fn process_pure_import<'a>(
    project: &'a crate::loader::LoadedProject,
    resolved_module: &crate::loader::ResolvedModuleTarget,
    import: &graphcal_compiler::desugar::desugared_ast::ImportDecl,
    importer: ModuleDeclarations<'_>,
    file_src: &NamedSource<Arc<String>>,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let import_path = import.path();
    let module_target = resolved_module.target();
    let dep_module = project.module(module_target).ok_or_else(|| {
        CompileError::Eval(GraphcalError::InternalError {
            message: format!("inline module `{module_target}` has no owning declaration"),
            src: file_src.clone(),
            span: import_path.span().into(),
        })
    })?;
    let declarations = dep_module.declarations();
    let dependency = ModuleDeclarations::new(
        declarations,
        StaticScope::new(module_target, module_resolver),
    );
    let dep_interface = dep_module.interface();
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

    match import {
        graphcal_compiler::desugar::desugared_ast::ImportDecl::Selective {
            items: names, ..
        } => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = DeclName::classify(import_item.local_name_atom().clone());

                let resolved_export = exported_bindings.iter().find(|binding| {
                    &binding.name == orig_name.atom()
                        && binding.target.kind().namespace() == import_item.namespace
                });
                if let Some(binding) = resolved_export {
                    validate_constructor_alias(binding.target.kind(), import_item, file_src)?;
                }
                // Boundary check: resolver exports include transitive public
                // re-exports, while the source scan preserves input-port
                // semantics for directly authored params.
                if resolved_export.is_none()
                    && !dep_interface.exposes_item(orig_name.atom(), import_item.namespace)
                {
                    if dep_interface.has_item(orig_name.atom(), import_item.namespace) {
                        return Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
                            name: orig_name.to_string(),
                            file_path: import_path.display_path(),
                            src: file_src.clone(),
                            span: import_item.name.span.into(),
                        }));
                    }
                    return Err(CompileError::Eval(import_item_not_found_error(
                        dep_interface,
                        orig_name.atom(),
                        import_item.namespace,
                        &import_path.display_path(),
                        file_src,
                        import_item.name.span,
                    )));
                }

                validate_static_import_capability(
                    dependency,
                    orig_name.atom(),
                    import_item.namespace,
                    file_src,
                    import_item.name.span,
                )?;

                if import_item.namespace != ImportItemNamespace::Term {
                    validate_reserved_alias(
                        Namespace::of(import_item.namespace),
                        import_item,
                        file_src,
                    )?;
                    if import_item.namespace == ImportItemNamespace::Unit {
                        reject_runtime_unit_import(
                            dep_interface,
                            orig_name.atom(),
                            file_src,
                            import_item.name.span,
                        )?;
                    }
                    // Static and unit items resolve canonically through the
                    // module resolver; nothing is registered here.
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
                        // A Term item never resolves to a Static or Unit export.
                        ExportedImportItemKind::Dimension
                        | ExportedImportItemKind::Unit(_)
                        | ExportedImportItemKind::Type
                        | ExportedImportItemKind::Index => None,
                    })
                    .or_else(|| dep_interface.pure_import_term_disposition(orig_name.atom()))
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
                        validate_reserved_alias(Namespace::Term, import_item, file_src)?;
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
                    PureImportTermDisposition::ResolverOnly => {}
                    PureImportTermDisposition::Reject(reason) => {
                        return Err(CompileError::Eval(reason.diagnostic(
                            orig_name.atom(),
                            file_src,
                            import_item.name.span,
                        )));
                    }
                }
            }
        }
        graphcal_compiler::desugar::desugared_ast::ImportDecl::Module { alias, .. } => {
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
                importer,
                dependency,
                dep_interface,
                &module_name,
                file_src,
                import_path.span(),
            )?;

            let role = graphcal_compiler::resolve::scope::ModuleAliasRole::ImportedDag;
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
        }
    }

    Ok(())
}

fn insert_imported_binding(
    imported_bindings: &mut HashMap<ScopedName, ResolvedDeclName>,
    imported_names: &ImportedValueNames,
    lexical_name: ScopedName,
    binding: ResolvedDeclName,
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
pub(super) fn import_selective_item(
    source_owner: &graphcal_compiler::dag_id::DagId,
    orig_name: &NameAtom,
    local_name: &DeclName,
    span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, ResolvedDeclName>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    import_selective_resolved_item(
        graphcal_compiler::resolved_name::ResolvedDeclName::for_test(
            source_owner.clone(),
            DeclName::classify(orig_name.clone()),
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
    canonical: graphcal_compiler::resolved_name::ResolvedDeclName,
    local_name: &DeclName,
    span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, ResolvedDeclName>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), CompileError> {
    let scoped = ScopedName::local(local_name.clone());
    imported_names.const_names.push((scoped.clone(), span));
    if let Some(source_order) = imported_source_order {
        source_order.push((
            scoped.clone(),
            DeclCategory::Value(ValueDeclCategory::Const),
        ));
    }
    insert_imported_binding(
        imported_bindings,
        imported_names,
        scoped,
        canonical,
        src,
        span,
    )
}

/// Import all resolver-visible exported constants under a module prefix.
fn import_module_values_from_resolver(
    exported_bindings: &[graphcal_compiler::resolve::exports::ExportedBinding],
    module_name: &ModuleAliasName,
    import_span: Span,
    src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut HashMap<ScopedName, ResolvedDeclName>,
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
        let local = DeclName::classify(binding.name.clone());
        let scoped = ScopedName::in_scope(module_name.clone(), local);
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        if let Some(source_order) = imported_source_order.as_deref_mut() {
            source_order.push((
                scoped.clone(),
                DeclCategory::Value(ValueDeclCategory::Const),
            ));
        }
        insert_imported_binding(
            imported_bindings,
            imported_names,
            scoped,
            canonical.clone(),
            src,
            import_span,
        )?;
    }
    Ok(())
}

/// Derive the source-facing module alias from a module path leaf.
pub(super) fn derive_module_name_from_import_path(import_path: &ModulePath) -> ModuleAliasName {
    ModuleAliasName::classify(import_path.leaf().name.atom().clone())
}

#[cfg(test)]
mod tests;
