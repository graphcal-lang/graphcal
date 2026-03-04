use std::collections::HashMap;

use indexmap::IndexMap;

use graphcal_syntax::ast::{Expr, MapEntry};
use graphcal_syntax::names::VariantName;

use crate::error::GraphcalError;
use crate::runtime_value::RuntimeValue;

use super::EvalContext;
use super::eval_expr;

/// Evaluate a map/table literal expression.
pub(super) fn eval_map_or_table(
    entries: &[MapEntry],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_map_literal(entries, values, local_values, ctx)
}

/// Evaluate a `for` comprehension expression.
pub(super) fn eval_for_comp_expr(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_for_comp(bindings, body, values, local_values, ctx)
}

/// Evaluate an index access expression.
pub(super) fn eval_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[graphcal_syntax::ast::IndexArg],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut current = eval_expr(inner, values, local_values, ctx)?;
    for arg in args {
        let RuntimeValue::Indexed { entries, .. } = current else {
            return Err(GraphcalError::EvalError {
                message: "indexing a non-indexed value".to_string(),
                src: ctx.src.clone(),
                span: expr.span.into(),
            });
        };
        let variant_name: VariantName = match arg {
            graphcal_syntax::ast::IndexArg::Variant { variant, .. } => variant.value.clone(),
            graphcal_syntax::ast::IndexArg::Var(ident) => {
                let var_val =
                    local_values
                        .get(&ident.name)
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!("undefined loop variable `{}`", ident.name),
                            src: ctx.src.clone(),
                            span: ident.span.into(),
                        })?;
                match var_val {
                    RuntimeValue::Label { variant, .. } | RuntimeValue::Struct { variant, .. } => {
                        variant.clone()
                    }
                    RuntimeValue::RangeLabel { step_index, .. } => {
                        VariantName::new(format!("#{step_index}"))
                    }
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!("`{}` is not a loop variable", ident.name),
                            src: ctx.src.clone(),
                            span: ident.span.into(),
                        });
                    }
                }
            }
        };
        current = entries.get(variant_name.as_str()).cloned().ok_or_else(|| {
            GraphcalError::EvalError {
                message: format!("variant `{variant_name}` not found"),
                src: ctx.src.clone(),
                span: expr.span.into(),
            }
        })?;
    }
    Ok(current)
}

/// Evaluate a `scan` expression.
#[expect(
    clippy::too_many_arguments,
    reason = "scan requires source, init, two binding names, body, plus eval context"
)]
pub(super) fn eval_scan(
    source: &Expr,
    init: &Expr,
    acc_name: &graphcal_syntax::ast::Ident,
    val_name: &graphcal_syntax::ast::Ident,
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let source_val = eval_expr(source, values, local_values, ctx)?;
    let RuntimeValue::Indexed {
        index_name,
        entries: source_entries,
    } = source_val
    else {
        return Err(GraphcalError::EvalError {
            message: "scan source must be an indexed value".to_string(),
            src: ctx.src.clone(),
            span: source.span.into(),
        });
    };
    let init_val = eval_expr(init, values, local_values, ctx)?;

    let mut acc = init_val;
    let mut result_entries = IndexMap::new();
    // Pre-build scan_locals with parent scope + the two loop-variable keys.
    // Reuse across iterations instead of cloning local_values each time.
    let mut scan_locals = local_values.clone();
    for (variant, val) in &source_entries {
        scan_locals.insert(acc_name.name.clone(), acc);
        scan_locals.insert(val_name.name.clone(), val.clone());
        let body_val = eval_expr(body, values, &scan_locals, ctx)?;
        result_entries.insert(variant.clone(), body_val.clone());
        acc = body_val;
    }
    Ok(RuntimeValue::Indexed {
        index_name,
        entries: result_entries,
    })
}

/// Evaluate an `unfold(init, |prev_i, i| body)` expression.
///
/// Builds results incrementally over a range index. Each iteration creates a
/// scoped overlay of `values` containing the partial result so that
/// `@self_name[prev_i]` resolves correctly, without mutating the shared map.
///
/// Requires `ctx.unfold_context` to be set with the self-referencing node name
/// and declared types map.
#[expect(
    clippy::needless_range_loop,
    reason = "loop index i is used for step_value(i), step_index fields, and variant indexing"
)]
pub(super) fn eval_unfold(
    expr: &Expr,
    init: &Expr,
    prev_name: &graphcal_syntax::ast::Ident,
    curr_name: &graphcal_syntax::ast::Ident,
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let unfold_ctx = ctx
        .unfold_context
        .as_ref()
        .ok_or_else(|| GraphcalError::EvalError {
            message:
                "unfold expression requires evaluation context with self_name and declared_types"
                    .to_string(),
            src: ctx.src.clone(),
            span: expr.span.into(),
        })?;
    let self_name = unfold_ctx.self_name;
    let declared_types = unfold_ctx.declared_types;

    // Find the range index from the node's declared type
    let declared = declared_types
        .get(self_name)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("no declared type for node `{self_name}`"),
            src: ctx.src.clone(),
            span: (0, 0).into(),
        })?;
    let index_name = match declared {
        crate::declared_type::DeclaredType::Indexed { index, .. } => index.clone(),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("node `{self_name}` must have an indexed type for time scan"),
                src: ctx.src.clone(),
                span: (0, 0).into(),
            });
        }
    };
    let idx_def = ctx
        .registry
        .indexes
        .get_index(index_name.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("unknown index `{index_name}`"),
            src: ctx.src.clone(),
            span: (0, 0).into(),
        })?;

    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    // Evaluate init expression
    let init_val = eval_expr(init, values, &empty_locals, ctx)?;

    // Build results incrementally
    let mut result_entries: IndexMap<VariantName, RuntimeValue> = IndexMap::new();

    // Step 0: init value
    result_entries.insert(variants[0].clone(), init_val);

    // Steps 1..N: evaluate body with prev_t and t bindings
    // Pre-allocate scan_locals with the two loop-variable keys (reused across iterations).
    let mut scan_locals = HashMap::with_capacity(2);
    for i in 1..step_count {
        // Build a scoped overlay: values + partial result for @self[prev_t]
        let mut overlay_values = values.clone();
        overlay_values.insert(
            self_name.to_string(),
            RuntimeValue::Indexed {
                index_name: index_name.clone(),
                entries: result_entries.clone(),
            },
        );

        let prev_value = idx_def
            .step_value(i - 1)
            .map_err(|e| GraphcalError::EvalError {
                message: format!("internal: range index step {} out of bounds: {e}", i - 1),
                src: ctx.src.clone(),
                span: (0, 0).into(),
            })?;
        let curr_value = idx_def
            .step_value(i)
            .map_err(|e| GraphcalError::EvalError {
                message: format!("internal: range index step {i} out of bounds: {e}"),
                src: ctx.src.clone(),
                span: (0, 0).into(),
            })?;

        scan_locals.insert(
            prev_name.name.clone(),
            RuntimeValue::RangeLabel {
                step_index: i - 1,
                value: prev_value,
            },
        );
        scan_locals.insert(
            curr_name.name.clone(),
            RuntimeValue::RangeLabel {
                step_index: i,
                value: curr_value,
            },
        );

        let body_val = eval_expr(body, &overlay_values, &scan_locals, ctx)?;
        result_entries.insert(variants[i].clone(), body_val);
    }

    Ok(RuntimeValue::Indexed {
        index_name,
        entries: result_entries,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Evaluate a map literal, handling both single-axis and multi-axis (tuple-key) entries.
///
/// For single-axis (`keys.len() == 1`), builds a flat `Indexed`.
/// For multi-axis, groups entries by the first key's variant and recursively
/// builds nested `Indexed` values from the remaining keys.
fn eval_map_literal(
    entries: &[MapEntry],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let arity = entries[0].keys.len();
    let idx_name = entries[0].keys[0].index.value.clone();

    if arity == 1 {
        // Single-axis: flat Indexed
        let mut result = IndexMap::new();
        for entry in entries {
            let val = eval_expr(&entry.value, values, local_values, ctx)?;
            result.insert(entry.keys[0].variant.value.clone(), val);
        }
        return Ok(RuntimeValue::Indexed {
            index_name: idx_name,
            entries: result,
        });
    }

    // Multi-axis: group by first key, recurse on remaining keys
    let idx_def = ctx
        .registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("unknown index `{idx_name}`"),
            src: ctx.src.clone(),
            span: entries[0].keys[0].index.span.into(),
        })?;
    let variants = idx_def.variants();

    let mut outer = IndexMap::new();
    for variant in &variants {
        // Collect entries whose first key matches this variant, stripping the first key
        let sub_entries: Vec<MapEntry> = entries
            .iter()
            .filter(|e| e.keys[0].variant.value.as_str() == variant.as_str())
            .map(|e| MapEntry {
                keys: e.keys[1..].to_vec(),
                value: e.value.clone(),
            })
            .collect();

        let inner = eval_map_literal(&sub_entries, values, local_values, ctx)?;
        outer.insert(variant.clone(), inner);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries: outer,
    })
}

/// Evaluate a `for` comprehension by iterating over index variants.
///
/// For single binding `for m: Maneuver { body }`, iterates over Maneuver variants
/// and collects results into `Indexed`.
/// For multi-binding, produces nested `Indexed` values.
fn eval_for_comp(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let idx_name = binding.index.value.clone();
    let idx_def = ctx
        .registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("unknown index `{idx_name}`"),
            src: ctx.src.clone(),
            span: binding.index.span.into(),
        })?;
    let remaining = &bindings[1..];

    let variants = idx_def.variants();
    let mut entries = IndexMap::new();
    for variant in &variants {
        let mut inner_locals = local_values.clone();
        let binding_value = match &idx_def.kind {
            crate::registry::IndexKind::Named { .. }
            | crate::registry::IndexKind::RequiredNamed => RuntimeValue::Label {
                index_name: idx_name.clone(),
                variant: variant.clone(),
            },
            crate::registry::IndexKind::Range { .. }
            | crate::registry::IndexKind::RequiredRange { .. } => {
                let step_index = variant
                    .as_str()
                    .strip_prefix('#')
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "range variant `{variant}` has unexpected format (expected #N)"
                        ),
                        src: ctx.src.clone(),
                        span: binding.index.span.into(),
                    })?;
                RuntimeValue::RangeLabel {
                    step_index,
                    value: idx_def.step_value(step_index).map_err(|e| {
                        GraphcalError::InternalError {
                            message: format!("range index step {step_index} out of bounds: {e}"),
                            src: ctx.src.clone(),
                            span: binding.index.span.into(),
                        }
                    })?,
                }
            }
        };
        inner_locals.insert(binding.var.name.clone(), binding_value);
        let val = if remaining.is_empty() {
            eval_expr(body, values, &inner_locals, ctx)?
        } else {
            eval_for_comp(remaining, body, values, &inner_locals, ctx)?
        };
        entries.insert(variant.clone(), val);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries,
    })
}
