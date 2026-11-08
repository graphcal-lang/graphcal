//! HIR lowering, registry composition, and include elaboration for projects.

use std::collections::{HashMap, HashSet};

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::instance::{
    ExposedValueBody, InstanceAssertionProjection, InstancePlotProjection, InstanceRecord,
    InstanceValueProjection, ProjectionExposure, template_declaration, template_reference,
};
use graphcal_compiler::ir::module_interface::ModuleInterface;
use graphcal_compiler::ir::static_dependencies::{ModuleDeclarations, StaticScope};
use graphcal_compiler::ir::static_substitution::StaticSubstitution;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::{ResolvedDeclName, ResolvedDimName, ResolvedIndexName};
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::dimension::DimensionError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::{DuplicateDeclaration, NameError};
use graphcal_compiler::source_id::SourceId;

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};
use graphcal_compiler::syntax::span::Span;

use super::imports;
use super::module_resolve_errors::module_resolve_compile_error;
use crate::compile_error::PipelineError;

use super::include_static_bindings::IncludeStaticBindings;
use super::model::{
    HirFile, ImportAlias, ImportContext, IncludeInstanceRequest, ModuleArtifactStore,
    ProjectModuleBinding,
};
use super::template::{ElaboratedModuleTemplate, ModuleTemplateStore};
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration, Expr, ExprKind, GraphRef};
use graphcal_compiler::syntax::phase::Desugared;
use graphcal_compiler::syntax::visitor::ExprVisitor;

use super::generic_leakage::check_generics_leakage;
use super::including_module::IncludingModule;

/// Project-wide semantic services shared by every module lowering pass.
pub(super) struct ProjectSemanticContext<'project, 'session> {
    pub(super) project: &'project crate::loader::loaded_project::LoadedProject,
    /// The module resolver, with the handle of every loaded module.
    pub(super) modules: &'project crate::loader::loaded_module_resolver::LoadedModuleResolver,
    pub(super) module_templates: &'session mut ModuleTemplateStore,
    /// Canonical dimensions, units, and indexes of every module, evaluated on demand.
    pub(super) definitions:
        &'session mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<
            'project,
        >,
}

struct DirectDagCallValidator<'a> {
    project: &'a crate::loader::loaded_project::LoadedProject,
    owner: &'a graphcal_compiler::dag_id::DagId,
    importer: &'a ModuleInterface,
    resolver: &'a graphcal_compiler::resolve::ModuleResolver,
    src: SourceId,
}

impl ExprVisitor<Desugared> for DirectDagCallValidator<'_> {
    type Error = PipelineError;

    fn visit_inline_dag_ref(
        &mut self,
        expr: &Expr,
        args: &[graphcal_compiler::desugar::desugared_ast::ParamBinding],
    ) -> Result<(), Self::Error> {
        let ExprKind::InlineDagRef { path, .. } = &expr.kind else {
            return Ok(());
        };
        if let Ok(target) = self.resolver.resolve_module_path(self.owner, path)
            && let Some(dependency) = self.project.module(&target)
        {
            imports::validate_direct_dag_call_bindings(
                args,
                dependency.interface(),
                self.importer,
                &graphcal_compiler::semantic_error::graph::DagReference::Dag(target.clone()),
                self.src,
                expr.span,
            )?;
        }
        args.iter()
            .try_for_each(|binding| self.visit_expr(&binding.value))
    }
}

/// Validate the direct DAG calls (`@dag(args)::out`) in one module's value
/// and assertion bodies. Self-imports carry no expressions, so a module's
/// full loaded body and its self-import-stripped lowering body are equivalent
/// here.
fn validate_direct_dag_calls(
    module: crate::loader::loaded_file::LoadedModule<'_>,
    project: &crate::loader::loaded_project::LoadedProject,
    owner: &graphcal_compiler::dag_id::DagId,
    resolver: &graphcal_compiler::resolve::ModuleResolver,
    src: SourceId,
) -> Result<(), PipelineError> {
    let mut validator = DirectDagCallValidator {
        project,
        owner,
        importer: module.interface(),
        resolver,
        src,
    };
    let mut visit = |expr: &Expr| validator.visit_expr(expr);
    for declaration in module.declarations() {
        match &declaration.kind {
            DeclKind::Param(param) => {
                if let Some(value) = &param.value {
                    visit(value)?;
                }
            }
            DeclKind::Node(node) => visit(&node.value)?,
            DeclKind::ConstNode(constant) => visit(&constant.value)?,
            DeclKind::Assert(assertion) => match &assertion.body {
                graphcal_compiler::desugar::desugared_ast::AssertBody::Expr(expr) => visit(expr)?,
                graphcal_compiler::desugar::desugared_ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    visit(actual)?;
                    visit(expected)?;
                    visit(tolerance)?;
                }
            },
            // Direct DAG calls are validated in value and assertion bodies
            // only; nested DAG bodies are validated as their own modules.
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_) => {}
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(sugar) => graphcal_compiler::syntax::phase::never(*sugar),
        }
    }
    Ok(())
}

fn imported_module_target(
    owner: &graphcal_compiler::syntax::non_empty::NonEmpty<
        graphcal_compiler::syntax::names::NameAtom,
    >,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
) -> Option<graphcal_compiler::dag_id::DagId> {
    let (root, children) = (owner.first(), &owner.as_slice()[1..]);
    let alias = ModuleAliasName::classify(root.clone());
    let binding = module_map.get(&alias)?;
    if binding.role != graphcal_compiler::resolve::scope::ModuleAliasRole::ImportedDag {
        return None;
    }
    Some(
        children
            .iter()
            .fold(binding.target.clone(), |target, child| {
                target.inline_dag_child(DeclName::classify(child.clone()))
            }),
    )
}

fn is_imported_dynamic_unit_during_lowering(
    alias: &graphcal_compiler::syntax::non_empty::NonEmpty<
        graphcal_compiler::syntax::names::NameAtom,
    >,
    name: &graphcal_compiler::syntax::dimension::UnitName,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    project: &crate::loader::loaded_project::LoadedProject,
) -> bool {
    imported_module_target(alias, module_map).is_some_and(|target| {
        project
            .module(&target)
            .is_some_and(|module| module.interface().runtime_units().contains(name))
    })
}

fn remap_imported_dynamic_unit_error(
    error: SemanticError,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    project: &crate::loader::loaded_project::LoadedProject,
) -> SemanticError {
    match error {
        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            src,
            primary,
            kind: SemanticErrorKind::Dimension(DimensionError::UnknownUnit { name }),
        }) if name.owner().is_some_and(|alias| {
            is_imported_dynamic_unit_during_lowering(alias, name.leaf(), module_map, project)
        }) =>
        {
            SemanticError::located(src, primary, ModuleError::ImportRuntimeUnit { name })
        }
        other => other,
    }
}

pub(super) fn validate_imported_runtime_units(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    module_map: &HashMap<ModuleAliasName, ProjectModuleBinding>,
    exported_runtime_units: &HashMap<
        graphcal_compiler::dag_id::DagId,
        HashSet<graphcal_compiler::syntax::dimension::UnitName>,
    >,
    src: SourceId,
) -> Result<(), SemanticError> {
    let mut invalid = None;
    dag.visit_unit_references(&mut |unit, span| {
        if invalid.is_none()
            && unit.spelling().owner().is_some_and(|alias| {
                imported_module_target(alias, module_map).is_some_and(|target| {
                    exported_runtime_units
                        .get(&target)
                        .is_some_and(|names| names.contains(unit.spelling().leaf()))
                })
            })
        {
            invalid = Some((unit.clone(), span));
        }
    });
    match invalid {
        Some((unit, span)) => Err(SemanticError::located(
            src,
            span,
            ModuleError::ImportRuntimeUnit {
                name: unit.spelling().clone(),
            },
        )),
        None => Ok(()),
    }
}

fn include_debug_name_map(
    ctx: &ImportContext<'_>,
) -> graphcal_compiler::display::include_scope_names::IncludeScopeNames {
    let anonymous_includes = || {
        ctx.include_instances
            .iter()
            .filter_map(|include| match include.instance_scope {
                ScopeSegment::IncludeInstance(id) => Some((id, &include.debug_scope)),
                ScopeSegment::Named(_) => None,
            })
    };
    let mut leaf_counts: HashMap<&ModuleAliasName, usize> = HashMap::new();
    anonymous_includes().for_each(|(_, debug_scope)| {
        leaf_counts
            .entry(debug_scope)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    });

    anonymous_includes()
        .filter(|(_, debug_scope)| leaf_counts.get(debug_scope) == Some(&1))
        .filter(|(_, debug_scope)| !ctx.module_map.contains_key(*debug_scope))
        .map(|(id, debug_scope)| (id, debug_scope.clone()))
        .collect()
}

/// One module body lowered with its own imports and includes elaborated,
/// with the import facts a file root's HIR retains.
struct ElaboratedBody {
    unfrozen: graphcal_compiler::ir::model::UnfrozenIR,
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
    output_surface: HashSet<ScopedName>,
    include_debug_names: graphcal_compiler::display::include_scope_names::IncludeScopeNames,
    module_map: HashMap<ModuleAliasName, ProjectModuleBinding>,
}

/// Lower one file-root or inline-DAG module body and elaborate its own
/// includes, always in the module's own source.
///
/// Every consumer — a file root, an inline DAG the file defines, and a
/// template an include instantiates before its own file is lowered —
/// elaborates a module through this one function.
fn elaborate_module_body(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    module: crate::loader::loaded_file::LoadedModule<'_>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ElaboratedBody, Outcome<PipelineError>> {
    use crate::loader::loaded_file::LoadedModule;

    cancellation.checkpoint()?;
    let project = semantic.project;
    let modules = semantic.modules;
    let module_resolver = modules.resolver();
    let dag_id = module.dag_id();
    let mut ctx = ImportContext::empty();
    let (src, ast) = match module {
        LoadedModule::FileRoot(file) => {
            imports::process_file_body_declarations(
                project,
                file,
                modules,
                &mut ctx,
                cancellation,
            )?;
            (file.source_id(), std::borrow::Cow::Borrowed(file.ast()))
        }
        LoadedModule::InlineDag { file, dag } => {
            let src = file.source_id();
            let body = dag.body(file);
            let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
                body,
                dag.parent_dag_id(),
                file.interface(),
                dag.resolved_imports(),
                modules,
                src,
            )
            .map_err(PipelineError::from)?;
            process_dag_body_import_declarations(project, dag, body, src, modules, &mut ctx)?;
            process_dag_body_include_declarations(project, dag, body, src, modules, &mut ctx)?;
            extend_imported_value_names(&mut ctx.imported_names, self_imports.names);
            extend_imported_bindings(&mut ctx.imported_bindings, self_imports.bindings, src)?;
            (
                src,
                std::borrow::Cow::Owned(graphcal_compiler::desugar::desugared_ast::File {
                    declarations: self_imports.stripped_body,
                }),
            )
        }
    };
    validate_direct_dag_calls(module, project, dag_id, module_resolver, src)?;
    let include_debug_names = include_debug_name_map(&ctx);
    let mut unfrozen =
        graphcal_compiler::ir::lower::lower_module_with_imported_bindings_and_cancellation(
            graphcal_compiler::ir::lower::ModuleBody {
                ast: &ast,
                interface: module.interface(),
            },
            src,
            &ctx.imported_names,
            super::model::hir_imported_bindings(ctx.imported_bindings),
            dag_id,
            semantic.definitions,
            cancellation,
        )
        .map_err(|outcome| {
            outcome
                .map_failed(|error| {
                    remap_imported_dynamic_unit_error(error, &ctx.module_map, project)
                })
                .map_into()
        })?;

    let output_surface: HashSet<ScopedName> = unfrozen
        .value_names()
        .cloned()
        .map(ScopedName::local)
        .chain(
            ctx.imported_source_order
                .iter()
                .filter(|(_, category)| matches!(category, DeclCategory::Value(_)))
                .map(|(name, _)| name.clone()),
        )
        .chain(
            ctx.include_instances
                .iter()
                .flat_map(|include| include.surface_outputs.iter().cloned()),
        )
        .collect();

    elaborate_include_instances(
        semantic,
        dag_id,
        &ctx.include_instances,
        src,
        module,
        &mut unfrozen,
        cancellation,
    )?;
    Ok(ElaboratedBody {
        unfrozen,
        imported_source_order: ctx.imported_source_order,
        output_surface,
        include_debug_names,
        module_map: ctx.module_map,
    })
}

/// Lower one physical file and every inline DAG it defines into authoritative HIR.
///
/// This phase performs elaboration and canonical reference resolution only.
/// It does not resolve checked declaration types, evaluate constants, verify
/// host signatures, or construct TIR.
pub(super) fn lower_file_to_hir(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    loaded_file: &crate::loader::loaded_file::LoadedFile,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HirFile, Outcome<PipelineError>> {
    let file_dag_id = loaded_file.dag_id();
    let file_src = loaded_file.source_id();
    let ElaboratedBody {
        unfrozen,
        imported_source_order,
        output_surface,
        include_debug_names,
        module_map,
    } = elaborate_module_body(semantic, loaded_file.module(), cancellation)?;

    cancellation.checkpoint()?;
    let root =
        store_and_freeze_module_template(semantic, file_dag_id, unfrozen, file_src, cancellation)?;
    let inline_dags = lower_inline_dag_modules(semantic, loaded_file, cancellation)?;

    Ok(HirFile {
        source: file_src,
        root,
        inline_dags,
        imported_source_order,
        output_surface,
        include_debug_names,
        module_map,
    })
}

fn lower_inline_dag_modules(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    loaded_file: &crate::loader::loaded_file::LoadedFile,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<graphcal_compiler::ir::model::HirDag>, Outcome<PipelineError>> {
    let file_src = loaded_file.source_id();
    loaded_file
        .inline_dags()
        .iter()
        .map(|loaded_dag| {
            cancellation.checkpoint()?;
            let dag_id = loaded_dag.dag_id();
            if let Some(template) = semantic.module_templates.get(dag_id) {
                return freeze_inline_module_template(
                    &template,
                    dag_id,
                    semantic.definitions,
                    file_src,
                    cancellation,
                );
            }
            let unfrozen =
                elaborate_module_body(semantic, loaded_dag.module(loaded_file), cancellation)?
                    .unfrozen;
            cancellation.checkpoint()?;
            store_and_freeze_module_template(semantic, dag_id, unfrozen, file_src, cancellation)
        })
        .collect()
}

fn freeze_inline_module_template(
    template: &ElaboratedModuleTemplate,
    dag_id: &graphcal_compiler::dag_id::DagId,
    definitions: &mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<'_>,
    src: SourceId,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::model::HirDag, Outcome<PipelineError>> {
    template
        .unfrozen
        .clone()
        .freeze_with_cancellation(dag_id, definitions, src, cancellation)
        .map_err(Outcome::map_into)
}

fn store_and_freeze_module_template(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    dag_id: &graphcal_compiler::dag_id::DagId,
    unfrozen: graphcal_compiler::ir::model::UnfrozenIR,
    src: SourceId,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::model::HirDag, Outcome<PipelineError>> {
    let template_unfrozen = unfrozen.clone();
    let frozen = unfrozen
        .freeze_with_cancellation(dag_id, semantic.definitions, src, cancellation)
        .map_err(Outcome::map_into)?;
    semantic.module_templates.insert(
        dag_id.clone(),
        ElaboratedModuleTemplate {
            unfrozen: template_unfrozen,
        },
    );
    Ok(frozen)
}

fn extend_imported_value_names(target: &mut ImportedValueNames, source: ImportedValueNames) {
    target.const_names.extend(source.const_names);
    target.param_names.extend(source.param_names);
    target.node_names.extend(source.node_names);
    target.assert_names.extend(source.assert_names);
}

fn extend_imported_bindings(
    target: &mut super::model::ImportedBindings,
    source: super::model::ImportedBindings,
    src: SourceId,
) -> Result<(), PipelineError> {
    for (name, binding) in source {
        if let Some(first) = target.get(&name) {
            return Err(PipelineError::Semantic(SemanticError::located(
                src,
                binding.span,
                NameError::DuplicateName {
                    name: DuplicateDeclaration::Scoped(name),
                    first: first.span,
                },
            )));
        }
        target.insert(name, binding);
    }
    Ok(())
}

fn process_dag_body_import_declarations<'a>(
    project: &'a crate::loader::loaded_project::LoadedProject,
    loaded_dag: &crate::loader::loaded_file::LoadedDag,
    dag_body: &[Declaration],
    file_src: SourceId,
    modules: &'a crate::loader::loaded_module_resolver::LoadedModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    let module_resolver = modules.resolver();
    for decl in dag_body {
        let DeclKind::Import(import_decl) = &decl.kind else {
            continue;
        };
        let Some(target) = loaded_dag.resolved_imports().get(&import_decl.path().key()) else {
            continue;
        };
        if target.target() == loaded_dag.parent_dag_id() {
            continue;
        }
        imports::process_pure_import(
            project,
            target,
            import_decl,
            ModuleDeclarations::new(
                dag_body,
                StaticScope::new(loaded_dag.dag_id(), module_resolver),
            ),
            file_src,
            modules,
            ctx,
        )?;
    }
    Ok(())
}

fn process_dag_body_include_declarations<'a>(
    project: &'a crate::loader::loaded_project::LoadedProject,
    loaded_dag: &crate::loader::loaded_file::LoadedDag,
    dag_body: &[Declaration],
    file_src: SourceId,
    modules: &'a crate::loader::loaded_module_resolver::LoadedModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    let module_resolver = modules.resolver();
    for decl in dag_body {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        let Some(target) = loaded_dag.resolved_imports().get(&include_decl.path.key()) else {
            continue;
        };
        imports::process_include(
            project.target_module(target),
            include_decl,
            decl,
            &IncludingModule {
                interface: loaded_dag.interface(),
                source: file_src,
                scope: StaticScope::new(loaded_dag.dag_id(), module_resolver),
            },
            ctx,
        )?;
    }
    Ok(())
}

/// Install shared handles to each published module's own DAG bodies and units.
///
/// HIR calls already carry canonical targets (including calls from imports
/// inside inline DAG bodies, which have their own lexical scopes), so all loaded
/// dependency TIRs must be available here.
/// Visibility is enforced while HIR resolves each import edge; retaining private
/// dependency DAGs internally is also necessary when a public DAG calls one of
/// its private implementation children.
///
/// Published stores exclude their imported bodies and unit overlays. Constants
/// remain in defining-body execution facts; no importer mutates a dependency's
/// bindings or injects values into its immutable body.
pub(super) fn install_shared_module_artifacts(
    tir: &mut graphcal_compiler::tir::typed::TirDraft,
    module_artifacts: &ModuleArtifactStore,
    src: SourceId,
) -> Result<(), PipelineError> {
    // Extern signatures travel with the deps' dag bodies: a qualified inline
    // call into a dep dag that uses extern functions resolves its signature
    // from the importer's merged TIR at eval time. Files may declare one
    // plugin function independently; structurally equivalent copies merge,
    // and a real conflict is W001 at the later declaration, in its own file.
    //
    // Deterministic conflict reporting: earliest source, then earliest
    // declaration, first.
    let mut functions: Vec<_> = module_artifacts
        .values()
        .flat_map(|artifact| artifact.extern_functions.values())
        .collect();
    functions.sort_by_key(|function| (function.source, function.decl_span.offset()));
    for function in functions {
        tir.insert_extern_function(function.clone())
            .map_err(PipelineError::Semantic)?;
    }
    // Every loaded module's store is installed at once, so each store's
    // callees (its own imports' bodies) are installed with it.
    tir.install_shared_dag_stores(
        module_artifacts
            .values()
            .map(|artifact| artifact.dag_store.as_ref()),
    )
    .map_err(|error| {
        PipelineError::Semantic(SemanticError::internal_error(
            error.to_string(),
            src,
            DiagnosticAnchor::WholeFile,
        ))
    })
}

fn resolve_projection_expected_fail(
    request: &IncludeInstanceRequest,
    source: &graphcal_compiler::syntax::decl_name::DeclName,
    importer: &graphcal_compiler::dag_id::DagId,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    src: SourceId,
) -> Result<Option<graphcal_compiler::assertion_expectation::ExpectedFail>, PipelineError> {
    use graphcal_compiler::assertion_expectation::{ExpectedFail, ExpectedFailKeyPart};
    use graphcal_compiler::semantic::checked_type::IndexTypeRef;
    use graphcal_compiler::syntax::attribute::AttributeName;

    request
        .import_item_attributes
        .get(source)
        .into_iter()
        .flatten()
        .find(|attribute| {
            attribute.name.name.as_str().parse::<AttributeName>() == Ok(AttributeName::ExpectedFail)
        })
        .map(|attribute| {
            let parsed =
                graphcal_compiler::ir::resolve::parse_expected_fail_args(&attribute.args, src)?;
            match parsed {
                ExpectedFail::All => Ok(ExpectedFail::All),
                ExpectedFail::Variants(keys) => keys
                    .try_map(|key| {
                        key.try_map(|part| match part {
                            ExpectedFailKeyPart::Named {
                                index,
                                variant,
                                span,
                            } => module_resolver
                                .resolve_index_path(importer, &index)
                                .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
                                .map(|index| ExpectedFailKeyPart::Named {
                                    index: IndexTypeRef::from_resolved(index),
                                    variant,
                                    span,
                                })
                                .map_err(|error| module_resolve_compile_error(error, src)),
                            ExpectedFailKeyPart::FinitePosition { position, span } => {
                                Ok(ExpectedFailKeyPart::FinitePosition { position, span })
                            }
                        })
                    })
                    .map(ExpectedFail::Variants),
            }
        })
        .transpose()
}

/// Leaves of the template's value declarations, each materialized by an instance.
fn template_value_ports(
    template: &graphcal_compiler::ir::model::UnfrozenIR,
) -> impl Iterator<Item = graphcal_compiler::syntax::decl_name::DeclName> + '_ {
    template.value_names().cloned()
}

/// Explicit value-port bindings keyed by their template declaration.
fn semantic_value_bindings(
    request: &IncludeInstanceRequest,
    instance: &graphcal_compiler::dag_id::InstanceId,
) -> HashMap<ResolvedDeclName, Expr> {
    request
        .bindings
        .iter()
        .map(|(name, expr)| (template_declaration(instance, name.clone()), expr.clone()))
        .collect()
}

/// The instance's canonical Static substitution: the include's own bindings
/// plus every selected type the include projects onto an importer type.
fn instance_substitution(
    request: &IncludeInstanceRequest,
    importer: &graphcal_compiler::dag_id::DagId,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
) -> StaticSubstitution {
    let mut substitution = request.static_bindings.substitution();
    for alias in &request.selected_types {
        let source_path =
            graphcal_compiler::syntax::names::NamePath::local(alias.original.atom().clone());
        let target_path =
            graphcal_compiler::syntax::names::NamePath::local(alias.local.atom().clone());
        if let (Ok(source), Ok(target)) = (
            module_resolver
                .resolve_struct_type_path(request.template.dag_id(), &source_path)
                .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved),
            module_resolver
                .resolve_struct_type_path(importer, &target_path)
                .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved),
        ) {
            substitution.types.insert(source, target);
        }
    }
    substitution
}

/// The values an include site exposes. A selected value whose producer
/// declaration is in `alias_declarations` is materialized as a local alias
/// in the importer (`add_selective_aliases_inner`), so the edge records it.
fn semantic_output_projections(
    request: &IncludeInstanceRequest,
    instance: &graphcal_compiler::dag_id::InstanceId,
    alias_declarations: &HashMap<DeclName, graphcal_compiler::ir::model::IncludeAliasDeclaration>,
) -> Vec<InstanceValueProjection> {
    request
        .surface_outputs
        .iter()
        .map(|exposed_name| {
            let local = exposed_name.leaf();
            request.selective_names.as_ref().map_or_else(
                || InstanceValueProjection::member(template_reference(instance, local.clone())),
                |aliases| {
                    let source_name = aliases
                        .iter()
                        .find(|alias| &alias.local == local)
                        .map_or_else(|| local.clone(), |alias| alias.original.clone());
                    let body = if alias_declarations.contains_key(&source_name) {
                        ExposedValueBody::LocalAlias
                    } else {
                        ExposedValueBody::Instance
                    };
                    InstanceValueProjection::selected(
                        template_reference(instance, source_name),
                        local.clone(),
                        body,
                    )
                },
            )
        })
        .collect()
}

/// The compiler-bug diagnostic of a projection whose template target
/// (`projection`, described for the message) the template does not have.
fn untargeted_projection(projection: &str, src: SourceId) -> PipelineError {
    PipelineError::Semantic(SemanticError::internal_error(
        format!("{projection} has no semantic target"),
        src,
        DiagnosticAnchor::WholeFile,
    ))
}

fn semantic_assertion_projections(
    request: &IncludeInstanceRequest,
    template: &graphcal_compiler::ir::model::UnfrozenIR,
    instance: &graphcal_compiler::dag_id::InstanceId,
    importer: &graphcal_compiler::dag_id::DagId,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    src: SourceId,
) -> Result<Vec<InstanceAssertionProjection>, PipelineError> {
    match &request.selective_names {
        Some(_) => request
            .assertion_aliases
            .iter()
            .map(|(source, exposed)| {
                Ok(InstanceAssertionProjection {
                    target: template
                        .assertion_projection_target(source)
                        .ok_or_else(|| {
                            untargeted_projection(
                                &format!("selected template assertion `{source}`"),
                                src,
                            )
                        })?,
                    exposure: ProjectionExposure::Selected(exposed.clone()),
                    expected_fail: resolve_projection_expected_fail(
                        request,
                        source,
                        importer,
                        module_resolver,
                        src,
                    )?,
                })
            })
            .collect(),
        None => Ok(template
            .assertion_names()
            .into_iter()
            .map(|name| InstanceAssertionProjection {
                target: template_reference(instance, name),
                exposure: ProjectionExposure::Member,
                expected_fail: None,
            })
            .collect()),
    }
}

fn semantic_plot_projections(
    request: &IncludeInstanceRequest,
    template: &graphcal_compiler::ir::model::UnfrozenIR,
    src: SourceId,
) -> Result<Vec<InstancePlotProjection>, PipelineError> {
    request
        .requested_plots
        .iter()
        .map(|(source, requested)| {
            template
                .plot_projection_target(source)
                .map(|target| InstancePlotProjection {
                    target,
                    alias: requested.alias.clone(),
                    visibility: requested.visibility,
                })
                .ok_or_else(|| {
                    untargeted_projection(&format!("requested template plot `{source}`"), src)
                })
        })
        .collect()
}

/// The producer declarations a selective include materializes as local
/// aliases in the importer, keyed by their names in the producer. Both the
/// include edge and the alias materialization read this one table.
fn selective_alias_declarations(
    request: &IncludeInstanceRequest,
    template: &graphcal_compiler::ir::model::UnfrozenIR,
) -> HashMap<DeclName, graphcal_compiler::ir::model::IncludeAliasDeclaration> {
    request
        .selective_names
        .as_ref()
        .map_or_else(HashMap::new, |selective| {
            selective
                .iter()
                .filter_map(|alias| {
                    template
                        .include_alias_declaration(&alias.original)
                        .map(|declaration| (alias.original.clone(), declaration))
                })
                .collect()
        })
}

fn record_semantic_instance(
    unfrozen: &mut graphcal_compiler::ir::model::UnfrozenIR,
    request: &IncludeInstanceRequest,
    template: &graphcal_compiler::ir::model::UnfrozenIR,
    override_reconciliations: graphcal_compiler::ir::include::IncludeOverrideReconciliations,
    importer: &graphcal_compiler::dag_id::DagId,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    src: SourceId,
) -> Result<(), PipelineError> {
    let template_id = request.template.dag_id();
    let instance_id = graphcal_compiler::dag_id::InstanceId::new(
        importer.clone(),
        request.instance_scope.clone(),
        template_id.clone(),
    );
    let value_bindings = semantic_value_bindings(request, &instance_id);
    let substitution = instance_substitution(request, importer, module_resolver);
    let output_projections = semantic_output_projections(
        request,
        &instance_id,
        &selective_alias_declarations(request, template),
    );
    let assertion_projections = semantic_assertion_projections(
        request,
        template,
        &instance_id,
        importer,
        module_resolver,
        src,
    )?;
    let plot_projections = semantic_plot_projections(request, template, src)?;
    unfrozen.add_semantic_dynamic_unit_bindings(&request.runtime_unit_names, &instance_id);
    unfrozen.record_semantic_instance(
        graphcal_compiler::ir::include::SemanticInstanceInput {
            instance: InstanceRecord::new(
                instance_id,
                substitution,
                template_value_ports(template),
            ),
            value_bindings,
            runtime_unit_names: request.runtime_unit_names.clone(),
            output_projections,
            assertion_projections,
            plot_projections,
            override_reconciliations,
        },
        src,
        request.include_span,
    )?;
    Ok(())
}

/// Elaborate every file-root, same-file inline, and cross-file qualified
/// include through one concrete-instance path:
///
/// 1. Take the template's elaborated body from the cache, or elaborate it in
///    its own source with [`elaborate_module_body`] (an inline DAG's
///    `import <self>::{...}` resolves against the DAG's parent file, Concept 9).
/// 2. Validate every index binding against the body's effective typed
///    contract, with both sides resolved canonically.
/// 3. Preserve canonical A8/V005 reconciliation facts and run
///    `check_generics_leakage` (A9/V006).
/// 4. Record a typed semantic instance edge without copying dependency bodies.
/// 5. Materialize only selective projection aliases in the importer.
fn elaborate_include_instances(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    importer_dag_id: &graphcal_compiler::dag_id::DagId,
    include_instances: &[IncludeInstanceRequest],
    importer_src: SourceId,
    importer: crate::loader::loaded_file::LoadedModule<'_>,
    unfrozen: &mut graphcal_compiler::ir::model::UnfrozenIR,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), Outcome<PipelineError>> {
    let module_resolver = semantic.modules.resolver();
    for instance in include_instances {
        cancellation.checkpoint()?;
        // ---- 1. Elaborated template body ------------------------------------
        let template_id = instance.template.dag_id();
        let template = if let Some(template) = semantic.module_templates.get(template_id) {
            template
        } else {
            let unfrozen =
                elaborate_module_body(semantic, instance.template, cancellation)?.unfrozen;
            semantic
                .module_templates
                .insert(template_id.clone(), ElaboratedModuleTemplate { unfrozen })
        };
        let dep_resolution_owner = template_id.clone();
        let body_decls_for_aliases = instance.template.declarations();
        let dep_unfrozen = &template.unfrozen;

        // ---- 2. Validate typed index binding contracts -------------------
        validate_index_binding_contracts(
            semantic.definitions,
            &IndexBindingSites {
                importer: importer_dag_id,
                template: &dep_resolution_owner,
                template_declarations: body_decls_for_aliases,
                importer_src,
            },
            &instance.static_bindings,
        )?;

        // ---- 3. Validation checks -----------------------------------------
        let static_substitution = instance.static_bindings.substitution();
        let override_reconciliations = dep_unfrozen
            .include_override_reconciliations(
                &instance.bindings,
                &static_substitution,
                module_resolver,
                &dep_resolution_owner,
                importer_src,
                instance.include_span,
            )
            .map_err(PipelineError::from)?;
        check_generics_leakage(
            body_decls_for_aliases,
            StaticScope::new(&dep_resolution_owner, module_resolver),
            &instance.pub_reexport_items,
            &instance.static_bindings,
            &IncludingModule {
                interface: importer.interface(),
                source: importer_src,
                scope: StaticScope::new(importer_dag_id, module_resolver),
            },
            instance.include_span,
        )?;

        let selective_alias_declarations = selective_alias_declarations(instance, dep_unfrozen);
        super::rebound_type_selection::check_rebound_type_selection(
            &super::rebound_type_selection::IncludedTemplate {
                owner: &dep_resolution_owner,
                declarations: body_decls_for_aliases,
                resolver: module_resolver,
                values: dep_unfrozen,
            },
            instance,
            importer_src,
            instance.include_span,
        )?;

        // ---- 4. Retain one typed semantic edge -------------------------------
        record_semantic_instance(
            unfrozen,
            instance,
            dep_unfrozen,
            override_reconciliations,
            importer_dag_id,
            module_resolver,
            importer_src,
        )?;

        // ---- 5. Add selective aliases -------------------------------------
        for projection in &instance.unit_projection_aliases {
            unfrozen.add_dynamic_unit_projection_alias(
                &importer_dag_id.instance_child(instance.instance_scope.clone()),
                &projection.source,
                projection.alias.clone(),
            );
        }
        if let Some(selective) = &instance.selective_names {
            add_selective_aliases_inner(
                &selective_alias_declarations,
                selective,
                &instance.instance_scope,
                &AliasResolutionOwners {
                    r#type: &dep_resolution_owner,
                    body: importer_dag_id,
                },
                instance.include_span,
                unfrozen,
            );
        }
    }
    Ok(())
}

/// The two modules one include's index bindings connect, with the source
/// file of the include for diagnostics.
struct IndexBindingSites<'a> {
    importer: &'a graphcal_compiler::dag_id::DagId,
    template: &'a graphcal_compiler::dag_id::DagId,
    template_declarations: &'a [graphcal_compiler::desugar::desugared_ast::Declaration],
    importer_src: SourceId,
}

fn validate_index_binding_contracts(
    definitions: &mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<'_>,
    sites: &IndexBindingSites<'_>,
    bindings: &IncludeStaticBindings,
) -> Result<(), PipelineError> {
    use graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget;
    use graphcal_compiler::semantic::index_def::IndexBindingContractError;

    for (port, site) in bindings.indexes() {
        let candidate = match &site.target {
            InstanceIndexBindingTarget::Declared(identity) => definitions.index(identity)?,
            InstanceIndexBindingTarget::Finite(finite) => {
                graphcal_compiler::semantic::index_def::IndexDef::finite(*finite)
            }
        };
        let contract = effective_index_binding_contract(
            definitions,
            sites,
            port,
            bindings.dimensions(),
            site.span,
        )?;
        let dep_index = port.to_unowned_def_name();
        match contract.validate(&candidate) {
            Ok(()) => {}
            Err(IndexBindingContractError::KindMismatch { expected, found }) => {
                return Err(PipelineError::Semantic(SemanticError::located(
                    sites.importer_src,
                    site.span,
                    ModuleError::IndexKindMismatch {
                        dep_index,
                        dep_kind: expected,
                        bound_index: site.authored.clone(),
                        bound_kind: found,
                    },
                )));
            }
            Err(IndexBindingContractError::DimensionMismatch { expected, found }) => {
                return Err(PipelineError::Semantic(SemanticError::located(
                    sites.importer_src,
                    site.span,
                    IndexError::IndexBindingDimensionMismatch {
                        dep_index,
                        expected_dim: definitions.dimension_spelling(sites.importer, &expected),
                        bound_index: site.authored.clone(),
                        found_dim: definitions.dimension_spelling(sites.importer, &found),
                    },
                )));
            }
        }
    }
    Ok(())
}

/// Each dimension port an include binds, with the dimension of its
/// importer-side target (`None` when the target has no definition of its
/// own, such as an instance-owned projection).
fn bound_dimension_ports(
    definitions: &mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<'_>,
    dimensions: &std::collections::BTreeMap<ResolvedDimName, ResolvedDimName>,
) -> Result<HashMap<ResolvedDimName, Option<graphcal_compiler::dimension::Dimension>>, SemanticError>
{
    dimensions
        .iter()
        .map(|(port, target)| {
            let dimension = if definitions.defines_dimension(target) {
                Some(definitions.dimension(target)?)
            } else {
                None
            };
            Ok((port.clone(), dimension))
        })
        .collect()
}

fn effective_index_binding_contract(
    definitions: &mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<'_>,
    sites: &IndexBindingSites<'_>,
    identity: &ResolvedIndexName,
    dimensions: &std::collections::BTreeMap<ResolvedDimName, ResolvedDimName>,
    binding_span: Span,
) -> Result<graphcal_compiler::semantic::index_def::IndexBindingContract, PipelineError> {
    use graphcal_compiler::desugar::desugared_ast::{DeclKind, IndexDeclKind};
    use graphcal_compiler::ir::static_definitions::DimExprFailure;
    use graphcal_compiler::semantic::index_def::{
        ConcreteIndexKind, IndexBindingContract, IndexKind, RequiredIndexKind,
    };

    let dep_index = identity.to_unowned_def_name();
    let definition = definitions.index(identity)?;

    match &definition.kind {
        IndexKind::Concrete(ConcreteIndexKind::Named { .. }) => Ok(IndexBindingContract::Named),
        IndexKind::Required(RequiredIndexKind::Named) => Ok(IndexBindingContract::Discrete),
        IndexKind::Concrete(ConcreteIndexKind::Coordinate(data)) => {
            Ok(IndexBindingContract::Coordinate {
                dimension: data.dimension().clone(),
            })
        }
        IndexKind::Required(RequiredIndexKind::Coordinate { .. }) => {
            let dimension_expr = sites
                .template_declarations
                .iter()
                .find_map(|declaration| {
                    let DeclKind::Index(index) = &declaration.kind else {
                        return None;
                    };
                    match &index.kind {
                        IndexDeclKind::RequiredCoordinate { dimension }
                            if index.name.value == dep_index =>
                        {
                            Some(dimension)
                        }
                        IndexDeclKind::RequiredCoordinate { .. }
                        | IndexDeclKind::RequiredNamed
                        | IndexDeclKind::Named { .. }
                        | IndexDeclKind::Range { .. }
                        | IndexDeclKind::Linspace { .. } => None,
                    }
                })
                .ok_or_else(|| {
                    PipelineError::Semantic(SemanticError::internal_error(
                        format!(
                            "required coordinate index `{dep_index}` has no source declaration"
                        ),
                        sites.importer_src,
                        graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(
                            binding_span,
                        ),
                    ))
                })?;
            // The expression is the dependency's source: a reference to a
            // bound dimension port evaluates to the importer's binding target,
            // every other reference in the dependency's own scope.
            let overrides = bound_dimension_ports(definitions, dimensions)?;
            let dimension = definitions
                .evaluate_dim_expr_with_overrides(sites.template, dimension_expr, &overrides)
                .map_err(|failure| {
                    PipelineError::Semantic(match failure {
                        DimExprFailure::Unknown(name) => SemanticError::located(
                            sites.importer_src,
                            binding_span,
                            DimensionError::UnknownDimension {
                                name: name.to_name_path(),
                            },
                        ),
                        DimExprFailure::Overflow => SemanticError::located(
                            sites.importer_src,
                            binding_span,
                            DimensionError::DimensionOverflow,
                        ),
                        DimExprFailure::Definition(error) => *error,
                    })
                })?;
            Ok(IndexBindingContract::Coordinate { dimension })
        }
        IndexKind::Concrete(ConcreteIndexKind::Finite { .. }) => {
            Err(PipelineError::Semantic(SemanticError::internal_error(
                format!("declared dependency index `{dep_index}` became structural"),
                sites.importer_src,
                graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(binding_span),
            )))
        }
    }
}

struct AliasResolutionOwners<'a> {
    r#type: &'a graphcal_compiler::dag_id::DagId,
    body: &'a graphcal_compiler::dag_id::DagId,
}

/// Add `local_name = @scope::orig_name` aliases (const or graph) for each
/// selected value.
///
/// The alias keeps the template declaration's annotation, resolved in the
/// template's scope: names it references (such as a dimension defined over a
/// dimension port) need not be visible in the importer. TIR specializes the
/// resolved type through the instance's Static substitution because every
/// alias is an output projection of its semantic instance.
///
/// `declarations` is the producer's effective HIR-facing value surface after
/// its own includes have been elaborated. Type-system-only items are absent.
fn add_selective_aliases_inner(
    declarations: &HashMap<DeclName, graphcal_compiler::ir::model::IncludeAliasDeclaration>,
    selective: &[ImportAlias],
    prefix: &ScopeSegment,
    owners: &AliasResolutionOwners<'_>,
    import_span: Span,
    unfrozen: &mut graphcal_compiler::ir::model::UnfrozenIR,
) {
    for alias in selective {
        let orig_name = &alias.original;
        let local_name = &alias.local;
        let Some(declaration) = declarations.get(orig_name) else {
            continue;
        };
        let type_ann = declaration.type_ann.clone();

        // The alias reads the instance's output through a synthesized,
        // typed include-output reference (never a fabricated source path).
        // Const and graph aliases share it; HIR
        // lowering binds it through the instance's merged entries, and a const
        // target keeps the alias const-evaluable.
        let alias_expr = Expr::new(
            ExprKind::GraphRef(GraphRef::IncludeOutput {
                scope: prefix.clone(),
                member: orig_name.clone(),
                span: import_span,
            }),
            import_span,
        );

        if alias.visibility.is_public() {
            unfrozen.export_term_alias(local_name.clone());
        }
        if declaration.is_const {
            unfrozen.add_const_alias(
                local_name.clone(),
                type_ann,
                owners.r#type.clone(),
                alias_expr,
                owners.body.clone(),
                import_span,
            );
        } else {
            unfrozen.add_node_alias(
                local_name.clone(),
                type_ann,
                owners.r#type.clone(),
                alias_expr,
                owners.body.clone(),
                import_span,
            );
        }
    }
}
