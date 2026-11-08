//! Import processing functions for project-based compilation.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "project compiler pass uses the shared internal model"
)]
use graphcal_compiler::syntax::span::Spanned;
use std::collections::{HashMap, HashSet};

use graphcal_compiler::declaration_category::{DeclCategory, ValueDeclCategory};
use graphcal_compiler::desugar::desugared_ast::ModulePath;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::graph::{DagReference, GraphError};
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::{DuplicateDeclaration, NameError};
use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::DimName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::type_name::StructTypeName;

use super::binding_values::{extract_index_binding_target, extract_type_name_from_binding_expr};
use super::including_module::IncludingModule;
use crate::compile_error::PipelineError;
use crate::loader::loaded_file::LoadedModule;

use super::include_static_bindings::{
    CoveredStaticBindings, StaticBindingResolution, validate_required_static_bindings,
};
use super::model::{
    ImportAlias, ImportContext, IncludeInstanceRequest, ProjectModuleBinding, ReexportedItem,
    SelectedTypeAlias, UnitProjectionAlias,
};
use super::module_resolve_errors::{module_resolve_compile_error, module_resolve_compile_error_at};
use super::selected_include_items::{SelectedItemKind, SelectedValueKind, exported_values};
use crate::import_surface::import_item_not_found_error;
use graphcal_compiler::declaration_kind::DeclarationKind;
use graphcal_compiler::desugar::desugared_ast::DeclKind;
use graphcal_compiler::ir::module_interface::{
    ModuleInterface, PureImportRejection, PureImportTermDisposition,
};
use graphcal_compiler::ir::resolve::attribute_validation::{
    IncludeItemAttributeRole, IncludeItemSite,
};
use graphcal_compiler::ir::static_dependencies::{
    ModuleDeclarations, StaticImportRejection, StaticScope, declaration_static_references,
    static_import_rejection,
};
use graphcal_compiler::plot_visibility::PlotVisibility;
use graphcal_compiler::resolve::category::DeclSymbolKind;
use graphcal_compiler::resolve::exports::{ExportedBinding, ExportedBindingTarget};
use graphcal_compiler::static_interface::{
    StaticInputKind, StaticInterface, StaticRole, static_binding_valid,
};
use graphcal_compiler::syntax::ast::{DeclExposure, ImportItemNamespace, IntroducedKind};
use graphcal_compiler::syntax::dimension::UnitName;
use graphcal_compiler::syntax::names::NameAtom;

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

/// Populate one file body's pure imports and concrete include requests.
///
/// Both top-level file compilation and recursive file-DAG instantiation use
/// this path. Keeping import classification here prevents nested instances
/// from silently dropping their own include graph.
pub(super) fn process_file_body_declarations<'a>(
    project: &'a crate::loader::loaded_project::LoadedProject,
    loaded_file: &crate::loader::loaded_file::LoadedFile,
    modules: &'a crate::loader::loaded_module_resolver::LoadedModuleResolver,
    ctx: &mut ImportContext<'a>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), Outcome<PipelineError>> {
    let module_resolver = modules.resolver();
    let file_dag_id = loaded_file.dag_id();
    let file_src = loaded_file.source_id();
    let including = IncludingModule {
        interface: loaded_file.interface(),
        source: file_src,
        scope: StaticScope::new(file_dag_id, module_resolver),
    };

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
            modules,
            ctx,
        )?;
    }

    for (declaration, include, target) in loaded_file.includes_with_targets() {
        cancellation.checkpoint()?;
        let dependency = project.target_module(target);
        if !matches!(dependency, LoadedModule::FileRoot(_)) {
            continue;
        }
        process_include(dependency, include, declaration, &including, ctx)?;
    }

    // A project load resolves or rejects every file-root include, including
    // one naming a top-level `dag` of this file. Only a single in-memory
    // buffer leaves a cross-file path without a target.
    if project.project_root().is_some()
        && let Some(include) = loaded_file
            .ast()
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.kind {
                DeclKind::Include(include) if !loaded_file.has_resolved_target(&include.path) => {
                    Some(include)
                }
                _ => None,
            })
    {
        return Err(PipelineError::Semantic(SemanticError::internal_error(
            format!(
                "the loader left the include of `{}` without a target",
                include.path.key()
            ),
            file_src,
            graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(include.path.span()),
        ))
        .into());
    }

    for (declaration, include, target) in loaded_file.includes_with_targets() {
        cancellation.checkpoint()?;
        let dependency = project.target_module(target);
        let LoadedModule::InlineDag {
            file: target_loaded,
            dag: target_dag,
        } = dependency
        else {
            continue;
        };
        if !target_dag.declaration(target_loaded).visibility.is_public()
            && target.source_file() != file_dag_id
        {
            return Err(PipelineError::Semantic(SemanticError::located(
                file_src,
                include.path.leaf().span,
                VisibilityError::ImportPrivateItem {
                    name: include.path.leaf().name.atom().clone(),
                    file_path: DagReference::Path(include.path.clone()),
                },
            ))
            .into());
        }
        process_include(dependency, include, declaration, &including, ctx)?;
    }
    Ok(())
}

fn ensure_include_item_selectable(
    interface: &ModuleInterface,
    name: &NameAtom,
    namespace: ImportItemNamespace,
    file_path: &DagReference,
    file_src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
    match interface.exposure(name, namespace) {
        Some(DeclExposure::ExplicitExport | DeclExposure::InputPort) => Ok(()),
        Some(DeclExposure::Private) => Err(PipelineError::Semantic(SemanticError::located(
            file_src,
            span,
            VisibilityError::ImportPrivateItem {
                name: name.clone(),
                file_path: file_path.clone(),
            },
        ))),
        None => Err(PipelineError::Semantic(SemanticError::located(
            file_src,
            span,
            ModuleError::ImportNameNotFound {
                name: name.clone(),
                file_path: file_path.clone(),
            },
        ))),
    }
}

fn validate_static_import_capability(
    dependency: ModuleDeclarations<'_>,
    name: &NameAtom,
    namespace: ImportItemNamespace,
    src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
    match static_import_rejection(dependency, name, namespace) {
        None => Ok(()),
        Some(StaticImportRejection::RequiredInput { kind, name }) => {
            Err(PipelineError::Semantic(SemanticError::located(
                src,
                span,
                ModuleError::ImportRequiredStaticInput { kind, name },
            )))
        }
        Some(StaticImportRejection::UnresolvedDependency(dependency)) => {
            Err(PipelineError::Semantic(SemanticError::located(
                src,
                span,
                ModuleError::ImportUnresolvedStaticDependency {
                    name: name.clone(),
                    dependency_kind: dependency.kind(),
                    dependency: dependency.name().clone(),
                },
            )))
        }
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
    src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
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
    src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
    let unit_name = graphcal_compiler::syntax::dimension::UnitName::classify(name.clone());
    if dep.runtime_units().contains(&unit_name) {
        return Err(PipelineError::Semantic(SemanticError::located(
            src,
            span,
            ModuleError::ImportRuntimeUnit {
                name: graphcal_compiler::syntax::dimension::UnitRef::local(unit_name),
            },
        )));
    }
    Ok(())
}

/// The graph values a whole-instance include exposes under its prefix: the
/// dependency's `param` input ports and exported values in source order,
/// then the values it re-exports from its own includes.
///
/// Private merged declarations remain debug-only and cannot be named by the
/// including DAG.
fn module_value_outputs(
    interface: &ModuleInterface,
    exports: &[ExportedBinding],
    template: &graphcal_compiler::dag_id::DagId,
) -> Vec<(DeclName, SelectedValueKind)> {
    let declared = interface
        .value_outputs()
        .iter()
        .map(|output| {
            let kind = if output.is_const() {
                SelectedValueKind::Const
            } else {
                SelectedValueKind::Runtime
            };
            (output.name().clone(), kind)
        })
        .collect::<Vec<_>>();
    let reexported = exported_values(exports, template)
        .filter(|(name, _)| !declared.iter().any(|(declared, _)| declared == name))
        .collect::<Vec<_>>();
    declared.into_iter().chain(reexported).collect()
}

/// Validate an include/import item's attributes and return the plot visibility
/// implied by `#[hidden]` (#847). Per-item attributes are intentionally limited:
/// `#[hidden]` is plot-only, and `#[expected_fail]` is assertion-only.
fn validate_include_item_attributes(
    import_item: &graphcal_compiler::desugar::desugared_ast::ImportItem,
    is_plot: bool,
    is_assert: bool,
    file_src: SourceId,
) -> Result<PlotVisibility, PipelineError> {
    let mut visibility = PlotVisibility::Standalone;
    let producer = match (is_plot, is_assert) {
        (true, _) => Some(DeclarationKind::Plot),
        (false, true) => Some(DeclarationKind::Assert),
        (false, false) => None,
    };
    let site = IncludeItemSite::new(producer, import_item.name.name.atom().clone());
    let attributes =
        graphcal_compiler::ir::resolve::attribute_validation::validate_attributes(
            &import_item.attributes,
            &site,
        )
        .map_err(|error| {
            PipelineError::Semantic(
                graphcal_compiler::ir::resolve::attribute_validation::attribute_validation_error_to_graphcal(
                    error,
                    file_src,
                ),
            )
        })?;
    for validated in attributes {
        match validated.role() {
            IncludeItemAttributeRole::Hidden => {
                // Shared structural validation guarantees no arguments.
                visibility = PlotVisibility::CompositionOnly;
            }
            IncludeItemAttributeRole::ExpectedFail => {}
        }
    }
    Ok(visibility)
}

fn validate_include_producers(
    items: &[graphcal_compiler::desugar::desugared_ast::ImportItem],
    file_src: SourceId,
) -> Result<(), PipelineError> {
    graphcal_compiler::ir::resolve::include_selection::validate_unique_include_producers(items)
        .map_err(|error| {
            PipelineError::Semantic(
                graphcal_compiler::ir::resolve::include_selection::duplicate_include_producer_to_graphcal(
                    error,
                    file_src,
                ),
            )
        })
}

/// Classified param bindings as authored: each entry routes to one of the
/// four binding maps based on what the dependency declares the binding name
/// as, keyed by the dependency-side name. Index values retain a typed
/// declared-or-structural target. The Static maps serve interface-level
/// validation only; [`CoveredStaticBindings::resolve`] turns them into the
/// canonical bindings an include instance carries.
struct ClassifiedBindings {
    params: HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    indexes: HashMap<IndexName, IndexBindingTarget>,
    index_spans: HashMap<IndexName, Span>,
    types: HashMap<StructTypeName, StructTypeName>,
    dims: HashMap<DimName, DimName>,
}

/// The Static binding resolution of one include site: each port in the
/// template's scope, each target in the importer's.
struct IncludeSite<'a> {
    template: &'a graphcal_compiler::dag_id::DagId,
    importer: StaticScope<'a>,
    src: SourceId,
}

impl IncludeSite<'_> {
    /// The canonical identity of the Static port `port` the template
    /// declares.
    fn port<Ns: graphcal_compiler::resolve::tables::NamespaceTables>(
        &self,
        port: &graphcal_compiler::syntax::names::NameDef<Ns>,
    ) -> Result<graphcal_compiler::resolved_name::ResolvedName<Ns>, PipelineError> {
        self.importer
            .resolver()
            .declaration(self.template, port)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map_err(|error| {
                PipelineError::Semantic(SemanticError::internal_error(
                    format!("template port `{port}` has no canonical identity: {error}"),
                    self.src,
                    graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::WholeFile,
                ))
            })
    }
}

impl StaticBindingResolution for IncludeSite<'_> {
    fn index(
        &self,
        port: &IndexName,
        authored: &IndexBindingTarget,
        span: Span,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedIndexName,
            graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget,
        ),
        PipelineError,
    > {
        use graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget;

        let identity = self.port(port)?;
        let target = match authored {
            IndexBindingTarget::Declared(target) => InstanceIndexBindingTarget::Declared(
                self.importer
                    .resolver()
                    .resolve_index_path(
                        self.importer.owner(),
                        &graphcal_compiler::syntax::names::NamePath::local(target.atom().clone()),
                    )
                    .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|_| {
                        PipelineError::Semantic(SemanticError::located(
                            self.src,
                            span,
                            ModuleError::IndexBindingNotAnIndex {
                                dep_index: port.clone(),
                                value: authored.clone(),
                            },
                        ))
                    })?,
            ),
            IndexBindingTarget::Finite(finite) => InstanceIndexBindingTarget::Finite(*finite),
        };
        Ok((identity, target))
    }

    fn struct_type(
        &self,
        port: &StructTypeName,
        target: &StructTypeName,
        span: Span,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedStructTypeName,
            graphcal_compiler::resolved_name::ResolvedStructTypeName,
        ),
        PipelineError,
    > {
        let identity = self.port(port)?;
        let target = self
            .importer
            .resolver()
            .resolve_struct_type_path(
                self.importer.owner(),
                &graphcal_compiler::syntax::names::NamePath::local(target.atom().clone()),
            )
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map_err(|error| module_resolve_compile_error_at(error, self.src, span))?;
        Ok((identity, target))
    }

    /// A dimension target may also name a prelude dimension.
    fn dimension(
        &self,
        port: &DimName,
        target: &DimName,
        span: Span,
    ) -> Result<
        (
            graphcal_compiler::resolved_name::ResolvedDimName,
            graphcal_compiler::resolved_name::ResolvedDimName,
        ),
        PipelineError,
    > {
        let identity = self.port(port)?;
        let path = graphcal_compiler::syntax::names::NamePath::local(target.atom().clone());
        let target = match self
            .importer
            .resolver()
            .resolve_dimension_path(self.importer.owner(), &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
        {
            Ok(identity) => identity,
            Err(error) => graphcal_compiler::resolve::prelude::prelude_type_scope()
                .resolve_dimension_path(&path)
                .ok_or_else(|| module_resolve_compile_error_at(error, self.src, span))?,
        };
        Ok((identity, target))
    }
}

/// Route each binding through the namespace/category selected by its authored
/// marker. No branch retries another namespace when the selected target is
/// absent or has the wrong capability.
fn classify_param_bindings(
    param_bindings: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dep: &ModuleInterface,
    file_src: SourceId,
    dep_path_for_error: &DagReference,
) -> Result<ClassifiedBindings, PipelineError> {
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
                    binding_name.atom(),
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
                    binding_name.atom(),
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
                    return Err(PipelineError::Semantic(SemanticError::located(
                        file_src,
                        binding.name.span,
                        ModuleError::BindingNotAParam {
                            name: binding_name.atom().clone(),
                            actual_kind: kind,
                        },
                    )));
                }
                return Err(PipelineError::Semantic(SemanticError::located(
                    file_src,
                    binding.name.span,
                    ModuleError::UnknownParamBinding {
                        name: binding_name.atom().clone(),
                        file_path: dep_path_for_error.clone(),
                    },
                )));
            }
            category => {
                return Err(PipelineError::Semantic(SemanticError::located(
                    file_src,
                    binding.name.span,
                    ModuleError::DagInputCategoryMismatch {
                        name: binding_name.atom().clone(),
                        expected: match category {
                            InputBindingCategory::Unmarked => "param",
                            InputBindingCategory::Type => "type",
                            InputBindingCategory::Dimension => "dim",
                            InputBindingCategory::Index => "index",
                        },
                    },
                )));
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
    file_src: SourceId,
    include_span: Span,
) -> Result<(), PipelineError> {
    let invalid_type = type_bindings.iter().find_map(|(input, target)| {
        let source = dep.static_interface(StaticInputKind::Type, input.atom())?;
        let target_interface = importer.static_interface(StaticInputKind::Type, target.atom())?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Type,
            input.atom().clone(),
            target.atom().clone(),
        ))
    });
    let invalid_dimension = dim_bindings.iter().find_map(|(input, target)| {
        let source = dep.static_interface(StaticInputKind::Dimension, input.atom())?;
        let target_interface =
            importer.static_interface(StaticInputKind::Dimension, target.atom())?;
        (!static_binding_composition_valid(source, target_interface)).then_some((
            StaticInputKind::Dimension,
            input.atom().clone(),
            target.atom().clone(),
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
            input.atom().clone(),
            target.atom().clone(),
        ))
    });
    match invalid_type.or(invalid_dimension).or(invalid_index) {
        None => Ok(()),
        Some((kind, name, target)) => Err(PipelineError::Semantic(SemanticError::located(
            file_src,
            include_span,
            ModuleError::InvalidStaticBindingTarget { kind, name, target },
        ))),
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

pub(super) fn validate_direct_dag_call_bindings(
    args: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    dependency: &ModuleInterface,
    importer: &ModuleInterface,
    dag_name: &DagReference,
    file_src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
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
    dag_name: &DagReference,
    file_src: SourceId,
    include_span: Span,
) -> Result<(), PipelineError> {
    let mut missing = dep
        .required_params()
        .iter()
        .filter(|name| !bindings.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    missing.sort_by_key(ToString::to_string);
    Err(PipelineError::Semantic(SemanticError::located(
        file_src,
        include_span,
        GraphError::MissingDagBindings {
            missing,
            dag_name: dag_name.clone(),
        },
    )))
}

/// Process one include of a file-root or inline-DAG template, deferring its
/// concrete instance for post-lowering IR merging.
///
/// Per Concept 9, inline DAGs are strictly isolated: every name a DAG uses
/// must come from its own declarations or its own `import` statements. There
/// is no parent-scope inheritance, so file-root, same-file inline, and
/// cross-file inline templates are handled identically here.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation and scope registration form a single cohesive pipeline over one include context"
)]
pub(super) fn process_include<'a>(
    dependency: LoadedModule<'a>,
    include_decl: &graphcal_compiler::desugar::desugared_ast::IncludeDecl,
    decl: &graphcal_compiler::desugar::desugared_ast::Declaration,
    including: &IncludingModule<'_>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    let IncludingModule {
        interface: importer,
        source: file_src,
        scope: importer_scope,
    } = *including;
    let import_dag_id = dependency.dag_id();
    let dep = dependency.interface();
    let dep_path_display = match dependency {
        LoadedModule::FileRoot(_) => DagReference::Path(include_decl.path.clone()),
        LoadedModule::InlineDag { file, dag } => {
            DagReference::InlineDag(dag.declaration(file).name.value.clone())
        }
    };

    // A module-form include introduces a source-visible alias and therefore
    // participates in duplicate-alias checks. A selective include introduces
    // only its selected local declarations, so give its merged implementation
    // an opaque private instance scope instead (#1132).
    let instance_scope = include_decl.instance_scope();
    if let ScopeSegment::Named(prefix) = &instance_scope {
        if let Some(first) = ctx.module_map.get(prefix) {
            return Err(PipelineError::Semantic(SemanticError::located(
                file_src,
                include_decl.path.span(),
                ModuleError::DuplicateModuleName {
                    name: prefix.clone(),
                    first: first.span(),
                },
            )));
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
    let exports = importer_scope
        .resolver()
        .exported_bindings(import_dag_id)
        .map_err(|error| module_resolve_compile_error(error, file_src))?;
    let mut surface_outputs = Vec::new();
    let mut selected_types = Vec::new();
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
                    &dep_path_display,
                    file_src,
                    import_item.name.span,
                )?;
                let selected = SelectedItemKind::classify(&exports, import_dag_id, import_item)
                    .ok_or_else(|| {
                        PipelineError::Semantic(SemanticError::located(
                            file_src,
                            import_item.name.span,
                            ModuleError::ImportNameNotFound {
                                name: orig_name.atom().clone(),
                                file_path: dep_path_display.clone(),
                            },
                        ))
                    })?;

                if matches!(selected, SelectedItemKind::Unit(_)) {
                    record_unit_projection(import_item, &mut unit_projection_aliases);
                }
                let visibility = validate_include_item_attributes(
                    import_item,
                    selected == SelectedItemKind::Plot,
                    selected == SelectedItemKind::Assertion,
                    file_src,
                )?;
                if selected == SelectedItemKind::Plot {
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
                let scoped = ScopedName::local(local.clone());
                let span = import_item.local_span();
                match selected {
                    SelectedItemKind::Assertion => {
                        ctx.imported_names.assert_names.push((local.clone(), span));
                        assertion_aliases.insert(original.clone(), local.clone());
                    }
                    SelectedItemKind::Value(kind) => {
                        surface_outputs.push(scoped.clone());
                        match kind {
                            SelectedValueKind::Const => {
                                ctx.imported_names.const_names.push((scoped, span));
                            }
                            SelectedValueKind::Runtime => {
                                ctx.imported_names.param_names.push((scoped, span));
                            }
                        }
                        selective.push(ImportAlias {
                            original,
                            local,
                            visibility: import_item.visibility,
                        });
                    }
                    SelectedItemKind::Type => selected_types.push(SelectedTypeAlias {
                        original: StructTypeName::classify(orig_name.atom().clone()),
                        local: StructTypeName::classify(import_item.local_name_atom().clone()),
                    }),
                    SelectedItemKind::Plot
                    | SelectedItemKind::ImportedConstant
                    | SelectedItemKind::Composition
                    | SelectedItemKind::Constructor
                    | SelectedItemKind::Static
                    | SelectedItemKind::Unit(_) => {}
                }
            }
            Some(selective)
        }
        graphcal_compiler::desugar::desugared_ast::ImportKind::Module { .. } => {
            // Register all dep names under the prefix for scope checking.
            let import_span = include_decl.path.span();
            for (name, kind) in module_value_outputs(dep, &exports, import_dag_id) {
                let scoped = ScopedName::in_scope(instance_scope.clone(), name);
                surface_outputs.push(scoped.clone());
                match kind {
                    SelectedValueKind::Const => {
                        ctx.imported_names.const_names.push((scoped, import_span));
                    }
                    SelectedValueKind::Runtime => {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            None
        }
    };

    let covered_static_bindings = CoveredStaticBindings::check(
        dep,
        index_bindings,
        index_binding_spans,
        type_bindings,
        dim_bindings,
        file_src,
        include_decl.path.span(),
    )?;
    validate_required_param_bindings(dep, &bindings, &dep_path_display, file_src, decl.span)?;
    let static_bindings = covered_static_bindings.resolve(
        &IncludeSite {
            template: import_dag_id,
            importer: importer_scope,
            src: file_src,
        },
        decl.span,
    )?;

    let pub_reexport_items: HashSet<ReexportedItem> = match &include_decl.kind {
        graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => items
            .iter()
            .filter(|item| item.visibility.is_public())
            .map(|item| ReexportedItem {
                namespace: item.namespace,
                name: item.name.name.atom().clone(),
            })
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
        selected_types,
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
    project: &'a crate::loader::loaded_project::LoadedProject,
    resolved_module: &crate::loader::module_path::ResolvedModuleTarget,
    import: &graphcal_compiler::desugar::desugared_ast::ImportDecl,
    importer: ModuleDeclarations<'_>,
    file_src: SourceId,
    modules: &crate::loader::loaded_module_resolver::LoadedModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    let module_resolver = modules.resolver();
    let import_path = import.path();
    let module_target = resolved_module.target();
    let dep_module = project.target_module(resolved_module);
    let declarations = dep_module.declarations();
    let dependency = ModuleDeclarations::new(
        declarations,
        StaticScope::new(module_target, module_resolver),
    );
    let dep_interface = dep_module.interface();
    let exported_bindings = modules.target(resolved_module).exported_bindings();

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
                // Boundary check: resolver exports include transitive public
                // re-exports, while the source scan preserves input-port
                // semantics for directly authored params.
                if resolved_export.is_none()
                    && !dep_interface.exposes_item(orig_name.atom(), import_item.namespace)
                {
                    if dep_interface.has_item(orig_name.atom(), import_item.namespace) {
                        return Err(PipelineError::Semantic(SemanticError::located(
                            file_src,
                            import_item.name.span,
                            VisibilityError::ImportPrivateItem {
                                name: orig_name.atom().clone(),
                                file_path: DagReference::Path(import_path.clone()),
                            },
                        )));
                    }
                    return Err(PipelineError::Semantic(import_item_not_found_error(
                        dep_interface,
                        orig_name.atom(),
                        import_item.namespace,
                        &DagReference::Path(import_path.clone()),
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

                let term_import = resolved_export
                    .and_then(|binding| TermImport::of_export(&binding.target))
                    .or_else(|| {
                        dep_interface
                            .pure_import_term_disposition(orig_name.atom())
                            .and_then(TermImport::of_source_disposition)
                    })
                    .ok_or_else(|| {
                        PipelineError::Semantic(SemanticError::located(
                            file_src,
                            import_item.name.span,
                            ModuleError::ImportNameNotFound {
                                name: orig_name.atom().clone(),
                                file_path: DagReference::Path(import_path.clone()),
                            },
                        ))
                    })?;
                let is_visualization = matches!(
                    term_import,
                    TermImport::Reject(PureImportRejection::Visualization)
                );
                let is_assertion = matches!(
                    term_import,
                    TermImport::Reject(PureImportRejection::Assertion)
                );
                validate_include_item_attributes(
                    import_item,
                    is_visualization,
                    is_assertion,
                    file_src,
                )?;

                match term_import {
                    TermImport::Constant(canonical) => {
                        import_selective_resolved_item(
                            canonical.clone(),
                            &local_name,
                            import_item.local_span(),
                            file_src,
                            &mut ctx.imported_names,
                            &mut ctx.imported_bindings,
                            Some(&mut ctx.imported_source_order),
                        )?;
                    }
                    TermImport::ResolverOnly => {}
                    TermImport::Reject(reason) => {
                        return Err(PipelineError::Semantic(reason.diagnostic(
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
                return Err(PipelineError::Semantic(SemanticError::located(
                    file_src,
                    import_path.span(),
                    ModuleError::DuplicateModuleName {
                        name: module_name.clone(),
                        first: first.span(),
                    },
                )));
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
    imported_bindings: &mut super::model::ImportedBindings,
    lexical_name: ScopedName,
    binding: ResolvedDeclName,
    src: SourceId,
    span: Span,
) -> Result<(), PipelineError> {
    if let Some(first) = imported_bindings.get(&lexical_name) {
        return Err(PipelineError::Semantic(SemanticError::located(
            src,
            span,
            NameError::DuplicateName {
                name: DuplicateDeclaration::Scoped(lexical_name),
                first: first.span,
            },
        )));
    }
    imported_bindings.insert(lexical_name, Spanned::new(binding, span));
    Ok(())
}

/// Register a selectively imported constant at the HIR boundary.
fn import_selective_resolved_item(
    canonical: graphcal_compiler::resolved_name::ResolvedDeclName,
    local_name: &DeclName,
    span: Span,
    src: SourceId,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut super::model::ImportedBindings,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), PipelineError> {
    let scoped = ScopedName::local(local_name.clone());
    imported_names.const_names.push((scoped.clone(), span));
    if let Some(source_order) = imported_source_order {
        source_order.push((
            scoped.clone(),
            DeclCategory::Value(ValueDeclCategory::Const),
        ));
    }
    insert_imported_binding(imported_bindings, scoped, canonical, src, span)
}

/// Import all resolver-visible exported constants under a module prefix.
fn import_module_values_from_resolver(
    exported_bindings: &[graphcal_compiler::resolve::exports::ExportedBinding],
    module_name: &ModuleAliasName,
    import_span: Span,
    src: SourceId,
    imported_names: &mut ImportedValueNames,
    imported_bindings: &mut super::model::ImportedBindings,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> Result<(), PipelineError> {
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
            scoped,
            canonical.clone(),
            src,
            import_span,
        )?;
    }
    Ok(())
}

/// Derive the source-facing module alias from a module path leaf.
/// How a pure import handles one Term item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermImport<'e> {
    /// Bind the local name to this exported constant.
    Constant(&'e ResolvedDeclName),
    /// The resolver registers the item (a DAG or a constructor).
    ResolverOnly,
    /// The item cannot be imported.
    Reject(PureImportRejection),
}

impl<'e> TermImport<'e> {
    /// The handling of a Term item the module exports as `target`; `None`
    /// for a Static or Unit export, which no Term item names.
    const fn of_export(target: &'e ExportedBindingTarget) -> Option<Self> {
        match target {
            ExportedBindingTarget::Decl { identity, kind } => Some(match kind {
                DeclSymbolKind::Const => Self::Constant(identity),
                DeclSymbolKind::Param | DeclSymbolKind::Node => {
                    Self::Reject(PureImportRejection::Runtime)
                }
                DeclSymbolKind::Assert => Self::Reject(PureImportRejection::Assertion),
                DeclSymbolKind::Plot | DeclSymbolKind::Figure | DeclSymbolKind::Layer => {
                    Self::Reject(PureImportRejection::Visualization)
                }
                DeclSymbolKind::Dag => Self::ResolverOnly,
            }),
            ExportedBindingTarget::Constructor(_) => Some(Self::ResolverOnly),
            ExportedBindingTarget::Dimension(_)
            | ExportedBindingTarget::Unit { .. }
            | ExportedBindingTarget::Type(_)
            | ExportedBindingTarget::Index(_) => None,
        }
    }

    /// The handling of a Term item the module's source declares but does
    /// not export, such as an input port.
    ///
    /// A constant binds only through the module's export surface, which
    /// carries its canonical identity; one the surface does not export is
    /// not importable.
    const fn of_source_disposition(disposition: PureImportTermDisposition) -> Option<Self> {
        match disposition {
            PureImportTermDisposition::BindConstant => None,
            PureImportTermDisposition::ResolverOnly => Some(Self::ResolverOnly),
            PureImportTermDisposition::Reject(reason) => Some(Self::Reject(reason)),
        }
    }
}

pub(super) fn derive_module_name_from_import_path(import_path: &ModulePath) -> ModuleAliasName {
    ModuleAliasName::classify(import_path.leaf().name.atom().clone())
}

#[cfg(test)]
mod tests;
