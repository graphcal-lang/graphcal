//! Static validation of plot encodings and plot-family properties.
//!
//! Every encoding expression is inferred, reduced to a plottable leaf plus
//! canonical index axes, and checked against the same subset/broadcast shape
//! contract retained by runtime alignment. Property names were classified
//! against the typed registry in [`crate::plot_props`] when the declaration
//! was lowered; property values are type-checked here (string literal vs.
//! dimensionless number vs. boolean).

use crate::outcome::Outcome;
use crate::semantic::checked_type::Symbolic;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension::{PlotChannelAxes, PlotPropertyValue};
use crate::semantic_error::name::NameError;
use std::collections::HashMap;

use crate::hir::expr::ExprKind;
use crate::ir::model::LoweredPlotField;
use crate::plot_props::{BlockProperty, PlotPropertyType};
use crate::plot_shape::{PlotChannelShape, PlotLeafKind, align_plot_channel_axes};
use crate::semantic_error::SemanticError;

use super::{CheckedType, DimCheckContext, check_ineffective_conversions};

pub(super) type CheckedPlotChannelShapes = HashMap<
    crate::resolved_name::ResolvedDeclName,
    HashMap<crate::syntax::ast::EncodingChannel, PlotChannelShape>,
>;

/// Check every plot/figure/layer declaration of one DAG and retain each
/// channel's already-inferred quantity dimension.
pub(super) fn check_plot_properties_dag(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
) -> Result<CheckedPlotChannelShapes, Outcome<SemanticError>> {
    check_plot_references(ctx, dag)?;
    let mut channel_types = HashMap::new();
    for entry in dag.plots() {
        let (owner, types) = check_plot_entry(ctx, entry)?;
        channel_types.insert(owner, types);
    }
    dag.figures()
        .try_for_each(|entry| check_figure_entry(ctx, entry))?;
    dag.layers()
        .try_for_each(|entry| check_layer_entry(ctx, entry))?;
    Ok(channel_types)
}

pub(super) fn check_plot_entry(
    ctx: &DimCheckContext<'_>,
    entry: &crate::tir::typed::TypedPlotEntry,
) -> Result<
    (
        crate::resolved_name::ResolvedDeclName,
        HashMap<crate::syntax::ast::EncodingChannel, PlotChannelShape>,
    ),
    Outcome<SemanticError>,
> {
    let body = &entry.body;
    let owner = entry.identity();
    let types = check_plot_encodings(ctx, &owner, body)?;
    for field in &body.mark_properties {
        check_property_value(ctx, &owner, field)?;
    }
    for field in &body.properties {
        check_property_value(ctx, &owner, field)?;
    }
    Ok((owner, types))
}

pub(super) fn check_figure_entry(
    ctx: &DimCheckContext<'_>,
    entry: &crate::tir::typed::TypedFigureEntry,
) -> Result<(), Outcome<SemanticError>> {
    let owner = entry.identity();
    for field in &entry.fields {
        check_property_value(ctx, &owner, field)?;
    }
    Ok(())
}

pub(super) fn check_layer_entry(
    ctx: &DimCheckContext<'_>,
    entry: &crate::tir::typed::TypedLayerEntry,
) -> Result<(), Outcome<SemanticError>> {
    let owner = entry.identity();
    for field in &entry.fields {
        check_property_value(ctx, &owner, field)?;
    }
    Ok(())
}

/// Validate the `plots:` lists of figure/layer declarations (#843):
/// every entry must name a plot declared in this DAG (a figure/layer name
/// gets a targeted "is not a plot" error), and no entry may repeat.
fn check_plot_references(
    ctx: &DimCheckContext<'_>,
    dag: &crate::tir::typed::DagTIR,
) -> Result<(), SemanticError> {
    let owners = dag
        .figures()
        .map(|f| ("figure", f.name(), &f.plot_names))
        .chain(dag.layers().map(|l| ("layer", l.name(), &l.plot_names)));
    for (owner_kind, owner, plot_names) in owners {
        for (i, reference) in plot_names.iter().enumerate() {
            let local = reference.value.as_bare();
            let is_known_plot = dag.plots().any(|p| Some(p.name()) == local)
                || dag.included_plots.iter().any(|p| p.name == reference.value);
            if !is_known_plot {
                let actual_kind = if dag.figures().any(|f| Some(f.name()) == local) {
                    Some("figure")
                } else if dag.layers().any(|l| Some(l.name()) == local) {
                    Some("layer")
                } else {
                    None
                };
                return Err(actual_kind.map_or_else(
                    || {
                        SemanticError::located(
                            ctx.env.src,
                            reference.span,
                            NameError::UnknownPlotReference {
                                owner_kind,
                                owner: owner.clone(),
                                name: reference.value.clone(),
                            },
                        )
                    },
                    |actual_kind| {
                        SemanticError::located(
                            ctx.env.src,
                            reference.span,
                            NameError::CompositionReferencesNonPlot {
                                owner_kind,
                                actual_kind,
                                name: reference.value.clone(),
                            },
                        )
                    },
                ));
            }
            if plot_names[..i].iter().any(|p| p.value == reference.value) {
                return Err(SemanticError::located(
                    ctx.env.src,
                    reference.span,
                    NameError::DuplicatePlotReference {
                        owner_kind,
                        owner: owner.clone(),
                        name: reference.value.clone(),
                    },
                ));
            }
        }
    }
    Ok(())
}

fn check_plot_encodings(
    ctx: &DimCheckContext<'_>,
    owner: &crate::resolved_name::ResolvedDeclName,
    body: &crate::ir::model::LoweredPlotBody,
) -> Result<HashMap<crate::syntax::ast::EncodingChannel, PlotChannelShape>, Outcome<SemanticError>>
{
    let shapes = body
        .encodings
        .iter()
        .map(|(channel, expr)| {
            ctx.checkpoint()?;
            check_ineffective_conversions(expr, true, ctx.env.src)?;
            if matches!(expr.kind(), ExprKind::StringLiteral(_)) {
                ctx.observations.record_contextual(expr, ctx.env.src)?;
                return Ok(PlotChannelShape::new(
                    Vec::new(),
                    PlotLeafKind::ContextualString,
                ));
            }
            let inferred = infer_expression_type(ctx, owner, expr)?;
            plot_channel_shape(&inferred).ok_or_else(|| {
                SemanticError::located(
                    ctx.env.src,
                    expr.span,
                    DimensionError::PlotEncodingTypeMismatch {
                        channel: *channel,
                        found: inferred.spelling(&ctx.env.registry.dimensions),
                    },
                )
                .into()
            })
        })
        .collect::<Result<Vec<_>, Outcome<SemanticError>>>()?;
    let axes = shapes
        .iter()
        .map(PlotChannelShape::axes)
        .collect::<Vec<_>>();
    if let Err(error) = align_plot_channel_axes(&axes) {
        let (_, expr) = &body.encodings[error.channel()];
        return Err(SemanticError::located(
            ctx.env.src,
            expr.span,
            DimensionError::PlotEncodingAxisMismatch {
                channels: body
                    .encodings
                    .iter()
                    .zip(&shapes)
                    .map(|((channel, _), shape)| PlotChannelAxes {
                        channel: *channel,
                        axes: shape.axes().to_vec(),
                    })
                    .collect(),
            },
        )
        .into());
    }
    Ok(body
        .encodings
        .iter()
        .zip(shapes)
        .map(|((channel, _), shape)| (*channel, shape))
        .collect())
}

fn plot_channel_shape(inferred: &CheckedType<Symbolic>) -> Option<PlotChannelShape> {
    let mut axes = Vec::new();
    let leaf = plot_leaf_kind(inferred, &mut axes)?;
    Some(PlotChannelShape::new(axes, leaf))
}

fn plot_leaf_kind(
    inferred: &CheckedType<Symbolic>,
    axes: &mut Vec<crate::semantic::checked_type::IndexTypeRef<Symbolic>>,
) -> Option<PlotLeafKind> {
    match inferred {
        CheckedType::Indexed { element, index } => {
            axes.push(index.clone());
            crate::stack::with_stack_growth(|| plot_leaf_kind(element, axes))
        }
        CheckedType::Quantity(dimension) => Some(PlotLeafKind::Quantity(dimension.clone())),
        CheckedType::Bool => Some(PlotLeafKind::Bool),
        CheckedType::Int => Some(PlotLeafKind::Int),
        CheckedType::Datetime(scale) => Some(PlotLeafKind::Datetime(*scale)),
        CheckedType::Key(index) => Some(PlotLeafKind::Key(index.clone())),
        CheckedType::Complex(_) | CheckedType::Struct(_, _) => None,
    }
}

/// Check one property value against its expected type.
pub(super) fn check_property_value<P: BlockProperty>(
    ctx: &DimCheckContext<'_>,
    owner: &crate::resolved_name::ResolvedDeclName,
    field: &LoweredPlotField<P>,
) -> Result<(), Outcome<SemanticError>> {
    let property = field.property.name();
    let expected = field.property.value_type();
    let is_string_literal = matches!(field.value.kind(), ExprKind::StringLiteral(_));
    let mismatch = |found: PlotPropertyValue| {
        SemanticError::located(
            ctx.env.src,
            field.value.span,
            DimensionError::PlotPropertyTypeMismatch {
                property,
                expected: expected.describe(),
                found,
            },
        )
    };

    match expected {
        PlotPropertyType::String => {
            if is_string_literal {
                ctx.observations
                    .record_contextual(&field.value, ctx.env.src)
                    .map_err(Outcome::Failed)
            } else {
                // No expression other than a literal can produce a string —
                // graphcal has no runtime string values.
                Err(mismatch(PlotPropertyValue::NotStringLiteral).into())
            }
        }
        PlotPropertyType::Number | PlotPropertyType::PositiveNumber => {
            if is_string_literal {
                return Err(mismatch(PlotPropertyValue::StringLiteral).into());
            }
            match infer_expression_type(ctx, owner, &field.value)? {
                CheckedType::Int => Ok(()),
                CheckedType::Quantity(d) if d.is_dimensionless() => Ok(()),
                CheckedType::Quantity(d) => Err(SemanticError::located(
                    ctx.env.src,
                    field.value.span,
                    DimensionError::PlotPropertyDimensioned {
                        property,
                        dimension: ctx.env.registry.dimensions.dimension_spelling(&d),
                    },
                )
                .into()),
                other => Err(mismatch(PlotPropertyValue::Type(
                    other.spelling(&ctx.env.registry.dimensions),
                ))
                .into()),
            }
        }
        PlotPropertyType::Bool => {
            if is_string_literal {
                return Err(mismatch(PlotPropertyValue::StringLiteral).into());
            }
            match infer_expression_type(ctx, owner, &field.value)? {
                CheckedType::Bool => Ok(()),
                other => Err(mismatch(PlotPropertyValue::Type(
                    other.spelling(&ctx.env.registry.dimensions),
                ))
                .into()),
            }
        }
    }
}

fn infer_expression_type(
    ctx: &DimCheckContext<'_>,
    owner: &crate::resolved_name::ResolvedDeclName,
    expr: &crate::hir::expr::Expr,
) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
    ctx.infer_hir(expr, Some(owner))
}
