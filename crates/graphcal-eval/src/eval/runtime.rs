//! Runtime evaluation: converting TIR execution results to Values,
//! evaluating unfold expressions, running execution plans, and checking asserts.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_syntax::ast::ExprKind;
use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{DeclName, IndexName, VariantName};
use graphcal_syntax::span::Span;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::dim_check::DeclaredType;
use crate::error::GraphcalError;
use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::registry::Registry;
use crate::resolve::DeclCategory;

use super::display::{attach_display_units, format_range_step};
use super::project::resolve_field_declared_type;
use super::types::{AssertResult, DeclType, EvalResult, NodeError, Value};

pub(super) fn runtime_to_value(
    rv: &RuntimeValue,
    declared_type: Option<&DeclaredType>,
    registry: &Registry,
) -> Value {
    match rv {
        RuntimeValue::Scalar(si_value) => {
            let dimension = match declared_type {
                Some(DeclaredType::Scalar(d)) => d.clone(),
                _ => Dimension::dimensionless(),
            };
            Value::Scalar {
                si_value: *si_value,
                dimension,
                display_unit: None,
            }
        }
        RuntimeValue::Bool(b) => Value::Bool(*b),
        RuntimeValue::Int(i) => Value::Int(*i),
        RuntimeValue::Label {
            index_name,
            variant,
        } => Value::Label {
            index_name: index_name.clone(),
            variant: variant.clone(),
        },
        RuntimeValue::Struct {
            type_name,
            variant,
            fields,
        } => {
            let type_def = registry.types.get_type(type_name.as_str());
            let variant_def = type_def.and_then(|td| td.get_variant(variant.as_str()));

            // Build a substitution map from generic param names to concrete DeclaredTypes
            // when we have concrete type args from the declared type.
            let generic_sub: HashMap<&str, &DeclaredType> =
                if let (Some(td), Some(DeclaredType::Struct(_, type_args))) =
                    (type_def, declared_type)
                {
                    td.generic_params
                        .iter()
                        .zip(type_args.iter())
                        .map(|(param, arg)| (param.name.as_str(), arg))
                        .collect()
                } else {
                    HashMap::new()
                };

            let converted_fields = fields
                .iter()
                .map(|(field_name, field_rv)| {
                    let field_declared = variant_def.and_then(|vd| {
                        vd.fields
                            .iter()
                            .find(|f| f.name == *field_name)
                            .and_then(|f| resolve_field_declared_type(f, &generic_sub, registry))
                    });
                    let val = runtime_to_value(field_rv, field_declared.as_ref(), registry);
                    (field_name.clone(), val)
                })
                .collect();
            Value::Struct {
                type_name: type_name.clone(),
                variant: variant.clone(),
                fields: converted_fields,
            }
        }
        RuntimeValue::Indexed {
            index_name,
            entries,
        } => {
            let element_declared = match declared_type {
                Some(DeclaredType::Indexed { element, .. }) => Some(element.as_ref()),
                _ => None,
            };
            // For range indexes, replace synthetic #N keys with formatted display values.
            let idx_def = registry.indexes.get_index(index_name.as_str());
            let converted_entries = entries
                .iter()
                .enumerate()
                .map(|(i, (variant, entry_rv))| {
                    let display_key = match idx_def {
                        Some(def) if def.is_range() => VariantName::new(format_range_step(def, i)),
                        _ => variant.clone(),
                    };
                    let val = runtime_to_value(entry_rv, element_declared, registry);
                    (display_key, val)
                })
                .collect();
            Value::Indexed {
                index_name: index_name.clone(),
                entries: converted_entries,
            }
        }
        RuntimeValue::RangeLabel { value, .. } => {
            // RangeLabel is an intermediate value used during unfold evaluation;
            // it should never appear in final output. Return a fallback scalar.
            debug_assert!(false, "RangeLabel should not appear in final values");
            Value::Scalar {
                si_value: *value,
                dimension: Dimension::dimensionless(),
                display_unit: None,
            }
        }
    }
}

/// Evaluate an `Unfold` expression: `unfold(init, |prev_i, i| body)`.
///
/// This builds up results incrementally over a range index, inserting partial
/// results into `values` so that `@self_name[prev_i]` resolves correctly.
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
#[expect(
    clippy::needless_range_loop,
    reason = "loop index i is used for step_value(i), step_index fields, and variant indexing"
)]
pub(super) fn eval_unfold(
    self_name: &str,
    init: &graphcal_syntax::ast::Expr,
    prev_name: &graphcal_syntax::ast::Ident,
    curr_name: &graphcal_syntax::ast::Ident,
    body: &graphcal_syntax::ast::Expr,
    values: &mut HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    registry: &Registry,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    // Find the range index from the node's declared type
    let declared = declared_types
        .get(self_name)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("no declared type for node `{self_name}`"),
            src: src.clone(),
            span: (0, 0).into(),
        })?;
    let index_name = match declared {
        DeclaredType::Indexed { index, .. } => index.clone(),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("node `{self_name}` must have an indexed type for time scan"),
                src: src.clone(),
                span: (0, 0).into(),
            });
        }
    };
    let idx_def = registry
        .indexes
        .get_index(index_name.as_str())
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("unknown index `{index_name}`"),
            src: src.clone(),
            span: (0, 0).into(),
        })?;

    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    // Evaluate init expression
    let init_val = eval_expr(
        init,
        values,
        &empty_locals,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;

    // Build results incrementally
    let mut result_entries: IndexMap<VariantName, RuntimeValue> = IndexMap::new();

    // Step 0: init value
    result_entries.insert(variants[0].clone(), init_val);

    // Steps 1..N: evaluate body with prev_t and t bindings
    for i in 1..step_count {
        // Insert partial result so @self[prev_t] can resolve
        values.insert(
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
                src: src.clone(),
                span: (0, 0).into(),
            })?;
        let curr_value = idx_def
            .step_value(i)
            .map_err(|e| GraphcalError::EvalError {
                message: format!("internal: range index step {i} out of bounds: {e}"),
                src: src.clone(),
                span: (0, 0).into(),
            })?;

        let mut scan_locals = HashMap::new();
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

        let body_val = eval_expr(
            body,
            values,
            &scan_locals,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        result_entries.insert(variants[i].clone(), body_val);
    }

    // Remove the partial value we inserted
    values.remove(self_name);

    Ok(RuntimeValue::Indexed {
        index_name,
        entries: result_entries,
    })
}

/// Evaluate using TIR + `ExecPlan` (new linear pipeline).
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
#[expect(
    clippy::too_many_lines,
    reason = "linear evaluation pipeline is clearest as a single function"
)]
pub(super) fn evaluate_plan(
    tir: &crate::tir::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<String, crate::dim_check::DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> EvalResult {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut values: HashMap<String, RuntimeValue> = HashMap::new();
    let mut errors: HashMap<String, NodeError> = HashMap::new();

    // Insert imported values into the lookup table (pre-evaluated by dependency files).
    // ScopedName → String: the runtime values map uses flat strings because it
    // merges imported, const, and locally computed values into a single namespace.
    for (name, val) in &plan.imported_values {
        values.insert(name.to_string(), val.clone());
    }

    // Insert const values into the lookup table
    for (name, val) in &plan.const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them)
    for name in &plan.topo_order {
        if values.contains_key(name) {
            continue;
        }

        // Check if any dependency has failed
        let failed_deps: Vec<DeclName> = tir
            .runtime_deps
            .get(name)
            .map(|deps| {
                deps.iter()
                    .filter(|dep| errors.contains_key(*dep))
                    .map(DeclName::new)
                    .collect()
            })
            .unwrap_or_default();

        if !failed_deps.is_empty() {
            errors.insert(name.clone(), NodeError::DependencyFailed { failed_deps });
            continue;
        }

        let expr = &plan.expressions[name];

        // Unfold requires special handling: it needs to build up results
        // incrementally and insert partial results into `values` so that
        // @self[prev_i] can resolve during body evaluation.
        let result = if let ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } = &expr.kind
        {
            eval_unfold(
                name,
                init,
                prev_name,
                curr_name,
                body,
                &mut values,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                declared_types,
                src,
            )
        } else {
            eval_expr(
                expr,
                &values,
                &empty_locals,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                src,
            )
        };

        match result {
            Ok(val) => {
                values.insert(name.clone(), val);
            }
            Err(e) => {
                let message = match &e {
                    GraphcalError::EvalError { message, .. } => message.clone(),
                    other => format!("{other}"),
                };
                errors.insert(name.clone(), NodeError::EvalFailed { message });
            }
        }
    }

    // Build a map from name -> expression for display unit extraction
    let expr_map: HashMap<&str, &graphcal_syntax::ast::Expr> = tir
        .consts
        .iter()
        .chain(tir.params.iter())
        .chain(tir.nodes.iter())
        .map(|(name, _, expr, _)| (name.as_str(), expr))
        .collect();

    let make_value = |name: &str, rv: &RuntimeValue| -> Value {
        let mut value = runtime_to_value(rv, declared_types.get(name), &tir.registry);
        if let Some(expr) = expr_map.get(name) {
            attach_display_units(&mut value, expr, &tir.registry);
        }
        value
    };

    let make_result = |name: &str| -> Result<Value, NodeError> {
        errors.get(name).map_or_else(
            || Ok(make_value(name, &values[name])),
            |err| Err(err.clone()),
        )
    };

    let consts = tir
        .consts
        .iter()
        .map(|(name, _, _, _)| {
            let val = make_value(name, &plan.const_values[name]);
            (DeclName::new(name), val)
        })
        .collect();
    let params = tir
        .params
        .iter()
        .map(|(name, _, _, _)| (DeclName::new(name), make_result(name)))
        .collect();
    let nodes = tir
        .nodes
        .iter()
        .map(|(name, _, _, _)| (DeclName::new(name), make_result(name)))
        .collect();

    let all = tir
        .source_order
        .iter()
        .filter_map(|(name, cat)| {
            let decl_type = match cat {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
                DeclCategory::Assert => return None,
            };
            let result = match cat {
                DeclCategory::Const => Ok(make_value(name, &plan.const_values[name])),
                DeclCategory::Param | DeclCategory::Node => make_result(name),
                DeclCategory::Assert => return None,
            };
            Some((DeclName::new(name), result, decl_type))
        })
        .collect();

    // Evaluate assertions in source order
    let assertions: Vec<(DeclName, AssertResult, Span)> = plan
        .assert_bodies
        .iter()
        .map(|(name, body, span)| {
            let assert_result = evaluate_assert_body(
                body,
                &values,
                &empty_locals,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                src,
            );
            (DeclName::new(name), assert_result, *span)
        })
        .collect();

    EvalResult {
        consts,
        params,
        nodes,
        all,
        assertions,
        assumes_map: plan.assumes_map.clone(),
        base_dim_symbols: tir.registry.dimensions.base_dim_symbols().clone(),
    }
}

/// Recursively check an indexed assertion value (possibly multi-dimensional).
///
/// For single-index: `Bool[Mode]` — entries are `Bool` values.
/// For multi-index: `Bool[Phase, Maneuver]` — entries are nested `Indexed` values.
///
/// Single-index failure message example:
///   `failed at Mode::Boost`
/// Multi-index failure message example:
///   `failed at (Phase::Launch, Maneuver::Correction), (Phase::Cruise, Maneuver::Insertion)`
pub(super) fn check_indexed_assert(
    index_name: &IndexName,
    entries: &IndexMap<VariantName, RuntimeValue>,
) -> AssertResult {
    match collect_failing_paths(index_name, entries) {
        Ok(paths) if paths.is_empty() => AssertResult::Pass,
        Ok(paths) => {
            let is_multi_index = paths.iter().any(|p| p.len() > 1);
            let formatted: Vec<String> = if is_multi_index {
                paths
                    .iter()
                    .map(|p| format!("({})", p.join(", ")))
                    .collect()
            } else {
                paths.iter().map(|p| p[0].clone()).collect()
            };
            AssertResult::Fail {
                message: format!("failed at {}", formatted.join(", ")),
            }
        }
        Err(msg) => AssertResult::Error { message: msg },
    }
}

/// Recursively collect failing variant paths from an indexed assertion value.
///
/// Each path is a `Vec<String>` of variant labels from outermost to innermost index.
/// For example, `vec!["Phase::Launch", "Maneuver::Correction"]` for a 2D failure.
fn collect_failing_paths(
    index_name: &IndexName,
    entries: &IndexMap<VariantName, RuntimeValue>,
) -> Result<Vec<Vec<String>>, String> {
    let mut paths = Vec::new();
    for (variant, value) in entries {
        let label = format!("{index_name}::{variant}");
        match value {
            RuntimeValue::Bool(true) => {}
            RuntimeValue::Bool(false) => {
                paths.push(vec![label]);
            }
            RuntimeValue::Indexed {
                index_name: inner_index,
                entries: inner_entries,
            } => {
                // Recurse into nested dimension, prepending current variant to each path
                for mut inner_path in collect_failing_paths(inner_index, inner_entries)? {
                    inner_path.insert(0, label.clone());
                    paths.push(inner_path);
                }
            }
            other => {
                return Err(format!(
                    "expected Bool for {index_name}::{variant}, got {other:?}"
                ));
            }
        }
    }
    Ok(paths)
}

/// Evaluate a single assert body and return an `AssertResult`.
#[expect(
    clippy::too_many_lines,
    reason = "tolerance evaluation has multiple eval_expr calls and error handling"
)]
pub(super) fn evaluate_assert_body(
    body: &graphcal_syntax::ast::AssertBody,
    values: &HashMap<String, RuntimeValue>,
    local_values: &HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> AssertResult {
    match body {
        graphcal_syntax::ast::AssertBody::Expr(body_expr) => {
            match eval_expr(
                body_expr,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            ) {
                Ok(RuntimeValue::Bool(true)) => AssertResult::Pass,
                Ok(RuntimeValue::Bool(false)) => AssertResult::Fail {
                    message: "assertion evaluated to false".to_string(),
                },
                Ok(RuntimeValue::Indexed {
                    index_name,
                    entries,
                }) => check_indexed_assert(&index_name, &entries),
                Ok(other) => AssertResult::Error {
                    message: format!("expected Bool, got {other:?}"),
                },
                Err(e) => AssertResult::Error {
                    message: format!("{e}"),
                },
            }
        }
        graphcal_syntax::ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
        } => {
            let actual_val = match eval_expr(
                actual,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            ) {
                Ok(RuntimeValue::Scalar(v)) => v,
                Ok(other) => {
                    return AssertResult::Error {
                        message: format!("expected scalar actual, got {other:?}"),
                    };
                }
                Err(e) => {
                    return AssertResult::Error {
                        message: format!("{e}"),
                    };
                }
            };
            let expected_val = match eval_expr(
                expected,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            ) {
                Ok(RuntimeValue::Scalar(v)) => v,
                Ok(other) => {
                    return AssertResult::Error {
                        message: format!("expected scalar expected, got {other:?}"),
                    };
                }
                Err(e) => {
                    return AssertResult::Error {
                        message: format!("{e}"),
                    };
                }
            };
            let tolerance_val = match eval_expr(
                tolerance,
                values,
                local_values,
                builtin_consts,
                builtin_fns,
                registry,
                src,
            ) {
                Ok(RuntimeValue::Scalar(v)) => v,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "tolerance values are small integers"
                )]
                Ok(RuntimeValue::Int(i)) => i as f64,
                Ok(other) => {
                    return AssertResult::Error {
                        message: format!("expected scalar tolerance, got {other:?}"),
                    };
                }
                Err(e) => {
                    return AssertResult::Error {
                        message: format!("{e}"),
                    };
                }
            };

            let delta = (actual_val - expected_val).abs();
            let limit = if *is_relative {
                expected_val.abs() * tolerance_val / 100.0
            } else {
                tolerance_val
            };

            if delta <= limit {
                AssertResult::Pass
            } else {
                let tol_display = if *is_relative {
                    format!("{tolerance_val}%")
                } else {
                    format!("{tolerance_val}")
                };
                AssertResult::Fail {
                    message: format!(
                        "actual {actual_val}, expected {expected_val} +/- {tol_display}, off by {delta}"
                    ),
                }
            }
        }
    }
}
