use std::collections::HashMap;

use indexmap::IndexMap;

use graphcal_compiler::syntax::ast::{Expr, MapEntry};
use graphcal_compiler::syntax::names::{DeclName, VariantName};
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use super::EvalContext;
use super::eval_expr;

/// Evaluate a `NatExpr` to a concrete `u64` during runtime.
///
/// Looks up nat parameter values from `local_values` (stored as `__nat_param_X`).
fn eval_nat_expr(
    expr: &graphcal_compiler::syntax::ast::NatExpr,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<u64, GraphcalError> {
    use graphcal_compiler::syntax::ast::NatExpr;
    match expr {
        NatExpr::Literal(n, _) => Ok(*n),
        NatExpr::Var(ident) => {
            let key = format!("__nat_param_{}", ident.name);
            let nat_val = local_values.get(&key).ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "unresolved nat parameter `{}` in for-range binding",
                        ident.name
                    ),
                    ident.span,
                )
            })?;
            let RuntimeValue::Int(n) = nat_val else {
                return Err(ctx.internal_error(
                    format!("nat parameter `{}` has non-integer value", ident.name),
                    ident.span,
                ));
            };
            u64::try_from(*n).map_err(|_| {
                ctx.internal_error(
                    format!("nat parameter `{}` has negative value {}", ident.name, n),
                    ident.span,
                )
            })
        }
        NatExpr::Add(lhs, rhs, span) => {
            let l = eval_nat_expr(lhs, local_values, ctx)?;
            let r = eval_nat_expr(rhs, local_values, ctx)?;
            l.checked_add(r)
                .ok_or_else(|| ctx.eval_error(format!("nat arithmetic overflow: {l} + {r}"), *span))
        }
        NatExpr::Mul(lhs, rhs, span) => {
            let l = eval_nat_expr(lhs, local_values, ctx)?;
            let r = eval_nat_expr(rhs, local_values, ctx)?;
            l.checked_mul(r)
                .ok_or_else(|| ctx.eval_error(format!("nat arithmetic overflow: {l} * {r}"), *span))
        }
    }
}

/// Evaluate an index access expression.
pub(super) fn eval_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[graphcal_compiler::syntax::ast::IndexArg],
    values: &HashMap<DeclName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut current = eval_expr(inner, values, local_values, ctx)?;
    for arg in args {
        let RuntimeValue::Indexed { entries, .. } = current else {
            return Err(ctx.eval_error("indexing a non-indexed value", expr.span));
        };
        let variant_name: VariantName = match arg {
            graphcal_compiler::syntax::ast::IndexArg::Variant { variant, .. } => {
                variant.value.clone()
            }
            graphcal_compiler::syntax::ast::IndexArg::Var(ident) => {
                let var_val = local_values.get(&ident.name).ok_or_else(|| {
                    ctx.eval_error(
                        format!("undefined loop variable `{}`", ident.name),
                        ident.span,
                    )
                })?;
                match var_val {
                    RuntimeValue::Label { variant, .. } => variant.clone(),
                    RuntimeValue::Struct { type_name, .. } => VariantName::new(type_name.as_str()),
                    RuntimeValue::RangeLabel { step_index, .. } => {
                        VariantName::new(format!("#{step_index}"))
                    }
                    RuntimeValue::Int(n) => {
                        // Nat range loop variable: integer value maps to #N variant
                        VariantName::new(format!("#{n}"))
                    }
                    _ => {
                        return Err(ctx.eval_error(
                            format!("`{}` is not a loop variable", ident.name),
                            ident.span,
                        ));
                    }
                }
            }
            graphcal_compiler::syntax::ast::IndexArg::Expr(index_expr) => {
                let val = eval_expr(index_expr, values, local_values, ctx)?;
                match val {
                    RuntimeValue::Int(n) => {
                        if n < 0 {
                            return Err(ctx.eval_error(
                                format!("index expression evaluated to negative value: {n}"),
                                index_expr.span,
                            ));
                        }
                        VariantName::new(format!("#{n}"))
                    }
                    _ => {
                        return Err(ctx.eval_error(
                            "index expression must evaluate to an integer",
                            index_expr.span,
                        ));
                    }
                }
            }
        };
        current = entries.get(variant_name.as_str()).cloned().ok_or_else(|| {
            ctx.eval_error(format!("variant `{variant_name}` not found"), expr.span)
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
    acc_name: &graphcal_compiler::syntax::ast::Ident,
    val_name: &graphcal_compiler::syntax::ast::Ident,
    body: &Expr,
    values: &HashMap<DeclName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let source_val = eval_expr(source, values, local_values, ctx)?;
    let RuntimeValue::Indexed {
        index_name,
        entries: source_entries,
    } = source_val
    else {
        return Err(ctx.eval_error("scan source must be an indexed value", source.span));
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
    prev_name: &graphcal_compiler::syntax::ast::Ident,
    curr_name: &graphcal_compiler::syntax::ast::Ident,
    body: &Expr,
    values: &HashMap<DeclName, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let unfold_ctx = ctx.unfold_context.as_ref().ok_or_else(|| {
        ctx.eval_error(
            "unfold expression requires evaluation context with self_name and declared_types",
            expr.span,
        )
    })?;
    let self_name = unfold_ctx.self_name;
    let declared_types = unfold_ctx.declared_types;

    // Find the range index from the node's declared type
    let declared = declared_types.get(self_name).ok_or_else(|| {
        ctx.eval_error(
            format!("no declared type for node `{self_name}`"),
            Span::new(0, 0),
        )
    })?;
    let index_name = match declared {
        graphcal_compiler::registry::declared_type::DeclaredType::Indexed { index, .. } => {
            index.clone()
        }
        _ => {
            return Err(ctx.eval_error(
                format!("node `{self_name}` must have an indexed type for time scan"),
                Span::new(0, 0),
            ));
        }
    };
    let idx_def = ctx
        .registry
        .indexes
        .get_index(index_name.as_str())
        .ok_or_else(|| ctx.eval_error(format!("unknown index `{index_name}`"), Span::new(0, 0)))?;

    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let range_data = idx_def.range_data().ok_or_else(|| {
        ctx.eval_error(
            format!("unfold requires a range index, but `{index_name}` is not a range"),
            Span::new(0, 0),
        )
    })?;
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
    // Clone values once before the loop; only the self-reference entry changes per iteration.
    let mut overlay_values = values.clone();
    // Seed the self-reference slot once. Per iteration we swap the accumulating
    // `result_entries` in/out of this slot via `std::mem::take` to avoid a full
    // O(N) clone of the map every iteration (which would make the loop O(N²)).
    overlay_values.insert(
        DeclName::new(self_name),
        RuntimeValue::Indexed {
            index_name: index_name.clone(),
            entries: IndexMap::new(),
        },
    );
    for i in 1..step_count {
        // Move the accumulated entries into the overlay (O(1)). `result_entries`
        // is left as an empty IndexMap — it will be restored after body eval.
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(self_name) {
            *entries = std::mem::take(&mut result_entries);
        }

        let prev_value = range_data.step_value(i - 1);
        let curr_value = range_data.step_value(i);

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

        // Take the entries back out of the overlay (O(1)) so we can append the
        // new result without cloning. The overlay's slot is left with an empty
        // IndexMap until the next iteration repopulates it.
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(self_name) {
            result_entries = std::mem::take(entries);
        }
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
pub(super) fn eval_map_literal(
    entries: &[MapEntry],
    values: &HashMap<DeclName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let first = entries.first().ok_or_else(|| {
        ctx.internal_error(
            "eval_map_literal called with no entries".to_string(),
            graphcal_compiler::syntax::span::Span::new(0, 0),
        )
    })?;
    let first_key = first.keys.first().ok_or_else(|| {
        ctx.internal_error(
            "map literal entry has no index keys".to_string(),
            first.value.span,
        )
    })?;
    let arity = first.keys.len();
    let idx_name = first_key.index.value.clone();

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
        .ok_or_else(|| {
            ctx.internal_error(format!("unknown index `{idx_name}`"), first_key.index.span)
        })?;
    let variants = idx_def.variants();

    let mut outer = IndexMap::new();
    for variant in &variants {
        // Collect entries whose first key matches this variant, stripping the first key
        let sub_entries: Vec<MapEntry> = entries
            .iter()
            .filter(|e| {
                e.keys
                    .first()
                    .is_some_and(|k| k.variant.value.as_str() == variant.as_str())
            })
            .map(|e| MapEntry {
                keys: e.keys[1..].to_vec(),
                value: e.value.clone(),
            })
            .collect();

        // A dim-checked program guarantees every variant has at least one
        // entry. If that invariant is violated (e.g. malformed IR), surface it
        // as an internal error rather than panicking on `entries[0]` inside
        // the recursive call.
        if sub_entries.is_empty() {
            return Err(ctx.internal_error(
                format!(
                    "map literal for index `{idx_name}` is missing entries for variant `{variant}`"
                ),
                first_key.index.span,
            ));
        }
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
pub(super) fn eval_for_comp(
    bindings: &[graphcal_compiler::syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<DeclName, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::syntax::ast::ForBindingIndex;

    let binding = &bindings[0];

    // Resolve the index name and get the index definition
    let (idx_name, error_span) = match &binding.index {
        ForBindingIndex::Named(spanned) => (spanned.value.clone(), spanned.span),
        ForBindingIndex::Range { arg, span } => {
            let size = eval_nat_expr(arg, local_values, ctx)?;
            let idx_name = graphcal_compiler::syntax::names::IndexName::new(
                graphcal_compiler::registry::types::nat_range_index_name(size),
            );
            (idx_name, *span)
        }
    };

    // For nat range indexes from generic Nat arithmetic (e.g., range(N + 1)),
    // the concrete size may not have been registered at compile time.
    // Create a temporary IndexDef for the computed size if not in the registry.
    let dynamic_nat_def;
    let idx_def = if let Some(def) = ctx.registry.indexes.get_index(idx_name.as_str()) {
        def
    } else if let Some(size) =
        graphcal_compiler::registry::types::parse_nat_range_index_name(idx_name.as_str())
    {
        dynamic_nat_def = graphcal_compiler::registry::types::IndexDef {
            name: idx_name.clone(),
            kind: graphcal_compiler::registry::types::IndexKind::NatRange { size },
        };
        &dynamic_nat_def
    } else {
        return Err(ctx.internal_error(format!("unknown index `{idx_name}`"), error_span));
    };
    let remaining = &bindings[1..];

    let variants = idx_def.variants();
    let mut entries = IndexMap::new();
    // Clone local_values once before the loop; only the binding value changes per iteration.
    let mut inner_locals = local_values.clone();
    // Iterating with `enumerate()` gives us the step index directly for
    // Range / NatRange kinds instead of round-tripping it through the
    // `#N`-prefixed variant name.
    for (step_index, variant) in variants.iter().enumerate() {
        let binding_value = match &idx_def.kind {
            graphcal_compiler::registry::types::IndexKind::Named { .. }
            | graphcal_compiler::registry::types::IndexKind::RequiredNamed => RuntimeValue::Label {
                index_name: idx_name.clone(),
                variant: variant.clone(),
            },
            graphcal_compiler::registry::types::IndexKind::Range(data) => {
                RuntimeValue::RangeLabel {
                    step_index,
                    value: data.step_value(step_index),
                }
            }
            // RequiredRange has 0 variants, so this loop body is never reached.
            graphcal_compiler::registry::types::IndexKind::RequiredRange { .. } => {
                return Err(ctx.internal_error(
                    "RequiredRange should have been bound before evaluation".to_string(),
                    error_span,
                ));
            }
            graphcal_compiler::registry::types::IndexKind::NatRange { .. } => {
                // Nat range loop variable: integer value from the step index.
                RuntimeValue::Int(i64::try_from(step_index).map_err(|_| {
                    ctx.internal_error(
                        format!("nat range step {step_index} too large for i64"),
                        error_span,
                    )
                })?)
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
