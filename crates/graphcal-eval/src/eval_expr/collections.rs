use std::collections::HashMap;

use indexmap::IndexMap;

use graphcal_compiler::desugar::resolved_ast::{Expr, MapEntry, MapEntryKey};
use graphcal_compiler::syntax::names::{
    IndexVariantName, ResolvedIndexVariant, ResolvedName, ScopedName, namespace,
};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;

use graphcal_compiler::registry::declared_type::IndexTypeRef;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::IndexDef;

use crate::decl_key::RuntimeDeclKey;

use super::EvalContext;
use super::RuntimeValueMap;
use super::eval_expr;
use super::index_ref_from_path;
use super::index_ref_matches_resolved_or_leaf;
use super::index_ref_with_eval_owner;

/// Evaluate a `NatExpr` to a concrete `u64` during runtime.
///
/// Looks up nat parameter values from `local_values` (stored as `__nat_param_X`).
fn eval_nat_expr(
    expr: &graphcal_compiler::desugar::resolved_ast::NatExpr,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<u64, GraphcalError> {
    use graphcal_compiler::desugar::resolved_ast::NatExpr;
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

fn resolved_collection_refs<'a>(
    ctx: &EvalContext<'a>,
) -> Option<&'a graphcal_compiler::tir::typed::ResolvedCollectionRefs> {
    ctx.current_dag
        .and_then(|dag| dag.resolved_collection_refs.as_ref())
}

fn resolved_map_entry_variant<'a>(
    ctx: &EvalContext<'a>,
    key: &MapEntryKey,
) -> Option<&'a ResolvedIndexVariant> {
    let span = key.index.span.merge(key.variant.span);
    resolved_collection_refs(ctx).and_then(|refs| refs.map_entry_variants.get(&span))
}

fn resolved_index_access_variant<'a>(
    ctx: &EvalContext<'a>,
    index_span: Span,
    variant_span: Span,
) -> Option<&'a ResolvedIndexVariant> {
    let span = index_span.merge(variant_span);
    resolved_collection_refs(ctx).and_then(|refs| refs.index_access_variants.get(&span))
}

fn index_owner_mismatch_message(actual: &IndexTypeRef, expected_leaf: &str) -> String {
    if actual.name().as_str() == expected_leaf {
        format!(
            "index argument belongs to a different `{expected_leaf}` index owner than the indexed value"
        )
    } else {
        format!(
            "index argument belongs to `{expected_leaf}`, but value is indexed by `{}`",
            actual.name()
        )
    }
}

fn ensure_index_ref_matches_resolved(
    actual: &IndexTypeRef,
    expected: &ResolvedName<namespace::Index>,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    if index_ref_matches_resolved_or_leaf(actual, expected) {
        return Ok(());
    }
    Err(ctx.eval_error(
        index_owner_mismatch_message(actual, expected.as_str()),
        span,
    ))
}

fn ensure_index_refs_match(
    actual: &IndexTypeRef,
    expected: &IndexTypeRef,
    span: Span,
    ctx: &EvalContext<'_>,
) -> Result<(), GraphcalError> {
    if actual.matches_ref(expected) {
        return Ok(());
    }
    Err(ctx.eval_error(
        index_owner_mismatch_message(actual, expected.name().as_str()),
        span,
    ))
}

fn index_def_for_ref<'a>(index_ref: &IndexTypeRef, ctx: &EvalContext<'a>) -> Option<&'a IndexDef> {
    resolved_collection_refs(ctx)
        .and_then(|refs| refs.index_defs.get(index_ref.resolved()))
        .or_else(|| ctx.registry.indexes.get_index(index_ref.as_str()))
}

fn map_entry_variant_for_axis(
    key: &MapEntryKey,
    axis: &IndexTypeRef,
    ctx: &EvalContext<'_>,
) -> Result<IndexVariantName, GraphcalError> {
    if let Some(resolved) = resolved_map_entry_variant(ctx, key) {
        ensure_index_ref_matches_resolved(axis, resolved.index(), key.index.span, ctx)?;
        Ok(resolved.variant().clone())
    } else {
        let index_ref = index_ref_with_eval_owner(ctx, key.index.value.registry_name());
        ensure_index_refs_match(axis, &index_ref, key.index.span, ctx)?;
        Ok(key.variant.value.clone())
    }
}

fn map_entry_index_ref(key: &MapEntryKey, ctx: &EvalContext<'_>) -> IndexTypeRef {
    resolved_map_entry_variant(ctx, key).map_or_else(
        || index_ref_with_eval_owner(ctx, key.index.value.registry_name()),
        |resolved| IndexTypeRef::from_resolved(resolved.index().clone()),
    )
}

fn map_entry_index_def<'a>(
    key: &MapEntryKey,
    index_name: &IndexTypeRef,
    ctx: &EvalContext<'a>,
) -> Option<&'a IndexDef> {
    resolved_map_entry_variant(ctx, key)
        .and_then(|resolved| {
            resolved_collection_refs(ctx).and_then(|refs| refs.index_defs.get(resolved.index()))
        })
        .or_else(|| index_def_for_ref(index_name, ctx))
}

fn resolved_for_binding_index<'a>(
    ctx: &EvalContext<'a>,
    span: graphcal_compiler::syntax::span::Span,
) -> Option<&'a ResolvedName<namespace::Index>> {
    resolved_collection_refs(ctx).and_then(|refs| refs.for_binding_indexes.get(&span))
}

/// Evaluate an index access expression.
pub(super) fn eval_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[graphcal_compiler::desugar::resolved_ast::IndexArg],
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut current = eval_expr(inner, values, local_values, ctx)?;
    for arg in args {
        let RuntimeValue::Indexed {
            index_name,
            entries,
        } = current
        else {
            return Err(ctx.eval_error("indexing a non-indexed value", expr.span));
        };
        let variant_name: IndexVariantName = match arg {
            graphcal_compiler::desugar::resolved_ast::IndexArg::Variant { index, variant } => {
                if let Some(resolved) = resolved_index_access_variant(ctx, index.span, variant.span)
                {
                    ensure_index_ref_matches_resolved(
                        &index_name,
                        resolved.index(),
                        index.span,
                        ctx,
                    )?;
                    resolved.variant().clone()
                } else {
                    let index_ref = index_ref_from_path(ctx, &index.value);
                    ensure_index_refs_match(&index_name, &index_ref, index.span, ctx)?;
                    variant.value.clone()
                }
            }
            graphcal_compiler::desugar::resolved_ast::IndexArg::Var(ident) => {
                let var_val = local_values.get(ident.name.as_str()).ok_or_else(|| {
                    ctx.eval_error(
                        format!("undefined loop variable `{}`", ident.name),
                        ident.span,
                    )
                })?;
                match var_val {
                    RuntimeValue::Label {
                        index_name: label_index,
                        variant,
                    } => {
                        ensure_index_refs_match(&index_name, label_index, ident.span, ctx)?;
                        variant.clone()
                    }
                    RuntimeValue::Struct { type_name, .. } => {
                        IndexVariantName::new(type_name.as_str())
                    }
                    RuntimeValue::RangeLabel { step_index, .. } => {
                        IndexVariantName::range_step(step_index)
                    }
                    RuntimeValue::Int(n) => {
                        // Nat range loop variable: integer value maps to #N variant
                        IndexVariantName::range_step(n)
                    }
                    _ => {
                        return Err(ctx.eval_error(
                            format!("`{}` is not a loop variable", ident.name),
                            ident.span,
                        ));
                    }
                }
            }
            graphcal_compiler::desugar::resolved_ast::IndexArg::Expr(index_expr) => {
                let val = eval_expr(index_expr, values, local_values, ctx)?;
                match val {
                    RuntimeValue::Int(n) => {
                        if n < 0 {
                            return Err(ctx.eval_error(
                                format!("index expression evaluated to negative value: {n}"),
                                index_expr.span,
                            ));
                        }
                        IndexVariantName::range_step(n)
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
    acc_name: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::names::LocalName,
    >,
    val_name: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::names::LocalName,
    >,
    body: &Expr,
    values: &RuntimeValueMap,
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
        scan_locals.insert(acc_name.value.as_str().to_owned(), acc);
        scan_locals.insert(val_name.value.as_str().to_owned(), val.clone());
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
    prev_name: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::names::LocalName,
    >,
    curr_name: &graphcal_compiler::syntax::span::Spanned<
        graphcal_compiler::syntax::names::LocalName,
    >,
    body: &Expr,
    values: &RuntimeValueMap,
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

    // Find the range index from the node's declared type. Top-level decls are bare locals.
    let declared = declared_types
        .get(&ScopedName::local(self_name))
        .ok_or_else(|| {
            ctx.eval_error(
                format!("no declared type for node `{self_name}`"),
                Span::new(0, 0),
            )
        })?;
    let index_ref = match declared {
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
    let idx_def = index_def_for_ref(&index_ref, ctx)
        .ok_or_else(|| ctx.eval_error(format!("unknown index `{index_ref}`"), Span::new(0, 0)))?;

    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let range_data = idx_def.range_data().ok_or_else(|| {
        ctx.eval_error(
            format!("unfold requires a range index, but `{index_ref}` is not a range"),
            Span::new(0, 0),
        )
    })?;
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    // Evaluate init expression
    let init_val = eval_expr(init, values, &empty_locals, ctx)?;

    // Build results incrementally
    let mut result_entries: IndexMap<IndexVariantName, RuntimeValue> = IndexMap::new();

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
    let self_scoped = ScopedName::local(self_name);
    let self_key = RuntimeDeclKey::for_local_decl(
        ctx.current_dag.unwrap_or_else(|| ctx.tir.root()),
        &self_scoped,
    );
    overlay_values.insert(
        self_key.clone(),
        RuntimeValue::Indexed {
            index_name: index_ref.clone(),
            entries: IndexMap::new(),
        },
    );
    for i in 1..step_count {
        // Move the accumulated entries into the overlay (O(1)). `result_entries`
        // is left as an empty IndexMap — it will be restored after body eval.
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(&self_key) {
            *entries = std::mem::take(&mut result_entries);
        }

        let prev_value = range_data.step_value(i - 1);
        let curr_value = range_data.step_value(i);

        scan_locals.insert(
            prev_name.value.as_str().to_owned(),
            RuntimeValue::RangeLabel {
                step_index: i - 1,
                value: prev_value,
            },
        );
        scan_locals.insert(
            curr_name.value.as_str().to_owned(),
            RuntimeValue::RangeLabel {
                step_index: i,
                value: curr_value,
            },
        );

        let body_val = eval_expr(body, &overlay_values, &scan_locals, ctx)?;

        // Take the entries back out of the overlay (O(1)) so we can append the
        // new result without cloning. The overlay's slot is left with an empty
        // IndexMap until the next iteration repopulates it.
        if let Some(RuntimeValue::Indexed { entries, .. }) = overlay_values.get_mut(&self_key) {
            result_entries = std::mem::take(entries);
        }
        result_entries.insert(variants[i].clone(), body_val);
    }

    Ok(RuntimeValue::Indexed {
        index_name: index_ref,
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
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    let first = entries.first().ok_or_else(|| {
        ctx.internal_error(
            "eval_map_literal called with no entries".to_string(),
            graphcal_compiler::syntax::span::Span::new(0, 0),
        )
    })?;
    let first_key = first.keys.first();
    let arity = first.keys.len();
    let idx_name = map_entry_index_ref(first_key, ctx);

    if arity == 1 {
        // Single-axis: flat Indexed
        let mut result = IndexMap::new();
        for entry in entries {
            let key = entry.keys.first();
            let variant = map_entry_variant_for_axis(key, &idx_name, ctx)?;
            let val = eval_expr(&entry.value, values, local_values, ctx)?;
            result.insert(variant, val);
        }
        return Ok(RuntimeValue::Indexed {
            index_name: idx_name,
            entries: result,
        });
    }

    // Multi-axis: group by first key, recurse on remaining keys
    let idx_def = map_entry_index_def(first_key, &idx_name, ctx).ok_or_else(|| {
        ctx.internal_error(format!("unknown index `{idx_name}`"), first_key.index.span)
    })?;
    let variants = idx_def.variants();

    let mut outer = IndexMap::new();
    for variant in &variants {
        // Collect entries whose first key matches this variant, stripping the first key.
        // Module-aware refs compare the resolved owning index; the variant leaf
        // is only the member within that resolved index.
        let mut sub_entries: Vec<MapEntry> = Vec::new();
        for entry in entries {
            let first_entry_key = entry.keys.first();
            if map_entry_variant_for_axis(first_entry_key, &idx_name, ctx)? != *variant {
                continue;
            }
            let keys =
                NonEmpty::try_from_vec(entry.keys.as_slice()[1..].to_vec()).map_err(|_| {
                    ctx.internal_error(
                        "multi-axis map literal entry lost all index keys".to_string(),
                        entry.value.span,
                    )
                })?;
            sub_entries.push(MapEntry {
                keys,
                value: entry.value.clone(),
            });
        }

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
#[expect(
    clippy::too_many_lines,
    reason = "for-comprehension evaluation handles cartesian products, filters, and result construction"
)]
pub(super) fn eval_for_comp(
    bindings: &[graphcal_compiler::desugar::resolved_ast::ForBinding],
    body: &Expr,
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Result<RuntimeValue, GraphcalError> {
    use graphcal_compiler::desugar::resolved_ast::ForBindingIndex;

    let binding = &bindings[0];

    // Resolve the index name and get the index definition
    let (idx_name, error_span, dynamic_nat_size, resolved_index) = match &binding.index {
        ForBindingIndex::Named(spanned) => {
            let resolved = resolved_for_binding_index(ctx, spanned.span).cloned();
            (
                resolved.as_ref().map_or_else(
                    || index_ref_from_path(ctx, &spanned.value),
                    |resolved| IndexTypeRef::from_resolved(resolved.clone()),
                ),
                spanned.span,
                None,
                resolved,
            )
        }
        ForBindingIndex::Range { arg, span } => {
            let size = eval_nat_expr(arg, local_values, ctx)?;
            let idx_name = graphcal_compiler::syntax::names::IndexName::new(
                graphcal_compiler::registry::types::nat_range_index_name(size),
            );
            (
                IndexTypeRef::from_resolved(
                    graphcal_compiler::registry::types::nat_range_resolved_index_name(idx_name),
                ),
                *span,
                Some(size),
                None,
            )
        }
    };

    // For nat range indexes from generic Nat arithmetic (e.g., range(N + 1)),
    // the concrete size may not have been registered at compile time.
    // Create a temporary IndexDef for the computed size if not in the registry.
    let dynamic_nat_def;
    let idx_def = if let Some(resolved) = &resolved_index {
        resolved_collection_refs(ctx)
            .and_then(|refs| refs.index_defs.get(resolved))
            .ok_or_else(|| {
                ctx.internal_error(
                    format!(
                        "resolved index `{resolved}` has no recorded definition in collection refs"
                    ),
                    error_span,
                )
            })?
    } else if let Some(def) = ctx.registry.indexes.get_index(idx_name.as_str()) {
        def
    } else if let Some(size) = dynamic_nat_size {
        let size = usize::try_from(size).map_err(|_| {
            ctx.eval_error(
                format!("nat range size {size} does not fit in usize on this target"),
                error_span,
            )
        })?;
        dynamic_nat_def = graphcal_compiler::registry::types::IndexDef {
            name: idx_name.to_unowned_name(),
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
        inner_locals.insert(binding.var.value.as_str().to_owned(), binding_value);
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
