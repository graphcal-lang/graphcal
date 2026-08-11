//! Checked presentation application, coordinate formatting, and display-unit rendering.

use std::collections::HashMap;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::format::{format_number, format_unit_terms_canonical};
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::presentation::{
    IndexedPresentation, LeafPresentation, PresentationCallKey, PresentationMatchPattern,
    PresentationProvenance, PresentationSelection, UnitPresentation,
};

use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValueMap, eval_hir_expr};

use super::types::{DisplayUnit, Value};

#[derive(Default)]
struct PresentationApplicationState {
    call_invocations: HashMap<PresentationCallKey, usize>,
}

impl PresentationApplicationState {
    fn next_call(&mut self, key: &PresentationCallKey) -> Result<usize, &'static str> {
        let invocation = self.call_invocations.entry(key.clone()).or_default();
        let current = *invocation;
        *invocation = invocation
            .checked_add(1)
            .ok_or("presentation-call invocation counter overflow")?;
        Ok(current)
    }
}

/// Apply checked, owner-qualified presentation facts to a public value.
///
/// # Errors
///
/// Returns an error when dynamic metadata cannot be evaluated or when checked
/// presentation structure does not match the evaluated value.
pub(super) fn attach_presentation(
    value: &mut Value,
    presentation: &PresentationProvenance,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<(), GraphcalError> {
    attach_presentation_with_locals(
        value,
        presentation,
        ctx,
        values,
        &HirLocalValueMap::root(),
        &mut PresentationApplicationState::default(),
    )
}

fn attach_presentation_with_locals(
    value: &mut Value,
    presentation: &PresentationProvenance,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
    locals: &HirLocalValueMap<'_>,
    state: &mut PresentationApplicationState,
) -> Result<(), GraphcalError> {
    match presentation {
        PresentationProvenance::None => Ok(()),
        PresentationProvenance::Leaf(leaf) => attach_leaf(value, leaf, ctx, values),
        PresentationProvenance::Struct {
            owning_type,
            fields: presentations,
        } => match value {
            Value::Struct { type_name, fields }
                if type_name.resolved() == owning_type.resolved() =>
            {
                presentations.iter().try_for_each(|(field, presentation)| {
                    fields.get_mut(field).map_or_else(
                        || {
                            Err(presentation_error(
                                ctx,
                                format!(
                                    "checked presentation field `{field}` is absent from runtime struct `{type_name}`"
                                ),
                                Span::new(0, 0),
                            ))
                        },
                        |field_value| {
                            attach_presentation_with_locals(
                                field_value,
                                presentation,
                                ctx,
                                values,
                                locals,
                                state,
                            )
                        },
                    )
                })
            }
            Value::Struct { type_name, .. } => Err(presentation_error(
                ctx,
                format!(
                    "checked presentation type `{owning_type}` does not match runtime struct `{type_name}`"
                ),
                Span::new(0, 0),
            )),
            _ => Err(presentation_error(
                ctx,
                "checked struct presentation was paired with a non-struct runtime value",
                Span::new(0, 0),
            )),
        },
        PresentationProvenance::Indexed { index, elements } => {
            attach_indexed(value, index, elements, ctx, values, locals, state)
        }
        PresentationProvenance::DagCall { key, output } => {
            let invocation = state
                .next_call(key)
                .map_err(|message| presentation_error(ctx, message, Span::new(0, 0)))?;
            let calls = ctx.presentation_calls.ok_or_else(|| {
                presentation_error(
                    ctx,
                    "checked DAG-call presentation has no evaluated call store",
                    Span::new(0, 0),
                )
            })?;
            let call_values = calls.invocation(key, invocation).map_err(|error| {
                presentation_error(
                    ctx,
                    format!("checked DAG-call presentation values are unavailable: {error}"),
                    Span::new(0, 0),
                )
            })?;
            attach_presentation_with_locals(
                value,
                output,
                ctx,
                &call_values,
                locals,
                state,
            )
        }
        PresentationProvenance::Select(selection) => {
            let selected = select_presentation(selection, ctx, values, locals)?;
            attach_presentation_with_locals(value, selected, ctx, values, locals, state)
        }
    }
}

fn attach_indexed(
    value: &mut Value,
    index: &graphcal_compiler::registry::declared_type::IndexTypeRef,
    elements: &IndexedPresentation,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
    locals: &HirLocalValueMap<'_>,
    state: &mut PresentationApplicationState,
) -> Result<(), GraphcalError> {
    match value {
        Value::Indexed {
            index_name,
            entries,
            ..
        } if index_name.matches_ref(index) => match elements {
            IndexedPresentation::Uniform { local, element } => {
                entries.iter_mut().try_for_each(|(key, entry)| match local {
                    Some(local) => {
                        let binding = presentation_index_binding(index, key, ctx)?;
                        let child = locals.child(vec![(*local, binding)]);
                        attach_presentation_with_locals(
                            entry, element, ctx, values, &child, state,
                        )
                    }
                    None => {
                        attach_presentation_with_locals(
                            entry, element, ctx, values, locals, state,
                        )
                    }
                })
            }
            IndexedPresentation::Entries(presentations) => presentations
                .iter()
                .try_for_each(|(key, presentation)| {
                    entries.get_mut(key).map_or_else(
                        || {
                            Err(presentation_error(
                                ctx,
                                format!(
                                    "checked presentation key `{key}` is absent from runtime indexed value"
                                ),
                                Span::new(0, 0),
                            ))
                        },
                        |entry| {
                            attach_presentation_with_locals(
                                entry,
                                presentation,
                                ctx,
                                values,
                                locals,
                                state,
                            )
                        },
                    )
                }),
        },
        Value::Indexed { index_name, .. } => Err(presentation_error(
            ctx,
            format!(
                "checked presentation index `{index}` does not match runtime index `{index_name}`"
            ),
            Span::new(0, 0),
        )),
        _ => Err(presentation_error(
            ctx,
            "checked indexed presentation was paired with a non-indexed runtime value",
            Span::new(0, 0),
        )),
    }
}

fn attach_leaf(
    value: &mut Value,
    leaf: &LeafPresentation,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<(), GraphcalError> {
    match (value, leaf) {
        (
            Value::Quantity {
                dimension,
                display_unit,
                ..
            }
            | Value::Complex {
                dimension,
                display_unit,
                ..
            },
            LeafPresentation::Unit(unit),
        ) if dimension == unit.dimension() => {
            *display_unit = Some(resolve_unit_to_display(unit, ctx, values)?);
            Ok(())
        }
        (Value::Indexed { entries, .. }, LeafPresentation::Unit(_))
        | (Value::Indexed { entries, .. }, LeafPresentation::Timezone(_)) => entries
            .values_mut()
            .try_for_each(|entry| attach_leaf(entry, leaf, ctx, values)),
        (Value::Datetime { display_tz, .. }, LeafPresentation::Timezone(timezone)) => {
            *display_tz = Some(timezone.clone());
            Ok(())
        }
        (
            Value::Quantity { dimension, .. } | Value::Complex { dimension, .. },
            LeafPresentation::Unit(unit),
        ) => Err(presentation_error(
            ctx,
            format!(
                "checked display dimension `{:?}` does not match runtime dimension `{dimension:?}`",
                unit.dimension()
            ),
            unit.unit().span,
        )),
        (_, LeafPresentation::Unit(unit)) => Err(presentation_error(
            ctx,
            "checked unit presentation was paired with a non-quantity runtime value",
            unit.unit().span,
        )),
        (_, LeafPresentation::Timezone(_)) => Err(presentation_error(
            ctx,
            "checked timezone presentation was paired with a non-datetime runtime value",
            Span::new(0, 0),
        )),
    }
}

fn presentation_index_binding(
    index: &graphcal_compiler::registry::declared_type::IndexTypeRef,
    key: &graphcal_compiler::syntax::index_name::IndexEntryKey,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::registry::types::IndexKind;
    use graphcal_compiler::syntax::index_name::IndexEntryKey;

    let definition = index.finite_index().map_or_else(
        || {
            index
                .declared_resolved()
                .and_then(|resolved| ctx.tir.declared_index_def(resolved))
        },
        |finite| ctx.registry.indexes.get_finite_index(finite),
    );
    let definition = definition.ok_or_else(|| {
        presentation_error(
            ctx,
            format!("checked presentation index `{index}` has no definition"),
            Span::new(0, 0),
        )
    })?;
    match (&definition.kind, key) {
        (IndexKind::Named { .. } | IndexKind::RequiredNamed, IndexEntryKey::Named(variant)) => {
            Ok(RuntimeValue::Label {
                index_name: index.clone(),
                variant: variant.clone(),
            })
        }
        (IndexKind::Coordinate(data), IndexEntryKey::Position(position)) => {
            let position = usize::try_from(*position).map_err(|_| {
                presentation_error(
                    ctx,
                    "coordinate presentation position exceeds the platform index range",
                    Span::new(0, 0),
                )
            })?;
            Ok(RuntimeValue::CoordinateLabel {
                index_name: index.clone(),
                position,
                value: data.coordinate_value(position),
            })
        }
        (IndexKind::Finite { .. }, IndexEntryKey::Position(position)) => i64::try_from(*position)
            .map(RuntimeValue::Int)
            .map_err(|_| {
                presentation_error(
                    ctx,
                    "finite presentation position exceeds the Int range",
                    Span::new(0, 0),
                )
            }),
        (IndexKind::RequiredCoordinate { .. }, _) => Err(presentation_error(
            ctx,
            "unbound required coordinate index reached presentation",
            Span::new(0, 0),
        )),
        (IndexKind::Named { .. } | IndexKind::RequiredNamed, IndexEntryKey::Position(_))
        | (IndexKind::Coordinate(_) | IndexKind::Finite { .. }, IndexEntryKey::Named(_)) => {
            Err(presentation_error(
                ctx,
                "checked presentation key does not match its index kind",
                Span::new(0, 0),
            ))
        }
    }
}

fn select_presentation<'a>(
    selection: &'a PresentationSelection,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
    locals: &HirLocalValueMap<'_>,
) -> Result<&'a PresentationProvenance, GraphcalError> {
    match selection {
        PresentationSelection::If {
            defining_dag,
            owner,
            condition,
            then_presentation,
            else_presentation,
        } => {
            let owner_ctx = checked_owner_context(ctx, defining_dag, owner, condition.span)?;
            match eval_hir_expr(condition, values, locals, &owner_ctx)? {
                RuntimeValue::Bool(true) => Ok(then_presentation),
                RuntimeValue::Bool(false) => Ok(else_presentation),
                other => Err(presentation_error(
                    &owner_ctx,
                    format!(
                        "checked presentation condition evaluated to {}, expected Bool",
                        other.kind()
                    ),
                    condition.span,
                )),
            }
        }
        PresentationSelection::Match {
            defining_dag,
            owner,
            scrutinee,
            arms,
        } => {
            let owner_ctx = checked_owner_context(ctx, defining_dag, owner, scrutinee.span)?;
            let scrutinee_value = eval_hir_expr(scrutinee, values, locals, &owner_ctx)?;
            arms.iter()
                .find(|arm| presentation_pattern_matches(&arm.pattern, &scrutinee_value))
                .map(|arm| &arm.presentation)
                .ok_or_else(|| {
                    presentation_error(
                        &owner_ctx,
                        "checked presentation match has no arm for its runtime scrutinee",
                        scrutinee.span,
                    )
                })
        }
    }
}

fn presentation_pattern_matches(pattern: &PresentationMatchPattern, value: &RuntimeValue) -> bool {
    match (pattern, value) {
        (
            PresentationMatchPattern::IndexLabel(expected),
            RuntimeValue::Label {
                index_name,
                variant,
            },
        ) => {
            index_name.declared_resolved() == Some(expected.index())
                && variant == expected.variant()
        }
        (
            PresentationMatchPattern::Constructor {
                owning_type,
                constructor,
            },
            RuntimeValue::Struct { type_name, .. },
        ) => {
            type_name.resolved() == owning_type.resolved()
                && type_name.as_str() == constructor.as_str()
        }
        _ => false,
    }
}

fn checked_owner_context<'a, 'ctx>(
    ctx: &'a EvalContext<'ctx>,
    defining_dag: &graphcal_compiler::dag_id::DagId,
    owner: &ResolvedDeclName,
    span: Span,
) -> Result<EvalContext<'a>, GraphcalError>
where
    'ctx: 'a,
{
    let dag = ctx.tir.dag_registry().get(defining_dag).ok_or_else(|| {
        presentation_error(
            ctx,
            format!("checked presentation owner `{defining_dag}` has no runtime DAG"),
            span,
        )
    })?;
    let source = ctx
        .checked_execution_facts
        .and_then(|facts| facts.for_dag(defining_dag))
        .map_or(ctx.src, |facts| facts.source());
    Ok(ctx.for_checked_decl(dag, source, owner))
}

/// Resolve a checked unit expression in its defining declaration environment.
fn resolve_unit_to_display(
    presentation: &UnitPresentation,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<DisplayUnit, GraphcalError> {
    let unit = presentation.unit();
    let owner_ctx = checked_owner_context(
        ctx,
        presentation.defining_dag(),
        presentation.owner(),
        unit.span,
    )?;
    let scale = crate::eval_expr::resolve_unit_scale(unit, values, &owner_ctx)?;
    let label = format_unit_terms_canonical(
        unit.terms
            .iter()
            .map(|item| (item.op, item.name.value.to_string(), item.power)),
    )
    .map_err(|_| GraphcalError::DimensionOverflow {
        src: owner_ctx.src.clone(),
        span: unit.span.into(),
    })?;
    DisplayUnit::try_new(label, scale).map_err(|error| GraphcalError::InternalError {
        message: format!("validated display-unit scale violated its invariant: {error}"),
        src: owner_ctx.src.clone(),
        span: unit.span.into(),
    })
}

fn presentation_error(
    ctx: &EvalContext<'_>,
    message: impl Into<String>,
    span: Span,
) -> GraphcalError {
    GraphcalError::InternalError {
        message: message.into(),
        src: ctx.src.clone(),
        span: span.into(),
    }
}

fn format_coordinate_impl(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
    exact: bool,
) -> String {
    idx_def.coordinate_data().map_or_else(
        || format!("#{position}"),
        |data| {
            let si_value = data.coordinate_value(position);
            let display_value = si_value / data.display_scale;
            let formatted = if exact {
                display_value.to_string()
            } else {
                format_number(display_value)
            };
            match &data.display_label {
                Some(label) => format!("{formatted} {label}"),
                None => formatted,
            }
        },
    )
}

/// Format a coordinate-index value for display, e.g. `"0 s"`, `"0.25 s"`.
pub(super) fn format_coordinate(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
) -> String {
    format_coordinate_impl(idx_def, position, false)
}

/// Format with enough binary64 precision to distinguish every coordinate.
pub(super) fn format_coordinate_exact(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    position: usize,
) -> String {
    format_coordinate_impl(idx_def, position, true)
}
