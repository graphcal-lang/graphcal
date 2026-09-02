//! Include assembly and typed substitution for unfrozen per-DAG IR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{DimExpr, Expr, ExprKind, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::instance::{
    InstanceAssertionProjection, InstancePlotProjection, InstanceRecord, InstanceValueProjection,
};
use crate::ir::resolve::DeclCategory;
use crate::registry::error::GraphcalError;
use crate::registry::types::{self, Registry};
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::dimension::{DimName, ResolvedUnitName, UnitName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::{NameAtom, NamespacePath};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, StructTypeName};
use crate::syntax::visitor::ExprVisitor;

use super::{
    extern_fns::resolve_plugin_imports,
    lower::{
        AssertEntry, BodySource, ConstEntry, DynamicUnitScaleEntry, FigureEntry, HirDag,
        IncludeAliasDeclaration, LayerEntry, LoweredPlotBody, LoweredPlotField,
        LoweredPlotProperty, NodeEntry, ParamDefault, ParamEntry, PlotEntry, UnfrozenConstEntry,
        UnfrozenIR, UnfrozenNodeEntry, UnfrozenSemanticInstance,
    },
};

/// V005 obligations carried atomically from one include-site preflight into
/// its semantic instance edge.
#[derive(Debug, Clone, Default)]
pub struct IncludeOverrideReconciliations(
    HashMap<
        ResolvedDeclName,
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
    >,
);

/// Complete importer-side data needed to record one semantic include edge.
#[derive(Debug, Clone)]
pub struct SemanticInstanceInput {
    pub instance: InstanceRecord,
    pub debug_scope: ModuleAliasName,
    pub value_bindings: HashMap<ResolvedDeclName, Expr>,
    pub runtime_unit_names: HashSet<UnitName>,
    pub output_projections: Vec<InstanceValueProjection>,
    pub assertion_projections: Vec<InstanceAssertionProjection>,
    pub plot_projections: Vec<InstancePlotProjection>,
    pub override_reconciliations: IncludeOverrideReconciliations,
}

impl UnfrozenIR {
    /// Typed Static ports authored directly by this reusable DAG template.
    #[must_use]
    pub fn static_ports(&self) -> &[crate::hir::StaticPort] {
        &self.static_ports
    }

    /// Resolve an exposed plot alias to its canonical template declaration.
    #[must_use]
    pub fn plot_projection_target(&self, name: &DeclName) -> Option<ResolvedDeclName> {
        self.plots
            .iter()
            .find(|entry| entry.name.member() == name)
            .map(|entry| {
                ResolvedDeclName::from_def(
                    entry.body_resolution_owner.clone(),
                    entry.name.member().clone(),
                )
            })
            .or_else(|| {
                self.semantic_instances.iter().find_map(|instance| {
                    instance
                        .plot_projections
                        .iter()
                        .find(|projection| projection.exposed_name.member() == name)
                        .map(|projection| {
                            ResolvedDeclName::from_def(
                                instance.instance.id.owner().clone(),
                                projection.target.to_unowned_def_name(),
                            )
                        })
                })
            })
    }

    /// Names of assertions authored by this reusable DAG template.
    #[must_use]
    pub fn assertion_names(&self) -> Vec<DeclName> {
        self.asserts
            .iter()
            .map(|entry| entry.name.member().clone())
            .collect()
    }

    /// Record one include as a semantic edge rather than merging its syntax tree.
    pub fn record_semantic_instance(
        &mut self,
        input: SemanticInstanceInput,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
        if self
            .semantic_instances
            .iter()
            .any(|existing| existing.instance.id.owner() == input.instance.id.owner())
        {
            return Err(GraphcalError::InternalError {
                message: format!(
                    "duplicate semantic instance identity `{}`",
                    input.instance.id.owner()
                ),
                src: src.clone(),
                span: span.into(),
            });
        }
        self.semantic_instances.push(UnfrozenSemanticInstance {
            instance: input.instance,
            debug_scope: input.debug_scope,
            value_bindings: input.value_bindings,
            runtime_unit_names: input.runtime_unit_names,
            output_projections: input.output_projections,
            assertion_projections: input.assertion_projections,
            plot_projections: input.plot_projections,
            override_reconciliations: input.override_reconciliations.0,
        });
        Ok(())
    }

    /// Return the effective local declaration exposed by an include selector.
    ///
    /// This observes aliases created by earlier include/import composition, not
    /// only declarations authored directly in the producer's source AST.
    #[must_use]
    pub fn include_alias_declaration(&self, name: &DeclName) -> Option<IncludeAliasDeclaration> {
        let local = ScopedName::local(name.clone());
        self.consts
            .iter()
            .find(|entry| entry.name == local)
            .map(|entry| IncludeAliasDeclaration {
                type_ann: entry.type_ann.clone(),
                is_const: true,
            })
            .or_else(|| {
                self.params
                    .iter()
                    .find(|entry| entry.name == local)
                    .map(|entry| IncludeAliasDeclaration {
                        type_ann: entry.type_ann.clone(),
                        is_const: false,
                    })
            })
            .or_else(|| {
                self.nodes
                    .iter()
                    .find(|entry| entry.name == local)
                    .map(|entry| IncludeAliasDeclaration {
                        type_ann: entry.type_ann.clone(),
                        is_const: false,
                    })
            })
    }

    /// Publish a synthesized Term alias as part of this module's external surface.
    pub fn export_term_alias(&mut self, name: DeclName) {
        self.external_surface.insert_explicit_export(name);
    }

    /// Expose public runtime units through one semantic instance namespace.
    pub fn add_semantic_dynamic_unit_bindings<'a>(
        &mut self,
        units: impl IntoIterator<Item = &'a UnitName>,
        prefix: &ModuleAliasName,
        instance_owner: &crate::dag_id::DagId,
    ) {
        self.unit_bindings.extend(units.into_iter().map(|unit| {
            (
                UnitRef::qualified(NamespacePath::root(prefix.atom().clone()), unit.clone()),
                ResolvedUnitName::from_def(instance_owner.clone(), unit.clone()),
            )
        }));
    }

    /// Add a local alias for a dynamic unit already exposed under an include prefix.
    pub fn add_dynamic_unit_projection_alias(
        &mut self,
        prefix: &ModuleAliasName,
        source: &UnitName,
        alias: UnitName,
    ) {
        let prefixed =
            UnitRef::qualified(NamespacePath::root(prefix.atom().clone()), source.clone());
        if let Some(target) = self.unit_bindings.get(&prefixed).cloned() {
            self.unit_bindings.insert(UnitRef::local(alias), target);
        }
    }

    /// Freeze into a complete [`HirDag`] by providing a built [`Registry`] and
    /// the resolution context.
    ///
    /// This is the lowering boundary of the pipeline: every source declaration
    /// body and importer-context semantic-instance binding assembled so far is
    /// lowered to HIR here, so the frozen [`HirDag`] carries no syntax-AST
    /// expression.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any body contains a reference that
    /// cannot be resolved.
    pub fn freeze(
        self,
        registry: Registry,
        owner: &crate::dag_id::DagId,
        resolver: &crate::syntax::module_resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HirDag, GraphcalError> {
        self.freeze_with_cancellation(
            registry,
            owner,
            resolver,
            src,
            &crate::cancellation::CancellationToken::unbounded(),
        )
    }

    /// Freeze into IR while observing cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] for unresolved bodies or cancellation.
    #[expect(
        clippy::too_many_lines,
        reason = "single lowering boundary over every declaration kind"
    )]
    pub fn freeze_with_cancellation(
        self,
        registry: Registry,
        owner: &crate::dag_id::DagId,
        resolver: &crate::syntax::module_resolve::ModuleResolver,
        src: &NamedSource<Arc<String>>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<HirDag, GraphcalError> {
        cancellation.checkpoint()?;
        // Entries already visible in this IR (including prefixed include
        // instances and dag self-imports) bind their written names to
        // canonical identities for the lowering below.
        let instance_templates = self
            .instances
            .iter()
            .map(|record| (record.id.owner().clone(), record.id.template().clone()))
            .chain(self.semantic_instances.iter().map(|record| {
                (
                    record.instance.id.owner().clone(),
                    record.instance.id.template().clone(),
                )
            }))
            .collect::<HashMap<_, _>>();
        let mut decl_bindings = HashMap::new();
        for (name, declaration_owner) in self
            .consts
            .iter()
            .map(|entry| (&entry.name, &entry.declaration_owner))
            .chain(
                self.params
                    .iter()
                    .map(|entry| (&entry.name, &entry.declaration_owner)),
            )
            .chain(
                self.nodes
                    .iter()
                    .map(|entry| (&entry.name, &entry.declaration_owner)),
            )
        {
            cancellation.checkpoint()?;
            let canonical =
                ResolvedDeclName::from_def(declaration_owner.clone(), name.member().clone());
            decl_bindings.insert(name.clone(), canonical);
        }
        for record in &self.semantic_instances {
            let scope = ModuleAliasName::expect_valid(record.instance.id.owner().name());
            for target in record.instance.bindings.value_ports.values() {
                let name = ScopedName::qualified(scope.clone(), target.to_unowned_def_name());
                if decl_bindings.insert(name.clone(), target.clone()).is_some() {
                    return Err(GraphcalError::internal_error(
                        format!("semantic instance binding `{name}` collides with a declaration"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    ));
                }
            }
        }
        for (name, binding) in &self.imported_bindings {
            cancellation.checkpoint()?;
            if decl_bindings
                .insert(name.clone(), binding.target().clone())
                .is_some()
            {
                return Err(GraphcalError::internal_error(
                    format!("imported lexical binding `{name}` collides with a local declaration"),
                    src,
                    DiagnosticAnchor::WholeFile,
                ));
            }
        }

        let generic_scope = crate::hir::GenericScope::new();
        let prelude = crate::hir::PreludeTypeScope::graphcal();
        // A merged dependency body keeps the dependency file's byte offsets, so
        // a lowering error must render against that body's own source rather
        // than the importer's `src` (#868); `BodySource::resolve` selects it.
        let lower_in = |expr: &Expr,
                        resolution_owner: &crate::dag_id::DagId,
                        body_src: &NamedSource<Arc<String>>| {
            let expr_ctx = crate::hir::ExprLoweringContext::new(
                resolution_owner,
                resolver,
                &generic_scope,
                &registry.time_zones,
            )
            .with_prelude(&prelude)
            .with_unit_registry(&registry.units)
            .with_unit_bindings(&self.unit_bindings)
            .with_decl_bindings(&decl_bindings)
            .with_instance_templates(&instance_templates);
            crate::hir::lower_expr(expr, expr_ctx).map_err(|err| {
                crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, body_src)
            })
        };
        let lower_type_annotation_in =
            |type_ann: &TypeExpr,
             resolution_owner: &crate::dag_id::DagId,
             annotation_src: &NamedSource<Arc<String>>|
             -> Result<crate::hir::TypeAnnotation, GraphcalError> {
                crate::hir::diagnostics::validate_type_annotation(type_ann, annotation_src)?;
                let type_ctx = crate::hir::TypeLoweringContext::new(
                    resolution_owner,
                    resolver,
                    &generic_scope,
                )
                .with_prelude(&prelude);
                let type_expr =
                    crate::hir::lower_type_expr(type_ann, type_ctx).map_err(|error| {
                        crate::hir::diagnostics::type_lower_error_to_graphcal(
                            &error,
                            type_ann,
                            annotation_src,
                        )
                    })?;
                let domain_bounds = type_ann
                    .domain_bounds()
                    .iter()
                    .map(|bound| {
                        Ok(crate::hir::DomainBound {
                            kind: bound.kind,
                            value: lower_in(&bound.value, resolution_owner, annotation_src)?,
                            span: bound.span,
                        })
                    })
                    .collect::<Result<_, GraphcalError>>()?;
                Ok(crate::hir::TypeAnnotation {
                    type_expr,
                    domain_bounds,
                    span: type_ann.span,
                })
            };

        let nominal_types = crate::hir::nominal::lower_nominal_type_registry(
            owner,
            &registry,
            resolver,
            src,
            cancellation,
        )?;

        let dynamic_unit_scales = self
            .dynamic_unit_scales
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let entry_src = entry.src.resolve(src);
                let unit = resolver
                    .resolve_unit_path(&entry.unit_owner, &entry.spelling.to_name_path())
                    .map_err(|err| GraphcalError::InternalError {
                        message: format!(
                            "registered dynamic unit `{}` did not resolve canonically: {err}",
                            entry.spelling
                        ),
                        src: entry_src.clone(),
                        span: entry.span.into(),
                    })?;
                Ok(DynamicUnitScaleEntry {
                    unit,
                    spelling: entry.spelling.clone(),
                    expr: lower_in(&entry.expr, &entry.body_resolution_owner, entry_src)?,
                    declared_dimension: entry.declared_dimension.clone(),
                    base_unit_dimension: entry.base_unit_dimension.clone(),
                    span: entry.span,
                    src: entry_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        let consts = self
            .consts
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(ConstEntry {
                    name: entry.name.clone(),
                    declaration_owner: entry.declaration_owner.clone(),
                    type_ann: lower_type_annotation_in(
                        &entry.type_ann,
                        &entry.type_resolution_owner,
                        entry.type_src.resolve(src),
                    )?,
                    expr: lower_in(
                        &entry.expr,
                        &entry.body_resolution_owner,
                        entry.body_src.resolve(src),
                    )?,
                    span: entry.span,
                    type_src: entry.type_src.clone(),
                    body_src: entry.body_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let params = self
            .params
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let default = entry
                    .default
                    .as_ref()
                    .map(|default| {
                        lower_in(
                            &default.expr,
                            &default.resolution_owner,
                            default.src.resolve(src),
                        )
                        .map(|expr| ParamDefault {
                            expr,
                            src: default.src.clone(),
                        })
                    })
                    .transpose()?;
                Ok(ParamEntry {
                    name: entry.name.clone(),
                    declaration_owner: entry.declaration_owner.clone(),
                    type_ann: lower_type_annotation_in(
                        &entry.type_ann,
                        &entry.type_resolution_owner,
                        entry.type_src.resolve(src),
                    )?,
                    default,
                    span: entry.span,
                    type_src: entry.type_src.clone(),
                    override_reconciliations: entry.override_reconciliations.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let nodes = self
            .nodes
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(NodeEntry {
                    name: entry.name.clone(),
                    declaration_owner: entry.declaration_owner.clone(),
                    type_ann: lower_type_annotation_in(
                        &entry.type_ann,
                        &entry.type_resolution_owner,
                        entry.type_src.resolve(src),
                    )?,
                    expr: lower_in(
                        &entry.expr,
                        &entry.body_resolution_owner,
                        entry.body_src.resolve(src),
                    )?,
                    span: entry.span,
                    type_src: entry.type_src.clone(),
                    body_src: entry.body_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let asserts = self
            .asserts
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let body_src = entry.body_src.resolve(src);
                Ok(AssertEntry {
                    name: entry.name.clone(),
                    declaration_owner: entry.declaration_owner.clone(),
                    body: {
                        let expr_ctx = crate::hir::ExprLoweringContext::new(
                            &entry.body_resolution_owner,
                            resolver,
                            &generic_scope,
                            &registry.time_zones,
                        )
                        .with_prelude(&prelude)
                        .with_unit_registry(&registry.units)
                        .with_unit_bindings(&self.unit_bindings)
                        .with_decl_bindings(&decl_bindings)
                        .with_instance_templates(&instance_templates);
                        crate::hir::lower_assert_body(&entry.body, expr_ctx).map_err(|err| {
                            crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, body_src)
                        })?
                    },
                    span: entry.span,
                    body_src: entry.body_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        // Sink expressions are semantic program bodies, not optional rendering
        // hints. Batch compilation lowers every one strictly; tolerant HIR is
        // reserved for editor-facing incomplete buffers.
        cancellation.checkpoint()?;
        let plots = self
            .plots
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let body_src = entry.body_src.resolve(src);
                let encodings = entry
                    .decl
                    .encodings
                    .iter()
                    .map(|encoding| {
                        lower_in(&encoding.value, &entry.body_resolution_owner, body_src)
                            .map(|lowered| (encoding.channel, lowered))
                    })
                    .collect::<Result<Vec<_>, GraphcalError>>()?;
                let lower_fields = |fields: &[crate::desugar::desugared_ast::PlotField],
                                    classify: fn(
                    crate::syntax::ast::PlotPropertyName,
                ) -> LoweredPlotProperty| {
                    fields
                        .iter()
                        .map(|field| {
                            Ok(LoweredPlotField {
                                property: classify(field.name.value.clone()),
                                name_span: field.name.span,
                                value: lower_in(
                                    &field.value,
                                    &entry.body_resolution_owner,
                                    body_src,
                                )?,
                            })
                        })
                        .collect::<Result<Vec<_>, GraphcalError>>()
                };
                Ok(PlotEntry {
                    name: entry.name.clone(),
                    mark_type: entry.decl.mark.mark_type,
                    body: LoweredPlotBody {
                        encodings,
                        mark_properties: lower_fields(
                            &entry.decl.mark.properties,
                            LoweredPlotProperty::mark,
                        )?,
                        properties: lower_fields(
                            &entry.decl.properties,
                            LoweredPlotProperty::plot,
                        )?,
                    },
                    body_src: entry.body_src.clone(),
                    displayed: entry.displayed,
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        let lower_fields = |fields: &[crate::desugar::desugared_ast::PlotField],
                            resolution_owner: &crate::dag_id::DagId,
                            body_src: &NamedSource<Arc<String>>| {
            fields
                .iter()
                .map(|field| {
                    Ok(LoweredPlotField {
                        property: LoweredPlotProperty::composition(field.name.value.clone()),
                        name_span: field.name.span,
                        value: lower_in(&field.value, resolution_owner, body_src)?,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()
        };
        cancellation.checkpoint()?;
        let figures = self
            .figures
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(FigureEntry {
                    name: entry.name.clone(),
                    plot_names: entry.decl.plot_names.clone(),
                    fields: lower_fields(
                        &entry.decl.fields,
                        &entry.body_resolution_owner,
                        entry.body_src.resolve(src),
                    )?,
                    body_src: entry.body_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        cancellation.checkpoint()?;
        let layers = self
            .layers
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok(LayerEntry {
                    name: entry.name.clone(),
                    plot_names: entry.decl.plot_names.clone(),
                    fields: lower_fields(
                        &entry.decl.fields,
                        &entry.body_resolution_owner,
                        entry.body_src.resolve(src),
                    )?,
                    body_src: entry.body_src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        cancellation.checkpoint()?;

        let semantic_instances = self
            .semantic_instances
            .iter()
            .map(|record| {
                let value_bindings = record
                    .value_bindings
                    .iter()
                    .map(|(port, expr)| lower_in(expr, owner, src).map(|expr| (port.clone(), expr)))
                    .collect::<Result<HashMap<_, _>, _>>()?;
                Ok(crate::ir::instance::HirInstanceRecord {
                    instance: record.instance.clone(),
                    debug_scope: record.debug_scope.clone(),
                    value_bindings,
                    runtime_unit_names: record.runtime_unit_names.clone(),
                    output_projections: record.output_projections.clone(),
                    assertion_projections: record.assertion_projections.clone(),
                    plot_projections: record.plot_projections.clone(),
                    owner_rebases: HashMap::new(),
                    override_reconciliations: record.override_reconciliations.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        let extern_functions =
            resolve_plugin_imports(&self.plugin_imports, &registry, owner, resolver, src)?;

        // Syntax-backed nominal definitions are a lowering capability, not a
        // HIR semantic authority. The phase-specific registry makes retaining
        // them after `nominal_types` is complete impossible.
        let registry = registry.into_semantic();

        Ok(HirDag {
            dag_id: owner.clone(),
            extern_functions,
            registry,
            nominal_types,
            consts,
            params,
            nodes,
            asserts,
            plots,
            figures,
            layers,
            included_plots: self.included_plots,
            source_order: self.source_order,
            source_declarations: self.source_declarations,
            static_ports: self.static_ports,
            assumes_map: self.assumes_map,
            expected_fail: self.expected_fail,
            dynamic_unit_scales,
            imported_bindings: self.imported_bindings,
            external_surface: self.external_surface,
            instances: self.instances,
            semantic_instances,
        })
    }

    /// Add a const alias: a synthetic const declaration that references another const.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_const_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.consts.push(UnfrozenConstEntry {
            name: name.clone(),
            declaration_owner: body_resolution_owner.clone(),
            type_ann,
            type_resolution_owner,
            expr,
            body_resolution_owner,
            span,
            // Alias declarations and bodies are synthesized from the
            // importer's include statement.
            type_src: BodySource::own(),
            body_src: BodySource::own(),
        });
        self.source_order.push((name, DeclCategory::Const));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: ScopedName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.nodes.push(UnfrozenNodeEntry {
            name: name.clone(),
            declaration_owner: body_resolution_owner.clone(),
            type_ann,
            type_resolution_owner,
            expr,
            body_resolution_owner,
            span,
            // Alias declarations and bodies are synthesized from the
            // importer's include statement.
            type_src: BodySource::own(),
            body_src: BodySource::own(),
        });
        self.source_order.push((name, DeclCategory::Node));
    }

    /// Record include-site nominal override obligations before substitution.
    ///
    /// Field and generic-argument ownership is checked later from canonical
    /// inferred HIR. Explicit index labels and constructors need a narrow
    /// registry-backed preflight because substitution can make HIR lowering
    /// reject them before TIR exists.
    #[expect(
        clippy::too_many_arguments,
        reason = "the include boundary supplies bindings, canonical owners, registry, and source provenance"
    )]
    pub fn include_override_reconciliations(
        &self,
        bindings: &HashMap<DeclName, Expr>,
        index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        dependency_registry: &Registry,
        dependency_owner: &crate::dag_id::DagId,
        importer_owner: &crate::dag_id::DagId,
        importer_src: &NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Result<IncludeOverrideReconciliations, GraphcalError> {
        self.params
            .iter()
            .filter(|param| !bindings.contains_key(param.name.member()))
            .map(|param| {
                let mut reconciliations = param.override_reconciliations.clone();
                if let Some(default) = &param.default
                    && (!index_bindings.is_empty() || !type_bindings.is_empty())
                {
                    NominalOverridePreflight {
                        index_bindings,
                        type_bindings,
                        type_registry: &dependency_registry.types,
                        orphan_decl: param.name.member(),
                        importer_src,
                        include_span,
                    }
                    .visit_expr(&default.expr)?;
                    reconciliations.push(
                        crate::ir::override_reconciliation::PendingOverrideReconciliation::new(
                            param.name.member().clone(),
                            dependency_owner,
                            importer_owner,
                            index_bindings,
                            type_bindings,
                            importer_src.clone(),
                            include_span,
                        ),
                    );
                }
                Ok((
                    ResolvedDeclName::from_def(
                        param.declaration_owner.clone(),
                        param.name.member().clone(),
                    ),
                    reconciliations,
                ))
            })
            .filter_map(|result| match result {
                Ok((_, reconciliations)) if reconciliations.is_empty() => None,
                result => Some(result),
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map(IncludeOverrideReconciliations)
    }
}

/// Narrow pre-HIR guard for nominal syntax resolved by the producer registry.
///
/// HIR lowering validates explicit labels and constructor patterns against the
/// substituted registry. An incompatible replacement could therefore fail
/// before TIR ownership is available. Field and generic-argument dependencies
/// remain deferred to canonical inference.
struct NominalOverridePreflight<'a> {
    index_bindings: &'a HashMap<IndexName, types::IndexBindingTarget>,
    type_bindings: &'a HashMap<StructTypeName, StructTypeName>,
    type_registry: &'a crate::registry::types::TypeRegistry,
    orphan_decl: &'a DeclName,
    importer_src: &'a NamedSource<Arc<String>>,
    include_span: Span,
}

impl NominalOverridePreflight<'_> {
    fn check_label(&self, index: &IndexName, detail: String) -> Result<(), GraphcalError> {
        if !self.index_bindings.contains_key(index) {
            return Ok(());
        }
        Err(GraphcalError::IncludeMustReconcileOverride {
            overridden: index.to_string(),
            overridden_kind: "index".to_string(),
            orphan_decl: self.orphan_decl.to_string(),
            detail,
            src: self.importer_src.clone(),
            span: self.include_span.into(),
        })
    }

    fn check_constructor(
        &self,
        constructor: &ConstructorName,
        detail: String,
    ) -> Result<(), GraphcalError> {
        let Some((owning_type, _)) = self.type_registry.lookup_ctor(constructor) else {
            return Ok(());
        };
        if !self.type_bindings.contains_key(owning_type.name()) {
            return Ok(());
        }
        Err(GraphcalError::IncludeMustReconcileOverride {
            overridden: owning_type.name().to_string(),
            overridden_kind: "type".to_string(),
            orphan_decl: self.orphan_decl.to_string(),
            detail,
            src: self.importer_src.clone(),
            span: self.include_span.into(),
        })
    }
}

impl ExprVisitor<crate::syntax::phase::Desugared> for NominalOverridePreflight<'_> {
    type Error = GraphcalError;

    fn visit_unresolved_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        let ExprKind::UnresolvedRef(reference) = &expr.kind else {
            return Ok(());
        };
        match reference {
            crate::syntax::ast::UnresolvedRef::IndexLabel { index, label, .. } => {
                let name = IndexName::from_atom(index.leaf().name.clone());
                self.check_label(&name, format!("`{index}#{}`", label.value))
            }
            crate::syntax::ast::UnresolvedRef::Path(path) => {
                if let Some(name) = path.as_bare() {
                    let constructor = ConstructorName::from_atom(name.name.clone());
                    self.check_constructor(&constructor, format!("constructor `{constructor}`"))?;
                }
                Ok(())
            }
        }
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::IndexAccess { args, .. } = &expr.kind {
            for arg in args {
                if let crate::desugar::desugared_ast::IndexArg::Variant { index, variant } = arg {
                    let name = IndexName::from_atom(index.value.leaf().clone());
                    self.check_label(&name, format!("`{}#{}`", index.value, variant.value))?;
                }
            }
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(
        &mut self,
        _expr: &Expr,
        entries: &[crate::desugar::desugared_ast::MapEntry],
    ) -> Result<(), Self::Error> {
        for entry in entries {
            for key in &entry.keys {
                if let crate::syntax::ast::MapEntryIndex::Named(index_name) = &key.index.value {
                    let index = IndexName::from_atom(index_name.leaf().clone());
                    self.check_label(&index, format!("`{}#{}`", index_name, key.variant.value))?;
                }
            }
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_match(
        &mut self,
        _expr: &Expr,
        scrutinee: &Expr,
        arms: &[crate::desugar::desugared_ast::MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            match &arm.pattern {
                crate::desugar::desugared_ast::MatchPattern::IndexLabel {
                    index, variant, ..
                } => {
                    let name = IndexName::from_atom(index.value.leaf().clone());
                    self.check_label(&name, format!("`{}#{}`", index.value, variant.value))?;
                }
                crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                    if let Some(name) = path.as_bare() {
                        let constructor = ConstructorName::from_atom(name.name.clone());
                        self.check_constructor(
                            &constructor,
                            format!("match constructor `{constructor}`"),
                        )?;
                    }
                }
                crate::desugar::desugared_ast::MatchPattern::Constructor { name, .. } => {
                    self.check_constructor(
                        &name.value,
                        format!("match constructor `{}`", name.value),
                    )?;
                }
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }

    fn visit_constructor_call(
        &mut self,
        expr: &Expr,
        fields: &[crate::desugar::desugared_ast::FieldInit],
    ) -> Result<(), Self::Error> {
        if let ExprKind::ConstructorCall { callee, .. } = &expr.kind
            && let Some(name) = callee.as_bare()
        {
            let constructor = ConstructorName::from_atom(name.name.clone());
            self.check_constructor(&constructor, format!("constructor `{constructor}(...)`"))?;
        }
        fields
            .iter()
            .try_for_each(|field| self.visit_expr(&field.value))
    }
}

fn rewrite_ambiguous_generic_arg_names<K>(
    arg: &mut crate::desugar::desugared_ast::AmbiguousGenericArg,
    bindings: &HashMap<K, K>,
) where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    match arg {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            if let Some(new_name) = bindings.get(ident.name.as_str())
                && let Ok(atom) = NameAtom::parse(new_name.as_ref())
            {
                ident.name = atom;
            }
        }
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                rewrite_ambiguous_generic_arg_names(operand, bindings);
            }
        }
    }
}

fn rewrite_ambiguous_index_arg_declared_names(
    arg: &mut crate::desugar::desugared_ast::AmbiguousGenericArg,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    match arg {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => {
            if let Some(name) = bindings
                .get(ident.name.as_str())
                .and_then(types::IndexBindingTarget::declared_name)
            {
                ident.name = name.atom().clone();
            }
        }
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                rewrite_ambiguous_index_arg_declared_names(operand, bindings);
            }
        }
    }
}

fn index_expr_for_binding_target(
    target: &types::IndexBindingTarget,
    span: Span,
) -> crate::desugar::desugared_ast::IndexExpr {
    use crate::desugar::desugared_ast::{IndexExpr, NatExpr};

    match target {
        types::IndexBindingTarget::Declared(name) => IndexExpr::Name(Spanned::new(
            crate::syntax::names::NamePath::expect_local(name.as_str()),
            span,
        )),
        types::IndexBindingTarget::Finite(finite) => IndexExpr::Finite {
            cardinality: NatExpr::Literal(finite.size_u64(), span),
            span,
        },
    }
}

fn substitute_generic_arg_indexes(
    arg: &mut crate::desugar::desugared_ast::GenericArg,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    use crate::desugar::desugared_ast::{AmbiguousGenericArg, GenericArg};

    let finite_replacement = match arg {
        GenericArg::Ambiguous(AmbiguousGenericArg::Name(ident)) => bindings
            .get(ident.name.as_str())
            .filter(|target| matches!(target, types::IndexBindingTarget::Finite(_)))
            .map(|target| GenericArg::Index(index_expr_for_binding_target(target, ident.span))),
        GenericArg::Type(_)
        | GenericArg::Index(_)
        | GenericArg::Nat(_)
        | GenericArg::Ambiguous(AmbiguousGenericArg::Mul(..)) => None,
    };
    if let Some(replacement) = finite_replacement {
        *arg = replacement;
        return;
    }

    match arg {
        GenericArg::Type(type_expr) => substitute_type_expr_indexes(type_expr, bindings),
        GenericArg::Index(index) => substitute_index_expr(index, bindings),
        GenericArg::Ambiguous(ambiguous) => {
            rewrite_ambiguous_index_arg_declared_names(ambiguous, bindings);
        }
        GenericArg::Nat(_) => {}
    }
}

fn substitute_generic_arg_nominal_names<K>(
    arg: &mut crate::desugar::desugared_ast::GenericArg,
    bindings: &HashMap<K, K>,
) where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            substitute_type_expr_nominal_names(type_expr, bindings);
        }
        crate::desugar::desugared_ast::GenericArg::Index(_)
        | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
        crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
            rewrite_ambiguous_generic_arg_names(ambiguous, bindings);
        }
    }
}

fn substitute_index_expr(
    index: &mut crate::desugar::desugared_ast::IndexExpr,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    let replacement = match index {
        crate::desugar::desugared_ast::IndexExpr::Name(path) => path
            .value
            .as_bare()
            .and_then(|atom| bindings.get(atom.as_str()))
            .map(|target| index_expr_for_binding_target(target, path.span)),
        crate::desugar::desugared_ast::IndexExpr::Finite { .. }
        | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => None,
    };
    if let Some(replacement) = replacement {
        *index = replacement;
    }
}

/// Rewrite index arguments within a type expression according to a binding map.
///
/// `TypeExpr` is not part of the `Expr` tree, so it needs a separate
/// substitution pass. This rewrites index identifiers in `Indexed` types to a
/// declared target or structural `Fin(N)` target and recurses into
/// `TypeApplication` arguments.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_type_expr_indexes(
    type_expr: &mut TypeExpr,
    bindings: &HashMap<IndexName, types::IndexBindingTarget>,
) {
    use crate::desugar::desugared_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::Indexed { base, indexes } => {
            for index in indexes {
                substitute_index_expr(index, bindings);
            }
            substitute_type_expr_indexes(base, bindings);
        }
        TypeExprKind::TypeApplication { generic_args, .. } => {
            for arg in generic_args {
                substitute_generic_arg_indexes(arg, bindings);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args {
                substitute_generic_arg_indexes(arg, bindings);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                substitute_type_expr_indexes(arg, bindings);
            }
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => {}
    }
}

/// Rewrite bare dimension names within a dimension expression.
///
/// This is the shared syntactic substitution used by include-time dimension
/// bindings. Qualified references retain their producer-owned path; only a bare
/// bindable declaration name can be replaced by an importer-side binding.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_dim_expr_names<K>(dim_expr: &mut DimExpr, bindings: &HashMap<K, K>)
where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    for item in &mut dim_expr.terms {
        if let Some(atom) = item.term.name.value.as_bare()
            && let Some(new_name) = bindings.get(atom.as_str())
        {
            item.term.name.value = crate::syntax::names::NamePath::expect_local(new_name.as_ref());
        }
    }
}

/// Rewrite nominally-tied names (types or dimensions) within a type expression.
///
/// `TypeExpr` uses `DimExpr` to carry single-identifier type references (the
/// resolver disambiguates them into `StructType` / `Dim` later). Both type and
/// dimension bindings therefore need to walk `DimExpr` terms and rewrite their
/// names. `TypeApplication.name` is rewritten for type bindings (generic
/// parametric types like `Vec3<Length>`), which is harmless for dim bindings
/// because type and dim names can't collide (A6 nominal identity).
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn substitute_type_expr_nominal_names<K>(type_expr: &mut TypeExpr, bindings: &HashMap<K, K>)
where
    K: std::hash::Hash + Eq + std::borrow::Borrow<str> + AsRef<str>,
{
    use crate::desugar::desugared_ast::TypeExprKind;

    if bindings.is_empty() {
        return;
    }
    match &mut type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => substitute_dim_expr_names(dim_expr, bindings),
        TypeExprKind::Indexed { base, .. } => {
            substitute_type_expr_nominal_names(base, bindings);
        }
        TypeExprKind::TypeApplication { name, generic_args } => {
            if let Some(atom) = name.value.as_bare()
                && let Some(new_name) = bindings.get(atom.as_str())
            {
                name.value = crate::syntax::names::NamePath::expect_local(new_name.as_ref());
            }
            for arg in generic_args {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            for arg in generic_args {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // The built-in `Datetime` name is fixed; only the type args can
            // carry user-bindable nominal names.
            for arg in type_args {
                substitute_type_expr_nominal_names(arg, bindings);
            }
        }
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => {}
    }
}

/// Apply include-time Static substitutions to a registered nominal signature.
#[expect(
    clippy::implicit_hasher,
    reason = "the include boundary uses the project's canonical binding-map types"
)]
pub fn specialize_type_definition(
    definition: &mut crate::registry::type_def::TypeDef,
    index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) {
    definition.transform_signatures(
        |argument| {
            substitute_generic_arg_indexes(argument, index_bindings);
            substitute_generic_arg_nominal_names(argument, type_bindings);
            substitute_generic_arg_nominal_names(argument, dim_bindings);
        },
        |type_expr| {
            substitute_type_expr_indexes(type_expr, index_bindings);
            substitute_type_expr_nominal_names(type_expr, type_bindings);
            substitute_type_expr_nominal_names(type_expr, dim_bindings);
        },
    );
}
