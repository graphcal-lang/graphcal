//! Include assembly and typed substitution for unfrozen per-DAG IR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::{Expr, ExprKind, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::expr::LocalDecl;
use crate::ir::instance::identity::{instance_declaration, projection_alias};
use crate::ir::instance::{
    InstanceAssertionProjection, InstancePlotProjection, InstanceRecord, InstanceValueProjection,
};
use crate::registry::error::GraphcalError;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{UnitName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::span::Span;
use crate::syntax::type_name::ConstructorName;
use crate::syntax::visitor::ExprVisitor;

use super::{
    decl_table::DeclTable,
    entry::{self, Decl, InScope},
    extern_fns::resolve_plugin_imports,
    lower::{
        AssertEntry, ConstEntry, DynamicUnitScaleEntry, FigureEntry, HirDag, HirDecl,
        IncludeAliasDeclaration, LayerEntry, LoweredPlotBody, LoweredPlotField,
        LoweredPlotProperty, NodeEntry, ParamEntry, PlotEntry, UnfrozenIR,
        UnfrozenSemanticInstance,
    },
};

/// V005 obligations carried atomically from one include-site preflight into
/// its semantic instance edge.
#[derive(Debug, Clone, Default)]
pub struct IncludeOverrideReconciliations(
    HashMap<ResolvedDeclName, Vec<crate::ir::override_reconciliation::OverrideReconciliation>>,
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
    pub fn plot_projection_target(&self, name: &DeclName) -> Option<LocalDecl> {
        self.decls
            .iter()
            .find_map(|decl| match decl {
                Decl::Plot(entry) if entry.name() == name => Some(LocalDecl::new(entry.identity())),
                _ => None,
            })
            .or_else(|| {
                self.semantic_instances.iter().find_map(|instance| {
                    instance
                        .plot_projections
                        .iter()
                        .find(|projection| projection.exposed_name.leaf() == name)
                        .map(|projection| {
                            LocalDecl::new(instance_declaration(
                                instance.instance.id(),
                                projection.target.leaf().clone(),
                            ))
                        })
                })
            })
    }

    /// Names of assertions authored by this reusable DAG template.
    #[must_use]
    pub fn assertion_names(&self) -> Vec<DeclName> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Assert(entry) => Some(entry.name().clone()),
                _ => None,
            })
            .collect()
    }

    /// Local names of the value declarations in this IR, in source order,
    /// including selective include aliases.
    pub fn value_names(&self) -> impl Iterator<Item = &DeclName> {
        self.decls
            .iter()
            .filter(|decl| matches!(decl.category(), DeclCategory::Value(_)))
            .map(Decl::name)
    }

    fn params(&self) -> impl Iterator<Item = &entry::ParamEntry<entry::Syntax>> {
        self.decls.iter().filter_map(|decl| match decl {
            Decl::Param(entry) => Some(entry),
            _ => None,
        })
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
            .any(|existing| existing.instance.id().owner() == input.instance.id().owner())
        {
            return Err(GraphcalError::InternalError {
                message: format!(
                    "duplicate semantic instance identity `{}`",
                    input.instance.id().owner()
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
        self.decls.iter().find_map(|decl| {
            let (type_ann, is_const) = match decl {
                Decl::Const(entry) if entry.name() == name => (&entry.type_ann, true),
                Decl::Param(entry) if entry.name() == name => (&entry.type_ann, false),
                Decl::Node(entry) if entry.name() == name => (&entry.type_ann, false),
                _ => return None,
            };
            Some(IncludeAliasDeclaration {
                type_ann: type_ann.syntax.clone(),
                is_const,
            })
        })
    }

    /// Publish a synthesized Term alias as part of this module's external surface.
    pub fn export_term_alias(&mut self, name: DeclName) {
        self.external_surface.insert_explicit_export(name);
    }

    /// Expose public runtime units through one semantic instance namespace.
    ///
    /// Only a source-visible alias makes `alias.unit` spellable; an anonymous
    /// selective include exposes its units solely through projection aliases
    /// (see [`Self::add_dynamic_unit_projection_alias`]).
    pub fn add_semantic_dynamic_unit_bindings<'a>(
        &mut self,
        units: impl IntoIterator<Item = &'a UnitName>,
        instance: &crate::dag_id::InstanceId,
    ) {
        let Some(alias) = instance.scope().alias() else {
            return;
        };
        self.unit_bindings.extend(units.into_iter().map(|unit| {
            (
                UnitRef::qualified(
                    crate::syntax::non_empty::NonEmpty::singleton(alias.atom().clone()),
                    unit.clone(),
                ),
                instance_declaration(instance, unit.clone()),
            )
        }));
    }

    /// Add a local alias for a dynamic unit exposed by a recorded semantic
    /// instance. Units the instance does not expose are ignored.
    pub fn add_dynamic_unit_projection_alias(
        &mut self,
        instance_owner: &crate::dag_id::DagId,
        source: &UnitName,
        alias: UnitName,
    ) {
        let exposed = self
            .semantic_instances
            .iter()
            .find(|record| {
                record.instance.id().owner() == instance_owner
                    && record.runtime_unit_names.contains(source)
            })
            .map(|record| instance_declaration(record.instance.id(), source.clone()));
        if let Some(target) = exposed {
            self.unit_bindings.insert(UnitRef::local(alias), target);
        }
    }

    /// Freeze into a complete [`HirDag`] given the module's frontend type
    /// table and the project's canonical definition evaluator.
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
        owner: &crate::dag_id::DagId,
        definitions: &mut super::static_definitions::StaticDefinitionEvaluator<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HirDag, GraphcalError> {
        self.freeze_with_cancellation(
            owner,
            definitions,
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
        owner: &crate::dag_id::DagId,
        definitions: &mut super::static_definitions::StaticDefinitionEvaluator<'_>,
        src: &NamedSource<Arc<String>>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<HirDag, GraphcalError> {
        cancellation.checkpoint()?;
        let resolver = definitions.resolver();
        let time_zones = crate::registry::time_zone::TimeZoneRegistry::bundled();
        // Entries already visible in this IR (including prefixed include
        // instances and dag self-imports) bind their written names to
        // canonical identities for the lowering below.
        let instance_templates = self
            .semantic_instances
            .iter()
            .map(|record| {
                (
                    record.instance.id().owner().clone(),
                    record.instance.id().template().clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let nominal_types = self.lower_nominal_types(owner, definitions, src, cancellation)?;
        let table = DeclTable::new(owner, self.decls).map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        // Value expressions may reference local values (never assertions or
        // visualizations), semantic instance ports, and imported values.
        let mut decl_bindings = table
            .iter()
            .filter(|decl| matches!(decl.category(), DeclCategory::Value(_)))
            .map(|decl| (ScopedName::local(decl.name().clone()), decl.identity()))
            .collect::<HashMap<_, _>>();
        // Instance ports, then imported values, join the local declarations;
        // no lexical binding may shadow another.
        let instance_ports = self.semantic_instances.iter().flat_map(|record| {
            let scope = record.instance.id().scope();
            record.instance.concrete_value_ports().map(move |target| {
                (
                    ScopedName::in_scope(scope.clone(), target.to_unowned_def_name()),
                    target,
                    "semantic instance binding",
                )
            })
        });
        let imported = self
            .imported_bindings
            .iter()
            .map(|(name, target)| (name.clone(), target.clone(), "imported lexical binding"));
        for (name, target, origin) in instance_ports.chain(imported) {
            cancellation.checkpoint()?;
            if decl_bindings.insert(name.clone(), target).is_some() {
                return Err(GraphcalError::internal_error(
                    format!("{origin} `{name}` collides with a declaration"),
                    src,
                    DiagnosticAnchor::WholeFile,
                ));
            }
        }

        let generic_scope = crate::hir::GenericScope::new();
        let overlay = crate::hir::BindingOverlay::Frozen(crate::hir::FrozenBindings {
            unit_bindings: &self.unit_bindings,
            decl_bindings: &decl_bindings,
            instance_templates: &instance_templates,
        });
        let lower_in = |expr: &Expr, resolution_owner: &crate::dag_id::DagId| {
            let scope = crate::hir::ModuleScope::new(resolution_owner, resolver, &generic_scope);
            let expr_ctx =
                crate::hir::ExprLoweringContext::with_overlay(scope, &time_zones, overlay);
            crate::hir::lower_expr(expr, expr_ctx)
                .map_err(|err| crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, src))
        };
        let lower_scoped = |expr: &InScope<Expr>| lower_in(&expr.syntax, &expr.resolution_owner);
        let lower_type_annotation =
            |type_ann: &InScope<TypeExpr>| -> Result<crate::hir::TypeAnnotation, GraphcalError> {
                let InScope {
                    syntax: type_ann,
                    resolution_owner,
                } = type_ann;
                crate::hir::diagnostics::validate_type_annotation(type_ann, src)?;
                let scope =
                    crate::hir::ModuleScope::new(resolution_owner, resolver, &generic_scope);
                let decl_type = crate::hir::lower_decl_type(type_ann, scope).map_err(|error| {
                    crate::hir::diagnostics::type_lower_error_to_graphcal(&error, src)
                })?;
                let domain_bounds = type_ann
                    .domain_bounds()
                    .iter()
                    .map(|bound| {
                        Ok(crate::hir::DomainBound {
                            kind: bound.kind,
                            value: lower_in(&bound.value, resolution_owner)?,
                            span: bound.span,
                        })
                    })
                    .collect::<Result<_, GraphcalError>>()?;
                Ok(crate::hir::TypeAnnotation {
                    decl_type,
                    domain_bounds,
                    span: type_ann.span,
                })
            };

        let dynamic_unit_scales = self
            .dynamic_unit_scales
            .iter()
            .map(|entry| {
                cancellation.checkpoint()?;
                let unit = resolver
                    .resolve_unit_path(&entry.unit, &entry.spelling.to_name_path())
                    .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|err| GraphcalError::InternalError {
                        message: format!(
                            "registered dynamic unit `{}` did not resolve canonically: {err}",
                            entry.spelling
                        ),
                        src: src.clone(),
                        span: entry.span.into(),
                    })?;
                Ok(DynamicUnitScaleEntry {
                    unit,
                    spelling: entry.spelling.clone(),
                    expr: lower_scoped(&entry.expr)?,
                    declared_dimension: entry.declared_dimension.clone(),
                    base_unit_dimension: entry.base_unit_dimension.clone(),
                    span: entry.span,
                    src: entry.src.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        let lower_fields = |fields: &[crate::desugar::desugared_ast::PlotField],
                            resolution_owner: &crate::dag_id::DagId,
                            classify: fn(
            crate::syntax::ast::PlotPropertyName,
        ) -> LoweredPlotProperty| {
            fields
                .iter()
                .map(|field| {
                    Ok(LoweredPlotField {
                        property: classify(field.name.value.clone()),
                        name_span: field.name.span,
                        value: lower_in(&field.value, resolution_owner)?,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()
        };
        let lower_composition_fields =
            |fields: &InScope<Vec<crate::desugar::desugared_ast::PlotField>>| {
                lower_fields(
                    &fields.syntax,
                    &fields.resolution_owner,
                    LoweredPlotProperty::composition,
                )
            };
        // Lower kind by kind, preserving the established diagnostic order when
        // several bodies are invalid.
        let decls = table.try_map(
            |decl| match decl {
                Decl::Const(_) => 0,
                Decl::Param(_) => 1,
                Decl::Node(_) => 2,
                Decl::Assert(_) => 3,
                Decl::Plot(_) => 4,
                Decl::Figure(_) => 5,
                Decl::Layer(_) => 6,
            },
            |decl| -> Result<HirDecl, GraphcalError> {
                cancellation.checkpoint()?;
                Ok(match decl {
                    Decl::Const(entry) => Decl::Const(ConstEntry {
                        type_ann: lower_type_annotation(&entry.type_ann)?,
                        expr: lower_scoped(&entry.expr)?,
                        identity: entry.identity,
                        span: entry.span,
                    }),
                    Decl::Param(entry) => Decl::Param(ParamEntry {
                        type_ann: lower_type_annotation(&entry.type_ann)?,
                        default: entry.default.as_ref().map(lower_scoped).transpose()?,
                        identity: entry.identity,
                        span: entry.span,
                        override_reconciliations: entry.override_reconciliations,
                    }),
                    Decl::Node(entry) => Decl::Node(NodeEntry {
                        type_ann: lower_type_annotation(&entry.type_ann)?,
                        definition: super::node_definition::lower(
                            &entry.definition.syntax,
                            crate::hir::ExprLoweringContext::with_overlay(
                                crate::hir::ModuleScope::new(
                                    &entry.definition.resolution_owner,
                                    resolver,
                                    &generic_scope,
                                ),
                                &time_zones,
                                overlay,
                            ),
                        )
                        .map_err(|error| crate::hir::expr_lower_error_to_graphcal(&error, src))?,
                        identity: entry.identity,
                        span: entry.span,
                    }),
                    Decl::Assert(entry) => Decl::Assert(AssertEntry {
                        body: crate::hir::lower_assert_body(
                            &entry.body.syntax,
                            crate::hir::ExprLoweringContext::with_overlay(
                                crate::hir::ModuleScope::new(
                                    &entry.body.resolution_owner,
                                    resolver,
                                    &generic_scope,
                                ),
                                &time_zones,
                                overlay,
                            ),
                        )
                        .map_err(|err| {
                            crate::hir::diagnostics::expr_lower_error_to_graphcal(&err, src)
                        })?,
                        identity: entry.identity,
                        span: entry.span,
                    }),
                    // Sink expressions are semantic program bodies, not
                    // optional rendering hints. Batch compilation lowers every
                    // one strictly; tolerant HIR is reserved for editor-facing
                    // incomplete buffers.
                    Decl::Plot(entry) => {
                        let InScope {
                            syntax: body,
                            resolution_owner,
                        } = &entry.body;
                        let encodings = body
                            .encodings
                            .iter()
                            .map(|encoding| {
                                lower_in(&encoding.value, resolution_owner)
                                    .map(|lowered| (encoding.channel, lowered))
                            })
                            .collect::<Result<Vec<_>, GraphcalError>>()?;
                        Decl::Plot(PlotEntry {
                            body: LoweredPlotBody {
                                encodings,
                                mark_properties: lower_fields(
                                    &body.mark_properties,
                                    resolution_owner,
                                    LoweredPlotProperty::mark,
                                )?,
                                properties: lower_fields(
                                    &body.properties,
                                    resolution_owner,
                                    LoweredPlotProperty::plot,
                                )?,
                            },
                            identity: entry.identity,
                            mark_type: entry.mark_type,
                            visibility: entry.visibility,
                        })
                    }
                    Decl::Figure(entry) => Decl::Figure(FigureEntry {
                        fields: lower_composition_fields(&entry.fields)?,
                        identity: entry.identity,
                        plot_names: entry.plot_names,
                    }),
                    Decl::Layer(entry) => Decl::Layer(LayerEntry {
                        fields: lower_composition_fields(&entry.fields)?,
                        identity: entry.identity,
                        plot_names: entry.plot_names,
                    }),
                })
            },
        )?;

        let lookup_assertion = |name: &ScopedName| {
            name.as_bare()
                .and_then(|local| decls.lookup(local))
                .or_else(|| self.imported_bindings.get(name))
                .cloned()
                .or_else(|| {
                    self.semantic_instances.iter().find_map(|record| {
                        record
                            .assertion_projections
                            .iter()
                            .find(|projection| &projection.exposed_name == name)
                            .map(|projection| {
                                instance_declaration(
                                    record.instance.id(),
                                    projection.target.leaf().clone(),
                                )
                            })
                    })
                })
                .ok_or_else(|| {
                    GraphcalError::internal_error(
                        format!("attribute target `{name}` has no canonical declaration"),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })
        };
        let assumes_map = self
            .assumes_map
            .iter()
            .map(|(assertion, assumers)| {
                Ok((
                    lookup_assertion(assertion)?,
                    assumers
                        .iter()
                        .map(&lookup_assertion)
                        .collect::<Result<Vec<_>, GraphcalError>>()?,
                ))
            })
            .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
        let expected_fail = self
            .expected_fail
            .into_iter()
            .map(|(assertion, metadata)| {
                Ok((
                    lookup_assertion(&assertion)?,
                    metadata.resolve(resolver, src)?,
                ))
            })
            .collect::<Result<HashMap<_, _>, GraphcalError>>()?;

        cancellation.checkpoint()?;

        let semantic_instances = self
            .semantic_instances
            .iter()
            .map(|record| {
                let value_bindings = record
                    .value_bindings
                    .iter()
                    .map(|(port, expr)| lower_in(expr, owner).map(|expr| (port.clone(), expr)))
                    .collect::<Result<HashMap<_, _>, _>>()?;
                Ok(crate::ir::instance::HirInstanceRecord {
                    instance: record.instance.clone(),
                    debug_scope: record.debug_scope.clone(),
                    value_bindings,
                    runtime_unit_names: record.runtime_unit_names.clone(),
                    output_projections: record.output_projections.clone(),
                    assertion_projections: record.assertion_projections.clone(),
                    plot_projections: record.plot_projections.clone(),
                    override_reconciliations: record.override_reconciliations.clone(),
                })
            })
            .collect::<Result<Vec<_>, GraphcalError>>()?;

        let extern_functions = resolve_plugin_imports(
            &self.plugin_imports,
            &mut super::extern_fns::ExternSignatureScope {
                owner,
                nominal_types: &nominal_types,
                definitions,
            },
            src,
        )?;
        let display_dimensions = definitions.display_dimensions(owner)?;

        let definitions = super::module_definitions::ModuleDefinitions::try_new(
            self.static_definitions,
            nominal_types,
        )
        .map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;

        Ok(HirDag {
            dag_id: owner.clone(),
            extern_functions,
            definitions,
            display_dimensions,
            decls,
            included_plots: self.included_plots,
            source_declarations: self.source_declarations,
            static_ports: self.static_ports,
            assumes_map,
            expected_fail,
            dynamic_unit_scales,
            imported_bindings: self.imported_bindings,
            external_surface: self.external_surface,
            semantic_instances,
        })
    }

    /// Lower every nominal type symbol of `owner` to its canonical definition.
    ///
    /// A declared type is lowered from its own declaration. A type a
    /// selective include projects under Static bindings is lowered from the
    /// template's declaration in the template's scope and specialized through
    /// that include's canonical substitution.
    fn lower_nominal_types(
        &self,
        owner: &crate::dag_id::DagId,
        definitions: &super::static_definitions::StaticDefinitionEvaluator<'_>,
        src: &NamedSource<Arc<String>>,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<crate::hir::NominalTypeRegistry, GraphcalError> {
        use crate::hir::nominal_lower::{
            NominalLowering, lower_type_declaration, specialize_nominal_type,
        };

        let resolver = definitions.resolver();
        let Some(symbols) = resolver.symbols(owner) else {
            return Ok(crate::hir::NominalTypeRegistry::default());
        };
        let lowering = NominalLowering {
            resolver,
            cancellation,
        };
        let invariant = |message: String, span: Span| GraphcalError::InternalError {
            message,
            src: src.clone(),
            span: span.into(),
        };
        let mut declarations = symbols.struct_types().iter().collect::<Vec<_>>();
        declarations.sort_by_key(|(_, symbol)| symbol.span().offset());
        declarations.into_iter().try_fold(
            crate::hir::NominalTypeRegistry::default(),
            |mut lowered, (name, symbol)| {
                cancellation.checkpoint()?;
                let identity = symbol.resolved().clone();
                let source = symbols.struct_type_projection(name).map_or(
                    &identity,
                    crate::resolve::symbols::StaticProjection::template,
                );
                let Some((declaration, declaration_src)) = definitions.type_declaration(source)
                else {
                    return Err(invariant(
                        format!("nominal type `{source}` has no source declaration"),
                        symbol.span(),
                    ));
                };
                let definition = match symbols.struct_type_projection(name) {
                    Some(projection) => {
                        let template = lower_type_declaration(
                            declaration,
                            projection.template().clone(),
                            declaration.name.span,
                            declaration_src,
                            lowering,
                        )?;
                        let substitution = self
                            .projection_substitution(symbols, projection)
                            .ok_or_else(|| {
                                invariant(
                                    format!("projected type `{identity}` has no include instance"),
                                    symbol.span(),
                                )
                            })?;
                        specialize_nominal_type(
                            &template,
                            identity,
                            &substitution,
                            src.clone(),
                            symbol.span(),
                        )
                        .map_err(|error| invariant(error.to_string(), symbol.span()))?
                    }
                    None => {
                        lower_type_declaration(declaration, identity, symbol.span(), src, lowering)?
                    }
                };
                lowered.insert(definition).map_err(|error| {
                    invariant(
                        format!("cannot register HIR nominal type: {error}"),
                        symbol.span(),
                    )
                })?;
                Ok(lowered)
            },
        )
    }

    /// The canonical substitution of the include that projects a template
    /// type: its Static bindings plus the template dimensions it projects as
    /// this module's own specialized declarations.
    fn projection_substitution(
        &self,
        symbols: &crate::resolve::symbols::ModuleSymbols,
        projection: &crate::resolve::symbols::StaticProjection<
            crate::syntax::type_name::StructTypeNameNamespace,
        >,
    ) -> Option<crate::ir::static_substitution::StaticSubstitution> {
        let template = projection.template().owner();
        let record = self.semantic_instances.iter().find(|record| {
            record.instance.id().scope() == projection.instance()
                && record.instance.id().template() == template
        })?;
        let mut substitution = record.instance.specialization().substitution.clone();
        substitution.dimensions.extend(
            symbols
                .dimension_projections()
                .filter(|(_, dimension)| {
                    dimension.instance() == projection.instance()
                        && dimension.template().owner() == template
                })
                .filter_map(|(local, dimension)| {
                    symbols
                        .dimensions()
                        .get(local)
                        .map(|symbol| (dimension.template().clone(), symbol.resolved().clone()))
                }),
        );
        Some(substitution)
    }

    /// Add a const alias: a synthetic const declaration that references another const.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_const_alias(
        &mut self,
        name: DeclName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.decls.push(Decl::Const(entry::ConstEntry {
            identity: projection_alias(&body_resolution_owner, name),
            type_ann: InScope::new(type_ann, type_resolution_owner),
            expr: InScope::new(expr, body_resolution_owner),
            span,
        }));
    }

    /// Add a node alias: a synthetic node declaration that references another node/param.
    ///
    /// Used for selective instantiated imports where `delta_v` aliases `prefix.delta_v`.
    pub fn add_node_alias(
        &mut self,
        name: DeclName,
        type_ann: TypeExpr,
        type_resolution_owner: crate::dag_id::DagId,
        expr: Expr,
        body_resolution_owner: crate::dag_id::DagId,
        span: Span,
    ) {
        self.decls.push(Decl::Node(entry::NodeEntry {
            identity: projection_alias(&body_resolution_owner, name),
            type_ann: InScope::new(type_ann, type_resolution_owner),
            definition: InScope::new(
                crate::node_definition::NodeDefinition::Formula(expr),
                body_resolution_owner,
            ),
            span,
        }));
    }

    /// Record include-site nominal override obligations before substitution.
    ///
    /// Field and generic-argument ownership is checked later from canonical
    /// inferred HIR. Explicit index labels and constructors need a narrow
    /// registry-backed preflight because substitution can make HIR lowering
    /// reject them before TIR exists.
    pub fn include_override_reconciliations(
        &self,
        bindings: &HashMap<DeclName, Expr>,
        substitution: &crate::ir::static_substitution::StaticSubstitution,
        resolver: &crate::resolve::ModuleResolver,
        dependency_owner: &crate::dag_id::DagId,
        importer_src: &NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Result<IncludeOverrideReconciliations, GraphcalError> {
        self.params()
            .filter(|param| !bindings.contains_key(param.name()))
            .map(|param| {
                let mut reconciliations = param.override_reconciliations.clone();
                if let Some(default) = &param.default
                    && (!substitution.indexes.is_empty() || !substitution.types.is_empty())
                {
                    NominalOverridePreflight {
                        substitution,
                        resolver,
                        dependency_owner,
                        orphan_decl: param.name(),
                        importer_src,
                        include_span,
                    }
                    .visit_expr(&default.syntax)?;
                    reconciliations.push(
                        crate::ir::override_reconciliation::OverrideReconciliation::new(
                            param.identity(),
                            substitution,
                            importer_src.clone(),
                            include_span,
                        ),
                    );
                }
                Ok((param.identity(), reconciliations))
            })
            .filter_map(|result| match result {
                Ok((_, reconciliations)) if reconciliations.is_empty() => None,
                result => Some(result),
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map(IncludeOverrideReconciliations)
    }
}

/// Narrow pre-HIR guard for nominal syntax of the producer's parameter
/// defaults.
///
/// HIR lowering validates explicit labels and constructor patterns against
/// the substituted definitions. An incompatible replacement could therefore
/// fail before TIR ownership is available. Field and generic-argument
/// dependencies remain deferred to canonical inference.
struct NominalOverridePreflight<'a> {
    substitution: &'a crate::ir::static_substitution::StaticSubstitution,
    resolver: &'a crate::resolve::ModuleResolver,
    dependency_owner: &'a crate::dag_id::DagId,
    orphan_decl: &'a DeclName,
    importer_src: &'a NamedSource<Arc<String>>,
    include_span: Span,
}

impl NominalOverridePreflight<'_> {
    fn check_label(&self, index: &IndexName, detail: String) -> Result<(), GraphcalError> {
        let Ok(symbol) = self.resolver.resolve_index_path(
            self.dependency_owner,
            &crate::syntax::names::NamePath::local(index.atom().clone()),
        ) else {
            return Ok(());
        };
        if !self.substitution.indexes.contains_key(symbol.resolved()) {
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
        let Ok(symbol) = self.resolver.resolve_constructor_path(
            self.dependency_owner,
            &crate::syntax::names::NamePath::local(constructor.atom().clone()),
        ) else {
            return Ok(());
        };
        let owning_type = symbol.kind().owner_type();
        let owning_identity = symbol.owner_type_identity();
        if !self.substitution.types.contains_key(&owning_identity) {
            return Ok(());
        }
        Err(GraphcalError::IncludeMustReconcileOverride {
            overridden: owning_type.to_string(),
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
                let name = IndexName::classify(index.leaf().name.atom().clone());
                self.check_label(&name, format!("`{index}#{}`", label.value))
            }
            crate::syntax::ast::UnresolvedRef::Path(path) => {
                if let Some(name) = path.as_bare() {
                    let constructor = ConstructorName::classify(name.name.atom().clone());
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
                    let name = IndexName::classify(index.value.leaf().clone());
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
                if let crate::syntax::ast::MapEntryKey::Named { index, .. } = key {
                    let index_name = IndexName::classify(index.value.leaf().clone());
                    self.check_label(&index_name, format!("`{key}`"))?;
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
                    let name = IndexName::classify(index.value.leaf().clone());
                    self.check_label(&name, format!("`{}#{}`", index.value, variant.value))?;
                }
                crate::desugar::desugared_ast::MatchPattern::Path { path, .. } => {
                    if let Some(name) = path.as_bare() {
                        let constructor = ConstructorName::classify(name.name.atom().clone());
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
            let constructor = ConstructorName::classify(name.name.atom().clone());
            self.check_constructor(&constructor, format!("constructor `{constructor}(...)`"))?;
        }
        fields
            .iter()
            .try_for_each(|field| self.visit_expr(&field.value))
    }
}
