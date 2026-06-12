//! Display unit resolution: attaching human-readable unit labels to computed values,
//! formatting range steps, and converting unit expressions to strings.

use graphcal_compiler::hir::ExprKind;
use graphcal_compiler::hir::expr::{ConstRef, IndexArg, MapEntry, MapEntryKey, MatchPattern};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::names::{IndexVariantName, ResolvedName, namespace};
use indexmap::IndexMap;

use crate::eval_expr::{EvalContext, HirLocalValueMap, RuntimeValueMap, eval_hir_expr};

use super::types::{DisplayUnit, Value};
use graphcal_compiler::registry::format::{format_number, format_unit_expr_canonical};

/// Maximum number of value reads (`@x`, field/index access, if/match
/// selection) followed when propagating display metadata. Read chains track
/// the acyclic value graph, so this is insurance, not a semantic limit.
const MAX_READ_DEPTH: usize = 64;

/// Attach display units to a computed value based on its defining expression.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when a display unit's scale cannot be resolved
/// (unknown unit, non-positive or non-finite scale, dynamic scale evaluation
/// failure). A conversion the user wrote must either take effect or fail
/// loudly — silently falling back to the base unit would misreport the value.
pub(super) fn attach_display_units<'a>(
    value: &mut Value,
    expr: &'a graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'a>,
    values: &RuntimeValueMap,
) -> Result<(), GraphcalError> {
    attach_display_units_depth(value, expr, ctx, values, MAX_READ_DEPTH)
}

fn attach_display_units_depth<'a>(
    value: &mut Value,
    expr: &'a graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'a>,
    values: &RuntimeValueMap,
    depth: usize,
) -> Result<(), GraphcalError> {
    match (&mut *value, &expr.kind) {
        (Value::Scalar { display_unit, .. }, ExprKind::UnitLiteral { unit, .. }) => {
            *display_unit = Some(resolve_unit_to_display(unit, ctx, values)?);
        }
        (Value::Scalar { display_unit, .. }, ExprKind::Convert { target, .. }) => {
            *display_unit = Some(resolve_unit_to_display(target, ctx, values)?);
        }
        // Element-wise conversion on indexed values (#648 U1): apply the
        // target uniformly to every scalar entry, through nested axes.
        (Value::Indexed { entries, .. }, ExprKind::Convert { target, .. }) => {
            let du = resolve_unit_to_display(target, ctx, values)?;
            for entry_val in entries.values_mut() {
                set_scalar_display_unit_deep(entry_val, &du);
            }
        }
        // Constructor call: recurse into each field initializer.
        (Value::Struct { fields, .. }, ExprKind::ConstructorCall { fields: inits, .. }) => {
            for init in inits {
                if let Some(field_val) = fields.get_mut(&init.name.value) {
                    attach_display_units_depth(field_val, &init.value, ctx, values, depth)?;
                }
            }
        }
        // Map/table literal: recurse into each entry, walking through nested
        // Indexed values for multi-axis maps.
        (
            Value::Indexed { entries, .. },
            ExprKind::MapLiteral {
                entries: map_entries,
            },
        ) => {
            for map_entry in map_entries {
                if let Some(target) = walk_indexed_keys(entries, map_entry.keys.as_slice()) {
                    attach_display_units_depth(target, &map_entry.value, ctx, values, depth)?;
                }
            }
        }
        // For comprehension: extract a single display unit from body, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::ForComp { body, .. }) => {
            if let Some(du) = extract_flat_display_unit(body, ctx, values)? {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Scan: extract a single display unit from init, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::Scan { init, .. })
        | (Value::Indexed { entries, .. }, ExprKind::Unfold { init, .. }) => {
            if let Some(du) = extract_flat_display_unit(init, ctx, values)? {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Timezone display: set display_tz on Datetime values
        (Value::Datetime { display_tz, .. }, ExprKind::DisplayTimezone { timezone, .. }) => {
            *display_tz = Some(timezone.clone());
        }
        // Value reads propagate the source's display metadata (#648 B1/N5):
        // follow `@x`, const refs, field/index access, inline-dag projections,
        // and runtime-selected if/match branches back to the constructing
        // expression, then attach from that expression instead.
        _ => {
            if depth > 0
                && let Some(src_expr) = resolve_defining_expr(expr, ctx, values, depth)?
            {
                attach_display_units_depth(value, src_expr, ctx, values, depth - 1)?;
            }
        }
    }
    Ok(())
}

/// Follow a value-read expression back to the expression that constructed the
/// value it reads (#648 B1/N5).
///
/// Returns `Ok(None)` when the expression is not a read (arithmetic, function
/// calls, literals without units, …) or the source cannot be determined
/// statically — display metadata is simply not propagated in that case.
///
/// `if`/`match` selections and conditions are evaluated against the final
/// value map to pick the live branch; evaluation failures fall back to `None`
/// (the read target itself reports its own error).
fn resolve_defining_expr<'a>(
    expr: &'a graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'a>,
    values: &RuntimeValueMap,
    depth: usize,
) -> Result<Option<&'a graphcal_compiler::hir::Expr>, GraphcalError> {
    if depth == 0 {
        return Ok(None);
    }
    let exprs = &ctx.tir.root().semantic.expressions;
    let decl_expr = |name: &ResolvedName<namespace::Decl>| {
        exprs.runtime_expr(name).or_else(|| exprs.consts.get(name))
    };
    let resolved = match &expr.kind {
        ExprKind::GraphRef(target) => decl_expr(&target.value),
        ExprKind::ConstRef(target) => match &target.value {
            ConstRef::Decl(name) => exprs.consts.get(name),
            _ => None,
        },
        ExprKind::InlineDagRef { output, .. } => decl_expr(&output.value),
        ExprKind::FieldAccess { expr: inner, field } => {
            let Some(ctor) = resolve_defining_expr(inner, ctx, values, depth - 1)? else {
                return Ok(None);
            };
            let ExprKind::ConstructorCall { fields, .. } = &ctor.kind else {
                return Ok(None);
            };
            fields
                .iter()
                .find(|init| init.name.value == field.value)
                .map(|init| &init.value)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            let Some(map_expr) = resolve_defining_expr(inner, ctx, values, depth - 1)? else {
                return Ok(None);
            };
            let ExprKind::MapLiteral { entries } = &map_expr.kind else {
                return Ok(None);
            };
            find_static_map_entry(entries, args)
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond = eval_hir_expr(condition, values, &HirLocalValueMap::root(), ctx).ok();
            let Some(RuntimeValue::Bool(b)) = cond else {
                return Ok(None);
            };
            Some(if b {
                then_branch.as_ref()
            } else {
                else_branch.as_ref()
            })
        }
        ExprKind::Match { scrutinee, arms } => {
            let scrut = eval_hir_expr(scrutinee, values, &HirLocalValueMap::root(), ctx).ok();
            let Some(RuntimeValue::Label { variant, .. }) = scrut else {
                return Ok(None);
            };
            arms.iter()
                .find(|arm| {
                    matches!(
                        &arm.pattern,
                        MatchPattern::IndexLabel { variant: pat, .. }
                            if *pat.variant.variant() == variant
                    )
                })
                .map(|arm| &arm.body)
        }
        _ => None,
    };
    Ok(resolved)
}

/// Find the map-literal entry selected by statically known index variants.
///
/// Only fully static accesses resolve (`@x[R.A]`, `@grid[R.A, C.X]`); a
/// dynamic key or a partial multi-axis access returns `None`.
fn find_static_map_entry<'a>(
    entries: &'a [MapEntry],
    args: &[IndexArg],
) -> Option<&'a graphcal_compiler::hir::Expr> {
    entries.iter().find_map(|entry| {
        if entry.keys.len() != args.len() {
            return None;
        }
        let all_match = entry.keys.iter().zip(args).all(|(key, arg)| {
            matches!(
                (key, arg),
                (MapEntryKey::IndexVariant(k), IndexArg::Variant(a))
                    if k.variant.variant() == a.variant.variant()
            )
        });
        all_match.then_some(&entry.value)
    })
}

/// Resolve a `UnitExpr` to a `DisplayUnit`.
///
/// Handles both static and dynamic unit scales. For dynamic units, the scale
/// expression is evaluated using the provided evaluation context and value map.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when the scale cannot be resolved; see
/// [`attach_display_units`].
pub(super) fn resolve_unit_to_display(
    unit: &graphcal_compiler::desugar::desugared_ast::UnitExpr,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<DisplayUnit, GraphcalError> {
    let scale = crate::eval_expr::resolve_unit_scale(unit, values, ctx)?;
    Ok(DisplayUnit {
        label: format_unit_expr_canonical(unit),
        scale,
    })
}

/// Extract a single display unit from a scalar-producing expression.
///
/// Used for indexed collections (for comprehensions, scan) where all entries
/// share the same display unit. `Ok(None)` means the expression carries no
/// display unit; `Err` means it carries one that failed to resolve.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when a present display unit's scale cannot be
/// resolved; see [`attach_display_units`].
pub(super) fn extract_flat_display_unit(
    expr: &graphcal_compiler::hir::Expr,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> Result<Option<DisplayUnit>, GraphcalError> {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => resolve_unit_to_display(unit, ctx, values).map(Some),
        ExprKind::Convert { target, .. } => resolve_unit_to_display(target, ctx, values).map(Some),
        ExprKind::MapLiteral { entries } => entries.first().map_or(Ok(None), |e| {
            extract_flat_display_unit(&e.value, ctx, values)
        }),
        ExprKind::ForComp { body, .. } => extract_flat_display_unit(body, ctx, values),
        ExprKind::Scan { init, .. } | ExprKind::Unfold { init, .. } => {
            extract_flat_display_unit(init, ctx, values)
        }
        _ => Ok(None),
    }
}

/// Format a range index step value for display, e.g. `"0 s"`, `"0.25 s"`.
pub(super) fn format_range_step(
    idx_def: &graphcal_compiler::registry::types::IndexDef,
    step_index: usize,
) -> String {
    idx_def.range_data().map_or_else(
        || format!("#{step_index}"),
        |data| {
            let si_value = data.step_value(step_index);
            let display_value = si_value / data.display_scale;
            let formatted = format_number(display_value);
            match &data.display_label {
                Some(label) => format!("{formatted} {label}"),
                None => formatted,
            }
        },
    )
}

/// Set display unit on a scalar value. No-op for non-scalar values.
pub(super) fn set_scalar_display_unit(value: &mut Value, du: &DisplayUnit) {
    if let Value::Scalar { display_unit, .. } = value {
        *display_unit = Some(du.clone());
    }
}

/// Set display unit on every scalar leaf, descending through nested
/// `Indexed` layers (multi-axis values).
pub(super) fn set_scalar_display_unit_deep(value: &mut Value, du: &DisplayUnit) {
    match value {
        Value::Scalar { display_unit, .. } => *display_unit = Some(du.clone()),
        Value::Indexed { entries, .. } => {
            for entry in entries.values_mut() {
                set_scalar_display_unit_deep(entry, du);
            }
        }
        _ => {}
    }
}

/// Walk through nested `Indexed` entries using successive map entry keys.
///
/// For a single-axis map (`keys.len() == 1`), returns the entry matching `keys[0]`.
/// For multi-axis maps, drills into nested `Value::Indexed` using each key in turn.
fn walk_indexed_keys<'a>(
    entries: &'a mut IndexMap<IndexVariantName, Value>,
    keys: &[MapEntryKey],
) -> Option<&'a mut Value> {
    let (first, rest) = keys.split_first()?;
    let variant = match first {
        MapEntryKey::IndexVariant(resolved) => resolved.variant.variant(),
        MapEntryKey::NatRangeVariant { variant, .. } => &variant.value,
    };
    let value = entries.get_mut(variant)?;
    if rest.is_empty() {
        Some(value)
    } else if let Value::Indexed { entries: inner, .. } = value {
        walk_indexed_keys(inner, rest)
    } else {
        None
    }
}
