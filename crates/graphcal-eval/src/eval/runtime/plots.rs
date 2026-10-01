//! The plot stage of runtime output assembly: every plot the root DAG
//! displays or composes, then its figures and layers.
//!
//! Plots are identified by declaration identity. A figure or layer names its
//! plots in the root's namespace; each reference is resolved once, to the
//! identity of the root plot or of the instance plot an include site
//! requests under that name, and composition failures are keyed by it.

use std::collections::HashMap;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::plot_shape::PlotLeafKind;
use graphcal_compiler::plot_visibility::PlotVisibility;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::checked_type::{CheckedType, Symbolic};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::{Span, Spanned};
use graphcal_compiler::tir::typed::{DeclarationBody, ResolvedProjection, Scoped};

use crate::eval::plot_unavailable::{ComposedPlotsUnavailable, PlotUnavailable};
use crate::eval::public_projection;
use crate::eval::types::{
    AxisMeta, CompositionProperty, FigureSpec, LayerSpec, NodeUnavailable, PlotError,
    PlotFieldValue, PlotPropertyType, PlotSpec,
};
use crate::eval_expr::{
    EvalSession, RuntimeValue, RuntimeValueMap, eval_root, eval_root_with_presentation,
};
use crate::execution_frame::eval_failed_node_error;
use crate::presentation_evidence::{
    LeafPresentationDiagnostic, PresentationDiagnostic, PresentationFailure,
};
use crate::runtime_presentation::PendingPresentedMap;
use crate::runtime_presentation::PresentedRef;
use crate::runtime_value::KeyElement;

use super::declaration_body::declaration_body;
use super::dependency_failures::dependency_failure_message;
use super::evaluated_root::EvaluatedRoot;

/// Everything the plot stage reports.
pub(super) struct PlotOutputs {
    pub plots: Vec<PlotSpec>,
    pub plot_errors: Vec<PlotError>,
    pub figures: Vec<FigureSpec>,
    pub layers: Vec<LayerSpec>,
}

/// One plot the root DAG displays or composes: a plot the root declares, or
/// an instance plot an include site requests under a local alias.
struct RootPlot<'p> {
    /// The plot declaration, owned by the DAG that runs it.
    identity: ResolvedDeclName,
    /// The name the plot has in the root's namespace.
    name: &'p DeclName,
    /// Whether it renders standalone: the declaration's own visibility for a
    /// root plot, the include item's for a requested one.
    visibility: PlotVisibility,
}

/// The plots of the root DAG, in output order: its own plots in source
/// order, then the plots each semantic instance's include site requests.
fn root_plots<'p>(plan: &'p crate::execution_plan::ExecPlan<'p>) -> Vec<RootPlot<'p>> {
    let own = plan
        .root()
        .scope()
        .dag()
        .declarations()
        .filter_map(|entry| match entry.kind() {
            graphcal_compiler::tir::typed::declaration_view::DeclarationKind::Plot {
                visibility,
            } => Some(RootPlot {
                identity: entry.identity().clone(),
                name: entry.name(),
                visibility,
            }),
            _ => None,
        });
    let requested = plan.root().semantic_instances().iter().flat_map(|planned| {
        planned
            .instance()
            .plot_projections()
            .map(|ResolvedProjection { target, projection }| RootPlot {
                identity: target,
                name: &projection.alias,
                visibility: projection.visibility,
            })
    });
    own.chain(requested).collect()
}

/// The root plots that could not be rendered, by their name in the root's
/// namespace, with the reason.
type UnavailablePlots<'p> = HashMap<&'p DeclName, NodeUnavailable>;

/// Evaluate every root plot, figure and layer.
///
/// Evaluation is per-plot best-effort, but a plot that cannot be rendered is
/// reported, never silently dropped (#842); a figure or layer over an
/// unavailable plot, or with a failing field, is reported the same way
/// (#845).
pub(super) fn evaluate_root_plots(
    plan: &crate::execution_plan::ExecPlan<'_>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<PlotOutputs, Outcome<GraphcalError>> {
    let tir = plan.tir();
    let root_plots = root_plots(plan);
    let mut plots = Vec::new();
    let mut plot_errors = Vec::new();
    let mut unavailable = UnavailablePlots::new();
    for plot in &root_plots {
        // The plot runs in the DAG that owns it, which may be an instance
        // nested in the requesting one when a template forwards its own plot.
        let unit = declaration_body(tir, &plot.identity, ctx.src)?;
        let entry = unit.plot().ok_or_else(|| {
            ctx.internal_error(
                format!("plot `{}` has no checked body", plot.identity),
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let name = ScopedName::local(plot.name.clone());
        match evaluate_plot(unit, entry, evaluated, ctx) {
            Ok(evaluated) => plots.push(evaluated.into_spec(name, plot.visibility)),
            Err(PlotEvaluationError::Unavailable(reason)) => {
                unavailable.insert(plot.name, reason.clone());
                plot_errors.push(PlotError {
                    name,
                    reason: reason.into(),
                });
            }
            Err(PlotEvaluationError::Fatal(error)) => return Err(error),
        }
    }
    ctx.cancellation.checkpoint()?;

    let mut compositions = Compositions {
        unavailable: &unavailable,
        evaluated,
        ctx,
        plot_errors: &mut plot_errors,
    };
    let figures = tir
        .root()
        .declarations()
        .filter_map(|entry| match entry.kind() {
            graphcal_compiler::tir::typed::declaration_view::DeclarationKind::Figure {
                plot_names,
            } => Some((entry, plot_names)),
            _ => None,
        })
        .map(|(entry, plot_names)| {
            let owner = entry.identity().clone();
            let fields = declaration_body(tir, &owner, ctx.src)?
                .figure()
                .map(|figure| figure.map(|figure| figure.fields.as_slice()));
            Ok(compositions
                .compose(&owner, entry.name(), fields, plot_names)?
                .map(|composed| FigureSpec {
                    name: ScopedName::local(entry.name().clone()),
                    plot_names: composed.plot_names,
                    properties: composed.properties,
                }))
        })
        .collect::<Result<Vec<_>, Outcome<GraphcalError>>>()?
        .into_iter()
        .flatten()
        .collect();
    let layers = tir
        .root()
        .declarations()
        .filter_map(|entry| match entry.kind() {
            graphcal_compiler::tir::typed::declaration_view::DeclarationKind::Layer {
                plot_names,
            } => Some((entry, plot_names)),
            _ => None,
        })
        .map(|(entry, plot_names)| {
            let owner = entry.identity().clone();
            let fields = declaration_body(tir, &owner, ctx.src)?
                .layer()
                .map(|layer| layer.map(|layer| layer.fields.as_slice()));
            Ok(compositions
                .compose(&owner, entry.name(), fields, plot_names)?
                .map(|composed| LayerSpec {
                    name: ScopedName::local(entry.name().clone()),
                    plot_names: composed.plot_names,
                    properties: composed.properties,
                }))
        })
        .collect::<Result<Vec<_>, Outcome<GraphcalError>>>()?
        .into_iter()
        .flatten()
        .collect();

    Ok(PlotOutputs {
        plots,
        plot_errors,
        figures,
        layers,
    })
}

/// Figures and layers of the root, evaluated after its plots.
struct Compositions<'a, 'p> {
    unavailable: &'a UnavailablePlots<'p>,
    evaluated: EvaluatedRoot<'a>,
    ctx: &'a EvalSession<'a>,
    plot_errors: &'a mut Vec<PlotError>,
}

impl Compositions<'_, '_> {
    /// Evaluate the figure or layer `owner`, named `name` in the root, over
    /// its checked `fields`; an unavailable one is reported as a plot error.
    fn compose(
        &mut self,
        owner: &ResolvedDeclName,
        name: &DeclName,
        fields: Option<Scoped<'_, [graphcal_compiler::tir::typed::LoweredPlotField]>>,
        references: &[Spanned<ScopedName>],
    ) -> Result<Option<CompositionFields>, Outcome<GraphcalError>> {
        let fields = fields.ok_or_else(|| {
            self.ctx.internal_error(
                format!("composition `{owner}` has no checked body"),
                DiagnosticAnchor::WholeFile,
            )
        })?;
        let reason = match composed_plots_unavailable(references, self.unavailable) {
            Some(reason) => PlotUnavailable::from(reason),
            None => {
                match eval_composition_fields(fields, references, self.evaluated.values, self.ctx) {
                    Ok(composed) => return Ok(Some(composed)),
                    Err(PlotEvaluationError::Fatal(error)) => return Err(error),
                    Err(PlotEvaluationError::Unavailable(reason)) => PlotUnavailable::from(reason),
                }
            }
        };
        self.plot_errors.push(PlotError {
            name: ScopedName::local(name.clone()),
            reason,
        });
        Ok(None)
    }
}

/// Evaluate one plot property expression to a `PlotFieldValue`. String
/// literals are passed through directly (Graphcal has no runtime String
/// value); any other expression is evaluated and converted from a
/// `RuntimeValue`. An evaluation failure aborts the whole plot; the error
/// message is reported on the plot (#842).
fn eval_plot_property(
    expr: Scoped<'_, graphcal_compiler::hir::expr::Expr>,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<PlotFieldValue, PlotEvaluationError> {
    ctx.cancellation
        .checkpoint()
        .map_err(Outcome::<GraphcalError>::from)?;
    if let graphcal_compiler::hir::expr::ExprKind::StringLiteral(_) = expr.get().kind() {
        let text = ctx
            .checked_string(expr)
            .map_err(PlotEvaluationError::from)?;
        return Ok(PlotFieldValue::String(text.to_owned()));
    }
    ctx.executable(expr)
        .map_err(Outcome::Failed)
        .and_then(|tree| eval_root(&tree, values, ctx))
        .map_err(PlotEvaluationError::from)
        .and_then(|rv| runtime_to_plot_field_value(&rv).map_err(PlotEvaluationError::from))
}

#[derive(Debug, thiserror::Error)]
enum PlotEvaluationError {
    #[error("{0}")]
    Unavailable(NodeUnavailable),
    /// Cancellation or a violated invariant, which aborts the whole run.
    #[error("plot evaluation aborted")]
    Fatal(Outcome<GraphcalError>),
}

impl PlotEvaluationError {
    /// An error that aborts the whole run instead of the plot.
    const fn fatal(error: GraphcalError) -> Self {
        Self::Fatal(Outcome::Failed(error))
    }
}

impl From<Outcome<GraphcalError>> for PlotEvaluationError {
    fn from(error: Outcome<GraphcalError>) -> Self {
        match error {
            error @ (Outcome::Cancelled | Outcome::Failed(GraphcalError::Internal(_))) => {
                Self::Fatal(error)
            }
            Outcome::Failed(error) => Self::Unavailable(eval_failed_node_error(&error)),
        }
    }
}

impl From<GraphcalError> for PlotEvaluationError {
    fn from(error: GraphcalError) -> Self {
        Self::from(Outcome::Failed(error))
    }
}

impl PlotEvaluationError {
    fn with_property(self, property: &graphcal_compiler::ir::model::LoweredPlotProperty) -> Self {
        match self {
            Self::Unavailable(NodeUnavailable::EvalFailed { message }) => {
                Self::from(match property {
                    graphcal_compiler::ir::model::LoweredPlotProperty::Mark(_) => {
                        format!("mark property `{}`: {message}", property.name())
                    }
                    property => format!("property `{}`: {message}", property.name()),
                })
            }
            other => other,
        }
    }
}

impl From<String> for PlotEvaluationError {
    fn from(message: String) -> Self {
        Self::Unavailable(NodeUnavailable::EvalFailed { message })
    }
}

/// A plot evaluated in the scope of the DAG that owns it, before the root
/// names it.
struct EvaluatedPlot {
    mark_type: graphcal_compiler::desugar::desugared_ast::MarkType,
    encodings: Vec<(
        graphcal_compiler::syntax::ast::EncodingChannel,
        PlotFieldValue,
    )>,
    encoding_meta: Vec<(graphcal_compiler::syntax::ast::EncodingChannel, AxisMeta)>,
    presentation_diagnostics: Vec<PresentationDiagnostic>,
    mark_properties: Vec<(graphcal_compiler::plot_props::MarkProperty, PlotFieldValue)>,
    properties: Vec<(graphcal_compiler::plot_props::PlotProperty, PlotFieldValue)>,
}

impl EvaluatedPlot {
    /// The plot as the root reports it, under `name`.
    fn into_spec(self, name: ScopedName, visibility: PlotVisibility) -> PlotSpec {
        PlotSpec {
            name,
            mark_type: self.mark_type,
            encodings: self.encodings,
            encoding_meta: self.encoding_meta,
            presentation_diagnostics: self.presentation_diagnostics,
            mark_properties: self.mark_properties,
            properties: self.properties,
            visibility,
        }
    }
}

/// Evaluate a plot declaration.
///
/// The authoritative TIR plot record carries both its lowered HIR body and
/// mark metadata. String literals are handled directly (they are
/// not runtime values in Graphcal).
///
/// Ordinary expression/display failures remain attached to this plot, while
/// cancellation and structural checked/runtime invariant failures abort the
/// enclosing evaluation.
fn evaluate_plot(
    unit: DeclarationBody<'_>,
    entry: Scoped<'_, graphcal_compiler::tir::typed::TypedPlotEntry>,
    evaluated: EvaluatedRoot<'_>,
    ctx: &EvalSession<'_>,
) -> Result<EvaluatedPlot, PlotEvaluationError> {
    let EvaluatedRoot {
        values,
        errors,
        frame_presentations,
        ..
    } = evaluated;
    // A reference to a failed declaration must report the root cause, not a
    // generic lookup failure on the missing value.
    let lowered = entry.map(|entry| &entry.body);
    let encodings = lowered
        .map(|lowered| lowered.encodings.as_slice())
        .iter()
        .map(|encoding| (encoding.get().0, encoding.map(|(_, expr)| &**expr)))
        .collect::<Vec<_>>();
    let mark_fields = lowered.map(|lowered| lowered.mark_properties.as_slice());
    let plot_fields = lowered.map(|lowered| lowered.properties.as_slice());
    let body_exprs = encodings
        .iter()
        .map(|(_, expr)| *expr)
        .chain(
            mark_fields
                .iter()
                .map(|field| field.map(|field| &*field.value)),
        )
        .chain(
            plot_fields
                .iter()
                .map(|field| field.map(|field| &*field.value)),
        )
        .collect::<Vec<_>>();
    check_plot_expression_dependencies(&body_exprs, errors, ctx)?;

    let owner = unit.identity();
    let channel_facts = unit.plot_channel_presentations().ok_or_else(|| {
        PlotEvaluationError::fatal(ctx.internal_error(
            format!("checked presentation facts are missing for plot `{owner}`"),
            DiagnosticAnchor::WholeFile,
        ))
    })?;
    let mut encoding_meta = Vec::new();
    let mut presentation_diagnostics = Vec::new();

    // Evaluate channels and apply their checked structured presentation before
    // row alignment. Numeric projection and axis labels consume the same fact.
    let mut channel_data = Vec::new();
    for (channel, expr) in encodings {
        let fact = channel_facts.get(&channel).ok_or_else(|| {
            PlotEvaluationError::fatal(ctx.internal_error(
                format!("checked presentation is missing channel `{channel}`"),
                expr.get().span,
            ))
        })?;
        let (data, unit_label, diagnostics) =
            evaluate_plot_channel(channel, expr, fact, values, frame_presentations, ctx)?;

        presentation_diagnostics.extend(diagnostics.into_iter().map(|detail| {
            PresentationDiagnostic {
                declaration: owner.clone(),
                channel: Some(channel),
                detail,
            }
        }));
        let dimension_label = match fact.leaf() {
            PlotLeafKind::Quantity(dimension) if !dimension.is_dimensionless() => {
                Some(ctx.registry.dimensions.format_dimension(dimension))
            }
            _ => None,
        };
        encoding_meta.push((
            channel,
            AxisMeta {
                dimension_label,
                unit_label,
            },
        ));
        channel_data.push((channel, data));
    }
    let encodings = crate::eval::plot_data::align_encoding_channels(&channel_data)?;

    // Evaluate mark properties (e.g., stroke_width, opacity). Unknown names
    // are rejected at check time (#845); one that still reaches evaluation
    // is an internal inconsistency.
    let mark_properties = evaluate_mark_properties(mark_fields, values, ctx)?;

    // Evaluate top-level properties (e.g., title, width, height)
    let mut properties = Vec::new();
    for scoped_field in plot_fields.iter() {
        let field = scoped_field.get();
        let graphcal_compiler::ir::model::LoweredPlotProperty::Plot(plot_prop) = &field.property
        else {
            return Err(PlotEvaluationError::fatal(ctx.internal_error(
                format!(
                    "checked plot property has incompatible classification `{}`",
                    field.property.name()
                ),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
            .map_err(|error| error.with_property(&field.property))?;
        check_positive_property(plot_prop.name(), plot_prop.value_type(), &field_value)?;
        properties.push((*plot_prop, field_value));
    }

    Ok(EvaluatedPlot {
        mark_type: entry.get().mark_type,
        encodings,
        encoding_meta,
        presentation_diagnostics,
        mark_properties,
        properties,
    })
}

fn evaluate_mark_properties(
    fields: Scoped<'_, [graphcal_compiler::ir::model::LoweredPlotField]>,
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<Vec<(graphcal_compiler::plot_props::MarkProperty, PlotFieldValue)>, PlotEvaluationError>
{
    fields
        .iter()
        .map(|scoped_field| {
            let field = scoped_field.get();
            let graphcal_compiler::ir::model::LoweredPlotProperty::Mark(mark_prop) =
                &field.property
            else {
                return Err(PlotEvaluationError::fatal(ctx.internal_error(
                    format!(
                        "checked mark property has incompatible classification `{}`",
                        field.property.name()
                    ),
                    field.value.span,
                )));
            };
            let value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
                .map_err(|error| error.with_property(&field.property))?;
            Ok((*mark_prop, value))
        })
        .collect()
}

fn evaluate_plot_channel(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    scoped_expr: Scoped<'_, graphcal_compiler::hir::expr::Expr>,
    fact: &graphcal_compiler::plot_shape::PlotChannelShape,
    values: &RuntimeValueMap,
    presentation_values: &PendingPresentedMap,
    ctx: &EvalSession<'_>,
) -> Result<
    (
        crate::eval::plot_data::ChannelData,
        Option<String>,
        Vec<LeafPresentationDiagnostic>,
    ),
    PlotEvaluationError,
> {
    let expr = scoped_expr.get();
    if let graphcal_compiler::hir::expr::ExprKind::StringLiteral(value) = expr.kind() {
        return Ok((
            crate::eval::plot_data::ChannelData::unindexed_label(value.clone()),
            None,
            Vec::new(),
        ));
    }
    let evaluated = ctx
        .executable(scoped_expr)
        .map_err(Outcome::Failed)
        .and_then(|tree| eval_root_with_presentation(&tree, values, presentation_values, ctx))
        .map_err(|error| classify_plot_channel_error(channel, error))?;
    let presented = crate::eval_expr::resolve_presentation(evaluated, values, ctx)
        .map_err(|error| classify_plot_channel_error(channel, error))?;
    let declared_type =
        plot_declared_type(fact, ctx, expr.span).map_err(PlotEvaluationError::fatal)?;
    let project = |value| {
        public_projection::project(value, &declared_type)
            .map_err(|invariant| PlotEvaluationError::fatal(invariant.into_internal_error(ctx.src)))
    };
    let (displayed, mut diagnostics) = project(presented.as_ref())?;
    // A numeric channel must use one scale: it is displayed only when every
    // leaf displays and all share one unit. Otherwise it keeps its SI
    // projection whole, rather than mixing converted leaves with SI leaves.
    let (shown, unit_label) = match crate::eval::plot_data::uniform_quantity_unit_label(&displayed)
    {
        Ok(label) if diagnostics.is_empty() => (displayed, label),
        label => {
            if let Err(error) = label {
                diagnostics.push(LeafPresentationDiagnostic {
                    path: Vec::new(),
                    failure: PresentationFailure::Projection { message: error },
                });
            }
            let (projected, _) = project(PresentedRef::plain(&presented.value()))?;
            (projected, None)
        }
    };
    let data =
        crate::eval::plot_data::channel_data_from_presented_value(&presented.value(), &shown)
            .map_err(|error| format!("encoding channel `{channel}`: {error}"))?;
    Ok((data, unit_label, diagnostics))
}

fn classify_plot_channel_error(
    channel: graphcal_compiler::syntax::ast::EncodingChannel,
    error: Outcome<GraphcalError>,
) -> PlotEvaluationError {
    match PlotEvaluationError::from(error) {
        PlotEvaluationError::Unavailable(NodeUnavailable::EvalFailed { message }) => {
            PlotEvaluationError::from(format!("encoding channel `{channel}`: {message}"))
        }
        other => other,
    }
}

/// Convert the retained checked plot shape into the public projection type.
fn plot_declared_type(
    shape: &graphcal_compiler::plot_shape::PlotChannelShape,
    ctx: &EvalSession<'_>,
    span: Span,
) -> Result<CheckedType, GraphcalError> {
    // A contextual string channel has no runtime value type, and a symbolic
    // `Fin(N)` axis belongs to a template body, never to an executed plot.
    let leaf: Option<CheckedType<Symbolic>> = match shape.leaf() {
        PlotLeafKind::Quantity(dimension) => Some(CheckedType::Quantity(dimension.clone())),
        PlotLeafKind::Int => Some(CheckedType::Int),
        PlotLeafKind::Bool => Some(CheckedType::Bool),
        PlotLeafKind::Datetime(scale) => Some(CheckedType::Datetime(*scale)),
        PlotLeafKind::Key(index) => Some(CheckedType::Key(index.clone())),
        PlotLeafKind::ContextualString => None,
    };
    leaf.and_then(|leaf| {
        shape
            .axes()
            .iter()
            .rev()
            .fold(leaf, |element, index| CheckedType::Indexed {
                element: Box::new(element),
                index: index.clone(),
            })
            .to_concrete()
    })
    .ok_or_else(|| {
        ctx.internal_error(
            "plot channel shape has no concrete runtime value type",
            span,
        )
    })
}

fn check_plot_expression_dependencies(
    expressions: &[Scoped<'_, graphcal_compiler::hir::expr::Expr>],
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
    ctx: &EvalSession<'_>,
) -> Result<(), PlotEvaluationError> {
    if let Some(reason) = ctx.unavailable_dependencies(expressions.iter().copied())?
        && reason.is_incomplete()
    {
        return Err(PlotEvaluationError::Unavailable(reason));
    }
    dependency_failure_message(expressions.iter().copied(), errors)
        .map_or(Ok(()), |message| Err(PlotEvaluationError::from(message)))
}

/// Evaluated fields of a figure/layer declaration.
#[derive(Debug)]
struct CompositionFields {
    properties: Vec<(CompositionProperty, PlotFieldValue)>,
    plot_names: Vec<ScopedName>,
}

/// The plots a figure or layer composes that could not be rendered, by the
/// names the root gives them.
///
/// `unavailable` holds the root plots that could not be rendered, by their
/// name in the root's namespace.
fn composed_plots_unavailable(
    references: &[Spanned<ScopedName>],
    unavailable: &UnavailablePlots<'_>,
) -> Option<ComposedPlotsUnavailable> {
    ComposedPlotsUnavailable::blocked_by(references.iter().filter_map(|reference| {
        reference
            .value
            .as_bare()
            .and_then(|name| unavailable.get_key_value(name))
            .map(|(name, reason)| (*name, reason))
    }))
}

/// Evaluate composition fields (properties and plot names) shared by figures and layers.
fn eval_composition_fields(
    fields: Scoped<'_, [graphcal_compiler::tir::typed::LoweredPlotField]>,
    plot_name_spans: &[Spanned<ScopedName>],
    values: &RuntimeValueMap,
    ctx: &EvalSession<'_>,
) -> Result<CompositionFields, PlotEvaluationError> {
    if let Some(reason) =
        ctx.unavailable_dependencies(fields.iter().map(|field| field.map(|field| &*field.value)))?
    {
        return Err(PlotEvaluationError::Unavailable(reason));
    }
    let mut properties = Vec::new();
    for scoped_field in fields.iter() {
        let field = scoped_field.get();
        let graphcal_compiler::ir::model::LoweredPlotProperty::Composition(comp_prop) =
            &field.property
        else {
            return Err(PlotEvaluationError::fatal(ctx.internal_error(
                format!(
                    "checked composition property has incompatible classification `{}`",
                    field.property.name()
                ),
                field.value.span,
            )));
        };
        let field_value = eval_plot_property(scoped_field.map(|field| &*field.value), values, ctx)
            .map_err(|error| error.with_property(&field.property))?;
        check_positive_property(comp_prop.name(), comp_prop.value_type(), &field_value)?;
        properties.push((*comp_prop, field_value));
    }
    let plot_names = plot_name_spans.iter().map(|p| p.value.clone()).collect();
    Ok(CompositionFields {
        properties,
        plot_names,
    })
}

/// Enforce strictly positive values for `PositiveNumber` properties
/// (`width`, `height`) — value-dependent, so checked at evaluation time
/// (#845).
fn check_positive_property(
    property: &'static str,
    value_type: PlotPropertyType,
    value: &PlotFieldValue,
) -> Result<(), String> {
    if value_type != PlotPropertyType::PositiveNumber {
        return Ok(());
    }
    match value {
        PlotFieldValue::Number(n) if n.is_finite() && *n > 0.0 => Ok(()),
        PlotFieldValue::Number(n) => Err(format!(
            "property `{property}` must be a positive number, got {n}"
        )),
        _ => Err(format!("property `{property}` must be a positive number")),
    }
}

/// Convert a `RuntimeValue` to a `PlotFieldValue`.
///
/// A value that cannot be represented in a plot (a struct, or an indexed
/// value mixing kinds) is an error — never silently replaced by a
/// placeholder or by index variant names (#840).
fn runtime_to_plot_field_value(rv: &RuntimeValue) -> Result<PlotFieldValue, String> {
    match rv {
        RuntimeValue::Quantity(v) => Ok(PlotFieldValue::Number(v.get())),
        RuntimeValue::Int(i) => crate::eval_expr::numeric::exact_i64_to_f64(*i)
            .map(PlotFieldValue::Number)
            .map_err(|_| {
                format!(
                    "Int value {i} cannot be plotted exactly; convert it explicitly with to_float()"
                )
            }),
        RuntimeValue::Bool(b) => Ok(PlotFieldValue::String(b.to_string())),
        RuntimeValue::Key(key) => match key.element() {
            KeyElement::Named(variant) => Ok(PlotFieldValue::String(variant.to_string())),
            KeyElement::Coordinate { value, .. } => Ok(PlotFieldValue::Number(value.get())),
            KeyElement::Finite(position) => {
                crate::eval::plot_data::fin_position_number(position).map(PlotFieldValue::Number)
            }
        },
        RuntimeValue::Indexed(_) => crate::eval::plot_data::flatten_to_field_value(rv),
        // The checker admits no complex or struct plot channel.
        RuntimeValue::Complex(_) | RuntimeValue::Struct(_) => {
            Err(format!("{} cannot be plotted", rv.describe()))
        }
        RuntimeValue::Datetime(epoch) => crate::eval::types::epoch_to_rfc3339(epoch)
            .map(PlotFieldValue::Datetime)
            .map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests;
