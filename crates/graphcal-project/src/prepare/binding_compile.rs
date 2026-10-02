//! Prepared-parameter binding compilation and row construction.

use crate::binding_error::{BindingError, BindingLiteralError, BindingValueKind};

use std::sync::Arc;

use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::ast::{
    FieldInit, GenericArg, Ident, IdentPath, MapEntry, MapEntryKey,
};
use graphcal_compiler::syntax::fin_position::FinPosition;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::parser::{ParseError, Parser};
use graphcal_compiler::syntax::phase::Desugared;
use graphcal_compiler::syntax::span::Spanned;
use graphcal_compiler::syntax::token::SourceIdentifier;
use graphcal_compiler::syntax::token::SourceIdentifierError;
use graphcal_compiler::syntax::type_name::ConstructorName;
use graphcal_eval::invariant::Failure;
use miette::{NamedSource, SourceSpan};

use super::{
    AstExprKind, CheckedEntryInterface, CompileError, DeclName, DiagnosticAnchor, EvalSession,
    Expr, ExprLoweringContext, GenericScope, HirExprKind, ModelIndexKind, ModelIndexSchema,
    ModelSchemaGraph, ModelSchemaGraphBuilder, ModelTypeId, ModelValueSchema, ModuleScope,
    ParameterBindingBuilder, ParameterPort, ParameterPosition, ParameterValue, PreparedProject,
    RuntimeParameterBinding, RuntimeParameterBindings, RuntimeValueMap, SemanticError, Span,
    parameter_domain,
};

/// One checked browser-editor value. Containers carry stable schema-arena
/// positions; only leaves contain parsed Graphcal literal syntax.
#[derive(Debug, Clone)]
pub enum StructuredValueExpr {
    /// A scalar/key/datetime/complex closed literal.
    Literal(Expr),
    /// One constructor and its declaration-order field values.
    Algebraic {
        definition: usize,
        constructor: usize,
        fields: Vec<Self>,
    },
    /// Every value on one fixed axis, in schema order.
    Indexed { entries: Vec<Self> },
}

/// Typed address into a structured editor value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredBindingPathSegment {
    Field(usize),
    Entry(usize),
}

/// A structured editor value rejected at a typed address.
#[derive(Debug, thiserror::Error)]
#[error("{kind}")]
pub struct StructuredBindingError {
    path: Vec<StructuredBindingPathSegment>,
    kind: StructuredBindingErrorKind,
}

impl StructuredBindingError {
    #[must_use]
    pub fn path(&self) -> &[StructuredBindingPathSegment] {
        &self.path
    }

    #[must_use]
    pub const fn kind(&self) -> &StructuredBindingErrorKind {
        &self.kind
    }
}

/// Why a structured editor value was rejected.
#[derive(Debug, thiserror::Error)]
pub enum StructuredBindingErrorKind {
    #[error("expected an algebraic constructor value")]
    ExpectedAlgebraic,
    #[error("expected a complete indexed value")]
    ExpectedIndexed,
    #[error("unexpected algebraic value")]
    UnexpectedAlgebraic,
    #[error("unexpected indexed value")]
    UnexpectedIndexed,
    #[error("missing expected algebraic schema")]
    MissingAlgebraicSchema,
    #[error("constructor belongs to a different algebraic type")]
    ForeignConstructor,
    #[error("unknown constructor")]
    UnknownConstructor,
    #[error("constructor `{constructor}` expects {expected} fields, received {received}")]
    FieldCount {
        constructor: ConstructorName,
        expected: usize,
        received: usize,
    },
    #[error("constructor `{constructor}` {reason}")]
    ConstructorSpelling {
        constructor: ConstructorName,
        reason: SourceIdentifierError,
    },
    #[error("indexed value expects {expected} entries, received {received}")]
    EntryCount { expected: usize, received: usize },
    #[error("finite index cardinality is too large")]
    CardinalityTooLarge,
    #[error("indexed entry position is too large")]
    EntryPositionTooLarge,
    #[error("indexed axis is not visible at the boundary")]
    AxisNotVisible,
    /// A coordinate axis has neither variant names nor a `Fin(N)`
    /// cardinality, so no source map key can address its entries.
    #[error("coordinate-indexed values cannot be bound entry by entry")]
    CoordinateEntries,
    /// A literal leaf or the assembled value failed compilation.
    #[error(transparent)]
    Compile(Box<CompileError>),
}

fn structured_error(
    path: &[StructuredBindingPathSegment],
    kind: StructuredBindingErrorKind,
) -> StructuredBindingError {
    StructuredBindingError {
        path: path.to_vec(),
        kind,
    }
}

/// Where a bound value's text lives. Diagnostics about the value are drawn
/// against `source` and rendered with `sources`, which also resolves every
/// project source id.
#[derive(Clone, Copy)]
struct ValueSite<'a> {
    /// Entry parameter receiving the value.
    name: &'a DeclName,
    source: SourceId,
    sources: &'a SourceRegistry,
}

impl ValueSite<'_> {
    fn render(self, error: SemanticError) -> CompileError {
        CompileError::semantic(error, self.sources)
    }

    /// A binding error located in the value at `span`.
    fn error(
        self,
        span: Span,
        error: impl FnOnce(DeclName, NamedSource<Arc<String>>, SourceSpan) -> BindingError,
    ) -> CompileError {
        CompileError::Binding(error(
            self.name.clone(),
            self.sources.renderable(self.source),
            span.into(),
        ))
    }

    /// Validate that a lowered binding value is a closed literal tree.
    fn closed(
        self,
        hir: graphcal_compiler::hir::expr::Expr<graphcal_compiler::hir::expr::Draft>,
    ) -> Result<graphcal_compiler::hir::closed_expr::ClosedExpr, CompileError> {
        let span = hir.span;
        graphcal_compiler::hir::closed_expr::ClosedExpr::try_new(hir).map_err(|reason| {
            self.error(span, |name, src, span| BindingError::NotClosed {
                name,
                reason,
                src,
                span,
            })
        })
    }
}

/// A closed value expression parsed from external text of its own.
///
/// A `--param` value or an editor field keeps its text next to the parsed
/// expression, so the expression's spans are always drawn against the text
/// they index.
#[derive(Debug, Clone)]
pub struct ExternalValue {
    label: String,
    text: Arc<String>,
    expr: Expr,
}

impl ExternalValue {
    /// Parse closed value syntax from `text`, named `label` in diagnostics.
    ///
    /// # Errors
    ///
    /// Returns the [`ParseError`] of `text`.
    pub fn parse(label: impl Into<String>, text: impl Into<String>) -> Result<Self, ParseError> {
        let text = text.into();
        let expr = Parser::new(&text).parse_single_expr()?.into();
        Ok(Self {
            label: label.into(),
            text: Arc::new(text),
            expr,
        })
    }

    /// Diagnostic name of the value's text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The text the expression's spans index.
    #[must_use]
    pub const fn text(&self) -> &Arc<String> {
        &self.text
    }

    /// The parsed, desugared expression.
    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }
}

fn structured_compile_error(
    path: &[StructuredBindingPathSegment],
    error: CompileError,
) -> StructuredBindingError {
    structured_error(path, StructuredBindingErrorKind::Compile(Box::new(error)))
}

impl PreparedProject {
    /// Begin constructing one plan-scoped parameter row.
    #[must_use]
    pub fn binding_builder(&self) -> ParameterBindingBuilder<'_> {
        ParameterBindingBuilder {
            project: self,
            slots: vec![None; self.parameter_ports.len()],
        }
    }

    /// Compile and validate one closed recursive value for a plan position.
    ///
    /// Diagnostics about the value are drawn against the value's own text.
    pub fn compile_parameter_value(
        &self,
        position: ParameterPosition,
        value: &ExternalValue,
    ) -> Result<ParameterValue, CompileError> {
        let port = self.port_at(position)?;
        let mut sources = SourceRegistry::extending(Arc::clone(&self.sources));
        let source = sources.register(value.label(), Arc::clone(value.text()));
        self.compile_value(
            port,
            value.expr(),
            ValueSite {
                name: &port.name,
                source,
                sources: &sources,
            },
        )
    }

    /// Resolve an entry parameter name and compile one closed recursive value.
    pub fn compile_named_parameter_value(
        &self,
        name: &DeclName,
        value: &ExternalValue,
    ) -> Result<ParameterValue, CompileError> {
        self.compile_parameter_value(self.parameter_position(name)?, value)
    }

    /// Compile a value synthesized without text of its own (from a JSON
    /// document or a structured editor value). Its diagnostics are reported
    /// as messages by the caller, so its synthetic spans are never drawn.
    pub(super) fn compile_synthesized_value(
        &self,
        position: ParameterPosition,
        expr: &Expr,
    ) -> Result<ParameterValue, CompileError> {
        let port = self.port_at(position)?;
        self.compile_value(port, expr, self.entry_site(port))
    }

    fn compile_value(
        &self,
        port: &ParameterPort,
        expr: &Expr,
        site: ValueSite<'_>,
    ) -> Result<ParameterValue, CompileError> {
        let normalized =
            normalize_binding_literal(expr.clone(), &port.value_schema, &self.schema_graph)
                .map_err(|reason| {
                    site.error(expr.span, |name, src, span| BindingError::InvalidLiteral {
                        name,
                        reason,
                        src,
                        span,
                    })
                })?;
        let tree = self.check_closed_binding(site, port, &normalized)?;
        Ok(ParameterValue {
            plan_id: self.plan_id,
            position: port.position,
            binding: self.evaluate_closed_binding(site, &tree)?,
        })
    }

    /// The value site of a synthesized value: the entry source, whose
    /// registry resolves every project source id.
    fn entry_site<'a>(&'a self, port: &'a ParameterPort) -> ValueSite<'a> {
        ValueSite {
            name: &port.name,
            source: self.source,
            sources: &self.sources,
        }
    }

    /// Build and compile a structured browser value without reparsing its
    /// algebraic or indexed container syntax.
    pub fn compile_structured_parameter_value(
        &self,
        position: ParameterPosition,
        value: &StructuredValueExpr,
    ) -> Result<ParameterValue, StructuredBindingError> {
        let port = self
            .port_at(position)
            .map_err(|error| structured_compile_error(&[], error))?;
        let expr = self.structured_binding_expr(
            self.entry_site(port),
            value,
            &port.value_schema,
            self.tir().root_dag_id(),
            &mut Vec::new(),
        )?;
        self.compile_synthesized_value(position, &expr)
            .map_err(|error| structured_compile_error(&[], error))
    }

    fn structured_binding_expr(
        &self,
        site: ValueSite<'_>,
        value: &StructuredValueExpr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
        path: &mut Vec<StructuredBindingPathSegment>,
    ) -> Result<Expr, StructuredBindingError> {
        match (value, expected) {
            (StructuredValueExpr::Literal(_), ModelValueSchema::Algebraic(_)) => Err(
                structured_error(path, StructuredBindingErrorKind::ExpectedAlgebraic),
            ),
            (StructuredValueExpr::Literal(_), ModelValueSchema::Indexed { .. }) => Err(
                structured_error(path, StructuredBindingErrorKind::ExpectedIndexed),
            ),
            (StructuredValueExpr::Literal(expr), _) => {
                self.structured_literal_expr(site, expr, expected, owner, path)
            }
            (
                StructuredValueExpr::Algebraic {
                    definition,
                    constructor,
                    fields,
                },
                ModelValueSchema::Algebraic(expected_id),
            ) => self.structured_algebraic_expr(
                site,
                *definition,
                *constructor,
                fields,
                expected_id,
                path,
            ),
            (
                StructuredValueExpr::Indexed { entries },
                ModelValueSchema::Indexed { element, axis },
            ) => self.structured_indexed_expr(site, entries, element, axis, owner, path),
            (StructuredValueExpr::Algebraic { .. }, _) => Err(structured_error(
                path,
                StructuredBindingErrorKind::UnexpectedAlgebraic,
            )),
            (StructuredValueExpr::Indexed { .. }, _) => Err(structured_error(
                path,
                StructuredBindingErrorKind::UnexpectedIndexed,
            )),
        }
    }

    fn structured_literal_expr(
        &self,
        site: ValueSite<'_>,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
        path: &[StructuredBindingPathSegment],
    ) -> Result<Expr, StructuredBindingError> {
        let normalized = normalize_binding_literal(expr.clone(), expected, &self.schema_graph)
            .map_err(|reason| {
                structured_compile_error(
                    path,
                    site.error(expr.span, |name, src, span| BindingError::InvalidLiteral {
                        name,
                        reason,
                        src,
                        span,
                    }),
                )
            })?;
        let hir = self
            .lower_closed_binding_expr(site, &normalized, expected, owner)
            .map_err(|error| structured_compile_error(path, error))?;
        let hir = site
            .closed(hir)
            .map_err(|error| structured_compile_error(path, error))?;
        graphcal_compiler::tir::dim_check::check_external_value_expr_type(
            self.tir(),
            &hir,
            &expected.declared_type(),
            site.source,
        )
        .map_err(|error| structured_compile_error(path, site.render(error)))?;
        Ok(normalized)
    }

    fn structured_algebraic_expr(
        &self,
        site: ValueSite<'_>,
        definition_index: usize,
        constructor_index: usize,
        fields: &[StructuredValueExpr],
        expected_id: &ModelTypeId,
        path: &mut Vec<StructuredBindingPathSegment>,
    ) -> Result<Expr, StructuredBindingError> {
        let expected_definition =
            self.schema_graph
                .definition_index(expected_id)
                .ok_or_else(|| {
                    structured_error(path, StructuredBindingErrorKind::MissingAlgebraicSchema)
                })?;
        if definition_index != expected_definition {
            return Err(structured_error(
                path,
                StructuredBindingErrorKind::ForeignConstructor,
            ));
        }
        let constructor = self
            .schema_graph
            .definition_at(definition_index)
            .and_then(|definition| definition.constructors().get(constructor_index))
            .ok_or_else(|| {
                structured_error(path, StructuredBindingErrorKind::UnknownConstructor)
            })?;
        if fields.len() != constructor.fields().len() {
            return Err(structured_error(
                path,
                StructuredBindingErrorKind::FieldCount {
                    constructor: constructor.name().clone(),
                    expected: constructor.fields().len(),
                    received: fields.len(),
                },
            ));
        }
        // Canonical constructor names are declared in source, so they always
        // occupy a source identifier position.
        let callee = SourceIdentifier::parse(constructor.name().as_str()).map_err(|reason| {
            structured_error(
                path,
                StructuredBindingErrorKind::ConstructorSpelling {
                    constructor: constructor.name().clone(),
                    reason,
                },
            )
        })?;
        let span = Span::new(0, 0);
        let owner = expected_id.identity().resolved().owner();
        let fields = fields
            .iter()
            .zip(constructor.fields())
            .enumerate()
            .map(|(index, (value, schema))| {
                path.push(StructuredBindingPathSegment::Field(index));
                let value = self.structured_binding_expr(site, value, schema.value(), owner, path);
                path.pop();
                value.map(|value| FieldInit {
                    name: Spanned::new(schema.name().clone(), span),
                    value,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Expr::new(
            AstExprKind::ConstructorCall {
                callee: IdentPath::local(Ident { name: callee, span }),
                generic_args: Vec::new(),
                fields,
            },
            span,
        ))
    }

    fn structured_indexed_expr(
        &self,
        site: ValueSite<'_>,
        entries: &[StructuredValueExpr],
        element: &ModelValueSchema,
        axis: &ModelIndexSchema,
        owner: &graphcal_compiler::dag_id::DagId,
        path: &mut Vec<StructuredBindingPathSegment>,
    ) -> Result<Expr, StructuredBindingError> {
        let expected_count = match axis.kind() {
            ModelIndexKind::Named { variants } => variants.len().get(),
            ModelIndexKind::Coordinate { coordinates_si, .. } => coordinates_si.len(),
            ModelIndexKind::Finite { index } => index.cardinality().get(),
        };
        if entries.len() != expected_count {
            return Err(structured_error(
                path,
                StructuredBindingErrorKind::EntryCount {
                    expected: expected_count,
                    received: entries.len(),
                },
            ));
        }
        let span = Span::new(0, 0);
        let entries = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                path.push(StructuredBindingPathSegment::Entry(index));
                let value = self.structured_binding_expr(site, entry, element, owner, path);
                path.pop();
                Ok(MapEntry {
                    keys: NonEmpty::singleton(self.structured_map_key(axis, index, span, path)?),
                    value: value?,
                })
            })
            .collect::<Result<Vec<_>, StructuredBindingError>>()?;
        Ok(Expr::new(AstExprKind::MapLiteral { entries }, span))
    }

    /// Key of entry `position` on `axis`.
    fn structured_map_key(
        &self,
        axis: &ModelIndexSchema,
        position: usize,
        span: Span,
        path: &[StructuredBindingPathSegment],
    ) -> Result<MapEntryKey, StructuredBindingError> {
        match axis.kind() {
            ModelIndexKind::Finite { index } => {
                let cardinality = u64::try_from(index.cardinality().get()).map_err(|_| {
                    structured_error(path, StructuredBindingErrorKind::CardinalityTooLarge)
                })?;
                let position = u64::try_from(position)
                    .ok()
                    .and_then(|position| FinPosition::try_new(cardinality, position).ok())
                    .ok_or_else(|| {
                        structured_error(path, StructuredBindingErrorKind::EntryPositionTooLarge)
                    })?;
                Ok(MapEntryKey::Finite {
                    axis_span: span,
                    position: Spanned::new(position, span),
                })
            }
            ModelIndexKind::Named { variants } => {
                let index = axis
                    .identity()
                    .declared_resolved()
                    .and_then(|resolved| self.source_index_path(resolved))
                    .ok_or_else(|| {
                        structured_error(path, StructuredBindingErrorKind::AxisNotVisible)
                    })?;
                Ok(MapEntryKey::named(
                    Spanned::new(index, span),
                    Spanned::new(variants.as_slice()[position].clone(), span),
                ))
            }
            // A coordinate axis has neither variant names nor a `Fin(N)`
            // cardinality, so no source map key can address its entries.
            ModelIndexKind::Coordinate { .. } => Err(structured_error(
                path,
                StructuredBindingErrorKind::CoordinateEntries,
            )),
        }
    }
}

impl ParameterBindingBuilder<'_> {
    /// Bind one recursively structured browser value by entry name.
    pub fn bind_structured_expression(
        &mut self,
        name: &DeclName,
        value: &StructuredValueExpr,
    ) -> Result<(), StructuredBindingError> {
        let index = self
            .project
            .parameter_index(name)
            .map_err(|error| structured_compile_error(&[], error))?;
        let parameter = self.project.compile_structured_parameter_value(
            self.project.parameter_ports[index].position,
            value,
        )?;
        self.bind_value(parameter)
            .map_err(|error| structured_compile_error(&[], error))
    }

    /// Validate required slots and finish this row.
    pub fn finish(self) -> Result<ParameterBindingRow, CompileError> {
        if let Some(port) = self
            .project
            .parameter_ports
            .iter()
            .find(|port| !port.has_default && self.slots[port.position.index].is_none())
        {
            return Err(CompileError::Binding(
                BindingError::RequiredParamNotProvided {
                    name: port.name.clone(),
                    src: self.project.sources.renderable(self.project.source),
                    span: port.span.into(),
                },
            ));
        }
        let bindings = self
            .project
            .parameter_ports
            .iter()
            .zip(self.slots)
            .filter_map(|(port, binding)| {
                binding.map(|binding| (port.runtime_key.clone(), binding))
            })
            .collect();
        Ok(ParameterBindingRow {
            plan_id: self.project.plan_id,
            bindings,
        })
    }

    pub(super) fn insert(
        &mut self,
        position: ParameterPosition,
        binding: RuntimeParameterBinding,
    ) -> Result<(), CompileError> {
        let port = self.project.port_at(position)?;
        if let Some(constraint) = self.project.plan().domain_constraint(&port.runtime_key)
            && let Err(failure) =
                graphcal_eval::domain_check::check_domain_constraint(&binding.value(), constraint)
        {
            return Err(match failure {
                Failure::Error(violation) => {
                    self.project
                        .port_error(port, |name, src, span| BindingError::DomainViolation {
                            name,
                            violation,
                            src,
                            span,
                        })
                }
                Failure::Invariant(invariant) => self
                    .project
                    .render(invariant.into_internal_error(self.project.source)),
            });
        }
        let slot = &mut self.slots[position.index];
        if slot.is_some() {
            return Err(self.project.port_error(port, |name, src, span| {
                BindingError::BoundMoreThanOnce { name, src, span }
            }));
        }
        *slot = Some(binding);
        Ok(())
    }
}

/// One validated row of optional/defaulted and supplied parameter values.
#[derive(Debug, Clone)]
pub struct ParameterBindingRow {
    pub(super) plan_id: u64,
    pub(super) bindings: RuntimeParameterBindings,
}

/// A bare constructor call written for an algebraic external value, with the
/// parts the binding dispatch matched.
#[derive(Clone, Copy)]
struct CanonicalConstructorCall<'a> {
    expr: &'a Expr,
    callee: &'a IdentPath,
    generic_args: &'a [GenericArg<Desugared>],
    fields: &'a [FieldInit<Desugared>],
}

impl PreparedProject {
    pub(super) fn parameter_position(
        &self,
        name: &DeclName,
    ) -> Result<ParameterPosition, CompileError> {
        let index = self.parameter_index(name)?;
        Ok(self.parameter_ports[index].position)
    }

    fn parameter_index(&self, name: &DeclName) -> Result<usize, CompileError> {
        self.parameter_lookup.get(name).copied().ok_or_else(|| {
            let actual_kind = self
                .tir()
                .root()
                .declarations()
                .find_map(|entry| (entry.name() == name).then_some(entry.category()));
            actual_kind.map_or_else(
                || CompileError::Binding(BindingError::UnknownParam { name: name.clone() }),
                |actual_kind| {
                    CompileError::Binding(BindingError::NotAParam {
                        name: name.clone(),
                        actual_kind,
                    })
                },
            )
        })
    }

    pub(super) fn port_at(
        &self,
        position: ParameterPosition,
    ) -> Result<&ParameterPort, CompileError> {
        if position.plan_id != self.plan_id {
            return Err(CompileError::semantic(
                SemanticError::internal_error(
                    "parameter position belongs to another prepared project",
                    self.source,
                    DiagnosticAnchor::Builtin,
                ),
                &self.sources,
            ));
        }
        self.parameter_ports.get(position.index).ok_or_else(|| {
            CompileError::semantic(
                SemanticError::internal_error(
                    format!("parameter position {} is out of bounds", position.index),
                    self.source,
                    DiagnosticAnchor::Builtin,
                ),
                &self.sources,
            )
        })
    }

    /// Lower and check one closed binding value; returns its executable tree.
    fn check_closed_binding(
        &self,
        site: ValueSite<'_>,
        port: &ParameterPort,
        expr: &Expr,
    ) -> Result<
        graphcal_compiler::tir::typed::ScopedTree<'_, graphcal_compiler::tir::texpr::TExpr>,
        CompileError,
    > {
        let hir = self.lower_closed_binding_expr(
            site,
            expr,
            &port.value_schema,
            self.tir().root_dag_id(),
        )?;
        let hir = site.closed(hir)?;
        graphcal_compiler::tir::dim_check::check_external_value_expr_type(
            self.tir(),
            &hir,
            &port.declared_type,
            site.source,
        )
        .map_err(|error| site.render(error))
    }

    /// Lower a closed boundary value against its canonical recursive schema.
    ///
    /// Constructor and field-value syntax is interpreted in the module that
    /// defines the expected algebraic type. This is deliberately different
    /// from lowering a source expression in the entry module: decoding a value
    /// for an imported type must not require the entry to import that type's
    /// constructors or the units used by its field contracts. Nested values
    /// switch owners again at each canonical constructor boundary.
    fn lower_closed_binding_expr(
        &self,
        site: ValueSite<'_>,
        expr: &Expr,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::expr::Expr<graphcal_compiler::hir::expr::Draft>, CompileError>
    {
        match (&expr.kind, expected) {
            (
                AstExprKind::ConstructorCall {
                    callee,
                    generic_args,
                    fields,
                },
                ModelValueSchema::Algebraic(type_id),
            ) if callee.as_bare().is_some() => self.lower_canonical_constructor_binding(
                site,
                CanonicalConstructorCall {
                    expr,
                    callee,
                    generic_args,
                    fields,
                },
                type_id,
                owner,
            ),
            (
                AstExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(
                    path,
                )),
                ModelValueSchema::Algebraic(type_id),
            ) if path.as_bare().is_some()
                && self
                    .schema_graph
                    .definition(type_id)
                    .is_some_and(|definition| {
                        definition.constructors().iter().any(|constructor| {
                            constructor.name().atom() == path.leaf().name.atom()
                                && constructor.fields().is_empty()
                        })
                    }) =>
            {
                let constructor_expr = Expr::new(
                    AstExprKind::ConstructorCall {
                        callee: path.clone(),
                        generic_args: Vec::new(),
                        fields: Vec::new(),
                    },
                    expr.span,
                );
                self.lower_closed_binding_expr(site, &constructor_expr, expected, owner)
            }
            (AstExprKind::MapLiteral { entries }, ModelValueSchema::Indexed { .. }) => {
                self.lower_closed_map_binding(site, entries, expr.span, expected, owner)
            }
            // The value's kind is checked before its shape: a map literal for
            // a non-indexed value is a kind mismatch, not a map to lower.
            (AstExprKind::MapLiteral { .. }, _) => {
                Err(
                    site.error(expr.span, |name, src, span| BindingError::KindMismatch {
                        name,
                        actual: BindingValueKind::MapLiteral,
                        expected: expected
                            .declared_type()
                            .spelling(&self.tir().registry().dimensions),
                        src,
                        span,
                    }),
                )
            }
            _ => self.lower_binding_expr_in_owner(site, expr, owner),
        }
    }

    fn lower_canonical_constructor_binding(
        &self,
        site: ValueSite<'_>,
        call: CanonicalConstructorCall<'_>,
        type_id: &ModelTypeId,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::expr::Expr<graphcal_compiler::hir::expr::Draft>, CompileError>
    {
        let CanonicalConstructorCall {
            expr,
            callee,
            generic_args,
            fields,
        } = call;
        let Some(definition) = self.schema_graph.definition(type_id) else {
            return Err(self.binding_internal_error(
                "canonical external constructor references a missing algebraic definition",
                expr.span,
            ));
        };
        let Some(constructor) = definition
            .constructors()
            .iter()
            .find(|constructor| constructor.name().atom() == callee.leaf().name.atom())
        else {
            return self.lower_binding_expr_in_owner(site, expr, owner);
        };
        let constructor_owner = type_id.identity().resolved().owner();
        let (callee, generic_args) = self.lower_in_owner(site, constructor_owner, |context| {
            graphcal_compiler::hir::lower_constructor_head(callee, generic_args, expr.span, context)
        })?;
        let fields = fields
            .iter()
            .map(|field| {
                let value = constructor
                    .fields()
                    .iter()
                    .find(|schema| schema.name() == &field.name.value)
                    .map_or_else(
                        || self.lower_binding_expr_in_owner(site, &field.value, constructor_owner),
                        |schema| {
                            self.lower_closed_binding_expr(
                                site,
                                &field.value,
                                schema.value(),
                                constructor_owner,
                            )
                        },
                    )?;
                Ok(graphcal_compiler::hir::expr::FieldInit {
                    name: field.name.clone(),
                    value,
                })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::expr::Expr::new(
            HirExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            },
            expr.span,
        ))
    }

    fn lower_closed_map_binding(
        &self,
        site: ValueSite<'_>,
        entries: &[MapEntry<Desugared>],
        span: Span,
        expected: &ModelValueSchema,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::expr::Expr<graphcal_compiler::hir::expr::Draft>, CompileError>
    {
        let entries = entries
            .iter()
            .map(|entry| {
                // Resolve source-shaped keys normally, but lower each value
                // against its recursive schema so nested constructor owners survive.
                let keys = self.lower_in_owner(site, owner, |context| {
                    graphcal_compiler::hir::lower_map_entry_keys(entry, span, context)
                })?;
                let entry_schema =
                    map_entry_value_schema(expected, entry.keys.len()).map_err(|reason| {
                        site.error(entry.value.span, |name, src, span| {
                            BindingError::InvalidLiteral {
                                name,
                                reason,
                                src,
                                span,
                            }
                        })
                    })?;
                let value =
                    self.lower_closed_binding_expr(site, &entry.value, entry_schema, owner)?;
                Ok(graphcal_compiler::hir::expr::MapEntry { keys, value })
            })
            .collect::<Result<Vec<_>, CompileError>>()?;
        Ok(graphcal_compiler::hir::expr::Expr::new(
            HirExprKind::MapLiteral { entries },
            span,
        ))
    }

    fn lower_binding_expr_in_owner(
        &self,
        site: ValueSite<'_>,
        expr: &Expr,
        owner: &graphcal_compiler::dag_id::DagId,
    ) -> Result<graphcal_compiler::hir::expr::Expr<graphcal_compiler::hir::expr::Draft>, CompileError>
    {
        self.lower_in_owner(site, owner, |context| {
            graphcal_compiler::hir::lower_expr_draft(expr, context)
        })
    }

    /// Run one HIR lowering in the scope of `owner`.
    fn lower_in_owner<T>(
        &self,
        site: ValueSite<'_>,
        owner: &graphcal_compiler::dag_id::DagId,
        lower: impl FnOnce(ExprLoweringContext<'_>) -> Result<T, graphcal_compiler::hir::ExprLowerError>,
    ) -> Result<T, CompileError> {
        let scope = GenericScope::new();
        let context = ExprLoweringContext::new(
            ModuleScope::new(owner, &self.module_resolver, &scope),
            &self.tir().registry().time_zones,
        );
        lower(context).map_err(|error| {
            site.render(graphcal_compiler::hir::expr_lower_error_to_semantic(
                &error,
                site.source,
            ))
        })
    }

    fn binding_internal_error(&self, message: &str, span: Span) -> CompileError {
        CompileError::semantic(
            SemanticError::internal_error(
                message.to_string(),
                self.source,
                graphcal_compiler::diagnostic_anchor::DiagnosticAnchor::Source(span),
            ),
            &self.sources,
        )
    }

    fn evaluate_closed_binding(
        &self,
        site: ValueSite<'_>,
        tree: &graphcal_compiler::tir::typed::ScopedTree<'_, graphcal_compiler::tir::texpr::TExpr>,
    ) -> Result<graphcal_eval::runtime_presentation::EvaluatedRuntimeValue, CompileError> {
        let values = RuntimeValueMap::new();
        graphcal_compiler::outcome::without_cancellation(|cancellation| {
            let session = EvalSession::checked(
                self.plan(),
                site.source,
                site.sources,
                &self.host_fns,
                cancellation.clone(),
            );
            graphcal_eval::eval_expr::eval_root_with_presentation(
                tree,
                &values,
                &graphcal_eval::runtime_presentation::PendingPresentedMap::new(),
                &session,
            )
        })
        .map_err(|error| site.render(error))
    }

    pub(super) fn binding_kind_error(
        &self,
        port: &ParameterPort,
        actual: BindingValueKind,
    ) -> CompileError {
        self.port_error(port, |name, src, span| BindingError::KindMismatch {
            name,
            actual,
            expected: port
                .declared_type
                .spelling(&self.tir().registry().dimensions),
            src,
            span,
        })
    }

    /// A binding error located at the declaration of `port`.
    pub(super) fn port_error(
        &self,
        port: &ParameterPort,
        error: impl FnOnce(DeclName, NamedSource<Arc<String>>, SourceSpan) -> BindingError,
    ) -> CompileError {
        CompileError::Binding(error(
            port.name.clone(),
            self.sources.renderable(self.source),
            port.span.into(),
        ))
    }
}

pub(super) fn build_parameter_ports(
    plan_id: u64,
    entry_interface: &CheckedEntryInterface,
    plan: &graphcal_eval::execution_plan::ExecPlan<'_>,
    sources: &graphcal_compiler::source_registry::SourceRegistry,
    schemas: &mut ModelSchemaGraphBuilder<'_>,
) -> Result<Vec<ParameterPort>, CompileError> {
    entry_interface
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let declared_type = parameter.declared_type().clone();
            let value_schema = schemas
                .value_schema(&declared_type)
                .map_err(|error| CompileError::semantic(error, sources))?;
            let runtime_key = parameter.runtime_key().clone();
            let domain = plan
                .domain_constraint(&runtime_key)
                .map(parameter_domain)
                .transpose()
                .map_err(|error| {
                    CompileError::semantic(
                        SemanticError::internal_error(
                            format!("domain of parameter `{}`: {error}", parameter.name()),
                            plan.root().scope().source(),
                            DiagnosticAnchor::Source(parameter.span()),
                        ),
                        sources,
                    )
                })?;
            Ok(ParameterPort {
                name: parameter.name().clone(),
                position: ParameterPosition { plan_id, index },
                declared_type,
                value_schema,
                domain,
                has_default: parameter.has_default(),
                runtime_key,
                span: parameter.span(),
            })
        })
        .collect()
}

fn normalize_binding_literal(
    mut expr: Expr,
    expected: &ModelValueSchema,
    schemas: &ModelSchemaGraph,
) -> Result<Expr, BindingLiteralError> {
    normalize_binding_literal_in_place(&mut expr, expected, schemas)?;
    Ok(expr)
}

fn map_entry_value_schema(
    expected: &ModelValueSchema,
    key_count: usize,
) -> Result<&ModelValueSchema, BindingLiteralError> {
    (0..key_count).try_fold(expected, |schema, _| match schema {
        ModelValueSchema::Indexed { element, .. } => Ok(element.as_ref()),
        _ => Err(BindingLiteralError::OverNestedMapEntry),
    })
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    reason = "the boundary checks exact f64/i64 range and round-trip identity before conversion"
)]
fn normalize_binding_literal_in_place(
    expr: &mut Expr,
    expected: &ModelValueSchema,
    schemas: &ModelSchemaGraph,
) -> Result<(), BindingLiteralError> {
    match (&mut expr.kind, expected) {
        (AstExprKind::Integer(value), ModelValueSchema::Quantity(quantity))
            if quantity.dimension().is_dimensionless() =>
        {
            const MAX_EXACT_INTEGER: u64 = 1_u64 << f64::MANTISSA_DIGITS;
            if value.unsigned_abs() > MAX_EXACT_INTEGER {
                return Err(BindingLiteralError::InexactRealInteger);
            }
            expr.kind = AstExprKind::Number(*value as f64);
        }
        (AstExprKind::Number(value), ModelValueSchema::Int) => {
            const I64_EXCLUSIVE_UPPER: f64 = 9_223_372_036_854_775_808.0;
            if !value.is_finite()
                || value.fract() != 0.0
                || *value < -I64_EXCLUSIVE_UPPER
                || *value >= I64_EXCLUSIVE_UPPER
            {
                return Err(BindingLiteralError::InexactInt);
            }
            let integer = *value as i64;
            if integer as f64 != *value {
                return Err(BindingLiteralError::InexactInt);
            }
            expr.kind = AstExprKind::Integer(integer);
        }
        (AstExprKind::UnaryOp { operand, .. }, _) => {
            normalize_binding_literal_in_place(operand, expected, schemas)?;
        }
        (AstExprKind::FnCall { args, .. }, ModelValueSchema::Complex(quantity)) => {
            let component = ModelValueSchema::Quantity(quantity.clone());
            for argument in args {
                normalize_binding_literal_in_place(argument, &component, schemas)?;
            }
        }
        (
            AstExprKind::ConstructorCall { callee, fields, .. },
            ModelValueSchema::Algebraic(type_id),
        ) => {
            if let Some(constructor) = schemas.definition(type_id).and_then(|definition| {
                definition
                    .constructors()
                    .iter()
                    .find(|constructor| constructor.name().atom() == callee.leaf().name.atom())
            }) {
                for field in fields {
                    if let Some(field_schema) = constructor
                        .fields()
                        .iter()
                        .find(|schema| schema.name() == &field.name.value)
                    {
                        normalize_binding_literal_in_place(
                            &mut field.value,
                            field_schema.value(),
                            schemas,
                        )?;
                    }
                }
            }
        }
        (AstExprKind::MapLiteral { entries }, ModelValueSchema::Indexed { .. }) => {
            for entry in entries {
                let entry_schema = map_entry_value_schema(expected, entry.keys.len())?;
                normalize_binding_literal_in_place(&mut entry.value, entry_schema, schemas)?;
            }
        }
        _ => {}
    }
    Ok(())
}
