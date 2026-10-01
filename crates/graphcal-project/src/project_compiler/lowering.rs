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
use graphcal_compiler::semantic_error::name::NameError;
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

use super::model::{
    HirFile, ImportAlias, ImportContext, IncludeInstanceRequest, IncludeStaticBindings,
    ModuleArtifactStore, ProjectModuleBinding,
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
    pub(super) module_resolver: &'project graphcal_compiler::resolve::ModuleResolver,
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
                &target.to_string(),
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
            DeclKind::Node(node) => node.definition.formula().map_or(Ok(()), &mut visit)?,
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
            SemanticError::located(
                src,
                primary,
                ModuleError::ImportRuntimeUnit {
                    name: name.to_string(),
                },
            )
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
                name: unit.to_string(),
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

/// Lower one physical file and every inline DAG it defines into authoritative HIR.
///
/// This phase performs elaboration and canonical reference resolution only.
/// It does not resolve checked declaration types, evaluate constants, verify
/// host signatures, or construct TIR.
pub(super) fn lower_file_to_hir(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    loaded_file: &crate::loader::loaded_file::LoadedFile,
    ctx: ImportContext<'_>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<HirFile, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    let file_dag_id = loaded_file.dag_id();
    let file_src = loaded_file.source_id();
    let file_ast = loaded_file.ast();
    let project = semantic.project;
    let importer = loaded_file.module();
    validate_direct_dag_calls(
        importer,
        project,
        file_dag_id,
        semantic.module_resolver,
        file_src,
    )?;
    let include_debug_names = include_debug_name_map(&ctx);
    let mut unfrozen =
        graphcal_compiler::ir::lower::lower_module_with_imported_bindings_and_cancellation(
            graphcal_compiler::ir::lower::ModuleBody {
                ast: file_ast,
                interface: loaded_file.interface(),
            },
            file_src,
            &ctx.imported_names,
            super::model::hir_imported_bindings(ctx.imported_bindings),
            file_dag_id,
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
        file_dag_id,
        &ctx.include_instances,
        file_src,
        importer,
        &mut unfrozen,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    let root =
        store_and_freeze_module_template(semantic, file_dag_id, unfrozen, file_src, cancellation)?;
    let inline_dags = lower_inline_dag_modules(semantic, loaded_file, cancellation)?;

    Ok(HirFile {
        source: file_src,
        root,
        inline_dags,
        imported_source_order: ctx.imported_source_order,
        output_surface,
        include_debug_names,
        module_map: ctx.module_map,
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
            let dag_body = loaded_dag.body(loaded_file);
            compile_loaded_dag_module_ir(
                semantic,
                loaded_file,
                loaded_dag,
                dag_body,
                file_src,
                cancellation,
            )
        })
        .collect()
}

fn compile_loaded_dag_module_ir(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    parent_loaded: &crate::loader::loaded_file::LoadedFile,
    loaded_dag: &crate::loader::loaded_file::LoadedDag,
    dag_body: &[Declaration],
    file_src: SourceId,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::ir::model::HirDag, Outcome<PipelineError>> {
    cancellation.checkpoint()?;
    if let Some(template) = semantic.module_templates.get(loaded_dag.dag_id()) {
        return freeze_inline_module_template(
            &template,
            loaded_dag.dag_id(),
            semantic.definitions,
            file_src,
            cancellation,
        );
    }
    let project = semantic.project;
    let module_resolver = semantic.module_resolver;
    let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
        dag_body,
        loaded_dag.parent_dag_id(),
        parent_loaded.interface(),
        loaded_dag.resolved_imports(),
        module_resolver,
        file_src,
    )
    .map_err(PipelineError::from)?;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_bindings: HashMap::new(),
        imported_source_order: Vec::new(),
        module_map: HashMap::new(),
        include_instances: Vec::new(),
    };

    process_dag_body_import_declarations(
        project,
        loaded_dag,
        dag_body,
        file_src,
        module_resolver,
        &mut ctx,
    )?;
    process_dag_body_include_declarations(
        project,
        loaded_dag,
        dag_body,
        file_src,
        module_resolver,
        &mut ctx,
    )?;

    extend_imported_value_names(&mut ctx.imported_names, self_imports.names);
    extend_imported_bindings(&mut ctx.imported_bindings, self_imports.bindings, file_src)?;

    let dag_ast = graphcal_compiler::desugar::desugared_ast::File {
        declarations: self_imports.stripped_body,
    };
    validate_direct_dag_calls(
        loaded_dag.module(parent_loaded),
        project,
        loaded_dag.dag_id(),
        module_resolver,
        file_src,
    )?;
    let mut unfrozen =
        graphcal_compiler::ir::lower::lower_dag_module_with_imported_bindings_and_cancellation(
            graphcal_compiler::ir::lower::ModuleBody {
                ast: &dag_ast,
                interface: loaded_dag.interface(),
            },
            &ctx.imported_names,
            super::model::hir_imported_bindings(ctx.imported_bindings),
            file_src,
            loaded_dag.dag_id(),
            semantic.definitions,
            cancellation,
        )
        .map_err(Outcome::map_into)?;

    elaborate_include_instances(
        semantic,
        loaded_dag.dag_id(),
        &ctx.include_instances,
        file_src,
        loaded_dag.module(parent_loaded),
        &mut unfrozen,
        cancellation,
    )?;

    cancellation.checkpoint()?;
    store_and_freeze_module_template(
        semantic,
        loaded_dag.dag_id(),
        unfrozen,
        file_src,
        cancellation,
    )
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
                    name: name.to_string(),
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
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    for decl in dag_body {
        let DeclKind::Import(import_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::module_path::InlineBodyImportResolution::Resolved(target)) =
            loaded_dag.resolved_imports().get(
                &crate::loader::module_path::ModulePathKey::from_path(import_decl.path()),
            )
        else {
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
            module_resolver,
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
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    ctx: &mut ImportContext<'a>,
) -> Result<(), PipelineError> {
    for decl in dag_body {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        let Some(crate::loader::module_path::InlineBodyImportResolution::Resolved(target)) =
            loaded_dag.resolved_imports().get(
                &crate::loader::module_path::ModulePathKey::from_path(&include_decl.path),
            )
        else {
            continue;
        };
        if target.target() == target.source_file() {
            imports::process_file_include(
                project,
                target,
                include_decl,
                decl,
                &IncludingModule {
                    interface: loaded_dag.interface(),
                    source: file_src,
                    scope: StaticScope::new(loaded_dag.dag_id(), module_resolver),
                },
                ctx,
            )?;
            continue;
        }

        let Some((target_file, target_dag)) = project.inline_dag(target.target()) else {
            continue;
        };
        imports::process_inline_dag_include(
            &imports::InlineDagIncludeTarget {
                module: target_dag.module(target_file),
                dag_name: target_dag.declaration(target_file).name.value.as_str(),
            },
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
    for dep_eval in module_artifacts.values() {
        // Extern signatures travel with the dep's dag bodies: a qualified
        // inline call into a dep dag that uses extern functions resolves its
        // signature from the importer's merged TIR at eval time. Repeated
        // identical signatures are idempotent; a divergent copy is an
        // internal project-assembly error even if upstream checks missed it.
        for (key, function) in &dep_eval.extern_functions {
            tir.insert_extern_function(key.clone(), function.clone())
                .map_err(|error| {
                    PipelineError::Semantic(SemanticError::internal_error(
                        error.to_string(),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ))
                })?;
        }
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
    let mut substitution = request.static_bindings.substitution.clone();
    if let Some(aliases) = &request.selective_names {
        for alias in aliases {
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
                    target: template_reference(instance, source.clone()),
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
                    PipelineError::Semantic(SemanticError::internal_error(
                        format!("requested template plot `{source}` has no semantic target"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ))
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
/// 1. Resolve the include's source — file include reads the dep's full
///    AST; inline DAG include reads the dag block's body and pre-processes
///    `import <self>::{...}` against the dag's parent file (Concept 9: a
///    DAG's `<self>` is its file of definition, regardless of where the
///    include sits).
/// 2. Assemble the body with canonical imported targets set up.
/// 3. Validate every index binding against the body's effective typed
///    contract, with both sides resolved canonically.
/// 4. Preserve canonical A8/V005 reconciliation facts and run
///    `check_generics_leakage` (A9/V006).
/// 5. Record a typed semantic instance edge without copying dependency bodies.
/// 6. Materialize only selective projection aliases in the importer.
#[expect(
    clippy::too_many_lines,
    reason = "single cohesive include pipeline: source resolution, validation, HIR merge"
)]
fn elaborate_include_instances(
    semantic: &mut ProjectSemanticContext<'_, '_>,
    importer_dag_id: &graphcal_compiler::dag_id::DagId,
    include_instances: &[IncludeInstanceRequest],
    importer_src: SourceId,
    importer: crate::loader::loaded_file::LoadedModule<'_>,
    unfrozen: &mut graphcal_compiler::ir::model::UnfrozenIR,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<(), Outcome<PipelineError>> {
    let project = semantic.project;
    let module_resolver = semantic.module_resolver;
    for instance in include_instances {
        cancellation.checkpoint()?;
        // ---- 1. Resolve and assemble source body -----------------------------
        let template_id = instance.template.dag_id();
        let (template, dep_resolution_owner, body_decls_for_aliases) = match (
            semantic.module_templates.get(template_id),
            instance.template,
        ) {
            (Some(template), template_module) => (
                template,
                template_id.clone(),
                template_module.declarations(),
            ),
            (None, crate::loader::loaded_file::LoadedModule::FileRoot(dep_loaded)) => {
                let dep_dag_id = dep_loaded.dag_id();
                let dep_src = dep_loaded.source_id();
                let mut body_ctx = ImportContext {
                    imported_names: ImportedValueNames::default(),
                    imported_bindings: HashMap::new(),
                    imported_source_order: Vec::new(),
                    module_map: HashMap::new(),
                    include_instances: Vec::new(),
                };
                imports::process_file_body_declarations(
                    project,
                    dep_loaded,
                    module_resolver,
                    &mut body_ctx,
                    cancellation,
                )?;
                let dep_body = dep_loaded.ast();
                validate_direct_dag_calls(
                    dep_loaded.module(),
                    project,
                    dep_dag_id,
                    module_resolver,
                    dep_src,
                )?;
                let mut dep_unfrozen = graphcal_compiler::ir::lower::lower_module_with_imported_bindings_and_cancellation(
                                graphcal_compiler::ir::lower::ModuleBody {
                                    ast: dep_body,
                                    interface: dep_loaded.interface(),
                                },
                                dep_src,
                                &body_ctx.imported_names,
                                super::model::hir_imported_bindings(body_ctx.imported_bindings),
                                dep_dag_id,
                                semantic.definitions,
                                cancellation,
                            ).map_err(Outcome::map_into)?;
                elaborate_include_instances(
                    semantic,
                    dep_dag_id,
                    &body_ctx.include_instances,
                    dep_src,
                    dep_loaded.module(),
                    &mut dep_unfrozen,
                    cancellation,
                )?;
                let template = semantic.module_templates.insert(
                    dep_dag_id.clone(),
                    ElaboratedModuleTemplate {
                        unfrozen: dep_unfrozen,
                    },
                );
                (
                    template,
                    dep_dag_id.clone(),
                    dep_loaded.ast().declarations.as_slice(),
                )
            }
            (
                None,
                crate::loader::loaded_file::LoadedModule::InlineDag {
                    file: parent_loaded,
                    dag: loaded_inline,
                },
            ) => {
                let dag_id = loaded_inline.dag_id();
                let parent_dag_id = parent_loaded.dag_id();
                let inline_body = loaded_inline.body(parent_loaded);
                let self_imports = crate::inline_dag::preprocess_dag_body_self_imports(
                    inline_body,
                    parent_dag_id,
                    parent_loaded.interface(),
                    loaded_inline.resolved_imports(),
                    module_resolver,
                    importer_src,
                )
                .map_err(PipelineError::from)?;

                let mut body_ctx = ImportContext {
                    imported_names: ImportedValueNames::default(),
                    imported_bindings: HashMap::new(),
                    imported_source_order: Vec::new(),
                    module_map: HashMap::new(),
                    include_instances: Vec::new(),
                };
                process_dag_body_import_declarations(
                    project,
                    loaded_inline,
                    inline_body,
                    importer_src,
                    module_resolver,
                    &mut body_ctx,
                )?;
                process_dag_body_include_declarations(
                    project,
                    loaded_inline,
                    inline_body,
                    importer_src,
                    module_resolver,
                    &mut body_ctx,
                )?;
                extend_imported_value_names(&mut body_ctx.imported_names, self_imports.names);
                let mut imported_bindings = body_ctx.imported_bindings;
                extend_imported_bindings(
                    &mut imported_bindings,
                    self_imports.bindings,
                    importer_src,
                )?;
                let stripped_body = graphcal_compiler::desugar::desugared_ast::File {
                    declarations: self_imports.stripped_body,
                };
                validate_direct_dag_calls(
                    loaded_inline.module(parent_loaded),
                    project,
                    dag_id,
                    module_resolver,
                    importer_src,
                )?;
                let mut dag_unfrozen = graphcal_compiler::ir::lower::lower_dag_module_with_imported_bindings_and_cancellation(
                                graphcal_compiler::ir::lower::ModuleBody {
                                    ast: &stripped_body,
                                    interface: loaded_inline.interface(),
                                },
                                &body_ctx.imported_names,
                                super::model::hir_imported_bindings(imported_bindings),
                                importer_src,
                                dag_id,
                                semantic.definitions,
                                cancellation,
                            ).map_err(Outcome::map_into)?;
                elaborate_include_instances(
                    semantic,
                    dag_id,
                    &body_ctx.include_instances,
                    importer_src,
                    loaded_inline.module(parent_loaded),
                    &mut dag_unfrozen,
                    cancellation,
                )?;
                let template = semantic.module_templates.insert(
                    dag_id.clone(),
                    ElaboratedModuleTemplate {
                        unfrozen: dag_unfrozen,
                    },
                );
                (template, dag_id.clone(), inline_body)
            }
        };
        let dep_unfrozen = &template.unfrozen;

        // ---- 2. Validate typed index binding contracts -------------------
        validate_index_binding_contracts(
            semantic.definitions,
            &IndexBindingSites {
                importer: importer_dag_id,
                template: &dep_resolution_owner,
                template_declarations: body_decls_for_aliases,
                importer_src,
                include_span: instance.include_span,
            },
            &instance.static_bindings,
        )?;

        // ---- 4. Validation checks -----------------------------------------
        let override_reconciliations = dep_unfrozen
            .include_override_reconciliations(
                &instance.bindings,
                &instance.static_bindings.substitution,
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
            &instance.static_bindings.substitution,
            &IncludingModule {
                interface: importer.interface(),
                source: importer_src,
                scope: StaticScope::new(importer_dag_id, module_resolver),
            },
            instance.include_span,
        )?;

        // ---- 5. Retain one typed semantic edge -------------------------------
        let selective_alias_declarations = selective_alias_declarations(instance, dep_unfrozen);
        record_semantic_instance(
            unfrozen,
            instance,
            dep_unfrozen,
            override_reconciliations,
            importer_dag_id,
            module_resolver,
            importer_src,
        )?;

        // ---- 6. Add selective aliases -------------------------------------
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
                &instance.pub_reexport_items,
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

/// The two modules one include's index bindings connect, with the
/// diagnostic provenance of the include.
struct IndexBindingSites<'a> {
    importer: &'a graphcal_compiler::dag_id::DagId,
    template: &'a graphcal_compiler::dag_id::DagId,
    template_declarations: &'a [graphcal_compiler::desugar::desugared_ast::Declaration],
    importer_src: SourceId,
    include_span: Span,
}

fn validate_index_binding_contracts(
    definitions: &mut graphcal_compiler::ir::static_definitions::StaticDefinitionEvaluator<'_>,
    sites: &IndexBindingSites<'_>,
    bindings: &IncludeStaticBindings,
) -> Result<(), PipelineError> {
    use graphcal_compiler::ir::static_substitution::InstanceIndexBindingTarget;
    use graphcal_compiler::semantic::index_def::IndexBindingContractError;

    for (port, target) in &bindings.substitution.indexes {
        let site = bindings.index_sites.get(port).ok_or_else(|| {
            PipelineError::Semantic(SemanticError::internal_error(
                format!("bound index port `{port}` has no binding site"),
                sites.importer_src,
                graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(sites.include_span),
            ))
        })?;
        let candidate = match target {
            InstanceIndexBindingTarget::Declared(identity) => definitions.index(identity)?,
            InstanceIndexBindingTarget::Finite(finite) => {
                graphcal_compiler::semantic::index_def::IndexDef::finite(*finite)
            }
        };
        let contract = effective_index_binding_contract(
            definitions,
            sites,
            port,
            &bindings.substitution.dimensions,
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
                        dep_index: dep_index.to_string(),
                        dep_kind: expected.to_string(),
                        bound_index: site.authored.to_string(),
                        bound_kind: found.to_string(),
                    },
                )));
            }
            Err(IndexBindingContractError::DimensionMismatch { expected, found }) => {
                return Err(PipelineError::Semantic(SemanticError::located(
                    sites.importer_src,
                    site.span,
                    IndexError::IndexBindingDimensionMismatch {
                        dep_index: dep_index.to_string(),
                        expected_dim: definitions.format_dimension(sites.importer, &expected),
                        bound_index: site.authored.to_string(),
                        found_dim: definitions.format_dimension(sites.importer, &found),
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
/// selected item.
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
    public_originals: &HashSet<graphcal_compiler::syntax::names::NameAtom>,
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

        if public_originals.contains(orig_name.atom()) {
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
