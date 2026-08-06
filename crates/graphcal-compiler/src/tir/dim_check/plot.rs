//! Static validation of plot encodings and plot-family properties.
//!
//! Every encoding expression is inferred, reduced to a plottable leaf plus
//! canonical index axes, and checked against the same subset/broadcast shape
//! contract retained by runtime alignment. Property names are checked against
//! the typed registry in [`crate::plot_props`], and property values are
//! type-checked (string literal vs. dimensionless number vs. boolean).

use crate::hir::ExprKind;
use crate::ir::lower::LoweredPlotField;
use crate::plot_props::{CompositionProperty, MarkProperty, PlotProperty, PlotPropertyType};
use crate::plot_shape::{PlotChannelShape, PlotLeafKind, align_plot_channel_axes};
use crate::registry::error::GraphcalError;

use super::{
    DimCheckContext, InferredType, check_ineffective_conversions, helpers::format_inferred_type,
    infer,
};

/// Check every plot/figure/layer declaration of one DAG.
pub(super) fn check_plot_properties_dag(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
) -> Result<(), GraphcalError> {
    check_plot_references(ctx, dag)?;
    for entry in &dag.plots {
        let body = &entry.body;
        let entry_ctx = ctx.for_body(entry.body_src.resolve(ctx.src));
        check_plot_encodings(&entry_ctx, dag, body)?;
        for field in &body.mark_properties {
            let Some(prop) = MarkProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    &entry_ctx,
                    field,
                    "a mark block",
                    &valid_names(MarkProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(&entry_ctx, dag, prop.name(), prop.value_type(), field)?;
        }
        for field in &body.properties {
            let Some(prop) = PlotProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    &entry_ctx,
                    field,
                    "a plot declaration",
                    &valid_names(PlotProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(&entry_ctx, dag, prop.name(), prop.value_type(), field)?;
        }
    }
    for entry in &dag.figures {
        let entry_ctx = ctx.for_body(entry.body_src.resolve(ctx.src));
        for field in &entry.fields {
            let prop = CompositionProperty::from_name(field.name.as_str())
                .filter(|p| p.applies_to_figure());
            let Some(prop) = prop else {
                return Err(invalid_property(
                    &entry_ctx,
                    field,
                    "a figure declaration",
                    &format!(
                        "{}; figures render as side-by-side concatenation, so sizes belong on \
                         the constituent plots or layers",
                        valid_names(
                            CompositionProperty::ALL
                                .iter()
                                .filter(|p| p.applies_to_figure())
                                .map(|p| p.name()),
                        )
                    ),
                ));
            };
            check_property_value(&entry_ctx, dag, prop.name(), prop.value_type(), field)?;
        }
    }
    for entry in &dag.layers {
        let entry_ctx = ctx.for_body(entry.body_src.resolve(ctx.src));
        for field in &entry.fields {
            let Some(prop) = CompositionProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    &entry_ctx,
                    field,
                    "a layer declaration",
                    &valid_names(CompositionProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(&entry_ctx, dag, prop.name(), prop.value_type(), field)?;
        }
    }
    Ok(())
}

/// Validate the `plots:` lists of figure/layer declarations (#843):
/// every entry must name a plot declared in this DAG (a figure/layer name
/// gets a targeted "is not a plot" error), and no entry may repeat.
fn check_plot_references(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
) -> Result<(), GraphcalError> {
    let owners = dag
        .figures
        .iter()
        .map(|f| ("figure", &f.name, &f.plot_names, &f.body_src))
        .chain(
            dag.layers
                .iter()
                .map(|l| ("layer", &l.name, &l.plot_names, &l.body_src)),
        );
    for (owner_kind, owner, plot_names, body_src) in owners {
        let owner_ctx = ctx.for_body(body_src.resolve(ctx.src));
        for (i, reference) in plot_names.iter().enumerate() {
            let is_known_plot = dag.plots.iter().any(|p| p.name == reference.value)
                || dag.included_plots.iter().any(|p| p.name == reference.value);
            if !is_known_plot {
                let actual_kind = if dag.figures.iter().any(|f| f.name == reference.value) {
                    Some("figure")
                } else if dag.layers.iter().any(|l| l.name == reference.value) {
                    Some("layer")
                } else {
                    None
                };
                return Err(actual_kind.map_or_else(
                    || GraphcalError::UnknownPlotReference {
                        owner_kind,
                        owner: owner.clone(),
                        name: reference.value.clone(),
                        src: owner_ctx.src.clone(),
                        span: reference.span.into(),
                    },
                    |actual_kind| GraphcalError::CompositionReferencesNonPlot {
                        owner_kind,
                        actual_kind,
                        name: reference.value.clone(),
                        src: owner_ctx.src.clone(),
                        span: reference.span.into(),
                    },
                ));
            }
            if plot_names[..i].iter().any(|p| p.value == reference.value) {
                return Err(GraphcalError::DuplicatePlotReference {
                    owner_kind,
                    owner: owner.clone(),
                    name: reference.value.clone(),
                    src: owner_ctx.src.clone(),
                    span: reference.span.into(),
                });
            }
        }
    }
    Ok(())
}

fn check_plot_encodings(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
    body: &crate::ir::lower::LoweredPlotBody,
) -> Result<(), GraphcalError> {
    let shapes = body
        .encodings
        .iter()
        .map(|(channel, expr)| {
            ctx.checkpoint()?;
            check_ineffective_conversions(expr, true, ctx.src)?;
            if matches!(&expr.kind, ExprKind::StringLiteral(_)) {
                return Ok(PlotChannelShape::new(
                    Vec::new(),
                    PlotLeafKind::ContextualString,
                ));
            }
            let inferred = infer_expression_type(ctx, dag, expr)?;
            plot_channel_shape(&inferred).ok_or_else(|| GraphcalError::PlotEncodingTypeMismatch {
                channel: *channel,
                found: format_inferred_type(&inferred, ctx.registry),
                src: ctx.src.clone(),
                span: expr.span.into(),
            })
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let axes = shapes
        .iter()
        .map(PlotChannelShape::axes)
        .collect::<Vec<_>>();
    if let Err(error) = align_plot_channel_axes(&axes) {
        let (_, expr) = &body.encodings[error.channel()];
        return Err(GraphcalError::PlotEncodingAxisMismatch {
            channels: describe_channel_axes(body, &shapes),
            src: ctx.src.clone(),
            span: expr.span.into(),
        });
    }
    Ok(())
}

fn plot_channel_shape(inferred: &InferredType) -> Option<PlotChannelShape> {
    let mut axes = Vec::new();
    let leaf = plot_leaf_kind(inferred, &mut axes)?;
    Some(PlotChannelShape::new(axes, leaf))
}

fn plot_leaf_kind(
    inferred: &InferredType,
    axes: &mut Vec<crate::registry::declared_type::IndexTypeRef>,
) -> Option<PlotLeafKind> {
    match inferred {
        InferredType::Indexed { element, index } => {
            axes.push(index.type_ref().clone());
            crate::stack::with_stack_growth(|| plot_leaf_kind(element, axes))
        }
        InferredType::Quantity(dimension) => Some(PlotLeafKind::Quantity(dimension.clone())),
        InferredType::Bool => Some(PlotLeafKind::Bool),
        InferredType::Int => Some(PlotLeafKind::Int),
        InferredType::Datetime(scale) => Some(PlotLeafKind::Datetime(*scale)),
        InferredType::Key(index) => Some(PlotLeafKind::Key(index.type_ref().clone())),
        InferredType::Complex(_) | InferredType::IndexArg(_) | InferredType::Struct(_, _) => None,
    }
}

fn describe_channel_axes(
    body: &crate::ir::lower::LoweredPlotBody,
    shapes: &[PlotChannelShape],
) -> String {
    body.encodings
        .iter()
        .zip(shapes)
        .map(|((channel, _), shape)| {
            let axes = if shape.axes().is_empty() {
                "no index".to_string()
            } else {
                shape
                    .axes()
                    .iter()
                    .map(|index| {
                        index
                            .declared_resolved()
                            .map_or_else(|| index.display_name().to_string(), ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join(" × ")
            };
            format!("`{channel}` ranges over {axes}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn valid_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    format!(
        "valid properties are: {}",
        names.collect::<Vec<_>>().join(", ")
    )
}

fn invalid_property(
    ctx: &DimCheckContext<'_>,
    field: &LoweredPlotField,
    context: &'static str,
    valid: &str,
) -> GraphcalError {
    GraphcalError::InvalidPlotProperty {
        property: field.name.as_str().to_string(),
        context,
        valid: valid.to_string(),
        src: ctx.src.clone(),
        span: field.name_span.into(),
    }
}

/// Check one property value against its expected type.
fn check_property_value(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
    property: &'static str,
    expected: PlotPropertyType,
    field: &LoweredPlotField,
) -> Result<(), GraphcalError> {
    let is_string_literal = matches!(&field.value.kind, ExprKind::StringLiteral(_));
    let mismatch = |found: String| GraphcalError::PlotPropertyTypeMismatch {
        property,
        expected: expected.describe(),
        found,
        src: ctx.src.clone(),
        span: field.value.span.into(),
    };

    match expected {
        PlotPropertyType::String => {
            if is_string_literal {
                Ok(())
            } else {
                // No expression other than a literal can produce a string —
                // graphcal has no runtime string values.
                Err(mismatch("not a string literal".to_string()))
            }
        }
        PlotPropertyType::Number | PlotPropertyType::PositiveNumber => {
            if is_string_literal {
                return Err(mismatch("a string literal".to_string()));
            }
            match infer_expression_type(ctx, dag, &field.value)? {
                InferredType::Int => Ok(()),
                InferredType::Quantity(d) if d.is_dimensionless() => Ok(()),
                InferredType::Quantity(d) => Err(GraphcalError::PlotPropertyDimensioned {
                    property,
                    dimension: ctx.registry.dimensions.format_dimension(&d),
                    src: ctx.src.clone(),
                    span: field.value.span.into(),
                }),
                other => Err(mismatch(format_inferred_type(&other, ctx.registry))),
            }
        }
        PlotPropertyType::Bool => {
            if is_string_literal {
                return Err(mismatch("a string literal".to_string()));
            }
            match infer_expression_type(ctx, dag, &field.value)? {
                InferredType::Bool => Ok(()),
                other => Err(mismatch(format_inferred_type(&other, ctx.registry))),
            }
        }
    }
}

fn infer_expression_type(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
    expr: &crate::hir::Expr,
) -> Result<InferredType, GraphcalError> {
    infer::hir::infer_hir_type_with_owner_and_cancellation(
        expr,
        None,
        ctx.declared_types,
        dag,
        ctx.tir,
        ctx.registry,
        ctx.builtin_fns,
        ctx.src,
        ctx.cancellation,
    )
}
