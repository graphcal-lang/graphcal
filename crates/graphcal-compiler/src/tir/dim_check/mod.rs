use crate::outcome::Outcome;
use crate::resolved_name::ResolvedDeclName;
use crate::semantic_error::attribute::AttributeError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::domain::DomainError;
use crate::semantic_error::graph::GraphError;
use crate::source_id::SourceId;
use std::collections::{HashMap, HashSet};

use crate::assertion_expectation::{ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::span::Span;

use crate::display::formatting_registry::FormattingRegistry;
use crate::semantic_error::SemanticError;

pub(crate) use helpers::{expect_quantity, format_checked_type};

use domain_bound_type::{
    ExpectedBound, check_one_bound_with_display_name, expected_bound_from_resolved,
};
use helpers::is_bool_type;

pub mod body_specialization;
mod builtins;
mod concrete_obligations;
mod domain_bound_type;
mod expression_axes;
mod generic_substitution;
mod helpers;
#[expect(
    clippy::too_many_lines,
    reason = "large match on ExprKind variants is inherently long"
)]
mod infer;
mod instance_bodies;
mod model_schema;
mod plot;
mod presentation;
mod schedules;
mod template_closure;

pub use model_schema::{
    ConcreteModelConstructor, ConcreteModelField, ConcreteModelType, ConcreteModelTypeError,
    ValidatedModelType,
};
#[cfg(test)]
mod tests;

pub use crate::semantic::checked_type::CheckedType;
pub use crate::tir::typed::override_dependencies::{
    NominalOverrideIdentity, OverrideDependencySummary,
};

/// Per-DAG context bundle threaded through the dimension-check passes.
///
/// Bundles the read-only inference environment with the operation-scoped
/// cancellation token and observation sink, so individual helpers take a
/// single `&DimCheckContext` instead of positional arguments.
#[derive(Clone, Copy)]
struct DimCheckContext<'a> {
    env: infer::hir::InferEnv<'a>,
    /// The unchecked project being checked, from which derived checking views
    /// (such as a template with rigid dimension ports) are built.
    assembly: &'a crate::tir::typed::UncheckedTir,
    cancellation: &'a crate::cancellation::CancellationToken,
    observations: &'a infer::hir::BodyObservations,
}

impl DimCheckContext<'_> {
    fn checkpoint(&self) -> Result<(), crate::cancellation::Cancelled> {
        self.cancellation.checkpoint()
    }

    /// Infer the type of a checked root, recording its observations in this
    /// context's sink.
    fn infer_hir(
        &self,
        expr: &crate::hir::expr::Expr,
        owner: Option<&ResolvedDeclName>,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        self.env
            .infer_root(expr, owner, self.cancellation, self.observations)
    }
}

fn validate_declared_shape(
    ctx: &DimCheckContext<'_>,
    annotation: &crate::tir::typed::CheckedTypeAnnotation,
) -> Result<(), SemanticError> {
    expression_axes::check_materializable(
        &annotation.checked().declared().to_symbolic(),
        ctx.env.tir,
        ctx.env.src,
        annotation.span,
    )
}

/// Check that a declaration's expression type matches its declared type
/// annotation.
///
/// An unfinished node (`todo`) has no expression: its explicit declaration
/// type is the entire contract, so it is not checked here.
fn check_decl_expr_type(
    ctx: &DimCheckContext<'_>,
    name: &DeclName,
    identity: &ResolvedDeclName,
    annotation: &crate::tir::typed::CheckedTypeAnnotation,
    hir_expr: &crate::hir::expr::Expr,
) -> Result<(), Outcome<SemanticError>> {
    let type_ann_span = &annotation.span;
    let declared = annotation.checked().declared();
    if ctx
        .env
        .dag
        .semantic_instances()
        .iter()
        .flat_map(|instance| &instance.output_projections)
        .any(|projection| {
            crate::ir::instance::InstanceProjection::exposure(projection).selected() == Some(name)
        })
    {
        // Projection bodies are generated from the already checked instance
        // interface. Retain that proof rather than treating them as unchecked.
        return ctx
            .observations
            .record(
                hir_expr,
                &declared.to_symbolic(),
                ctx.env.dag,
                ctx.env.tir,
                ctx.env.src,
            )
            .map_err(Outcome::Failed);
    }
    let inferred = ctx.infer_hir(hir_expr, Some(identity))?;
    if declared.to_symbolic() != inferred {
        return Err(SemanticError::located(
            ctx.env.src,
            *type_ann_span,
            DimensionError::DimensionMismatchInAnnotation {
                declared: format_checked_type(declared, ctx.env.registry),
                inferred: format_checked_type(&inferred, ctx.env.registry),
            },
        )
        .into());
    }
    check_ineffective_conversions(hir_expr, true, ctx.env.src)?;
    Ok(())
}

/// Require every runtime unit factor to be one scalar Dimensionless quantity.
fn check_dynamic_unit_scale_types(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<SemanticError>> {
    ctx.env
        .dag
        .semantic
        .dynamic_unit_scales
        .values()
        .try_for_each(|entry| check_dynamic_unit_scale_type(ctx, entry))
}

fn check_dynamic_unit_scale_type(
    ctx: &DimCheckContext<'_>,
    entry: &crate::ir::model::DynamicUnitScaleEntry,
) -> Result<(), Outcome<SemanticError>> {
    ctx.checkpoint()?;
    if entry.declared_dimension != entry.base_unit_dimension {
        return Err(SemanticError::located(
            ctx.env.src,
            entry.span,
            DimensionError::UnitDefinitionDimensionMismatch {
                name: entry.spelling.leaf().clone(),
                declared: ctx
                    .env
                    .registry
                    .dimensions
                    .format_dimension(&entry.declared_dimension),
                definition: ctx
                    .env
                    .registry
                    .dimensions
                    .format_dimension(&entry.base_unit_dimension),
            },
        )
        .into());
    }
    let inferred = ctx.infer_hir(&entry.expr, None)?;
    if !matches!(
        &inferred,
        CheckedType::Quantity(dimension) if dimension.is_dimensionless()
    ) {
        return Err(SemanticError::located(
            ctx.env.src,
            entry.expr.span,
            DimensionError::DynamicUnitScaleTypeMismatch {
                name: entry.spelling.clone(),
                found: format_checked_type(&inferred, ctx.env.registry),
            },
        )
        .into());
    }
    Ok(())
}

/// Reject `->` conversions whose display effect is discarded (#648 B3).
///
/// A conversion only matters in a *display position*: the top level of a
/// declaration body, a selected `if`/`match` branch, a constructor field
/// initializer, a map-literal entry, a for-comprehension body, or a
/// `scan`/`unfold` init or body. Anywhere else — arithmetic operands, function
/// arguments, comparison operands, conditions, scrutinees, assertion bodies —
/// the conversion evaluates to the unchanged SI value and its display target
/// is silently dropped, so it is either a typo or dead code.
fn check_ineffective_conversions(
    expr: &crate::hir::expr::Expr,
    display_position: bool,
    src: SourceId,
) -> Result<(), SemanticError> {
    // Recursion choke point: recurses once per tree level.
    crate::stack::with_stack_growth(|| {
        check_ineffective_conversions_inner(expr, display_position, src)
    })
}

fn check_ineffective_conversions_inner(
    expr: &crate::hir::expr::Expr,
    display_position: bool,
    src: SourceId,
) -> Result<(), SemanticError> {
    use crate::hir::expr::ExprKind;
    match expr.kind() {
        ExprKind::Convert { expr: inner, .. } | ExprKind::DisplayTimezone { expr: inner, .. } => {
            if !display_position {
                return Err(SemanticError::located(
                    src,
                    expr.span,
                    DimensionError::IneffectiveConversion,
                ));
            }
            // The operand of a conversion is not itself a display position
            // (direct nesting is already rejected as D012).
            check_ineffective_conversions(inner, false, src)
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_ineffective_conversions(condition, false, src)?;
            check_ineffective_conversions(then_branch, display_position, src)?;
            check_ineffective_conversions(else_branch, display_position, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_ineffective_conversions(scrutinee, false, src)?;
            for arm in arms {
                check_ineffective_conversions(&arm.body, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::ConstructorCall { fields, .. } => {
            for init in fields {
                check_ineffective_conversions(&init.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                check_ineffective_conversions(&entry.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => {
            check_ineffective_conversions(body, display_position, src)
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_ineffective_conversions(source, display_position, src)?;
            check_ineffective_conversions(init, display_position, src)?;
            check_ineffective_conversions(body, display_position, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_ineffective_conversions(init, display_position, src)?;
            check_ineffective_conversions(body, display_position, src)
        }
        ExprKind::KeyForm { arg, .. } => check_ineffective_conversions(arg, false, src),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_ineffective_conversions(lhs, false, src)?;
            check_ineffective_conversions(rhs, false, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_ineffective_conversions(operand, false, src),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                check_ineffective_conversions(arg, false, src)?;
            }
            Ok(())
        }
        ExprKind::FieldAccess { expr: inner, .. } => {
            check_ineffective_conversions(inner, false, src)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            check_ineffective_conversions(inner, false, src)?;
            for arg in args {
                if let crate::hir::expr::IndexArg::Expr(e) = arg {
                    check_ineffective_conversions(e, false, src)?;
                }
            }
            Ok(())
        }
        // Inline-dag param bindings flow into the callee's params, whose
        // reads propagate display metadata; treat them as display positions.
        ExprKind::DagCall { args, .. } => {
            for binding in args {
                check_ineffective_conversions(&binding.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::Error(no_error) => no_error.absurd(),
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::VariantLiteral(_) => Ok(()),
    }
}

#[derive(Debug)]
struct AssertionIndexShape {
    axes: Vec<IndexTypeRef<Symbolic>>,
}

impl AssertionIndexShape {
    fn from_bool_type(ty: &CheckedType<Symbolic>) -> Self {
        Self {
            axes: peel_index_axes(ty).0,
        }
    }

    const fn is_indexed(&self) -> bool {
        !self.axes.is_empty()
    }

    const fn rank(&self) -> usize {
        self.axes.len()
    }
}

/// Check dimensions for a lowered HIR assertion body.
fn check_hir_assert_body(
    ctx: &DimCheckContext<'_>,
    owner: &ResolvedDeclName,
    body: &crate::hir::expr::AssertBody,
    span: crate::syntax::span::Span,
) -> Result<AssertionIndexShape, Outcome<SemanticError>> {
    let registry = ctx.env.registry;
    let src = ctx.env.src;
    match body {
        crate::hir::expr::AssertBody::Expr(body_expr) => {
            let inferred = ctx.infer_hir(body_expr, Some(owner))?;
            if !is_bool_type(&inferred) {
                return Err(SemanticError::located(
                    src,
                    span,
                    AttributeError::AssertBodyNotBool {
                        found: format_checked_type(&inferred, registry),
                    },
                )
                .into());
            }
            Ok(AssertionIndexShape::from_bool_type(&inferred))
        }
        crate::hir::expr::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => {
            let actual_type = ctx.infer_hir(actual, Some(owner))?;
            let expected_type = ctx.infer_hir(expected, Some(owner))?;
            let tolerance_type = ctx.infer_hir(tolerance, Some(owner))?;

            // Element-wise broadcasting (#809): the assertion's index shape
            // comes from `actual`; `expected` and `tolerance` are each unindexed
            // (broadcast to every key) or indexed by exactly the same axes.
            let (actual_axes, actual_elem) = peel_index_axes(&actual_type);
            let expected_elem = broadcast_operand_element(
                &actual_axes,
                &actual_type,
                &expected_type,
                expected.span,
                registry,
                src,
            )?;
            let tolerance_elem = broadcast_operand_element(
                &actual_axes,
                &actual_type,
                &tolerance_type,
                tolerance.span,
                registry,
                src,
            )?;

            let actual_dim = expect_quantity(actual_elem, registry, src, actual.span)?;
            let expected_dim = expect_quantity(expected_elem, registry, src, expected.span)?;
            if actual_dim != expected_dim {
                return Err(SemanticError::located(src, expected.span, DimensionError::DimensionMismatch { expected: registry.dimensions.format_dimension(&actual_dim), found: registry.dimensions.format_dimension(&expected_dim), help: "actual and expected in tolerance assertion must have the same dimension"
                        .to_string() })
                .into());
            }

            let tolerance_dim = expect_quantity(tolerance_elem, registry, src, tolerance.span)?;
            if tolerance_dim != actual_dim {
                return Err(SemanticError::located(
                    src,
                    tolerance.span,
                    DimensionError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(&actual_dim),
                        found: format_checked_type(&tolerance_type, registry),
                        help: "absolute tolerance must have the same dimension as actual/expected"
                            .to_string(),
                    },
                )
                .into());
            }

            // A sign-negative literal tolerance is rejected even when its
            // IEEE value is `-0.0`, preserving the source-level rule that
            // negative tolerance literals are invalid (#815, #1415).
            // Tolerances computed at runtime are validated by the evaluator.
            if let Some(value) = statically_known_tolerance(tolerance)
                && value.is_sign_negative()
            {
                let found = match value {
                    value if value == 0.0 && value.is_sign_negative() => "-0".to_string(),
                    value => crate::display::number::format_number(value),
                };
                return Err(SemanticError::located(
                    src,
                    tolerance.span,
                    AttributeError::NegativeTolerance { found },
                )
                .into());
            }
            Ok(AssertionIndexShape { axes: actual_axes })
        }
    }
}

/// Peel the index axes off an inferred type, outermost first.
fn peel_index_axes(
    ty: &CheckedType<Symbolic>,
) -> (Vec<IndexTypeRef<Symbolic>>, &CheckedType<Symbolic>) {
    let mut axes = Vec::new();
    let mut current = ty;
    while let CheckedType::Indexed { element, index } = current {
        axes.push(index.clone());
        current = element;
    }
    (axes, current)
}

/// Validate that a tolerance-assertion operand broadcasts against `actual`'s
/// axes (#809): it is either unindexed (applied to every key) or indexed by
/// exactly the same axes in the same order. Returns the operand's element
/// type.
fn broadcast_operand_element<'a>(
    actual_axes: &[IndexTypeRef<Symbolic>],
    actual_type: &CheckedType<Symbolic>,
    operand_type: &'a CheckedType<Symbolic>,
    operand_span: crate::syntax::span::Span,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<&'a CheckedType<Symbolic>, SemanticError> {
    let (operand_axes, operand_elem) = peel_index_axes(operand_type);
    if !operand_axes.is_empty() && operand_axes != *actual_axes {
        return Err(SemanticError::located(
            src,
            operand_span,
            DimensionError::IndexedShapeMismatch {
                context: "tolerance assertion".to_string(),
                lhs: format_checked_type(actual_type, registry),
                rhs: format_checked_type(operand_type, registry),
            },
        ));
    }
    Ok(operand_elem)
}

/// Structurally fold a tolerance expression to its written literal value
/// when the sign is statically known: a numeric literal (`0.1`, `5`,
/// `0.1 m` — unit scales are always positive, so the written value carries
/// the sign), optionally under unary negation. Returns `None` for anything
/// computed at runtime; those are sign-checked by the evaluator instead.
fn statically_known_tolerance(expr: &crate::hir::expr::Expr) -> Option<f64> {
    match expr.kind() {
        crate::hir::expr::ExprKind::Number(n) => Some(*n),
        #[expect(
            clippy::cast_precision_loss,
            reason = "tolerance literals are small integers"
        )]
        crate::hir::expr::ExprKind::Integer(i) => Some(*i as f64),
        crate::hir::expr::ExprKind::QuantityLiteral { value, .. } => Some(*value),
        crate::hir::expr::ExprKind::UnaryOp {
            op: crate::syntax::ast::UnaryOp::Neg,
            operand,
        } => statically_known_tolerance(operand).map(|v| -v),
        _ => None,
    }
}

fn expected_fail_key_span(key: &ExpectedFailKey) -> Span {
    let (first, rest) = key.split_first();
    rest.iter()
        .map(ExpectedFailKeyPart::span)
        .fold(first.span(), Span::merge)
}

fn expected_fail_key_signature(
    key: &ExpectedFailKey,
) -> Vec<(Option<IndexTypeRef>, IndexEntryKey)> {
    key.iter()
        .map(|part| (part.named_index().cloned(), part.entry_key()))
        .collect()
}

fn validate_expected_fail_key(
    key: &ExpectedFailKey,
    shape: &AssertionIndexShape,
    src: SourceId,
) -> Result<(), SemanticError> {
    if key.len() != shape.rank() {
        return Err(SemanticError::located(
            src,
            expected_fail_key_span(key),
            AttributeError::ExpectedFailKeyShapeMismatch {
                expected: shape.rank(),
                found: key.len(),
            },
        ));
    }

    for (part, expected_axis) in key.iter().zip(&shape.axes) {
        match part {
            ExpectedFailKeyPart::Named { index, .. } => {
                if !index.to_symbolic().matches_ref(expected_axis) {
                    return Err(SemanticError::located(
                        src,
                        part.span(),
                        AttributeError::ExpectedFailKeyIndexMismatch {
                            expected: expected_axis.display_name().to_string(),
                            found: part.display(),
                        },
                    ));
                }
            }
            ExpectedFailKeyPart::FinitePosition { position, span } => {
                let Some(finite) = expected_axis.finite_index_ref() else {
                    return Err(SemanticError::located(
                        src,
                        *span,
                        AttributeError::ExpectedFailKeyIndexMismatch {
                            expected: expected_axis.display_name().to_string(),
                            found: part.display(),
                        },
                    ));
                };
                // Bound-check `#N` against a statically known Fin cardinality.
                // Symbolic cardinalities are checked after substitution.
                if let Some(concrete) = finite.concrete_index()
                    && *position >= concrete.size_u64()
                {
                    return Err(SemanticError::located(
                        src,
                        *span,
                        AttributeError::ExpectedFailFinitePositionOutOfBounds {
                            position: *position,
                            size: concrete.size_u64(),
                        },
                    ));
                }
            }
        }
    }

    Ok(())
}

fn validate_expected_fail(
    expected_fail: &ExpectedFail,
    shape: &AssertionIndexShape,
    src: SourceId,
    attribute_span: crate::syntax::span::Span,
) -> Result<(), SemanticError> {
    match expected_fail {
        ExpectedFail::All if shape.is_indexed() => Err(SemanticError::located(
            src,
            attribute_span,
            AttributeError::ExpectedFailAllOnIndexed,
        )),
        ExpectedFail::All => Ok(()),
        ExpectedFail::Variants(keys) if !shape.is_indexed() => {
            let span = expected_fail_key_span(keys.first());
            Err(SemanticError::located(
                src,
                span,
                AttributeError::ExpectedFailNotIndexed,
            ))
        }
        ExpectedFail::Variants(keys) => {
            let mut seen = HashSet::new();
            for key in keys {
                validate_expected_fail_key(key, shape, src)?;
                if !seen.insert(expected_fail_key_signature(key)) {
                    return Err(SemanticError::located(
                        src,
                        expected_fail_key_span(key),
                        AttributeError::ExpectedFailDuplicateKey,
                    ));
                }
            }
            Ok(())
        }
    }
}

impl crate::tir::typed::InstantiatedTir {
    /// Check every local body and publish its facts: the only transition
    /// into a [`CheckedTir`](crate::tir::typed::CheckedTir).
    ///
    /// For each const/param/node, infers the dimension of the RHS expression
    /// and verifies it matches the checked type carried by its declaration
    /// record. Canonical bodies are inferred once; semantic instances
    /// specialize their template's checked trees.
    ///
    /// # Errors
    ///
    /// Returns a [`SemanticError`] for invalid dimensions or cancellation.
    pub fn check(
        self,
        src: SourceId,
        cancellation: &crate::cancellation::CancellationToken,
    ) -> Result<crate::tir::typed::CheckedTir, Outcome<SemanticError>> {
        let tir = self.tir;
        cancellation.checkpoint()?;
        let schedules = schedules::Schedules::build(&tir, src)?;
        detect_cross_dag_cycles(&tir, src)?;

        // Canonical bodies are checked once. Instance trees are specialized
        // below from the canonical trees; only independently lowered bindings
        // infer.
        let inferred = tir.dags.map_local(|dag| {
            if let Some(specialization) = dag.frame().specialization() {
                return Ok(Inferred::Instance(instance_bodies::InstanceOf {
                    dag,
                    specialization,
                }));
            }
            cancellation.checkpoint()?;
            let observations = infer::hir::BodyObservations::default();
            let plot_shapes = check_dimensions_dag(dag, &tir, src, cancellation, &observations)?;
            Ok::<_, Outcome<SemanticError>>(Inferred::Canonical {
                dag,
                observations: Box::new(observations),
                plot_shapes,
            })
        })?;
        let sinks: HashMap<_, _> = inferred
            .iter()
            .filter_map(|(owner, inferred)| match inferred {
                Inferred::Canonical { observations, .. } => Some((owner, &**observations)),
                Inferred::Instance(_) => None,
            })
            .collect();
        check_field_domain_constraint_targets(&tir)?;
        check_field_domain_constraint_dimensions(&tir, cancellation, &sinks)?;
        drop(sinks);
        let canonical = inferred.try_map(|dag_id, inferred| match inferred {
            Inferred::Canonical {
                dag,
                observations,
                plot_shapes,
            } => {
                let bodies = observations
                    .finish()
                    .publish(
                        &dag.owned_expression_roots().collect::<Vec<_>>(),
                        &|index| expression_axes::checked_index_cardinality(&tir, index),
                    )
                    .map_err(|error| {
                        SemanticError::internal_error(
                            format!("DAG `{dag_id}`: {error}"),
                            src,
                            DiagnosticAnchor::WholeFile,
                        )
                    })?;
                Ok::<_, SemanticError>(instance_bodies::CanonicalStage::Canonical {
                    bodies,
                    plot_shapes,
                })
            }
            Inferred::Instance(instance) => Ok(instance_bodies::CanonicalStage::Instance(instance)),
        })?;

        let (bodies, plots) =
            instance_bodies::local_bodies(&tir, &canonical, src, cancellation)?.unzip();
        drop(canonical);
        let checking = crate::tir::typed::local_dag_facts::CheckingTir {
            tir: &tir,
            bodies: &bodies,
        };
        concrete_obligations::validate_project(&checking, src, cancellation)?;

        // Field targets and dimensions were checked before publication;
        // specializing instance trees does not change their nominal
        // definitions.
        cancellation.checkpoint()?;
        let presentation =
            presentation::collect_presentation_facts(&tir, &plots, src, cancellation)?;
        drop(plots);
        let published = bodies.zip(presentation).zip(schedules.callables).map(
            |((bodies, presentation), runtime_schedule)| crate::tir::typed::PublishedDag {
                bodies,
                presentation,
                runtime_schedule,
            },
        );
        tir.into_checked(schedules.constants, published, src)
            .map_err(Outcome::Failed)
    }
}

/// What inference left to publish for one local body: a canonical body's
/// observations and plot shapes, or a semantic instance, whose trees are
/// specialized from its template's.
enum Inferred<'t> {
    Canonical {
        dag: &'t crate::tir::typed::DagTIR,
        observations: Box<infer::hir::BodyObservations>,
        plot_shapes: plot::CheckedPlotChannelShapes,
    },
    Instance(instance_bodies::InstanceOf<'t>),
}

/// Collect canonical nominal dependencies for every checked parameter default
/// in every DAG module in `tir`.
///
/// Each default is inferred exactly once before include substitution. Canonical
/// field, constructor, match, index, and type-argument events are collected by
/// that inference and filtered to the module's typed `pub(bind)` interface.
/// The resulting summary is reusable at every include site.
///
/// # Errors
///
/// Returns [`Cancelled`](crate::cancellation::Cancelled) when `cancellation`
/// is cancelled.
pub fn collect_override_dependency_summary(
    tir: &crate::tir::typed::CheckedTir,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<OverrideDependencySummary, crate::cancellation::Cancelled> {
    let mut summary = OverrideDependencySummary::new();

    for (_, dag) in tir.local_dags() {
        cancellation.checkpoint()?;
        let bodies = dag.bodies();
        for param in dag.params() {
            let Some(default) = &param.default else {
                continue;
            };
            cancellation.checkpoint()?;
            let owner = param.identity();
            // A checked body has a tree for each of its expression roots,
            // its parameter defaults included.
            let mut dependencies: HashSet<_> = bodies
                .nominal_uses(default.id())
                .iter()
                .filter_map(|observation| {
                    use crate::tir::texpr::NominalObservation;
                    match observation {
                        NominalObservation::Field { identity, .. }
                        | NominalObservation::Constructor { identity, .. }
                        | NominalObservation::TypeArgument(identity) => {
                            Some(NominalOverrideIdentity::Type(identity.clone()))
                        }
                        NominalObservation::IndexLabel { identity, .. }
                        | NominalObservation::IndexArgument(identity) => identity
                            .declared_resolved()
                            .cloned()
                            .map(NominalOverrideIdentity::Index),
                    }
                })
                .collect();
            dependencies.retain(|identity| is_bindable_nominal(dag.body(), identity));
            if !dependencies.is_empty() {
                summary.insert(owner, dependencies);
            }
        }
    }
    Ok(summary)
}

fn is_bindable_nominal(
    dag: &crate::tir::typed::DagTIR,
    identity: &NominalOverrideIdentity,
) -> bool {
    let bindable = match identity {
        NominalOverrideIdentity::Index(index) => {
            crate::tir::typed::BindableNominalIdentity::Index(index.clone())
        }
        NominalOverrideIdentity::Type(struct_type) => {
            crate::tir::typed::BindableNominalIdentity::Type(struct_type.clone())
        }
    };
    dag.semantic.bindable_nominals.contains(&bindable)
}

/// Check one already-lowered external value expression against a concrete
/// declared type in this TIR's root module, returning its tree in the root's
/// scope.
///
/// This is the compiler-facing half of runtime parameter binding: callers may
/// lower a closed value expression independently of declaration compilation,
/// then reuse the normal HIR inference rules rather than duplicating unit,
/// datetime, constructor, generic, key, or indexed type checking.
///
/// # Errors
///
/// Returns a [`SemanticError`] when the expression is not well typed in the
/// root module or does not exactly match `expected`, or when its tree is not
/// executable.
pub fn check_external_value_expr_type<'t>(
    tir: &'t crate::tir::typed::CheckedTir,
    expr: &crate::hir::closed_expr::ClosedExpr,
    expected: &CheckedType,
    src: SourceId,
) -> Result<crate::tir::typed::ScopedTree<'t, crate::tir::texpr::TExpr>, SemanticError> {
    check_callless_value_expr_type(tir, expr, expected, src)
}

/// [`check_external_value_expr_type`] for an expression that calls no DAG,
/// as a closed expression cannot: its tree runs in the root's scope, whose
/// call targets do not number it.
fn check_callless_value_expr_type<'t>(
    tir: &'t crate::tir::typed::CheckedTir,
    expr: &crate::hir::expr::Expr,
    expected: &CheckedType,
    src: SourceId,
) -> Result<crate::tir::typed::ScopedTree<'t, crate::tir::texpr::TExpr>, SemanticError> {
    let observations = infer::hir::BodyObservations::default();
    let inferred = crate::outcome::without_cancellation(|cancellation| {
        let inferred = infer::hir::InferEnv {
            dag: tir.root().body(),
            tir,
            registry: tir.registry(),
            src,
        }
        .infer_root(expr, None, cancellation, &observations)?;
        concrete_obligations::validate_concrete_type_obligations(
            &inferred,
            tir.root().body(),
            tir,
            src,
            expr.span,
            cancellation,
        )?;
        Ok(inferred)
    })?;
    if expected.to_symbolic() == inferred {
        observations
            .finish()
            .publish(&[expr], &|index| {
                expression_axes::checked_index_cardinality(tir, index)
            })
            .map_err(|error| error.to_string())
            .and_then(|bodies| {
                bodies
                    .executable_value(expr.id())
                    .cloned()
                    .map_err(|error| error.to_string())
            })
            .map(|tree| tir.external_value_tree(tree))
            .map_err(|message| {
                SemanticError::internal_error(message, src, DiagnosticAnchor::Source(expr.span))
            })
    } else {
        Err(SemanticError::located(
            src,
            expr.span,
            DimensionError::DimensionMismatchInAnnotation {
                declared: format_checked_type(expected, tir.registry()),
                inferred: format_checked_type(&inferred, tir.registry()),
            },
        ))
    }
}

fn check_param_defaults(ctx: &DimCheckContext<'_>) -> Result<(), Outcome<SemanticError>> {
    for entry in ctx.env.dag.params() {
        ctx.checkpoint()?;
        validate_declared_shape(ctx, &entry.type_ann)?;
        let Some(default) = &entry.default else {
            continue;
        };
        check_decl_expr_type(
            ctx,
            entry.name(),
            &entry.identity(),
            &entry.type_ann,
            default,
        )?;
    }
    Ok(())
}

/// Dim-check a single [`DagTIR`] against the file's shared registry and
/// the full flat dag map.
fn check_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::UncheckedTir,
    src: SourceId,
    cancellation: &crate::cancellation::CancellationToken,
    observations: &infer::hir::BodyObservations,
) -> Result<plot::CheckedPlotChannelShapes, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let ctx = DimCheckContext {
        env: infer::hir::InferEnv {
            dag,
            tir,
            registry: tir.registry(),
            src,
        },
        assembly: tir,
        cancellation,
        observations,
    };

    for entry in dag.consts() {
        ctx.checkpoint()?;
        validate_declared_shape(&ctx, &entry.type_ann)?;
        check_decl_expr_type(
            &ctx,
            entry.name(),
            &entry.identity(),
            &entry.type_ann,
            &entry.expr,
        )?;
    }
    for entry in dag.nodes() {
        ctx.checkpoint()?;
        validate_declared_shape(&ctx, &entry.type_ann)?;
        if let Some(formula) = entry.definition.formula() {
            check_decl_expr_type(
                &ctx,
                entry.name(),
                &entry.identity(),
                &entry.type_ann,
                formula,
            )?;
        }
    }
    check_param_defaults(&ctx)?;

    ctx.checkpoint()?;
    check_dynamic_unit_scale_types(&ctx)?;

    for entry in dag.asserts() {
        ctx.checkpoint()?;
        let owner = entry.identity();
        let body = &*entry.body;
        let shape = check_hir_assert_body(&ctx, &owner, body, entry.span)?;
        if let Some(metadata) = dag.expected_fail.get(&owner) {
            validate_expected_fail(&metadata.expected, &shape, src, metadata.attribute_span)?;
        }
        // Assertion results are never displayed with units, so no position
        // inside an assert body is display-effective.
        match body {
            crate::hir::expr::AssertBody::Expr(e) => {
                check_ineffective_conversions(e, false, src)?;
            }
            crate::hir::expr::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                check_ineffective_conversions(actual, false, src)?;
                check_ineffective_conversions(expected, false, src)?;
                check_ineffective_conversions(tolerance, false, src)?;
            }
        }
    }

    ctx.checkpoint()?;
    let plot_shapes = plot::check_plot_properties_dag(&ctx, dag)?;

    ctx.checkpoint()?;
    check_domain_constraint_targets_dag(dag, src)?;
    ctx.checkpoint()?;
    check_domain_constraint_dimensions_dag(&ctx)?;

    ctx.checkpoint()?;
    template_closure::check_template_body_closure(&ctx)?;

    Ok(plot_shapes)
}

/// Check that domain constraint bound expressions have the correct type.
///
/// For each param/node with `(min: ..., max: ...)` constraints whose target type
/// is a quantity, `Int`, or `Datetime<S>`, infers the type of each bound
/// expression using the regular type checker and verifies it matches:
/// - `Quantity(d)` target: bound must be `Quantity(d)` (or `Int` if `d` is dimensionless).
/// - `Dimensionless` target: bound must be `Quantity(dimensionless)` or `Int`.
/// - `Int` target: bound must be exactly `Int`.
/// - `Datetime<S>` target: bound must be exactly `Datetime<S>`.
///
/// Other targets (e.g., `Bool`) are rejected by
/// [`check_domain_constraint_targets_dag`] before this bound check runs.
fn check_domain_constraint_dimensions_dag(
    ctx: &DimCheckContext<'_>,
) -> Result<(), Outcome<SemanticError>> {
    let dag = ctx.env.dag;
    let decl_iter = dag
        .consts()
        .map(|e| (e.name(), e.identity(), &e.type_ann))
        .chain(dag.params().map(|e| (e.name(), e.identity(), &e.type_ann)))
        .chain(dag.nodes().map(|e| (e.name(), e.identity(), &e.type_ann)));

    for (name, key, annotation) in decl_iter {
        let bounds = dag.semantic.domain_bounds.get(&key);
        let Some(bounds) = bounds else {
            continue;
        };

        let Some(expected) =
            expected_bound_from_resolved(annotation.checked().resolved().element())
        else {
            continue;
        };

        for bound in bounds {
            let inferred = ctx.infer_hir(&bound.value, Some(&key))?;
            check_one_bound(
                name,
                bound,
                &inferred,
                &expected,
                ctx.env.registry,
                ctx.env.src,
            )?;
        }
    }

    Ok(())
}

fn check_one_bound(
    name: &DeclName,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &CheckedType<Symbolic>,
    expected: &ExpectedBound,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<(), SemanticError> {
    check_one_bound_with_display_name(&name.to_string(), bound, inferred, expected, registry, src)
}

/// Reject domain constraints on base types that don't accept them.
///
/// Bool, Label, and algebraic types cannot carry `(min: …, max: …)` bounds.
/// The check is a pure function of the resolved declaration type—independent
/// of any bound expression's value—so it belongs in compile-time validation
/// rather than runtime resolution.
fn check_domain_constraint_targets_dag(
    dag: &crate::tir::typed::DagTIR,
    src: SourceId,
) -> Result<(), SemanticError> {
    let decl_iter = dag
        .consts()
        .map(|entry| (entry.identity(), &entry.type_ann, entry.span))
        .chain(
            dag.params()
                .map(|entry| (entry.identity(), &entry.type_ann, entry.span)),
        )
        .chain(
            dag.nodes()
                .map(|entry| (entry.identity(), &entry.type_ann, entry.span)),
        );

    for (key, annotation, decl_span) in decl_iter {
        if !dag.semantic.domain_bounds.contains_key(&key) {
            continue;
        }
        if let Some(type_kind) = invalid_domain_target_kind(annotation.checked().resolved()) {
            return Err(SemanticError::located(
                src,
                decl_span,
                DomainError::InvalidDomainTarget { type_kind },
            ));
        }
    }
    Ok(())
}

fn invalid_domain_target_kind(resolved: &crate::tir::typed::ResolvedDeclType) -> Option<String> {
    use crate::tir::typed::ResolvedValueType;

    match resolved.element() {
        ResolvedValueType::Bool => Some("Bool".to_string()),
        ResolvedValueType::Complex { .. } => Some("Complex".to_string()),
        ResolvedValueType::Key { .. } => Some("Key".to_string()),
        ResolvedValueType::Struct {
            name: struct_name, ..
        } => Some(format!("struct `{}`", struct_name.as_str())),
        ResolvedValueType::GenericTypeParam(param, _) => {
            Some(format!("generic Type parameter `{param}`"))
        }
        ResolvedValueType::Quantity(_)
        | ResolvedValueType::Int
        | ResolvedValueType::Datetime(_) => None,
    }
}

/// Reject constraints on struct/union fields outside the explicitly
/// constrainable value families. This mirrors
/// [`check_domain_constraint_targets_dag`] using the field's resolved semantic
/// type rather than reclassifying source names.
fn check_field_domain_constraint_targets(
    tir: &crate::tir::typed::UncheckedTir,
) -> Result<(), SemanticError> {
    let mut seen = std::collections::HashSet::new();
    for (_, dag) in tir.local_dags() {
        for (key, field_semantics, bounds) in dag.semantic.type_defs.constrained_fields() {
            if !seen.insert(key) {
                continue;
            }
            let Some(type_kind) = invalid_domain_target_kind(field_semantics.resolved_type())
            else {
                continue;
            };
            let first_bound = bounds.first();
            let span = field_type_annotation(dag, key)
                .map_or(first_bound.span, |field| field.type_annotation().span);
            return Err(SemanticError::located(
                first_bound.src,
                span,
                DomainError::InvalidDomainTarget { type_kind },
            ));
        }
    }
    Ok(())
}

fn field_type_annotation<'a>(
    dag: &'a crate::tir::typed::DagTIR,
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
) -> Option<&'a crate::hir::nominal::NominalField> {
    dag.semantic
        .type_defs
        .struct_types
        .get(&key.owning_type)?
        .union_members()?
        .iter()
        .find(|member| member.name() == key.constructor)?
        .fields()
        .iter()
        .find(|field| field.name() == &key.field)
}

fn field_constraint_definition_dag<'a>(
    tir: &'a crate::tir::typed::UncheckedTir,
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
    src: SourceId,
    span: Span,
) -> Result<&'a crate::tir::typed::DagTIR, SemanticError> {
    tir.dags.get(key.owning_type.owner()).ok_or_else(|| {
        SemanticError::internal_error(
            format!(
                "field-constraint owner `{}` has no checked DAG",
                key.owning_type.owner()
            ),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
        )
    })
}

/// The nominal type, constructor, and field a constrained-field key names.
fn constrained_field_definition<'d>(
    dag: &'d crate::tir::typed::DagTIR,
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
    src: SourceId,
    span: Span,
) -> Result<
    (
        &'d crate::hir::nominal::NominalTypeDef,
        &'d crate::hir::nominal::NominalConstructor,
        &'d crate::hir::nominal::NominalField,
    ),
    SemanticError,
> {
    let type_def = dag
        .semantic
        .type_defs
        .struct_types
        .get(&key.owning_type)
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "semantic type metadata missing constrained type `{}`",
                    key.owning_type
                ),
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
            )
        })?;
    let (variant, field) = type_def
        .union_members()
        .and_then(|members| {
            members
                .iter()
                .flat_map(|member| member.fields().iter().map(move |field| (member, field)))
                .find(|(member, field)| {
                    member.name() == key.constructor && field.name() == &key.field
                })
        })
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "semantic type metadata missing constrained field `{}.{}`",
                    key.constructor, key.field
                ),
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
            )
        })?;
    Ok((type_def, variant, field))
}

/// Check that domain bound expressions on struct/union fields have the
/// correct type. Mirrors [`check_domain_constraint_dimensions_dag`] for
/// top-level decls.
///
/// Field bounds cross into HIR with their nominal definition; the same
/// owner-qualified field can be referenced
/// from several DAGs, so a seen-set dedupes the checks.
fn check_field_domain_constraint_dimensions(
    tir: &crate::tir::typed::UncheckedTir,
    cancellation: &crate::cancellation::CancellationToken,
    sinks: &HashMap<&crate::dag_id::DagId, &infer::hir::BodyObservations>,
) -> Result<(), Outcome<SemanticError>> {
    let registry = tir.registry();
    let mut seen = HashSet::new();
    for (_, dag) in tir.local_dags() {
        for (key, field_semantics, bounds) in dag.semantic.type_defs.constrained_fields() {
            if !seen.insert(key) {
                continue;
            }
            let diagnostic_bound = bounds.first();
            let diagnostic_src = &diagnostic_bound.src;
            let diagnostic_span = diagnostic_bound.span;
            let (type_def, variant, field) =
                constrained_field_definition(dag, key, *diagnostic_src, diagnostic_span)?;
            let resolved_target = field_semantics.resolved_type().element();
            let expected = expected_bound_from_resolved(resolved_target);
            let deferred_generic_quantity = matches!(
                resolved_target,
                crate::tir::typed::ResolvedValueType::Quantity(
                    crate::tir::typed::ResolvedDim::Symbolic { .. }
                )
            );
            if expected.is_none() && !deferred_generic_quantity {
                return Err(SemanticError::internal_error(
                    format!(
                        "constrained field target `{}` was not classified",
                        resolved_target.format(registry)
                    ),
                    *diagnostic_src,
                    crate::diagnostic_anchor::DiagnosticAnchor::Source(diagnostic_span),
                )
                .into());
            }
            // For a single-variant collision (record-shape) the display
            // name is `Type.field`; for a true multi-variant union it's
            // `Type#Variant.field` so diagnostics disambiguate which
            // constructor a violating bound belongs to.
            let display_name = if variant.name().as_str() == type_def.name().as_str() {
                format!("{}.{}", type_def.name(), field.name())
            } else {
                format!("{}.{}.{}", type_def.name(), variant.name(), field.name())
            };
            let definition_dag =
                field_constraint_definition_dag(tir, key, *diagnostic_src, diagnostic_span)?;
            let Some(observations) = sinks.get(definition_dag.dag_id()) else {
                // Imported definitions already carry their canonical proof.
                continue;
            };
            for bound in bounds {
                let inferred = infer::hir::InferEnv {
                    dag: definition_dag,
                    tir,
                    registry,
                    src: bound.src,
                }
                .infer_root(&bound.value, None, cancellation, observations)?;
                match &expected {
                    Some(expected) => check_one_bound_with_display_name(
                        &display_name,
                        bound,
                        &inferred,
                        expected,
                        registry,
                        bound.src,
                    )?,
                    None => check_deferred_generic_quantity_bound(
                        &display_name,
                        resolved_target,
                        bound,
                        &inferred,
                        registry,
                    )?,
                }
            }
        }
    }
    Ok(())
}

fn check_deferred_generic_quantity_bound(
    display_name: &str,
    resolved_target: &crate::tir::typed::ResolvedValueType,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &CheckedType<Symbolic>,
    registry: &FormattingRegistry,
) -> Result<(), SemanticError> {
    if inferred.quantity_dimension().is_some() || matches!(inferred, CheckedType::Int) {
        return Ok(());
    }
    Err(SemanticError::located(
        bound.src,
        bound.span,
        DomainError::DomainDimensionMismatch {
            name: display_name.to_string(),
            type_dim: resolved_target.format(registry),
            bound_name: bound.kind.to_string(),
            bound_dim: format_checked_type(inferred, registry),
        },
    ))
}

/// Collect DAG-call targets and the first source span for each call edge.
fn collect_dag_call_targets_from_dag(
    dag: &crate::tir::typed::DagTIR,
    out: &mut std::collections::BTreeMap<crate::dag_id::DagId, Span>,
) {
    dag.visit_expressions(&mut |expr| {
        if let crate::hir::expr::ExprKind::DagCall { target, .. } = expr.kind() {
            out.entry(target.value.clone()).or_insert(target.span);
        }
    });
}

/// Detect cycles in the cross-dag inline-call graph.
///
/// A dag `A` that transitively inline-calls itself — directly or through a
/// chain `A → B → … → A` — would recurse unboundedly at evaluation time. We
/// reject such programs at compile time with
/// [`GraphError::CyclicDependency`](GraphError::CyclicDependency) naming the dag at which the
/// dependency-graph search (dags and call targets in `DagId` order)
/// re-entered the cycle, spanning the call that re-entered it.
///
/// Per the issue thread, a dag — not a file — is the semantic unit of
/// cycle detection, so the same check applies whether the cycle is within
/// a single file or spans multiple files.
fn detect_cross_dag_cycles(
    tir: &crate::tir::typed::UncheckedTir,
    src: SourceId,
) -> Result<(), SemanticError> {
    use std::collections::BTreeMap;

    use crate::dag_id::DagId;

    let calls: BTreeMap<&DagId, BTreeMap<DagId, Span>> = tir
        .dags
        .iter()
        .map(|(dag_id, dag)| {
            let mut targets = BTreeMap::new();
            collect_dag_call_targets_from_dag(dag, &mut targets);
            (dag_id, targets)
        })
        .collect();
    let mut graph = crate::dependency_graph::DependencyGraph::new();
    for caller in calls.keys() {
        graph.add_node(*caller);
    }
    for (caller, targets) in &calls {
        for target in targets.keys().filter(|target| calls.contains_key(target)) {
            graph.add_dependency(*caller, target);
        }
    }
    let Err(cycle) = graph.into_topo_order() else {
        return Ok(());
    };
    let entry = *cycle.entry();
    // The last dag on the cycle path is the caller that re-entered the entry.
    let reentering_caller = cycle.path().last().copied().unwrap_or(entry);
    let span = calls
        .get(reentering_caller)
        .and_then(|targets| targets.get(entry))
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!("cycle entry `{entry}` has no incoming call span"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    Err(SemanticError::located(
        src,
        *span,
        GraphError::CyclicDependency {
            name: entry.to_string(),
        },
    ))
}
