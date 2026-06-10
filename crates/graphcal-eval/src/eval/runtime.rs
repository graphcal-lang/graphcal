//! Runtime evaluation: converting TIR execution results to Values,
//! running execution plans, and checking asserts.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use graphcal_compiler::syntax::dimension::Dimension;
use graphcal_compiler::syntax::names::{DeclName, IndexVariantName, ScopedName};
use graphcal_compiler::syntax::span::Span;

use crate::decl_key::RuntimeDeclKey;
use crate::eval_expr::{
    EvalContext, HirLocalValueMap, RuntimeValue, RuntimeValueMap, UnfoldContext, eval_expr,
    eval_hir_expr,
};
use graphcal_compiler::ir::resolve::{DeclCategory, ExpectedFail, ExpectedFailKey};
use graphcal_compiler::registry::builtins::{
    BuiltinFunction, builtin_constants, builtin_functions,
};
use graphcal_compiler::registry::declared_type::{DeclaredType, IndexTypeRef, StructTypeRef};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::Registry;

use super::display::{attach_display_units, extract_flat_display_unit, format_range_step};
use super::project::resolve_field_declared_type;
use super::types::{
    AssertResult, AxisMeta, DeclType, EvalResult, NodeError, PlotFieldValue, PlotSpec, Value,
};

const fn declared_label_index_ref(declared_type: Option<&DeclaredType>) -> Option<&IndexTypeRef> {
    match declared_type {
        Some(DeclaredType::Label(index)) => Some(index),
        _ => None,
    }
}

const fn declared_indexed_index_ref(declared_type: Option<&DeclaredType>) -> Option<&IndexTypeRef> {
    match declared_type {
        Some(DeclaredType::Indexed { index, .. }) => Some(index),
        _ => None,
    }
}

const fn declared_struct_type_ref(declared_type: Option<&DeclaredType>) -> Option<&StructTypeRef> {
    match declared_type {
        Some(DeclaredType::Struct(type_name, _)) => Some(type_name),
        _ => None,
    }
}

fn merge_index_ref_owner(
    runtime_ref: &IndexTypeRef,
    declared_ref: Option<&IndexTypeRef>,
) -> IndexTypeRef {
    let _ = declared_ref;
    runtime_ref.clone()
}

fn merge_struct_ref_owner(
    runtime_ref: &StructTypeRef,
    declared_ref: Option<&StructTypeRef>,
) -> StructTypeRef {
    let _ = declared_ref;
    runtime_ref.clone()
}

fn public_label_index_ref(
    runtime_ref: &IndexTypeRef,
    declared_type: Option<&DeclaredType>,
) -> IndexTypeRef {
    merge_index_ref_owner(runtime_ref, declared_label_index_ref(declared_type))
}

fn public_indexed_index_ref(
    runtime_ref: &IndexTypeRef,
    declared_type: Option<&DeclaredType>,
) -> IndexTypeRef {
    merge_index_ref_owner(runtime_ref, declared_indexed_index_ref(declared_type))
}

fn public_struct_type_ref(
    runtime_ref: &StructTypeRef,
    declared_type: Option<&DeclaredType>,
) -> StructTypeRef {
    merge_struct_ref_owner(runtime_ref, declared_struct_type_ref(declared_type))
}

#[expect(
    clippy::too_many_lines,
    reason = "runtime value conversion mirrors all value variants"
)]
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
            index_name: public_label_index_ref(index_name, declared_type),
            variant: variant.clone(),
        },
        RuntimeValue::Struct { type_name, fields } => {
            let public_type_name = public_struct_type_ref(type_name, declared_type);
            let registry_type_name = declared_struct_type_ref(declared_type).unwrap_or(type_name);
            let type_def = registry.types.get_type(registry_type_name.as_str());

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
                    let field_declared = type_def.and_then(|td| {
                        td.fields()
                            .iter()
                            .find(|f| f.name == *field_name)
                            .and_then(|f| resolve_field_declared_type(f, &generic_sub, registry))
                    });
                    let val = runtime_to_value(field_rv, field_declared.as_ref(), registry);
                    (field_name.clone(), val)
                })
                .collect();
            Value::Struct {
                type_name: public_type_name,
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
            // For range indexes, keep semantic #N keys in the public value and
            // carry formatted step labels as presentation metadata. This keeps
            // display strings at I/O boundaries instead of fabricating variant
            // leaves like `0.5 s`.
            let idx_def = index_name
                .declared_name()
                .and_then(|name| registry.indexes.get_index(name.as_str()));
            let entry_display_names = idx_def.filter(|def| def.is_range()).map(|def| {
                entries
                    .keys()
                    .enumerate()
                    .map(|(i, variant)| (variant.clone(), format_range_step(def, i)))
                    .collect()
            });
            let converted_entries = entries
                .iter()
                .map(|(variant, entry_rv)| {
                    let val = runtime_to_value(entry_rv, element_declared, registry);
                    (variant.clone(), val)
                })
                .collect();
            Value::Indexed {
                index_name: public_indexed_index_ref(index_name, declared_type),
                entries: converted_entries,
                entry_display_names,
            }
        }
        RuntimeValue::RangeLabel { value, .. } => {
            // RangeLabel is an intermediate value used during range-index
            // iteration, but it can surface in final output when a body
            // returns its loop variable (e.g. `for i: Step { i }`). Expose it
            // as a plain scalar, consistent with `expect_scalar` which
            // already treats it as one.
            let dimension = match declared_type {
                Some(DeclaredType::Scalar(d)) => d.clone(),
                _ => Dimension::dimensionless(),
            };
            Value::Scalar {
                si_value: *value,
                dimension,
                display_unit: None,
            }
        }
        RuntimeValue::Datetime(epoch) => {
            let time_scale = match declared_type {
                Some(DeclaredType::Datetime(s)) => *s,
                _ => graphcal_compiler::registry::time_scale::TimeScale::UTC,
            };
            Value::Datetime {
                epoch: *epoch,
                time_scale,
                display_tz: None,
            }
        }
    }
}

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub(super) struct EvalLoopResult {
    pub values: RuntimeValueMap,
    pub errors: HashMap<RuntimeDeclKey, NodeError>,
}

/// Core evaluation loop shared by `evaluate_plan` and `extract_runtime_values`.
///
/// Inserts imported and const values, then iterates in topological order.
/// Unfold expressions are handled inline by `eval_expr` via `EvalContext`.
/// Domain constraints are checked after successful evaluation.
///
/// Returns all computed values and any per-node errors.
pub(super) fn run_eval_loop(
    plan: &crate::exec_plan::ExecPlan,
    tir: &graphcal_compiler::tir::typed::TIR,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    src: &NamedSource<Arc<String>>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, BuiltinFunction>,
) -> EvalLoopResult {
    let empty_hir_locals = HirLocalValueMap::new();

    let mut values: RuntimeValueMap = HashMap::new();
    let mut errors: HashMap<RuntimeDeclKey, NodeError> = HashMap::new();

    // Insert imported values into the lookup table (pre-evaluated by dependency files).
    // Imported values keep their original `ScopedName` qualification.
    for (name, val) in &plan.imported_values {
        values.insert(name.clone(), val.clone());
    }

    // Insert const values into the lookup table
    for (name, val) in &plan.const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them).
    // Top-level declarations in a single file are always `Local`-form names.
    for name in &plan.topo_order {
        let name_str = name.member().to_string();
        if values.contains_key(name) {
            continue;
        }

        // Check if any local runtime dependency has failed. Module-aware TIRs
        // carry canonical dependency identities; use those when present so a
        // qualified imported dependency with the same leaf as a local failure
        // cannot be mistaken for the local declaration.
        let failed_deps = failed_runtime_dependencies(tir.root(), name, &errors);

        if !failed_deps.is_empty() {
            errors.insert(name.clone(), NodeError::DependencyFailed { failed_deps });
            continue;
        }

        // Build eval context with unfold support for this node.
        let unfold_ctx = UnfoldContext {
            self_name: &name_str,
            declared_types,
        };
        let ctx = EvalContext {
            builtin_consts,
            builtin_fns,
            registry: &tir.registry,
            src,
            unfold_context: Some(unfold_ctx),
            tir,
            current_dag: Some(tir.root()),
            root_values: Some(&values),
            struct_field_constraints: Some(&plan.struct_field_constraints),
        };

        let result = tir
            .root()
            .semantic
            .expressions
            .runtime_expr(name.as_resolved())
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("semantic TIR missing HIR runtime expression for `{name}`"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })
            .and_then(|hir_expr| eval_hir_expr(hir_expr, &values, &empty_hir_locals, &ctx));

        match result {
            Ok(val) => {
                // Check domain constraints after successful evaluation.
                if let Some(constraint) = plan.domain_constraints.get(name)
                    && let Err(violation) =
                        crate::domain_check::check_domain_constraint(&val, constraint)
                {
                    errors.insert(
                        name.clone(),
                        NodeError::EvalFailed {
                            message: violation.message,
                        },
                    );
                    continue;
                }
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

    EvalLoopResult { values, errors }
}

fn failed_runtime_dependencies(
    dag: &graphcal_compiler::tir::typed::DagTIR,
    name: &RuntimeDeclKey,
    errors: &HashMap<RuntimeDeclKey, NodeError>,
) -> Vec<DeclName> {
    dag.semantic
        .dependencies
        .runtime_deps
        .get(name.as_resolved())
        .map(|deps| {
            deps.iter()
                .filter(|dep| errors.contains_key(&RuntimeDeclKey::resolved((*dep).clone())))
                .map(|dep| DeclName::from_atom(dep.atom().clone()))
                .collect()
        })
        .unwrap_or_default()
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
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> EvalResult {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();
    let empty_hir_locals: HirLocalValueMap = HashMap::new();

    let EvalLoopResult { values, errors } =
        run_eval_loop(plan, tir, declared_types, src, builtin_consts, builtin_fns);

    let ctx = EvalContext {
        builtin_consts,
        builtin_fns,
        registry: &tir.registry,
        src,
        unfold_context: None,
        tir,
        current_dag: Some(tir.root()),
        root_values: Some(&values),
        struct_field_constraints: Some(&plan.struct_field_constraints),
    };

    // Build a map from name -> expression for display unit extraction.
    // Top-level decls are always `Local`-form names.
    let expr_map: HashMap<ScopedName, &graphcal_compiler::desugar::resolved_ast::Expr> = tir
        .root()
        .consts
        .iter()
        .map(|e| (e.name.clone(), &e.expr))
        .chain(
            tir.root()
                .params
                .iter()
                .filter_map(|e| e.default_expr.as_ref().map(|ex| (e.name.clone(), ex))),
        )
        .chain(tir.root().nodes.iter().map(|e| (e.name.clone(), &e.expr)))
        .collect();

    let local_key = |name: &ScopedName| RuntimeDeclKey::for_local_decl(tir.root(), name);

    let make_value = |name: &ScopedName, rv: &RuntimeValue| -> Value {
        let mut value = runtime_to_value(rv, declared_types.get(name), &tir.registry);
        if let Some(expr) = expr_map.get(name) {
            attach_display_units(&mut value, expr, &ctx, &values);
        }
        value
    };

    let make_result = |name: &ScopedName| -> Result<Value, NodeError> {
        let key = local_key(name);
        errors.get(&key).map_or_else(
            || Ok(make_value(name, &values[&key])),
            |err| Err(err.clone()),
        )
    };

    let consts = tir
        .root()
        .consts
        .iter()
        .map(|e| {
            let key = local_key(&e.name);
            let val = make_value(&e.name, &plan.const_values[&key]);
            (DeclName::new(e.name.member()), val)
        })
        .collect();
    let params = tir
        .root()
        .params
        .iter()
        .map(|e| (DeclName::new(e.name.member()), make_result(&e.name)))
        .collect();
    let nodes = tir
        .root()
        .nodes
        .iter()
        .map(|e| (DeclName::new(e.name.member()), make_result(&e.name)))
        .collect();

    let all = tir
        .root()
        .source_order
        .iter()
        .filter_map(|(name, cat)| {
            let decl_type = match cat {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => return None,
            };
            let result = match cat {
                DeclCategory::Const => {
                    let key = local_key(name);
                    Ok(make_value(name, &plan.const_values[&key]))
                }
                DeclCategory::Param | DeclCategory::Node => make_result(name),
                DeclCategory::Assert
                | DeclCategory::Plot
                | DeclCategory::Figure
                | DeclCategory::Layer => return None,
            };
            Some((DeclName::new(name.member()), result, decl_type))
        })
        .collect();

    // Evaluate assertions in source order, applying expected_fail inversion
    let assertions: Vec<(DeclName, AssertResult, Span)> = plan
        .assert_bodies
        .iter()
        .map(|entry| {
            let ef = plan.expected_fail.get(&entry.name);
            let assert_result = evaluate_assert_with_expected_fail(
                &entry.body,
                ef,
                &values,
                &empty_hir_locals,
                &ctx,
            );
            (
                DeclName::new(entry.name.member()),
                assert_result,
                entry.span,
            )
        })
        .collect();

    // Evaluate plot declarations
    let plots: Vec<PlotSpec> = plan
        .plot_bodies
        .iter()
        .filter_map(|entry| {
            evaluate_plot(
                &entry.decl,
                &entry.name,
                entry.is_pub,
                &values,
                &empty_locals,
                &ctx,
                declared_types,
            )
        })
        .collect();

    // Evaluate figure declarations
    let figures: Vec<super::types::FigureSpec> = plan
        .figure_bodies
        .iter()
        .map(|entry| {
            let (properties, plot_names) = eval_composition_fields(
                &entry.decl.fields,
                &entry.decl.plot_names,
                &values,
                &empty_locals,
                &ctx,
            );
            super::types::FigureSpec {
                name: entry.name.clone(),
                plot_names,
                properties,
            }
        })
        .collect();

    // Evaluate layer declarations
    let layers: Vec<super::types::LayerSpec> = plan
        .layer_bodies
        .iter()
        .map(|entry| {
            let (properties, plot_names) = eval_composition_fields(
                &entry.decl.fields,
                &entry.decl.plot_names,
                &values,
                &empty_locals,
                &ctx,
            );
            super::types::LayerSpec {
                name: entry.name.clone(),
                plot_names,
                properties,
            }
        })
        .collect();

    let domain_constraints: HashMap<DeclName, _> = plan
        .domain_constraints
        .iter()
        .map(|(k, v)| (k.to_decl_name(), v.clone()))
        .collect();
    let assumes_map: HashMap<DeclName, Vec<DeclName>> = plan
        .assumes_map
        .iter()
        .map(|(k, v)| {
            (
                DeclName::new(k.member()),
                v.iter().map(|n| DeclName::new(n.member())).collect(),
            )
        })
        .collect();

    EvalResult {
        consts,
        params,
        nodes,
        all,
        assertions,
        plots,
        figures,
        layers,
        assumes_map,
        base_dim_symbols: tir.registry.dimensions.base_dim_symbols().clone(),
        domain_constraints,
    }
}

/// Evaluate an assertion body with optional `#[expected_fail]` handling.
///
/// For `None` (no `expected_fail`): evaluate and return the result as-is.
/// For `Some(ExpectedFail::All)`: invert the final result (Pass↔Fail).
/// For `Some(ExpectedFail::Variants(keys))`: evaluate the expression to get
/// the raw indexed `RuntimeValue`, invert only the matching variant entries,
/// then aggregate.
fn evaluate_assert_with_expected_fail(
    body: &graphcal_compiler::hir::AssertBody,
    ef: Option<&ExpectedFail>,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap,
    ctx: &EvalContext<'_>,
) -> AssertResult {
    match ef {
        None => evaluate_assert_body(body, values, local_values, ctx),
        Some(ExpectedFail::All) => {
            let result = evaluate_assert_body(body, values, local_values, ctx);
            match result {
                AssertResult::Pass => AssertResult::Fail {
                    message: "assertion passed but was marked #[expected_fail]".to_string(),
                },
                AssertResult::Fail { .. } => AssertResult::Pass,
                AssertResult::Error { .. } => result,
            }
        }
        Some(ExpectedFail::Variants(keys)) => {
            // Per-variant: we need the raw RuntimeValue to invert specific entries.
            // Only Expr-based assertions can be indexed; Tolerance assertions are scalar.
            let graphcal_compiler::hir::AssertBody::Expr(body_expr) = body else {
                // Tolerance assertions cannot be indexed, so Variants makes no sense.
                // The resolver should have caught this, but be safe.
                return AssertResult::Error {
                    message: "per-variant #[expected_fail] on a tolerance assertion".to_string(),
                };
            };
            match eval_hir_expr(body_expr, values, local_values, ctx) {
                Ok(RuntimeValue::Indexed {
                    index_name,
                    entries,
                }) => {
                    let inverted = invert_indexed_variants(&index_name, entries, keys);
                    check_indexed_assert_with_expected_fail(&inverted.0, &inverted.1, keys)
                }
                Ok(RuntimeValue::Bool(_)) => AssertResult::Error {
                    message:
                        "invalid compiled plan: per-variant #[expected_fail(...)] on a non-indexed assertion"
                            .to_string(),
                },
                Ok(other) => AssertResult::Error {
                    message: format!("expected Bool or Indexed, got {other:?}"),
                },
                Err(e) => AssertResult::Error {
                    message: format!("{e}"),
                },
            }
        }
    }
}

fn expected_index_key_matches(actual: &IndexTypeRef, expected: &IndexTypeRef) -> bool {
    actual.matches_ref(expected)
}

fn expected_fail_key_matches_path(
    path: &[(IndexTypeRef, IndexVariantName)],
    key: &ExpectedFailKey,
) -> bool {
    path.len() == key.len()
        && path
            .iter()
            .zip(key.iter())
            .all(|((actual_index, actual_variant), expected)| {
                expected_index_key_matches(actual_index, &expected.index)
                    && actual_variant == &expected.variant
            })
}

/// Invert specific variant entries in an indexed `RuntimeValue`.
///
/// For each entry in the indexed value, if the variant key matches one of the
/// expected-fail keys, flip `Bool(true)` → `Bool(false)` and vice versa.
/// For nested indexed values (multi-index), recurse.
fn invert_indexed_variants(
    index_name: &IndexTypeRef,
    entries: IndexMap<IndexVariantName, RuntimeValue>,
    keys: &[ExpectedFailKey],
) -> (IndexTypeRef, IndexMap<IndexVariantName, RuntimeValue>) {
    let inverted_entries = entries
        .into_iter()
        .map(|(variant, value)| {
            let new_value = match value {
                RuntimeValue::Bool(b) => {
                    // Single-index: check if this variant is in any key
                    let should_invert = keys.iter().any(|key| {
                        key.len() == 1
                            && expected_index_key_matches(index_name, &key[0].index)
                            && key[0].variant == variant
                    });
                    if should_invert {
                        RuntimeValue::Bool(!b)
                    } else {
                        RuntimeValue::Bool(b)
                    }
                }
                RuntimeValue::Indexed {
                    index_name: inner_index,
                    entries: inner_entries,
                } => {
                    // Multi-index: filter keys that match the current variant at position 0,
                    // then strip the first element and recurse.
                    let sub_keys: Vec<ExpectedFailKey> = keys
                        .iter()
                        .filter(|key| {
                            key.len() >= 2
                                && expected_index_key_matches(index_name, &key[0].index)
                                && key[0].variant == variant
                        })
                        .map(|key| key[1..].to_vec())
                        .collect();
                    if sub_keys.is_empty() {
                        // No expected-fail keys apply to this subtree — leave as-is
                        RuntimeValue::Indexed {
                            index_name: inner_index,
                            entries: inner_entries,
                        }
                    } else {
                        let (idx, ents) =
                            invert_indexed_variants(&inner_index, inner_entries, &sub_keys);
                        RuntimeValue::Indexed {
                            index_name: idx,
                            entries: ents,
                        }
                    }
                }
                other => other,
            };
            (variant, new_value)
        })
        .collect();
    (index_name.clone(), inverted_entries)
}

/// Format a list of indexed paths for assertion failure messages.
///
/// Each path is a slice of index/variant pairs from outermost to innermost.
/// For single-index paths, formats as `Mode.Boost, Mode.Cruise`.
/// For multi-index paths, formats as `(Phase.Launch, Maneuver.Correction), (Phase.Cruise, Maneuver.Insertion)`.
fn format_indexed_paths(
    paths: &[&[(IndexTypeRef, IndexVariantName)]],
    is_multi_index: bool,
) -> String {
    let formatted: Vec<String> = if is_multi_index {
        paths
            .iter()
            .map(|p| {
                format!(
                    "({})",
                    p.iter()
                        .map(|(idx, var)| format!("{}.{var}", idx.display_name()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect()
    } else {
        paths
            .iter()
            .map(|p| format!("{}.{}", p[0].0.display_name(), p[0].1))
            .collect()
    };
    formatted.join(", ")
}

/// Check an indexed assertion with expected-fail variant awareness.
///
/// After inversion, the semantics are:
/// - A variant matching an expected-fail key that is `true` (was `false` before inversion)
///   means "expected failure occurred" → good.
/// - A variant matching an expected-fail key that is `false` (was `true` before inversion)
///   means "unexpected pass" → report as failure.
/// - A variant NOT matching any key behaves normally (`true` = pass, `false` = fail).
///
/// We reuse `collect_failing_paths` on the inverted entries, then classify each
/// failing path as either "unexpected pass" or "unexpected fail".
fn check_indexed_assert_with_expected_fail(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
    keys: &[ExpectedFailKey],
) -> AssertResult {
    match collect_failing_paths(index_name, entries) {
        Ok(paths) if paths.is_empty() => AssertResult::Pass,
        Ok(paths) => {
            // Classify each failing path
            let mut unexpected_passes = Vec::new();
            let mut unexpected_fails = Vec::new();

            for path in &paths {
                let is_expected_fail_key = keys
                    .iter()
                    .any(|key| expected_fail_key_matches_path(path, key));
                if is_expected_fail_key {
                    // This was an expected-fail key but the value is false after inversion,
                    // meaning the original was true → unexpected pass
                    unexpected_passes.push(path.as_slice());
                } else {
                    unexpected_fails.push(path.as_slice());
                }
            }

            let is_multi_index = paths.iter().any(|p| p.len() > 1);
            let mut parts = Vec::new();

            if !unexpected_passes.is_empty() {
                parts.push(format!(
                    "unexpected pass at {}",
                    format_indexed_paths(&unexpected_passes, is_multi_index)
                ));
            }

            if !unexpected_fails.is_empty() {
                parts.push(format!(
                    "failed at {}",
                    format_indexed_paths(&unexpected_fails, is_multi_index)
                ));
            }

            AssertResult::Fail {
                message: parts.join("; "),
            }
        }
        Err(msg) => AssertResult::Error { message: msg },
    }
}

/// Recursively check an indexed assertion value (possibly multi-dimensional).
///
/// For single-index: `Bool[Mode]` — entries are `Bool` values.
/// For multi-index: `Bool[Phase, Maneuver]` — entries are nested `Indexed` values.
///
/// Single-index failure message example:
///   `failed at Mode.Boost`
/// Multi-index failure message example:
///   `failed at (Phase.Launch, Maneuver.Correction), (Phase.Cruise, Maneuver.Insertion)`
pub(super) fn check_indexed_assert(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> AssertResult {
    match collect_failing_paths(index_name, entries) {
        Ok(paths) if paths.is_empty() => AssertResult::Pass,
        Ok(paths) => {
            let is_multi_index = paths.iter().any(|p| p.len() > 1);
            AssertResult::Fail {
                message: format!(
                    "failed at {}",
                    format_indexed_paths(
                        &paths.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                        is_multi_index,
                    )
                ),
            }
        }
        Err(msg) => AssertResult::Error { message: msg },
    }
}

/// Recursively collect failing variant paths from an indexed assertion value.
///
/// Each path is a `Vec<(IndexTypeRef, VariantName)>` of index/variant pairs from outermost to innermost.
/// For example, `vec![(IndexTypeRef::with_owner(owner, IndexName::new("Phase")), VariantName::new("Launch")), ...]` for a 2D failure.
fn collect_failing_paths(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<Vec<Vec<(IndexTypeRef, IndexVariantName)>>, String> {
    let mut paths = Vec::new();
    for (variant, value) in entries {
        let key = (index_name.clone(), variant.clone());
        match value {
            RuntimeValue::Bool(true) => {}
            RuntimeValue::Bool(false) => {
                paths.push(vec![key]);
            }
            RuntimeValue::Indexed {
                index_name: inner_index,
                entries: inner_entries,
            } => {
                // Recurse into nested dimension, prepending current variant to each path
                for mut inner_path in collect_failing_paths(inner_index, inner_entries)? {
                    inner_path.insert(0, key.clone());
                    paths.push(inner_path);
                }
            }
            other => {
                return Err(format!(
                    "expected Bool for {}::{variant}, got {other:?}",
                    index_name.display_name()
                ));
            }
        }
    }
    Ok(paths)
}

/// Evaluate a single assert body and return an `AssertResult`.
pub(super) fn evaluate_assert_body(
    body: &graphcal_compiler::hir::AssertBody,
    values: &RuntimeValueMap,
    local_values: &HirLocalValueMap,
    ctx: &EvalContext<'_>,
) -> AssertResult {
    match body {
        graphcal_compiler::hir::AssertBody::Expr(body_expr) => {
            match eval_hir_expr(body_expr, values, local_values, ctx) {
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
        graphcal_compiler::hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
        } => {
            let actual_val = match eval_hir_expr(actual, values, local_values, ctx) {
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
            let expected_val = match eval_hir_expr(expected, values, local_values, ctx) {
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
            let tolerance_val = match eval_hir_expr(tolerance, values, local_values, ctx) {
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

/// Evaluate one plot property expression to a `PlotFieldValue`. String
/// literals are passed through directly (Graphcal has no runtime String
/// value); any other expression is evaluated and converted from a
/// `RuntimeValue`. Returns `None` on evaluation failure — plots are
/// best-effort, so a single bad encoding/property aborts the plot.
fn eval_plot_property(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> Option<PlotFieldValue> {
    if let graphcal_compiler::desugar::resolved_ast::ExprKind::StringLiteral(s) = &expr.kind {
        return Some(PlotFieldValue::String(s.clone()));
    }
    let rv = eval_expr(expr, values, local_values, ctx).ok()?;
    Some(runtime_to_plot_field_value(&rv))
}

/// Evaluate a plot declaration, producing a `PlotSpec`.
///
/// Encoding channel expressions and property expressions are evaluated and
/// flattened into a single `fields` list. String literals are handled directly
/// (they are not runtime values in Graphcal).
/// Returns `None` if any expression evaluation fails (plots are best-effort).
fn evaluate_plot(
    decl: &graphcal_compiler::desugar::resolved_ast::PlotDecl,
    name: &ScopedName,
    is_pub: bool,
    values: &RuntimeValueMap,
    local_values: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
) -> Option<PlotSpec> {
    let mut encodings = Vec::new();
    let mut encoding_meta = Vec::new();

    // Evaluate encoding channels
    for encoding in &decl.encodings {
        let field_value = eval_plot_property(&encoding.value, values, local_values, ctx)?;

        // Extract axis metadata: dimension from graph refs, display unit from expression
        let meta = extract_encoding_axis_meta(&encoding.value, declared_types, ctx, values);
        encoding_meta.push((encoding.channel, meta));

        encodings.push((encoding.channel, field_value));
    }

    // Evaluate mark properties (e.g., stroke_width, opacity)
    let mut mark_properties = Vec::new();
    for prop in &decl.mark.properties {
        let Some(mark_prop) = super::types::MarkProperty::from_name(prop.name.value.as_str())
        else {
            // Unknown mark property — skip (could be reported as a warning in the future)
            continue;
        };
        let field_value = eval_plot_property(&prop.value, values, local_values, ctx)?;
        mark_properties.push((mark_prop, field_value));
    }

    // Evaluate top-level properties (e.g., title, width, height)
    let mut properties = Vec::new();
    for prop in &decl.properties {
        let Some(plot_prop) = super::types::PlotProperty::from_name(prop.name.value.as_str())
        else {
            // Unknown plot property — skip
            continue;
        };
        let field_value = eval_plot_property(&prop.value, values, local_values, ctx)?;
        properties.push((plot_prop, field_value));
    }

    Some(PlotSpec {
        name: name.clone(),
        mark_type: decl.mark.mark_type,
        encodings,
        encoding_meta,
        mark_properties,
        properties,
        is_pub,
    })
}

/// Extract axis metadata (dimension name + display unit) from an encoding expression.
///
/// Walks the expression tree to find `@`-references (graph refs) and looks up
/// their declared type for the dimension. Also extracts display unit info from
/// unit literals and conversion targets.
fn extract_encoding_axis_meta(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    ctx: &EvalContext<'_>,
    values: &RuntimeValueMap,
) -> AxisMeta {
    let dimension_label = extract_dimension_from_expr(expr, declared_types, ctx.registry);
    let unit_label = extract_flat_display_unit(expr, ctx, values).map(|du| du.label);
    AxisMeta {
        dimension_label,
        unit_label,
    }
}

/// Walk an expression tree to find the first `@`-reference and extract its dimension name.
fn extract_dimension_from_expr(
    expr: &graphcal_compiler::desugar::resolved_ast::Expr,
    declared_types: &HashMap<ScopedName, graphcal_compiler::registry::declared_type::DeclaredType>,
    registry: &Registry,
) -> Option<String> {
    use graphcal_compiler::desugar::resolved_ast::ExprKind;
    match &expr.kind {
        ExprKind::GraphRef(name) => {
            let dt = declared_types.get(&name.value)?;
            dimension_label_from_declared_type(dt, registry)
        }
        ExprKind::ForComp { body, .. } => {
            extract_dimension_from_expr(body, declared_types, registry)
        }
        ExprKind::IndexAccess { expr: inner, .. } | ExprKind::Convert { expr: inner, .. } => {
            extract_dimension_from_expr(inner, declared_types, registry)
        }
        ExprKind::BinOp { lhs, .. } => {
            // For binary ops like `@x[m] * @x[m]`, try left first
            extract_dimension_from_expr(lhs, declared_types, registry)
        }
        _ => None,
    }
}

/// Convert a `DeclaredType` to a human-readable dimension label.
///
/// Returns `None` for dimensionless, bool, int, etc.
fn dimension_label_from_declared_type(
    dt: &graphcal_compiler::registry::declared_type::DeclaredType,
    registry: &Registry,
) -> Option<String> {
    match dt {
        graphcal_compiler::registry::declared_type::DeclaredType::Scalar(dim) => {
            if dim.is_dimensionless() {
                None
            } else {
                Some(registry.dimensions.format_dimension(dim))
            }
        }
        graphcal_compiler::registry::declared_type::DeclaredType::Indexed { element, .. } => {
            dimension_label_from_declared_type(element, registry)
        }
        _ => None,
    }
}

/// Evaluate composition fields (properties and plot names) shared by figures and layers.
fn eval_composition_fields(
    fields: &[graphcal_compiler::desugar::resolved_ast::PlotField],
    plot_name_spans: &[graphcal_compiler::syntax::span::Spanned<ScopedName>],
    values: &RuntimeValueMap,
    empty_locals: &HashMap<String, RuntimeValue>,
    ctx: &EvalContext<'_>,
) -> (
    Vec<(super::types::CompositionProperty, PlotFieldValue)>,
    Vec<ScopedName>,
) {
    let mut properties = Vec::new();
    for field in fields {
        let Some(comp_prop) =
            super::types::CompositionProperty::from_name(field.name.value.as_str())
        else {
            continue;
        };
        if let graphcal_compiler::desugar::resolved_ast::ExprKind::StringLiteral(s) =
            &field.value.kind
        {
            properties.push((comp_prop, PlotFieldValue::String(s.clone())));
            continue;
        }
        if let Ok(rv) = eval_expr(&field.value, values, empty_locals, ctx) {
            properties.push((comp_prop, runtime_to_plot_field_value(&rv)));
        }
    }
    let plot_names = plot_name_spans.iter().map(|p| p.value.clone()).collect();
    (properties, plot_names)
}

/// Convert a `RuntimeValue` to a `PlotFieldValue`.
#[expect(
    clippy::cast_precision_loss,
    reason = "plot data loss of precision from i64 to f64 is acceptable"
)]
fn runtime_to_plot_field_value(rv: &RuntimeValue) -> PlotFieldValue {
    match rv {
        RuntimeValue::Scalar(v) => PlotFieldValue::Number(*v),
        RuntimeValue::Int(i) => PlotFieldValue::Number(*i as f64),
        RuntimeValue::Bool(b) => PlotFieldValue::String(b.to_string()),
        RuntimeValue::Label { variant, .. } => PlotFieldValue::String(variant.to_string()),
        RuntimeValue::Indexed { entries, .. } => {
            // Try to interpret as a list of numbers or labels
            let mut numbers = Vec::new();
            let mut labels = Vec::new();
            let mut all_numeric = true;
            for (_variant, entry_rv) in entries {
                match entry_rv {
                    RuntimeValue::Scalar(v) => numbers.push(*v),
                    RuntimeValue::Int(i) => numbers.push(*i as f64),
                    RuntimeValue::Label { variant, .. } => {
                        labels.push(variant.to_string());
                        all_numeric = false;
                    }
                    _ => {
                        all_numeric = false;
                    }
                }
            }
            if all_numeric && !numbers.is_empty() {
                PlotFieldValue::Numbers(numbers)
            } else if !labels.is_empty() {
                PlotFieldValue::Labels(labels)
            } else {
                // Fallback: extract variant names as labels
                PlotFieldValue::Labels(
                    entries
                        .keys()
                        .map(graphcal_compiler::syntax::names::IndexVariantName::to_string)
                        .collect(),
                )
            }
        }
        RuntimeValue::Struct { .. } => PlotFieldValue::String("<struct>".to_string()),
        RuntimeValue::Datetime(epoch) => PlotFieldValue::String(format!("{epoch}")),
        RuntimeValue::RangeLabel { value, .. } => PlotFieldValue::Number(*value),
    }
}
