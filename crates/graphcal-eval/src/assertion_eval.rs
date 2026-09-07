//! Assertion semantics over an expression-evaluation callback, independent of frame adapters.

use crate::eval::types::AssertResult;
use graphcal_compiler::assertion_expectation::{ExpectedFail, ExpectedFailKey};
use graphcal_compiler::registry::declared_type::IndexTypeRef;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use indexmap::IndexMap;

/// Evaluate an assertion body with optional `#[expected_fail]` handling.
///
/// For `None` (no `expected_fail`): evaluate and return the result as-is.
/// For `Some(ExpectedFail::All)`: invert the final result (Pass↔Fail).
/// For `Some(ExpectedFail::Variants(keys))`: evaluate the expression to get
/// the raw indexed `RuntimeValue`, invert only the matching variant entries,
/// then aggregate.
pub fn evaluate_assert_with_expected_fail(
    body: &graphcal_compiler::hir::expr::AssertBody,
    ef: Option<&ExpectedFail>,
    evaluate_expression: &mut impl FnMut(
        &graphcal_compiler::hir::expr::Expr,
    ) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    match ef {
        None => evaluate_assert_body(body, evaluate_expression),
        Some(ExpectedFail::All) => {
            let result = evaluate_assert_body(body, evaluate_expression);
            match result {
                AssertResult::Pass => AssertResult::Fail {
                    message: "assertion passed but was marked #[expected_fail]".to_string(),
                },
                AssertResult::Fail { .. } => AssertResult::Pass,
                AssertResult::Error { .. } => result,
            }
        }
        Some(ExpectedFail::Variants(keys)) => {
            // Per-variant: we need the raw per-key Bool tree to invert
            // specific entries. For `Expr` bodies that is the evaluated
            // expression; for tolerance bodies it is the element-wise
            // pass/fail tree (#809).
            let bool_tree = match body {
                graphcal_compiler::hir::expr::AssertBody::Expr(body_expr) => {
                    match evaluate_expression(body_expr) {
                        Ok(value) => value,
                        Err(e) => {
                            return AssertResult::Error {
                                message: format!("{e}"),
                            };
                        }
                    }
                }
                graphcal_compiler::hir::expr::AssertBody::Tolerance {
                    actual,
                    expected,
                    tolerance,
                } => {
                    let operands =
                        eval_tolerance_operands(actual, expected, tolerance, evaluate_expression);
                    let (actual_val, expected_val, tolerance_val) = match operands {
                        Ok(operands) => operands,
                        Err(result) => return result,
                    };
                    match eval_tolerance_tree(&actual_val, &expected_val, &tolerance_val) {
                        Ok((tree, _)) => tree,
                        Err(message) => return AssertResult::Error { message },
                    }
                }
            };
            match bool_tree {
                RuntimeValue::Indexed {
                    index_name,
                    entries,
                } => {
                    let inverted = invert_indexed_variants(&index_name, entries, keys);
                    check_indexed_assert_with_expected_fail(&inverted.0, &inverted.1, keys)
                }
                RuntimeValue::Bool(_) => AssertResult::Error {
                    message:
                        "invalid compiled plan: per-variant #[expected_fail(...)] on a non-indexed assertion"
                            .to_string(),
                },
                other => AssertResult::Error {
                    message: format!("expected Bool or Indexed, got {other:?}"),
                },
            }
        }
    }
}

fn expected_fail_key_matches_path(
    path: &[(IndexTypeRef, IndexEntryKey)],
    key: &ExpectedFailKey,
) -> bool {
    path.len() == key.len()
        && path
            .iter()
            .zip(key.iter())
            .all(|((actual_index, actual_variant), expected)| {
                expected.matches_entry(actual_index, actual_variant)
            })
}

/// Invert specific variant entries in an indexed `RuntimeValue`.
///
/// For each entry in the indexed value, if the variant key matches one of the
/// expected-fail keys, flip `Bool(true)` → `Bool(false)` and vice versa.
/// For nested indexed values (multi-index), recurse.
fn invert_indexed_variants(
    index_name: &IndexTypeRef,
    entries: IndexMap<IndexEntryKey, RuntimeValue>,
    keys: &[ExpectedFailKey],
) -> (IndexTypeRef, IndexMap<IndexEntryKey, RuntimeValue>) {
    let inverted_entries = entries
        .into_iter()
        .map(|(variant, value)| {
            let new_value = match value {
                RuntimeValue::Bool(b) => {
                    // Single-index: check if this variant is in any key
                    let should_invert = keys
                        .iter()
                        .any(|key| key.len() == 1 && key[0].matches_entry(index_name, &variant));
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
                        .filter(|key| key.len() >= 2 && key[0].matches_entry(index_name, &variant))
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
/// For single-index paths, formats as `Mode#Boost, Mode#Cruise`.
/// For multi-index paths, formats as `(Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`.
fn format_indexed_path_part(index: &IndexTypeRef, key: &IndexEntryKey) -> String {
    match key {
        IndexEntryKey::Named(variant) => format!("{}#{variant}", index.display_name()),
        IndexEntryKey::Position(_) => key.to_string(),
    }
}

fn format_indexed_paths(
    paths: &[&[(IndexTypeRef, IndexEntryKey)]],
    is_multi_index: bool,
) -> String {
    let formatted: Vec<String> = if is_multi_index {
        paths
            .iter()
            .map(|p| {
                format!(
                    "({})",
                    p.iter()
                        .map(|(index, key)| format_indexed_path_part(index, key))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect()
    } else {
        paths
            .iter()
            .map(|p| format_indexed_path_part(&p[0].0, &p[0].1))
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
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
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
///   `failed at Mode#Boost`
/// Multi-index failure message example:
///   `failed at (Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`
fn check_indexed_assert(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
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
/// For example, `vec![(IndexTypeRef::with_owner(owner, IndexName::expect_valid("Phase")), VariantName::new("Launch")), ...]` for a 2D failure.
fn collect_failing_paths(
    index_name: &IndexTypeRef,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<Vec<Vec<(IndexTypeRef, IndexEntryKey)>>, String> {
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
fn evaluate_assert_body(
    body: &graphcal_compiler::hir::expr::AssertBody,
    evaluate_expression: &mut impl FnMut(
        &graphcal_compiler::hir::expr::Expr,
    ) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    match body {
        graphcal_compiler::hir::expr::AssertBody::Expr(body_expr) => {
            match evaluate_expression(body_expr) {
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
        graphcal_compiler::hir::expr::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => evaluate_tolerance_assert(actual, expected, tolerance, evaluate_expression),
    }
}

/// Evaluate a tolerance assertion body (`actual ~= expected +/- tolerance`).
///
/// Indexed operands broadcast element-wise (#809): the assertion's shape
/// comes from `actual`; `expected` and `tolerance` are each unindexed (applied
/// to every key) or indexed by the same axes. Failures report each failing
/// key with its actual/expected/delta detail.
fn evaluate_tolerance_assert(
    actual: &graphcal_compiler::hir::expr::Expr,
    expected: &graphcal_compiler::hir::expr::Expr,
    tolerance: &graphcal_compiler::hir::expr::Expr,
    evaluate_expression: &mut impl FnMut(
        &graphcal_compiler::hir::expr::Expr,
    ) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    let (actual_val, expected_val, tolerance_val) =
        match eval_tolerance_operands(actual, expected, tolerance, evaluate_expression) {
            Ok(operands) => operands,
            Err(result) => return result,
        };
    match eval_tolerance_tree(&actual_val, &expected_val, &tolerance_val) {
        Err(message) => AssertResult::Error { message },
        Ok((_, failures)) if failures.is_empty() => AssertResult::Pass,
        Ok((_, failures)) => AssertResult::Fail {
            message: format_tolerance_failures(&failures),
        },
    }
}

/// Evaluate the three operand expressions of a tolerance assertion.
///
/// Returns the raw runtime values (any shape — shape checking happens in
/// [`eval_tolerance_tree`]), or the `AssertResult::Error` to report.
fn eval_tolerance_operands(
    actual: &graphcal_compiler::hir::expr::Expr,
    expected: &graphcal_compiler::hir::expr::Expr,
    tolerance: &graphcal_compiler::hir::expr::Expr,
    evaluate_expression: &mut impl FnMut(
        &graphcal_compiler::hir::expr::Expr,
    ) -> Result<RuntimeValue, GraphcalError>,
) -> Result<(RuntimeValue, RuntimeValue, RuntimeValue), AssertResult> {
    let mut operand = |expr: &graphcal_compiler::hir::expr::Expr| {
        evaluate_expression(expr).map_err(|e| AssertResult::Error {
            message: format!("{e}"),
        })
    };
    Ok((operand(actual)?, operand(expected)?, operand(tolerance)?))
}

/// A failing key of a tolerance assertion, with its numeric detail.
struct ToleranceFailure {
    /// Index path from outermost to innermost axis; empty for an unindexed
    /// assertion.
    path: Vec<(IndexTypeRef, IndexEntryKey)>,
    /// `actual X, expected Y +/- T, off by D`.
    detail: String,
}

/// Walk a tolerance assertion's operands element-wise, producing the per-key
/// `Bool` tree (mirroring `actual`'s index structure) plus the detail for
/// every failing key. A shape/sign/type problem aborts with `Err` —
/// reported as an assertion ERROR.
fn eval_tolerance_tree(
    actual: &RuntimeValue,
    expected: &RuntimeValue,
    tolerance: &RuntimeValue,
) -> Result<(RuntimeValue, Vec<ToleranceFailure>), String> {
    let mut failures = Vec::new();
    let mut path = Vec::new();
    let tree = tolerance_tree_inner(actual, expected, tolerance, &mut path, &mut failures)?;
    Ok((tree, failures))
}

fn tolerance_tree_inner(
    actual: &RuntimeValue,
    expected: &RuntimeValue,
    tolerance: &RuntimeValue,
    path: &mut Vec<(IndexTypeRef, IndexEntryKey)>,
    failures: &mut Vec<ToleranceFailure>,
) -> Result<RuntimeValue, String> {
    if let RuntimeValue::Indexed {
        index_name,
        entries,
    } = actual
    {
        let checked_entries = entries
            .iter()
            .map(|(variant, actual_entry)| {
                let expected_entry = tolerance_entry_or_broadcast(expected, index_name, variant)?;
                let tolerance_entry = tolerance_entry_or_broadcast(tolerance, index_name, variant)?;
                path.push((index_name.clone(), variant.clone()));
                let result = tolerance_tree_inner(
                    actual_entry,
                    expected_entry,
                    tolerance_entry,
                    path,
                    failures,
                );
                path.pop();
                Ok((variant.clone(), result?))
            })
            .collect::<Result<_, String>>()?;
        return Ok(RuntimeValue::Indexed {
            index_name: index_name.clone(),
            entries: checked_entries,
        });
    }

    let actual_val = tolerance_quantity_operand(actual, "actual")?;
    let expected_val = tolerance_quantity_operand(expected, "expected")?;
    let tolerance_val = tolerance_quantity_operand(tolerance, "tolerance")?;
    let tol_display = format!("{tolerance_val}");

    // A negative tolerance makes the assertion unsatisfiable (even an
    // exact match fails). Statically-known negatives are rejected at
    // check time (#815); this guards tolerances computed at runtime.
    if tolerance_val < 0.0 {
        return Err(format!("tolerance must be non-negative, got {tol_display}"));
    }

    let delta = (actual_val - expected_val).abs();
    let ok = delta <= tolerance_val;
    if !ok {
        failures.push(ToleranceFailure {
            path: path.clone(),
            detail: format!(
                "actual {actual_val}, expected {expected_val} +/- {tol_display}, off by {delta}"
            ),
        });
    }
    Ok(RuntimeValue::Bool(ok))
}

/// Select the entry of a broadcastable tolerance operand for one key of
/// `actual`'s axis: indexed operands index per key (axes were checked
/// statically; mismatches here are evaluation errors), unindexed operands
/// broadcast unchanged.
fn tolerance_entry_or_broadcast<'a>(
    operand: &'a RuntimeValue,
    axis: &IndexTypeRef,
    variant: &IndexEntryKey,
) -> Result<&'a RuntimeValue, String> {
    match operand {
        RuntimeValue::Indexed {
            index_name,
            entries,
        } => {
            if !index_name.matches_ref(axis) {
                return Err(format!(
                    "tolerance assertion operand has mismatched index axes: `{}` vs `{}`",
                    axis.display_name(),
                    index_name.display_name()
                ));
            }
            entries.get(variant).ok_or_else(|| {
                format!(
                    "tolerance assertion operand is missing entry `{}`",
                    format_indexed_path_part(index_name, variant)
                )
            })
        }
        other => Ok(other),
    }
}

fn tolerance_quantity_operand(value: &RuntimeValue, role: &str) -> Result<f64, String> {
    match value {
        RuntimeValue::Quantity(v) => Ok(v.get()),
        other => Err(format!("expected quantity {role}, got {other:?}")),
    }
}

/// Render tolerance failures: an unindexed assertion reports its detail bare
/// (`actual X, expected Y +/- T, off by D`); indexed assertions report each
/// failing key with its detail.
fn format_tolerance_failures(failures: &[ToleranceFailure]) -> String {
    if let [failure] = failures
        && failure.path.is_empty()
    {
        return failure.detail.clone();
    }
    let is_multi_index = failures.iter().any(|f| f.path.len() > 1);
    let formatted: Vec<String> = failures
        .iter()
        .map(|f| {
            let key = format_indexed_paths(&[f.path.as_slice()], is_multi_index);
            format!("failed at {key} ({})", f.detail)
        })
        .collect();
    formatted.join("; ")
}
