//! Static validation of plot, mark, and figure/layer properties (#845).
//!
//! Property names are checked against the typed registry in
//! [`crate::syntax::ast::plot_props`], and property values are type-checked
//! (string literal vs. dimensionless number vs. boolean). A typo'd or
//! wrongly-typed property is a check-time error, never a silently dropped
//! field; a dimensioned value is rejected instead of having its unit
//! silently stripped.

use crate::hir::ExprKind;
use crate::ir::lower::LoweredPlotField;
use crate::registry::error::GraphcalError;
use crate::syntax::ast::{CompositionProperty, MarkProperty, PlotProperty, PlotPropertyType};

use super::{DimCheckContext, InferredType, helpers::format_inferred_type, infer};

/// Check every plot/figure/layer declaration of one DAG.
pub(super) fn check_plot_properties_dag(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    let Some(dag) = ctx.dag else {
        return Ok(());
    };
    for entry in &dag.plots {
        let Some(body) = &entry.body else { continue };
        for field in &body.mark_properties {
            let Some(prop) = MarkProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    ctx,
                    field,
                    "a mark block",
                    &valid_names(MarkProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(ctx, prop.name(), prop.value_type(), field)?;
        }
        for field in &body.properties {
            let Some(prop) = PlotProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    ctx,
                    field,
                    "a plot declaration",
                    &valid_names(PlotProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(ctx, prop.name(), prop.value_type(), field)?;
        }
    }
    for entry in &dag.figures {
        for field in &entry.fields {
            let prop = CompositionProperty::from_name(field.name.as_str())
                .filter(|p| p.applies_to_figure());
            let Some(prop) = prop else {
                return Err(invalid_property(
                    ctx,
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
            check_property_value(ctx, prop.name(), prop.value_type(), field)?;
        }
    }
    for entry in &dag.layers {
        for field in &entry.fields {
            let Some(prop) = CompositionProperty::from_name(field.name.as_str()) else {
                return Err(invalid_property(
                    ctx,
                    field,
                    "a layer declaration",
                    &valid_names(CompositionProperty::ALL.iter().map(|p| p.name())),
                ));
            };
            check_property_value(ctx, prop.name(), prop.value_type(), field)?;
        }
    }
    Ok(())
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
            match infer_property_type(ctx, field)? {
                InferredType::Int => Ok(()),
                InferredType::Scalar(d) if d.is_dimensionless() => Ok(()),
                InferredType::Scalar(d) => Err(GraphcalError::PlotPropertyDimensioned {
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
            match infer_property_type(ctx, field)? {
                InferredType::Bool => Ok(()),
                other => Err(mismatch(format_inferred_type(&other, ctx.registry))),
            }
        }
    }
}

fn infer_property_type(
    ctx: &DimCheckContext<'_>,
    field: &LoweredPlotField,
) -> Result<InferredType, GraphcalError> {
    let Some(dag) = ctx.dag else {
        return Err(GraphcalError::InternalError {
            message: "plot property inference requires DAG context".to_string(),
            src: ctx.src.clone(),
            span: field.value.span.into(),
        });
    };
    infer::hir::infer_hir_type_with_owner(
        &field.value,
        None,
        ctx.declared_types,
        dag,
        ctx.tir,
        ctx.registry,
        ctx.builtin_fns,
        ctx.src,
    )
}
