use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::{Expr, MapEntry};
use graphcal_syntax::names::VariantName;

use crate::builtins::BuiltinFunction;
use crate::error::GraphcalError;
use crate::registry::Registry;
use crate::runtime_value::RuntimeValue;

use super::eval_expr;

/// Evaluate a map/table literal expression.
pub(super) fn eval_map_or_table(
    entries: &[MapEntry],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_map_literal(
        entries,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )
}

/// Evaluate a `for` comprehension expression.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through evaluation context to recursive calls"
)]
pub(super) fn eval_for_comp_expr(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    eval_for_comp(
        bindings,
        body,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )
}

/// Evaluate an index access expression.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through evaluation context"
)]
pub(super) fn eval_index_access(
    expr: &Expr,
    inner: &Expr,
    args: &[graphcal_syntax::ast::IndexArg],
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let mut current = eval_expr(
        inner,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
    for arg in args {
        let RuntimeValue::Indexed { entries, .. } = current else {
            return Err(GraphcalError::EvalError {
                message: "indexing a non-indexed value".to_string(),
                src: src.clone(),
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
                            src: src.clone(),
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
                            src: src.clone(),
                            span: ident.span.into(),
                        });
                    }
                }
            }
        };
        current = entries.get(variant_name.as_str()).cloned().ok_or_else(|| {
            GraphcalError::EvalError {
                message: format!("variant `{variant_name}` not found"),
                src: src.clone(),
                span: expr.span.into(),
            }
        })?;
    }
    Ok(current)
}

/// Evaluate a `scan` expression.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through evaluation context"
)]
pub(super) fn eval_scan(
    source: &Expr,
    init: &Expr,
    acc_name: &graphcal_syntax::ast::Ident,
    val_name: &graphcal_syntax::ast::Ident,
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let source_val = eval_expr(
        source,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;
    let RuntimeValue::Indexed {
        index_name,
        entries: source_entries,
    } = source_val
    else {
        return Err(GraphcalError::EvalError {
            message: "scan source must be an indexed value".to_string(),
            src: src.clone(),
            span: source.span.into(),
        });
    };
    let init_val = eval_expr(
        init,
        values,
        local_values,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;

    let mut acc = init_val;
    let mut result_entries = IndexMap::new();
    for (variant, val) in &source_entries {
        let mut scan_locals = local_values.clone();
        scan_locals.insert(acc_name.name.clone(), acc);
        scan_locals.insert(val_name.name.clone(), val.clone());
        let body_val = eval_expr(
            body,
            values,
            &scan_locals,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        result_entries.insert(variant.clone(), body_val.clone());
        acc = body_val;
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
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let arity = entries[0].keys.len();
    let idx_name = entries[0].keys[0].index.value.clone();

    if arity == 1 {
        // Single-axis: flat Indexed
        let mut result = IndexMap::new();
        for entry in entries {
            let val = eval_expr(
                &entry.value,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?;
            result.insert(entry.keys[0].variant.value.clone(), val);
        }
        return Ok(RuntimeValue::Indexed {
            index_name: idx_name,
            entries: result,
        });
    }

    // Multi-axis: group by first key, recurse on remaining keys
    let idx_def = registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("unknown index `{idx_name}`"),
            src: src.clone(),
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

        let inner = eval_map_literal(
            &sub_entries,
            values,
            local_values,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
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
    clippy::too_many_arguments,
    reason = "passes through evaluation context to recursive calls"
)]
fn eval_for_comp(
    bindings: &[graphcal_syntax::ast::ForBinding],
    body: &Expr,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    let binding = &bindings[0];
    let idx_name = binding.index.value.clone();
    let idx_def = registry
        .indexes
        .get_index(idx_name.as_str())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("unknown index `{idx_name}`"),
            src: src.clone(),
            span: binding.index.span.into(),
        })?;
    let remaining = &bindings[1..];

    let variants = idx_def.variants();
    let mut entries = IndexMap::new();
    for variant in &variants {
        let mut inner_locals = local_values.clone();
        let binding_value = match &idx_def.kind {
            crate::registry::IndexKind::Named { .. } => RuntimeValue::Label {
                index_name: idx_name.clone(),
                variant: variant.clone(),
            },
            crate::registry::IndexKind::Range { .. } => {
                let step_index = variant
                    .as_str()
                    .strip_prefix('#')
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "range variant `{variant}` has unexpected format (expected #N)"
                        ),
                        src: src.clone(),
                        span: binding.index.span.into(),
                    })?;
                RuntimeValue::RangeLabel {
                    step_index,
                    value: idx_def.step_value(step_index).map_err(|e| {
                        GraphcalError::InternalError {
                            message: format!("range index step {step_index} out of bounds: {e}"),
                            src: src.clone(),
                            span: binding.index.span.into(),
                        }
                    })?,
                }
            }
        };
        inner_locals.insert(binding.var.name.clone(), binding_value);
        let val = if remaining.is_empty() {
            eval_expr(
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        } else {
            eval_for_comp(
                remaining,
                body,
                values,
                &inner_locals,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            )?
        };
        entries.insert(variant.clone(), val);
    }
    Ok(RuntimeValue::Indexed {
        index_name: idx_name,
        entries,
    })
}
