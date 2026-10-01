//! The freeze boundary: lowering every assembled body of an [`UnfrozenIR`]
//! to HIR in the scope that authored it, producing a frozen [`HirDag`].

use std::collections::HashMap;

use crate::declaration_category::DeclCategory;
use crate::desugar::desugared_ast::{Expr, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::ir::instance::identity::instance_declaration;
use crate::outcome::Outcome;
use crate::semantic_error::SemanticError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::source_id::SourceId;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;

use super::{
    decl_table::DeclTable,
    entry::{Decl, InScope},
    extern_fns::resolve_plugin_imports,
    model::{
        AssertEntry, ConstEntry, DynamicUnitScaleEntry, FigureEntry, HirDag, HirDecl, LayerEntry,
        LoweredPlotBody, LoweredPlotField, LoweredPlotProperty, NodeEntry, ParamEntry,
        ParsedExpectedFailMetadata, PlotEntry, ResolvedExpectedFailMetadata, UnfrozenIR,
    },
};

impl UnfrozenIR {
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
    /// Returns a [`SemanticError`] if any body contains a reference that
    /// cannot be resolved.
    pub fn freeze(
        self,
        owner: &crate::dag_id::DagId,
        definitions: &mut super::static_definitions::StaticDefinitionEvaluator<'_>,
        src: SourceId,
    ) -> Result<HirDag, SemanticError> {
        crate::outcome::without_cancellation(|cancellation| {
            self.freeze_with_cancellation(owner, definitions, src, cancellation)
        })
    }

    /// Freeze into IR while observing cooperative cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`Outcome::Cancelled`] on cancellation, or a [`SemanticError`]
    /// for unresolved bodies.
    #[expect(
        clippy::too_many_lines,
        reason = "single lowering boundary over every declaration kind"
    )]
    pub fn freeze_with_cancellation(
        self,
        owner: &crate::dag_id::DagId,
        definitions: &mut super::static_definitions::StaticDefinitionEvaluator<'_>,
        src: SourceId,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<HirDag, Outcome<SemanticError>> {
        cancellation.checkpoint()?;
        let resolver = definitions.resolver();
        let time_zones = crate::semantic::time_zone::TimeZoneRegistry::bundled();
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
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
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
                return Err(SemanticError::internal_error(
                    format!("{origin} `{name}` collides with a declaration"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
                .into());
            }
        }

        let generic_scope = crate::hir::lower::GenericScope::new();
        let overlay = crate::hir::expr_lower::context::BindingOverlay::Frozen(
            crate::hir::expr_lower::context::FrozenBindings {
                unit_bindings: &self.unit_bindings,
                decl_bindings: &decl_bindings,
                instance_templates: &instance_templates,
            },
        );
        let lower_in = |expr: &Expr, resolution_owner: &crate::dag_id::DagId| {
            let scope =
                crate::hir::lower::ModuleScope::new(resolution_owner, resolver, &generic_scope);
            let expr_ctx = crate::hir::expr_lower::context::ExprLoweringContext::with_overlay(
                scope,
                &time_zones,
                overlay,
            );
            crate::hir::expr_lower::lower::lower_expr(expr, expr_ctx)
                .map_err(|err| crate::hir::diagnostics::expr_lower_error_to_semantic(&err, src))
        };
        let lower_scoped = |expr: &InScope<Expr>| lower_in(&expr.syntax, &expr.resolution_owner);
        let lower_type_annotation = |type_ann: &InScope<TypeExpr>| -> Result<
            crate::hir::type_annotation::TypeAnnotation,
            SemanticError,
        > {
            let InScope {
                syntax: type_ann,
                resolution_owner,
            } = type_ann;
            crate::hir::diagnostics::validate_type_annotation(type_ann, src)?;
            let scope =
                crate::hir::lower::ModuleScope::new(resolution_owner, resolver, &generic_scope);
            let decl_type =
                crate::hir::lower::lower_decl_type(type_ann, scope).map_err(|error| {
                    crate::hir::diagnostics::type_lower_error_to_graphcal(&error, src)
                })?;
            let domain_bounds = type_ann
                .domain_bounds()
                .iter()
                .map(|bound| {
                    Ok(crate::hir::type_annotation::DomainBound {
                        kind: bound.kind,
                        value: lower_in(&bound.value, resolution_owner)?,
                        span: bound.span,
                    })
                })
                .collect::<Result<_, SemanticError>>()?;
            Ok(crate::hir::type_annotation::TypeAnnotation {
                decl_type,
                domain_bounds,
                span: type_ann.span,
            })
        };

        let dynamic_unit_scales =
            self.dynamic_unit_scales
                .iter()
                .map(|entry| {
                    cancellation.checkpoint()?;
                    let unit = resolver
                    .resolve_unit_path(&entry.unit, &entry.spelling.to_name_path())
                    .map(crate::resolve::symbols::SymbolRef::into_resolved)
                    .map_err(|err| SemanticError::internal_error(format!(
                            "registered dynamic unit `{}` did not resolve canonically: {err}",
                            entry.spelling
                        ), src, crate::diagnostic_anchor::DiagnosticAnchor::Source(entry.span)))?;
                    Ok(DynamicUnitScaleEntry {
                        unit,
                        spelling: entry.spelling.clone(),
                        expr: lower_scoped(&entry.expr)?,
                        declared_dimension: entry.declared_dimension.clone(),
                        base_unit_dimension: entry.base_unit_dimension.clone(),
                        span: entry.span,
                        src: entry.src,
                    })
                })
                .collect::<Result<Vec<_>, Outcome<SemanticError>>>()?;

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
                .collect::<Result<Vec<_>, SemanticError>>()
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
            |decl| -> Result<HirDecl, Outcome<SemanticError>> {
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
                            crate::hir::expr_lower::context::ExprLoweringContext::with_overlay(
                                crate::hir::lower::ModuleScope::new(
                                    &entry.definition.resolution_owner,
                                    resolver,
                                    &generic_scope,
                                ),
                                &time_zones,
                                overlay,
                            ),
                        )
                        .map_err(|error| {
                            crate::hir::diagnostics::expr_lower_error_to_semantic(&error, src)
                        })?,
                        identity: entry.identity,
                        span: entry.span,
                    }),
                    Decl::Assert(entry) => Decl::Assert(AssertEntry {
                        body: crate::hir::expr_lower::lower::lower_assert_body(
                            &entry.body.syntax,
                            crate::hir::expr_lower::context::ExprLoweringContext::with_overlay(
                                crate::hir::lower::ModuleScope::new(
                                    &entry.body.resolution_owner,
                                    resolver,
                                    &generic_scope,
                                ),
                                &time_zones,
                                overlay,
                            ),
                        )
                        .map_err(|err| {
                            crate::hir::diagnostics::expr_lower_error_to_semantic(&err, src)
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
                            .collect::<Result<Vec<_>, SemanticError>>()?;
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
                            .find(|projection| &record.instance.exposed_name(*projection) == name)
                            .map(|projection| {
                                instance_declaration(
                                    record.instance.id(),
                                    projection.target.leaf().clone(),
                                )
                            })
                    })
                })
                .ok_or_else(|| {
                    SemanticError::internal_error(
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
                        .collect::<Result<Vec<_>, SemanticError>>()?,
                ))
            })
            .collect::<Result<HashMap<_, _>, SemanticError>>()?;
        let expected_fail = self
            .expected_fail
            .into_iter()
            .map(|(assertion, metadata)| {
                Ok((
                    lookup_assertion(&assertion)?,
                    metadata.resolve(resolver, src)?,
                ))
            })
            .collect::<Result<HashMap<_, _>, SemanticError>>()?;

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
                    value_bindings,
                    runtime_unit_names: record.runtime_unit_names.clone(),
                    output_projections: record.output_projections.clone(),
                    assertion_projections: record.assertion_projections.clone(),
                    plot_projections: record.plot_projections.clone(),
                    override_reconciliations: record.override_reconciliations.clone(),
                })
            })
            .collect::<Result<Vec<_>, SemanticError>>()?;

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
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
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
        src: SourceId,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<crate::hir::nominal::NominalTypeRegistry, Outcome<SemanticError>> {
        use crate::hir::nominal_lower::{
            NominalLowering, lower_type_declaration, specialize_nominal_type,
        };

        let resolver = definitions.resolver();
        let Some(symbols) = resolver.symbols(owner) else {
            return Ok(crate::hir::nominal::NominalTypeRegistry::default());
        };
        let lowering = NominalLowering {
            resolver,
            cancellation,
        };
        let invariant = |message: String, span: Span| {
            SemanticError::internal_error(
                message,
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
            )
        };
        let mut declarations = symbols.struct_types().iter().collect::<Vec<_>>();
        declarations.sort_by_key(|(_, symbol)| symbol.span().offset());
        declarations.into_iter().try_fold(
            crate::hir::nominal::NominalTypeRegistry::default(),
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
                    )
                    .into());
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
                            src,
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
}

impl ParsedExpectedFailMetadata {
    /// Resolve every named index key in the scope that authored the attribute.
    pub(super) fn resolve(
        self,
        resolver: &crate::resolve::ModuleResolver,
        src: SourceId,
    ) -> Result<ResolvedExpectedFailMetadata, SemanticError> {
        use crate::assertion_expectation::{ExpectedFail, ExpectedFailKeyPart};

        let Self {
            expected,
            resolution_owner,
            attribute_span,
        } = self;
        let expected = match expected {
            ExpectedFail::All => ExpectedFail::All,
            ExpectedFail::Variants(keys) => ExpectedFail::Variants(keys.try_map(|key| {
                key.try_map(|part| match part {
                    ExpectedFailKeyPart::Named {
                        index,
                        variant,
                        span,
                    } => resolver
                        .resolve_index_variant_parts(&resolution_owner, &index, &variant)
                        .map(|resolved| ExpectedFailKeyPart::resolved(resolved, span))
                        .map_err(|err| {
                            SemanticError::located(
                                src,
                                span,
                                EvaluationError::Failed {
                                    message: err.to_string(),
                                },
                            )
                        }),
                    ExpectedFailKeyPart::FinitePosition { position, span } => {
                        Ok(ExpectedFailKeyPart::FinitePosition { position, span })
                    }
                })
            })?),
        };
        Ok(ResolvedExpectedFailMetadata {
            expected,
            attribute_span,
        })
    }
}
