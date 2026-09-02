//! Include assembly and typed substitution for unfrozen per-DAG IR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DescendantRebase;
use crate::desugar::desugared_ast::{DimExpr, Expr, ExprKind, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::instance::{InstanceBindingEnvironment, InstanceIndexBindingTarget, InstanceRecord};
use crate::ir::resolve::{DeclCategory, ExpectedFail};
use crate::registry::error::GraphcalError;
use crate::registry::types::{self, Registry};
use crate::syntax::decl_name::{DeclName, ResolvedDeclName};
use crate::syntax::dimension::{DimName, ResolvedDimName, ResolvedUnitName, UnitName, UnitRef};
use crate::syntax::index_name::{IndexName, ResolvedIndexName};
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::{NameAtom, NameNamespace, NamespacePath, ResolvedName};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, ResolvedStructTypeName, StructTypeName};
use crate::syntax::visitor::{ExprVisitor, ExprVisitorMut};

use super::{
    extern_fns::resolve_plugin_imports,
    lower::{
        AssertEntry, BodySource, ConstEntry, DynamicUnitScaleEntry, FigureEntry, HirDag,
        IncludeAliasDeclaration, LayerEntry, LoweredPlotBody, LoweredPlotField,
        LoweredPlotProperty, NodeEntry, ParamDefault, ParamEntry, ParsedExpectedFailMetadata,
        PlotEntry, RequestedPlot, UnfrozenAssertEntry, UnfrozenConstEntry, UnfrozenIR,
        UnfrozenNodeEntry, UnfrozenParamDefault, UnfrozenParamEntry, UnfrozenPlotEntry,
    },
    registry_build::{assert_body_uses_local_units, expr_uses_local_units, plot_uses_local_units},
};

impl UnfrozenIR {
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
    /// This is the lowering boundary of the pipeline: every declaration body
    /// assembled so far (including merged include instances and applied
    /// overrides) is lowered to HIR here, so the frozen [`HirDag`] carries no
    /// syntax-AST expression.
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
        })
    }

    /// Retarget the resolution scope of declarations already present in this
    /// unfrozen IR.
    ///
    /// Inline DAG include instances are lowered under an instance owner so
    /// merged declaration keys stay unique, but their source bodies must still
    /// resolve type-system names and constructors in the DAG's definition
    /// scope. Call this before merging nested includes so later nested entries
    /// keep their own producer scopes.
    pub fn retarget_existing_resolution_owners(&mut self, owner: &crate::dag_id::DagId) {
        for entry in &mut self.dynamic_unit_scales {
            entry.unit_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.consts {
            entry.declaration_owner = owner.clone();
            entry.type_resolution_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.params {
            entry.declaration_owner = owner.clone();
            entry.type_resolution_owner = owner.clone();
            if let Some(default) = &mut entry.default {
                default.resolution_owner = owner.clone();
            }
        }
        for entry in &mut self.nodes {
            entry.declaration_owner = owner.clone();
            entry.type_resolution_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.asserts {
            entry.declaration_owner = owner.clone();
            entry.body_resolution_owner = owner.clone();
        }
        for metadata in self.expected_fail.values_mut() {
            metadata.resolution_owner = owner.clone();
        }
        for entry in &mut self.plots {
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.figures {
            entry.body_resolution_owner = owner.clone();
        }
        for entry in &mut self.layers {
            entry.body_resolution_owner = owner.clone();
        }
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
    pub fn record_include_override_reconciliations(
        &mut self,
        bindings: &HashMap<DeclName, Expr>,
        index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        dependency_registry: &Registry,
        dependency_owner: &crate::dag_id::DagId,
        importer_owner: &crate::dag_id::DagId,
        importer_src: &NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Result<(), GraphcalError> {
        if index_bindings.is_empty() && type_bindings.is_empty() {
            return Ok(());
        }
        for param in &mut self.params {
            if bindings.contains_key(param.name.member()) {
                param.override_reconciliations.clear();
                continue;
            }
            let Some(default) = &param.default else {
                continue;
            };
            NominalOverridePreflight {
                index_bindings,
                type_bindings,
                type_registry: &dependency_registry.types,
                orphan_decl: param.name.member(),
                importer_src,
                include_span,
            }
            .visit_expr(&default.expr)?;
            param.override_reconciliations.push(
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
        Ok(())
    }

    /// Interleave declaration blocks merged after local lowering at their
    /// include sites, while preserving each block's producer source order.
    pub fn interleave_merged_source_order(
        &mut self,
        mut blocks: Vec<(Span, Vec<(ScopedName, DeclCategory)>)>,
    ) {
        fn entry_span(ir: &UnfrozenIR, name: &ScopedName, category: DeclCategory) -> Option<Span> {
            match category {
                DeclCategory::Const => ir
                    .consts
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.span),
                DeclCategory::Param => ir
                    .params
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.span),
                DeclCategory::Node => ir
                    .nodes
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.span),
                DeclCategory::Assert => ir
                    .asserts
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.span),
                DeclCategory::Plot => ir
                    .plots
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.span),
                DeclCategory::Figure => ir
                    .figures
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.decl.name.span),
                DeclCategory::Layer => ir
                    .layers
                    .iter()
                    .find(|entry| &entry.name == name)
                    .map(|entry| entry.decl.name.span),
            }
        }

        blocks.sort_by_key(|(span, _)| span.offset());
        let merged_count = blocks
            .iter()
            .map(|(_, entries)| entries.len())
            .sum::<usize>();
        let local_order = std::mem::take(&mut self.source_order);
        let mut interleaved = Vec::with_capacity(local_order.len() + merged_count);
        let mut blocks = blocks.into_iter().peekable();
        for entry in local_order {
            let offset = entry_span(self, &entry.0, entry.1).map_or(usize::MAX, Span::offset);
            while blocks
                .peek()
                .is_some_and(|(span, _)| span.offset() < offset)
            {
                if let Some((_, entries)) = blocks.next() {
                    interleaved.extend(entries);
                }
            }
            interleaved.push(entry);
        }
        interleaved.extend(blocks.flat_map(|(_, entries)| entries));
        self.source_order = interleaved;
    }

    /// Merge an instantiated dependency's IR into this IR.
    ///
    /// All declarations from the dependency are prefixed with `prefix.` and
    /// appended to this IR's declaration lists. Param bindings replace the
    /// dependency's param default expressions. Internal references within the
    /// dependency's expressions are rewritten to use prefixed names.
    ///
    /// `dep_names` is the set of all declaration names in the dependency (before
    /// prefixing), used to determine which references should be rewritten.
    ///
    /// `dep_src` is the dependency body's [`NamedSource`]: merged declarations
    /// keep the dependency file's byte offsets, so each is tagged with it (via
    /// `BodySource::or_dependency`) for diagnostics raised at the importer's
    /// freeze/TIR boundary (#868). For inline-DAG includes the body shares the
    /// importer's source, so `dep_src` equals `importer_src` there.
    #[expect(
        clippy::too_many_lines,
        reason = "single logical operation: prefix and merge all declaration kinds"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "merge_dependency coordinates every binding kind plus prefixing state"
    )]
    pub fn merge_dependency(
        &mut self,
        mut dep: Self,
        prefix: &ModuleAliasName,
        bindings: &HashMap<DeclName, Expr>,
        dep_names: &HashSet<DeclName>,
        index_bindings: &HashMap<IndexName, types::IndexBindingTarget>,
        type_bindings: &HashMap<StructTypeName, StructTypeName>,
        dim_bindings: &HashMap<DimName, DimName>,
        assertion_aliases: &HashMap<DeclName, DeclName>,
        import_item_attributes: &HashMap<DeclName, Vec<crate::desugar::desugared_ast::Attribute>>,
        requested_plots: &HashMap<DeclName, RequestedPlot>,
        dependency_owner: &crate::dag_id::DagId,
        importer_owner: &crate::dag_id::DagId,
        include_span: Span,
        importer_src: &NamedSource<Arc<String>>,
        dep_src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        /// Prefix a `ScopedName` if it is an unqualified member owned by
        /// the dependency.
        ///
        /// Already-qualified names that identify nested declarations owned by
        /// this dependency gain an outer instance segment. Other qualified
        /// names (for example a module import used by the dependency) retain
        /// their original external namespace.
        fn prefix_dep(
            name: &ScopedName,
            prefix: &ModuleAliasName,
            dep_names: &HashSet<DeclName>,
            dep_scoped_names: &HashSet<ScopedName>,
        ) -> ScopedName {
            if dep_scoped_names.contains(name) {
                name.within_scope(prefix)
            } else if !name.is_qualified() && dep_names.contains(name.member()) {
                name.with_prefix(prefix)
            } else {
                name.clone()
            }
        }

        fn prefix_unit_ref(reference: &UnitRef, prefix: &ModuleAliasName) -> UnitRef {
            let rest = reference
                .qualifier()
                .map_or_else(Vec::new, |owner| owner.segments().to_vec());
            UnitRef::qualified(
                NamespacePath::new(NonEmpty::new(prefix.atom().clone(), rest)),
                reference.name().clone(),
            )
        }

        fn rebase_descendant_or_preserve_external(
            owner: crate::dag_id::DagId,
            dependency_owner: &crate::dag_id::DagId,
            instance_owner: &crate::dag_id::DagId,
        ) -> crate::dag_id::DagId {
            match owner.rebase_descendant(dependency_owner, instance_owner) {
                DescendantRebase::Rebased(rebased) => rebased,
                DescendantRebase::OutsideSubtree => owner,
            }
        }

        fn require_rebased_instance_owner(
            owner: &crate::dag_id::DagId,
            dependency_owner: &crate::dag_id::DagId,
            instance_owner: &crate::dag_id::DagId,
            importer_src: &NamedSource<Arc<String>>,
            include_span: Span,
        ) -> Result<crate::dag_id::DagId, GraphcalError> {
            match owner.rebase_descendant(dependency_owner, instance_owner) {
                DescendantRebase::Rebased(rebased) => Ok(rebased),
                DescendantRebase::OutsideSubtree => Err(GraphcalError::internal_error(
                    format!(
                        "instance-owned DAG `{owner}` is outside dependency subtree `{dependency_owner}`"
                    ),
                    importer_src,
                    DiagnosticAnchor::Source(include_span),
                )),
            }
        }

        fn rebase_resolved_name<Ns: NameNamespace>(
            name: &ResolvedName<Ns>,
            dependency_owner: &crate::dag_id::DagId,
            instance_owner: &crate::dag_id::DagId,
        ) -> ResolvedName<Ns> {
            let owner = rebase_descendant_or_preserve_external(
                name.owner().clone(),
                dependency_owner,
                instance_owner,
            );
            ResolvedName::from_def(owner, name.to_unowned_def_name())
        }

        // Include-time type-system bindings rewrite selected producer names
        // to importer-side identifiers. Keep those rewritten bodies/signatures
        // in the importer scope; otherwise preserve producer scope so an
        // include consumer need not import constructors/types it never names.
        let type_system_bindings_present =
            !index_bindings.is_empty() || !type_bindings.is_empty() || !dim_bindings.is_empty();
        let instance_owner = importer_owner.instance_child(prefix.as_str());
        let merge_resolution_owner = |producer_owner: crate::dag_id::DagId| {
            if type_system_bindings_present {
                importer_owner.clone()
            } else {
                producer_owner
            }
        };
        let dynamic_unit_names = dep
            .dynamic_unit_scales
            .iter()
            .map(|entry| entry.spelling.name().clone())
            .collect::<HashSet<_>>();
        let merge_body_resolution_owner =
            |producer_owner: crate::dag_id::DagId, uses_dynamic_unit: bool| {
                if uses_dynamic_unit {
                    require_rebased_instance_owner(
                        &producer_owner,
                        dependency_owner,
                        &instance_owner,
                        importer_src,
                        include_span,
                    )
                } else {
                    Ok(merge_resolution_owner(producer_owner))
                }
            };

        let value_ports = dep
            .params
            .iter()
            .filter(|entry| !entry.name.is_qualified())
            .map(|entry| {
                let template = ResolvedDeclName::from_def(
                    dependency_owner.clone(),
                    entry.name.member().clone(),
                );
                let concrete =
                    ResolvedDeclName::from_def(instance_owner.clone(), entry.name.member().clone());
                (template, concrete)
            })
            .collect::<HashMap<_, _>>();
        let explicitly_bound_values = bindings
            .keys()
            .map(|name| ResolvedDeclName::from_def(dependency_owner.clone(), name.clone()))
            .collect();
        let indexes = index_bindings
            .iter()
            .map(|(source, target)| {
                let source = ResolvedIndexName::from_def(dependency_owner.clone(), source.clone());
                let target = match target {
                    types::IndexBindingTarget::Declared(target) => {
                        InstanceIndexBindingTarget::Declared(ResolvedIndexName::from_def(
                            importer_owner.clone(),
                            target.clone(),
                        ))
                    }
                    types::IndexBindingTarget::Finite(target) => {
                        InstanceIndexBindingTarget::Finite(*target)
                    }
                };
                (source, target)
            })
            .collect();
        let types = type_bindings
            .iter()
            .map(|(source, target)| {
                (
                    ResolvedStructTypeName::from_def(dependency_owner.clone(), source.clone()),
                    ResolvedStructTypeName::from_def(importer_owner.clone(), target.clone()),
                )
            })
            .collect();
        let dimensions = dim_bindings
            .iter()
            .map(|(source, target)| {
                (
                    ResolvedDimName::from_def(dependency_owner.clone(), source.clone()),
                    ResolvedDimName::from_def(importer_owner.clone(), target.clone()),
                )
            })
            .collect();
        let mut instance_records = vec![InstanceRecord {
            id: crate::dag_id::InstanceId::new(instance_owner.clone(), dependency_owner.clone()),
            parent_owner: importer_owner.clone(),
            bindings: InstanceBindingEnvironment {
                value_ports,
                explicitly_bound_values,
                indexes,
                types,
                dimensions,
            },
        }];
        let nested_instance_records = std::mem::take(&mut dep.instances)
            .into_iter()
            .map(|mut record| {
                record.parent_owner = require_rebased_instance_owner(
                    &record.parent_owner,
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?;
                let concrete_owner = require_rebased_instance_owner(
                    record.id.owner(),
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?;
                record.id =
                    crate::dag_id::InstanceId::new(concrete_owner, record.id.template().clone());
                record
                    .bindings
                    .value_ports
                    .values_mut()
                    .for_each(|concrete| {
                        *concrete =
                            rebase_resolved_name(concrete, dependency_owner, &instance_owner);
                    });
                record.bindings.indexes.values_mut().for_each(|target| {
                    if let InstanceIndexBindingTarget::Declared(concrete) = target {
                        *concrete =
                            rebase_resolved_name(concrete, dependency_owner, &instance_owner);
                    }
                });
                record.bindings.types.values_mut().for_each(|concrete| {
                    *concrete = rebase_resolved_name(concrete, dependency_owner, &instance_owner);
                });
                record
                    .bindings
                    .dimensions
                    .values_mut()
                    .for_each(|concrete| {
                        *concrete =
                            rebase_resolved_name(concrete, dependency_owner, &instance_owner);
                    });
                Ok(record)
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;
        instance_records.extend(nested_instance_records);
        for record in instance_records {
            if self
                .instances
                .iter()
                .any(|existing| existing.id.owner() == record.id.owner())
            {
                return Err(GraphcalError::internal_error(
                    format!(
                        "duplicate concrete instance identity `{}`",
                        record.id.owner()
                    ),
                    importer_src,
                    DiagnosticAnchor::Source(include_span),
                ));
            }
            self.instances.push(record);
        }

        // Source-order entries are declarations owned by the dependency,
        // including declarations already nested by an inner include. Imported
        // metadata is different: a qualified key can name an external module
        // used by the dependency and must retain that canonical qualifier.
        let all_dep_scoped_names = dep
            .source_order
            .iter()
            .map(|(name, _)| name.clone())
            .chain(dep.imported_bindings.keys().cloned())
            .collect::<HashSet<_>>();
        let dep_scoped_names = &all_dep_scoped_names;

        let mut all_dep_names = dep_names.clone();
        all_dep_names.extend(dep_scoped_names.iter().map(|name| name.member().clone()));
        let dep_names = &all_dep_names;
        let merged_assert_name = |name: &ScopedName| {
            if name.is_qualified() {
                name.within_scope(prefix)
            } else {
                assertion_aliases.get(name.member()).map_or_else(
                    || name.within_scope(prefix),
                    |alias| ScopedName::local(alias.clone()),
                )
            }
        };
        let merged_source_order = dep
            .source_order
            .iter()
            .filter_map(|(name, category)| match category {
                DeclCategory::Const | DeclCategory::Param | DeclCategory::Node => {
                    Some((name.within_scope(prefix), *category))
                }
                DeclCategory::Assert => Some((merged_assert_name(name), *category)),
                DeclCategory::Plot => requested_plots.get(name.member()).map(|requested| {
                    (
                        ScopedName::local(requested.alias.clone()),
                        DeclCategory::Plot,
                    )
                }),
                DeclCategory::Figure | DeclCategory::Layer => None,
            })
            .collect::<Vec<_>>();

        // Dynamic units belong to the concrete include instance. Repeated
        // instances may use the same source spelling with different value
        // bindings; canonical ownership, not that spelling, distinguishes the
        // scale definitions.
        let dep_unit_bindings = std::mem::take(&mut dep.unit_bindings);
        for mut entry in dep.dynamic_unit_scales {
            let is_public = dep
                .external_surface
                .is_unit_explicit_export(entry.spelling.name().atom());
            substitute_indexes(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names, dep_scoped_names);
            entry.unit_owner = require_rebased_instance_owner(
                &entry.unit_owner,
                dependency_owner,
                &instance_owner,
                importer_src,
                include_span,
            )?;
            entry.body_resolution_owner = require_rebased_instance_owner(
                &entry.body_resolution_owner,
                dependency_owner,
                &instance_owner,
                importer_src,
                include_span,
            )?;
            entry.src = entry.src.or_dependency(dep_src);
            if is_public {
                self.unit_bindings.insert(
                    prefix_unit_ref(&entry.spelling, prefix),
                    ResolvedUnitName::from_def(
                        entry.unit_owner.clone(),
                        entry.spelling.name().clone(),
                    ),
                );
            }
            if self.dynamic_unit_scales.iter().any(|existing| {
                existing.unit_owner == entry.unit_owner && existing.spelling == entry.spelling
            }) {
                return Err(GraphcalError::ConflictingImportedUnit {
                    name: entry.spelling,
                    src: importer_src.clone(),
                    span: include_span.into(),
                });
            }
            self.dynamic_unit_scales.push(entry);
        }
        for (spelling, target) in dep_unit_bindings {
            let rebased_owner = require_rebased_instance_owner(
                target.owner(),
                dependency_owner,
                &instance_owner,
                importer_src,
                include_span,
            )?;
            self.unit_bindings.insert(
                prefix_unit_ref(&spelling, prefix),
                ResolvedUnitName::from_def(rebased_owner, target.to_unowned_def_name()),
            );
        }

        // Merge consts
        for mut entry in dep.consts {
            substitute_indexes(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names, dep_scoped_names);
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.within_scope(prefix);
            let uses_dynamic_unit = expr_uses_local_units(&entry.expr, &dynamic_unit_names);
            self.consts.push(UnfrozenConstEntry {
                name: prefixed,
                declaration_owner: require_rebased_instance_owner(
                    &entry.declaration_owner,
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?,
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                expr: entry.expr,
                body_resolution_owner: merge_body_resolution_owner(
                    entry.body_resolution_owner,
                    uses_dynamic_unit,
                )?,
                span: entry.span,
                type_src: entry.type_src.or_dependency(dep_src),
                body_src: entry.body_src.or_dependency(dep_src),
            });
        }

        // Merge params — replace defaults with bindings where provided.
        // Unrebound defaults carry the typed obligations recorded before
        // substitution until inference observes their canonical owners.
        for mut entry in dep.params {
            let prefixed = entry.name.within_scope(prefix);
            let rebound = bindings.get(entry.name.member());
            if rebound.is_some() {
                entry.override_reconciliations.clear();
            }
            match (rebound, &mut entry.default) {
                (Some(binding_expr), _) => {
                    // The binding expression and its spans belong to the immediate
                    // importer, independently of the producer-owned signature.
                    entry.default = Some(UnfrozenParamDefault {
                        expr: binding_expr.clone(),
                        resolution_owner: importer_owner.clone(),
                        src: BodySource::own(),
                    });
                }
                (None, Some(default)) => {
                    // Keep the producer default, substitute its type-level names,
                    // and carry its existing source across this merge boundary.
                    substitute_indexes(&mut default.expr, index_bindings);
                    substitute_type_names_in_expr(&mut default.expr, type_bindings);
                    prefix_expr_refs(&mut default.expr, prefix, dep_names, dep_scoped_names);
                    default.src = default.src.clone().or_dependency(dep_src);
                    default.resolution_owner = merge_body_resolution_owner(
                        default.resolution_owner.clone(),
                        expr_uses_local_units(&default.expr, &dynamic_unit_names),
                    )?;
                }
                (None, None) => {}
            }
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            self.params.push(UnfrozenParamEntry {
                name: prefixed,
                declaration_owner: require_rebased_instance_owner(
                    &entry.declaration_owner,
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?,
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                default: entry.default,
                span: entry.span,
                type_src: entry.type_src.or_dependency(dep_src),
                override_reconciliations: entry.override_reconciliations,
            });
        }

        // Merge nodes
        for mut entry in dep.nodes {
            substitute_indexes(&mut entry.expr, index_bindings);
            substitute_type_names_in_expr(&mut entry.expr, type_bindings);
            prefix_expr_refs(&mut entry.expr, prefix, dep_names, dep_scoped_names);
            substitute_type_expr_indexes(&mut entry.type_ann, index_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, type_bindings);
            substitute_type_expr_nominal_names(&mut entry.type_ann, dim_bindings);
            let prefixed = entry.name.within_scope(prefix);
            let uses_dynamic_unit = expr_uses_local_units(&entry.expr, &dynamic_unit_names);
            self.nodes.push(UnfrozenNodeEntry {
                name: prefixed,
                declaration_owner: require_rebased_instance_owner(
                    &entry.declaration_owner,
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?,
                type_ann: entry.type_ann,
                type_resolution_owner: merge_resolution_owner(entry.type_resolution_owner),
                expr: entry.expr,
                body_resolution_owner: merge_body_resolution_owner(
                    entry.body_resolution_owner,
                    uses_dynamic_unit,
                )?,
                span: entry.span,
                type_src: entry.type_src.or_dependency(dep_src),
                body_src: entry.body_src.or_dependency(dep_src),
            });
        }

        // Merge asserts
        for mut entry in dep.asserts {
            match &mut entry.body {
                crate::desugar::desugared_ast::AssertBody::Expr(e) => {
                    substitute_indexes(e, index_bindings);
                    substitute_type_names_in_expr(e, type_bindings);
                    prefix_expr_refs(e, prefix, dep_names, dep_scoped_names);
                }
                crate::desugar::desugared_ast::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                    ..
                } => {
                    substitute_indexes(actual, index_bindings);
                    substitute_type_names_in_expr(actual, type_bindings);
                    prefix_expr_refs(actual, prefix, dep_names, dep_scoped_names);
                    substitute_indexes(expected, index_bindings);
                    substitute_type_names_in_expr(expected, type_bindings);
                    prefix_expr_refs(expected, prefix, dep_names, dep_scoped_names);
                    substitute_indexes(tolerance, index_bindings);
                    substitute_type_names_in_expr(tolerance, type_bindings);
                    prefix_expr_refs(tolerance, prefix, dep_names, dep_scoped_names);
                }
            }
            let uses_dynamic_unit = assert_body_uses_local_units(&entry.body, &dynamic_unit_names);
            let merged_name = merged_assert_name(&entry.name);
            self.asserts.push(UnfrozenAssertEntry {
                name: merged_name.clone(),
                declaration_owner: require_rebased_instance_owner(
                    &entry.declaration_owner,
                    dependency_owner,
                    &instance_owner,
                    importer_src,
                    include_span,
                )?,
                body: entry.body,
                body_resolution_owner: merge_body_resolution_owner(
                    entry.body_resolution_owner,
                    uses_dynamic_unit,
                )?,
                span: entry.span,
                body_src: entry.body_src.or_dependency(dep_src),
            });
            self.assert_names.insert(merged_name);
        }

        // Merge only the plots requested by the include's brace list (#847):
        // display is a consumer-side opt-in, so unrequested dep plots do not
        // travel with the instance. A requested plot enters the root
        // namespace under its local alias, evaluating against this instance's
        // bindings; `#[hidden]` on the include item keeps it composition-only.
        for mut entry in dep.plots {
            let Some(requested) = requested_plots.get(entry.name.member()) else {
                continue;
            };
            for encoding in &mut entry.decl.encodings {
                substitute_indexes(&mut encoding.value, index_bindings);
                substitute_type_names_in_expr(&mut encoding.value, type_bindings);
                prefix_expr_refs(&mut encoding.value, prefix, dep_names, dep_scoped_names);
            }
            for prop in &mut entry.decl.mark.properties {
                substitute_indexes(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names, dep_scoped_names);
            }
            for prop in &mut entry.decl.properties {
                substitute_indexes(&mut prop.value, index_bindings);
                substitute_type_names_in_expr(&mut prop.value, type_bindings);
                prefix_expr_refs(&mut prop.value, prefix, dep_names, dep_scoped_names);
            }
            let uses_dynamic_unit = plot_uses_local_units(&entry.decl, &dynamic_unit_names);
            let local = ScopedName::local(requested.alias.clone());
            self.plots.push(UnfrozenPlotEntry {
                name: local,
                decl: entry.decl,
                body_resolution_owner: merge_body_resolution_owner(
                    entry.body_resolution_owner,
                    uses_dynamic_unit,
                )?,
                span: entry.span,
                body_src: entry.body_src.or_dependency(dep_src),
                displayed: !requested.hidden,
            });
        }

        // Dep figures and layers do not merge: they cannot be requested by a
        // brace list, and display is controlled by the consumer (#847).

        // Merge assumes_map and expected_fail
        for (assert_name, assumers) in dep.assumes_map {
            let prefixed_assert = merged_assert_name(&assert_name);
            let prefixed_assumers: Vec<ScopedName> = assumers
                .iter()
                .map(|assumer| assumer.within_scope(prefix))
                .collect();
            self.assumes_map
                .entry(prefixed_assert)
                .or_default()
                .extend(prefixed_assumers);
        }
        for (assert_name, mut metadata) in dep.expected_fail {
            let prefixed = merged_assert_name(&assert_name);
            metadata.src = metadata.src.or_dependency(dep_src);

            // If the expected_fail references overridden indexes, filter or drop.
            if index_bindings.is_empty() {
                self.expected_fail.insert(prefixed, metadata);
            } else {
                match &mut metadata.expected {
                    ExpectedFail::All => {
                        self.expected_fail.insert(prefixed, metadata);
                    }
                    ExpectedFail::Variants(keys) => {
                        keys.retain(|key| {
                            // Drop keys that reference any overridden index.
                            // `#N` finite-position segments never name an
                            // index, so they cannot reference an overridden one.
                            !key.iter().any(|part| {
                                part.index_path().is_some_and(|index_path| {
                                    index_bindings.contains_key(&IndexName::from_atom(
                                        index_path.leaf().clone(),
                                    ))
                                })
                            })
                        });
                        if !keys.is_empty() {
                            self.expected_fail.insert(prefixed, metadata);
                        }
                        // If all keys were dropped, don't insert any expected_fail.
                    }
                }
            }
        }

        // Apply import-item expected_fail attributes (from the importing file).
        // Malformed args are surfaced as `ExpectedFailInvalidArg`, matching the
        // behavior for non-imported `#[expected_fail]` attributes.
        for (orig_name, attrs) in import_item_attributes {
            for attr in attrs {
                if attr
                    .name
                    .name
                    .parse::<crate::syntax::attribute::AttributeName>()
                    == Ok(crate::syntax::attribute::AttributeName::ExpectedFail)
                {
                    let prefixed_assert = merged_assert_name(&ScopedName::local(orig_name.clone()));
                    let expected = crate::ir::resolve::names::parse_expected_fail_args(
                        &attr.args,
                        importer_src,
                    )?;
                    self.expected_fail.insert(
                        prefixed_assert,
                        ParsedExpectedFailMetadata {
                            expected,
                            resolution_owner: importer_owner.clone(),
                            src: BodySource::own(),
                            attribute_span: attr.span,
                        },
                    );
                }
            }
        }

        // Hidden imports used by dependency expressions receive the same
        // lexical instance prefix as those expressions. Their canonical target,
        // declared type, and optional value remain one unchanged record.
        self.source_order.extend(merged_source_order);

        for (name, binding) in dep.imported_bindings {
            let lexical = prefix_dep(&name, prefix, dep_names, dep_scoped_names);
            if self
                .imported_bindings
                .insert(lexical.clone(), binding)
                .is_some()
            {
                return Err(GraphcalError::internal_error(
                    format!(
                        "duplicate imported lexical binding `{lexical}` while merging include instance `{prefix}`"
                    ),
                    importer_src,
                    DiagnosticAnchor::Source(include_span),
                ));
            }
        }
        Ok(())
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

/// Visitor that prefixes references to dependency declarations.
///
/// When a `@name` (or bare const `NAME`) refers to a name owned by the
/// dependency being merged, rewrite its typed [`ScopedName`] so the merged-IR
/// key matches the enclosing instance path. Existing qualifiers are preserved
/// for exact nested declarations and left untouched for external module
/// imports. No flat separator strings are constructed here.
struct RefPrefixer<'a> {
    prefix: &'a ModuleAliasName,
    dep_names: &'a HashSet<DeclName>,
    dep_scoped_names: &'a HashSet<ScopedName>,
}

impl RefPrefixer<'_> {
    fn rewrite(&self, scoped: &ScopedName) -> Option<ScopedName> {
        if self.dep_scoped_names.contains(scoped) {
            Some(scoped.within_scope(self.prefix))
        } else if !scoped.is_qualified() && self.dep_names.contains(scoped.member()) {
            Some(scoped.with_prefix(self.prefix))
        } else {
            None
        }
    }
}

impl ExprVisitorMut<crate::syntax::phase::Desugared> for RefPrefixer<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &mut expr.kind
            && let Some(prefixed) = self.rewrite(&ident.value)
        {
            ident.value = prefixed;
        }
        Ok(())
    }

    fn visit_unresolved_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // A bare reference path owned by the dependency becomes a qualified
        // path under the merge prefix, mirroring the prefixed entry name it
        // resolves to. Already-qualified paths belong to another namespace
        // (a transitive import inside the dep) and keep their qualifier.
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) =
            &mut expr.kind
            && let Some(ident) = path.as_bare()
            && self.dep_names.contains(ident.name.as_str())
        {
            let leaf = ident.clone();
            let prefix_segment = crate::syntax::ast::Ident {
                name: self.prefix.atom().clone(),
                span: leaf.span,
            };
            *path = crate::syntax::ast::IdentPath::new(crate::syntax::non_empty::NonEmpty::new(
                prefix_segment,
                vec![leaf],
            ));
        }
        Ok(())
    }

    // Function calls don't need rewriting: built-ins (`sqrt`, `sum`, …)
    // are unqualified and never appear in `dep_names`, and there are no
    // user-defined functions in graphcal. The default `visit_fn_call_mut`
    // (which recurses into args) is correct.
}

/// Rewrite `@`-references and const/fn references within an expression to use
/// prefixed names, but only for names that belong to the dependency.
///
/// For example, `GraphRef("dry_mass")` becomes `GraphRef("r.dry_mass")` when
/// `"dry_mass"` is in `dep_names` and `prefix` is `"r"`.
///
/// Built-in names and names from the importer's scope are left unchanged.
fn prefix_expr_refs(
    expr: &mut Expr,
    prefix: &ModuleAliasName,
    dep_names: &HashSet<DeclName>,
    dep_scoped_names: &HashSet<ScopedName>,
) {
    let mut prefixer = RefPrefixer {
        prefix,
        dep_names,
        dep_scoped_names,
    };
    let _ = prefixer.visit_expr_mut(expr);
}

/// Visitor that rewrites index references in expressions according to a
/// resolved include binding map.
///
/// Index positions such as `for i: Axis` can become either another declared
/// axis or a structural `Fin(N)` axis. Label-bearing positions can only be
/// renamed to another declared axis; a required index has no labels of its own,
/// so a valid generic body cannot require such a rewrite for a finite target.
struct IndexSubstituter<'a> {
    bindings: &'a HashMap<IndexName, types::IndexBindingTarget>,
}

impl ExprVisitorMut<crate::syntax::phase::Desugared> for IndexSubstituter<'_> {
    type Error = std::convert::Infallible;

    fn visit_expr_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::KeyForm { axis, .. } = &mut expr.kind {
            substitute_index_expr(axis, self.bindings);
        }
        self.dispatch_mut(expr)
    }

    fn visit_generic_args_mut(
        &mut self,
        args: &mut [crate::desugar::desugared_ast::GenericArg],
    ) -> Result<(), Self::Error> {
        for arg in args {
            substitute_generic_arg_indexes(arg, self.bindings);
            if let crate::desugar::desugared_ast::GenericArg::Type(type_expr) = arg {
                self.visit_type_expr_mut(type_expr)?;
            }
        }
        Ok(())
    }

    fn visit_unresolved_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::IndexLabel {
            index, ..
        }) = &mut expr.kind
            && let Some(new) = self
                .bindings
                .get(index.leaf().name.as_str())
                .and_then(types::IndexBindingTarget::declared_name)
        {
            let span = index.span();
            *index = crate::syntax::ast::IdentPath::from(crate::syntax::ast::Ident {
                name: new.atom().clone(),
                span,
            });
        }
        Ok(())
    }

    fn visit_for_comp_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        use crate::desugar::desugared_ast::{ForBindingIndex, NatExpr};

        if let ExprKind::ForComp { bindings, body } = &mut expr.kind {
            for binding in bindings {
                let replacement = match &binding.index {
                    ForBindingIndex::Named(index) => self
                        .bindings
                        .get(index.value.leaf().as_str())
                        .map(|target| match target {
                            types::IndexBindingTarget::Declared(name) => ForBindingIndex::Named(
                                Spanned::new(name.clone().into(), index.span),
                            ),
                            types::IndexBindingTarget::Finite(finite) => ForBindingIndex::Finite {
                                cardinality: NatExpr::Literal(finite.size_u64(), index.span),
                                span: index.span,
                            },
                        }),
                    ForBindingIndex::Finite { .. } => None,
                };
                if let Some(replacement) = replacement {
                    binding.index = replacement;
                }
            }
            self.visit_expr_mut(body)?;
        }
        Ok(())
    }

    fn visit_unfold_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::Unfold {
            axis, init, body, ..
        } = &mut expr.kind
        {
            if let Some(new) = self
                .bindings
                .get(axis.value.leaf().as_str())
                .and_then(types::IndexBindingTarget::declared_name)
            {
                axis.value = new.clone().into();
            }
            self.visit_expr_mut(init)?;
            self.visit_expr_mut(body)?;
        }
        Ok(())
    }

    fn visit_index_access_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        use crate::desugar::desugared_ast::IndexArg;
        if let ExprKind::IndexAccess { expr: inner, args } = &mut expr.kind {
            for arg in args.iter_mut() {
                match arg {
                    IndexArg::Variant { index, .. } => {
                        if let Some(new) = self
                            .bindings
                            .get(index.value.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                        {
                            index.value = new.clone().into();
                        }
                    }
                    IndexArg::Expr(e) => {
                        self.visit_expr_mut(e)?;
                    }
                    IndexArg::Var(_) => {}
                }
            }
            self.visit_expr_mut(inner)?;
        }
        Ok(())
    }

    fn visit_map_literal_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::MapLiteral { entries } = &mut expr.kind {
            for entry in entries.iter_mut() {
                for key in &mut entry.keys {
                    if let crate::syntax::ast::MapEntryIndex::Named(index_name) = &key.index.value
                        && let Some(new) = self
                            .bindings
                            .get(index_name.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                    {
                        key.index.value =
                            crate::syntax::ast::MapEntryIndex::Named(new.clone().into());
                    }
                }
                self.visit_expr_mut(&mut entry.value)?;
            }
        }
        Ok(())
    }

    fn visit_match_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        if let ExprKind::Match { scrutinee, arms } = &mut expr.kind {
            self.visit_expr_mut(scrutinee)?;
            for arm in arms {
                match &mut arm.pattern {
                    crate::desugar::desugared_ast::MatchPattern::IndexLabel { index, .. } => {
                        if let Some(new) = self
                            .bindings
                            .get(index.value.leaf().as_str())
                            .and_then(types::IndexBindingTarget::declared_name)
                        {
                            index.value = new.clone().into();
                        }
                    }
                    crate::desugar::desugared_ast::MatchPattern::Path { .. }
                    | crate::desugar::desugared_ast::MatchPattern::Constructor { .. } => {}
                }
                self.visit_expr_mut(&mut arm.body)?;
            }
        }
        Ok(())
    }
}

/// Rewrite index names within an expression according to a binding map.
///
/// For example, if `bindings` maps `"Phase"` to `"MyPhase"`, then
/// `VariantLiteral { index: Phase, variant: A }` becomes
/// `VariantLiteral { index: MyPhase, variant: A }`.
///
/// This must be called **before** `prefix_expr_refs` so that index names are
/// correct before ref-prefixing adds the `prefix.` qualifier.
fn substitute_indexes(expr: &mut Expr, bindings: &HashMap<IndexName, types::IndexBindingTarget>) {
    if bindings.is_empty() {
        return;
    }
    let mut sub = IndexSubstituter { bindings };
    let _ = sub.visit_expr_mut(expr);
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

/// Rewrite struct-type names within an expression according to a binding map.
///
/// Covers `ConstructorCall.constructor` and both call variants' `generic_args`.
/// Recurses through child expressions so nested constructor calls are also
/// rewritten.
#[expect(
    clippy::too_many_lines,
    reason = "single recursion covering every ExprKind variant"
)]
fn substitute_type_names_in_expr(
    expr: &mut Expr,
    bindings: &HashMap<StructTypeName, StructTypeName>,
) {
    use crate::desugar::desugared_ast::IndexArg;

    if bindings.is_empty() {
        return;
    }
    match &mut expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::GraphRef(_)
        | ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::IndexLabel { .. }) => {}

        // A bare reference path naming a rebound type is a nullary
        // constructor use; rewrite it to the importer's constructor name.
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            if let Some(ident) = path.as_bare_mut()
                && let Some(new_name) = bindings.get(ident.name.as_str())
                && let Ok(parsed_name) = NameAtom::parse(new_name.as_ref())
            {
                ident.name = parsed_name;
            }
        }
        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                substitute_type_names_in_expr(&mut binding.value, bindings);
            }
        }

        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => {
            if let Some(constructor) = callee.as_bare_mut()
                && let Some(new_name) = bindings.get(constructor.name.as_str())
                && let Ok(parsed_name) = NameAtom::parse(new_name.as_ref())
            {
                constructor.name = parsed_name;
            }
            for arg in generic_args.iter_mut() {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
            for field in fields {
                substitute_type_names_in_expr(&mut field.value, bindings);
            }
        }

        ExprKind::FnCall {
            generic_args, args, ..
        } => {
            for arg in generic_args.iter_mut() {
                substitute_generic_arg_nominal_names(arg, bindings);
            }
            for arg in args {
                substitute_type_names_in_expr(arg, bindings);
            }
        }

        ExprKind::BinOp { lhs, rhs, .. } => {
            substitute_type_names_in_expr(lhs, bindings);
            substitute_type_names_in_expr(rhs, bindings);
        }
        ExprKind::UnaryOp { operand, .. } => {
            substitute_type_names_in_expr(operand, bindings);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            substitute_type_names_in_expr(condition, bindings);
            substitute_type_names_in_expr(then_branch, bindings);
            substitute_type_names_in_expr(else_branch, bindings);
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::FieldAccess { expr: inner, .. } => {
            substitute_type_names_in_expr(inner, bindings);
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            substitute_type_names_in_expr(inner, bindings);
            for arg in args {
                if let IndexArg::Expr(e) = arg {
                    substitute_type_names_in_expr(e, bindings);
                }
            }
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                substitute_type_names_in_expr(&mut entry.value, bindings);
            }
        }
        ExprKind::ForComp { body, .. } => {
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            substitute_type_names_in_expr(source, bindings);
            substitute_type_names_in_expr(init, bindings);
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::Unfold { init, body, .. } => {
            substitute_type_names_in_expr(init, bindings);
            substitute_type_names_in_expr(body, bindings);
        }
        ExprKind::KeyForm { arg, .. } => {
            substitute_type_names_in_expr(arg, bindings);
        }
        ExprKind::Match { scrutinee, arms } => {
            substitute_type_names_in_expr(scrutinee, bindings);
            for arm in arms {
                substitute_type_names_in_expr(&mut arm.body, bindings);
            }
        }
        // `Sugar` payload is `Infallible` post-desugar — statically
        // unreachable.
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar(Infallible) — proof of unreachability"
        )]
        ExprKind::Sugar(s) => match *s {},
    }
}
