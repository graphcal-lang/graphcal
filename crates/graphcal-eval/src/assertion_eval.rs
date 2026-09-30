//! Assertion semantics over an expression-evaluation callback, independent of frame adapters.
//!
//! A checked assertion condition is a `Bool` or a `Bool` indexed by its axes,
//! and tolerance operands are quantities (optionally indexed). Their values
//! are read into [`Verdicts`] and [`Measured`] trees at one place each; a
//! value of any other shape contradicts the checked types and is reported as
//! a violated invariant, never interpreted.

use crate::eval::types::AssertResult;
use crate::invariant::Invariant;
use crate::runtime_value::{IndexedValue, RuntimeValue};
use graphcal_compiler::assertion_expectation::{ExpectedFail, ExpectedFailKey};
use graphcal_compiler::hir::expr::{AssertBody, Expr};
use graphcal_compiler::registry::checked_type::IndexTypeRef;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::tir::typed::evaluation_unit::{AssertionOperands, Scoped};

/// The value of a checked assertion condition.
#[derive(Debug, Clone, PartialEq)]
enum Verdicts {
    /// An unindexed condition.
    Single(bool),
    /// A condition indexed by one axis (entries may be indexed further).
    Indexed(IndexedValue<Self>),
}

impl Verdicts {
    /// Read a condition's value; any shape but (indexed) `Bool` contradicts
    /// the condition's checked type.
    fn try_from_value(value: RuntimeValue) -> Result<Self, Invariant> {
        match value {
            RuntimeValue::Bool(verdict) => Ok(Self::Single(verdict)),
            RuntimeValue::Indexed(indexed) => indexed
                .try_map(|_, entry| Self::try_from_value(entry))
                .map(Self::Indexed),
            other => Err(Invariant::violated(format_args!(
                "assertion condition evaluated to {}, not Bool",
                other.describe()
            ))),
        }
    }
}

/// The value of a checked tolerance-assertion operand.
#[derive(Debug, Clone)]
enum Measured {
    /// An unindexed quantity.
    Single(f64),
    /// A quantity indexed by one axis (entries may be indexed further).
    Indexed(IndexedValue<Self>),
}

impl Measured {
    /// Read a tolerance operand's value; any shape but (indexed) quantity
    /// contradicts the operand's checked type.
    fn try_from_value(value: RuntimeValue, role: &str) -> Result<Self, Invariant> {
        match value {
            RuntimeValue::Quantity(value) => Ok(Self::Single(value.get())),
            RuntimeValue::Indexed(indexed) => indexed
                .try_map(|_, entry| Self::try_from_value(entry, role))
                .map(Self::Indexed),
            other => Err(Invariant::violated(format_args!(
                "tolerance {role} evaluated to {}, not a quantity",
                other.describe()
            ))),
        }
    }
}

fn evaluation_error(error: GraphcalError) -> AssertResult {
    match error {
        GraphcalError::EvaluationUnavailable { reason, .. } if reason.is_incomplete() => {
            AssertResult::Blocked { reason }
        }
        error => AssertResult::Error {
            message: error.to_string(),
        },
    }
}

/// The result reporting a violated invariant.
fn invariant_result(invariant: &Invariant) -> AssertResult {
    AssertResult::Error {
        message: invariant.to_string(),
    }
}

/// Evaluate an assertion body with optional `#[expected_fail]` handling.
///
/// For `None` (no `expected_fail`): evaluate and return the result as-is.
/// For `Some(ExpectedFail::All)`: invert the final result (Pass↔Fail).
/// For `Some(ExpectedFail::Variants(keys))`: evaluate the expression to get
/// the raw indexed verdicts, invert only the matching variant entries,
/// then aggregate.
pub fn evaluate_assert_with_expected_fail<'t>(
    body: Scoped<'t, AssertBody>,
    ef: Option<&ExpectedFail>,
    evaluate_expression: &mut impl FnMut(Scoped<'t, Expr>) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    let body = body.operands();
    match ef {
        None => evaluate_assert_body(body, evaluate_expression),
        Some(ExpectedFail::All) => {
            let result = evaluate_assert_body(body, evaluate_expression);
            match result {
                AssertResult::Pass => AssertResult::Fail {
                    message: "assertion passed but was marked #[expected_fail]".to_string(),
                },
                AssertResult::Fail { .. } => AssertResult::Pass,
                AssertResult::Error { .. } | AssertResult::Blocked { .. } => result,
            }
        }
        Some(ExpectedFail::Variants(keys)) => {
            // Per-variant: we need the raw per-key verdicts to invert
            // specific entries. For `Expr` bodies these are the evaluated
            // condition; for tolerance bodies the element-wise pass/fail
            // verdicts (#809).
            let verdicts = match body {
                AssertionOperands::Condition(body_expr) => match evaluate_expression(body_expr) {
                    Ok(value) => match Verdicts::try_from_value(value) {
                        Ok(verdicts) => verdicts,
                        Err(invariant) => return invariant_result(&invariant),
                    },
                    Err(error) => return evaluation_error(error),
                },
                AssertionOperands::Tolerance {
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
                        Ok((verdicts, _)) => verdicts,
                        Err(error) => return error.result(),
                    }
                }
            };
            // Checking admits per-variant `#[expected_fail]` only on an
            // indexed assertion.
            match verdicts {
                Verdicts::Indexed(indexed) => {
                    let inverted = invert_indexed_variants(&indexed, keys.as_slice());
                    check_indexed_assert_with_expected_fail(&inverted, keys.as_slice())
                }
                Verdicts::Single(_) => invariant_result(&Invariant::violated(
                    "per-variant #[expected_fail(...)] reached a non-indexed assertion",
                )),
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

/// Invert specific variant entries of indexed verdicts.
///
/// For each entry, if the variant key matches one of the expected-fail keys,
/// flip its verdict. For nested indexed verdicts (multi-index), recurse.
fn invert_indexed_variants(
    indexed: &IndexedValue<Verdicts>,
    keys: &[ExpectedFailKey],
) -> IndexedValue<Verdicts> {
    let index_name = indexed.index();
    let inverted = indexed.try_map_ref(|variant, value| {
        Ok::<_, std::convert::Infallible>(match value {
            Verdicts::Single(verdict) => {
                // Single-index: check if this variant is in any key
                let should_invert = keys
                    .iter()
                    .any(|key| key.len() == 1 && key[0].matches_entry(index_name, variant));
                Verdicts::Single(if should_invert { !verdict } else { *verdict })
            }
            Verdicts::Indexed(inner) => {
                // Multi-index: filter keys that match the current variant at position 0,
                // then strip the first element and recurse.
                let sub_keys: Vec<ExpectedFailKey> = keys
                    .iter()
                    .filter(|key| key.len() >= 2 && key[0].matches_entry(index_name, variant))
                    .map(|key| key[1..].to_vec())
                    .collect();
                if sub_keys.is_empty() {
                    // No expected-fail keys apply to this subtree — leave as-is
                    value.clone()
                } else {
                    Verdicts::Indexed(invert_indexed_variants(inner, &sub_keys))
                }
            }
        })
    });
    match inverted {
        Ok(inverted) => inverted,
        Err(never) => match never {},
    }
}

/// Format a list of indexed paths for assertion failure messages.
///
/// Each path is a slice of index/variant pairs from outermost to innermost.
/// For single-index paths, formats as `Mode#Boost, Mode#Cruise`.
/// For multi-index paths, formats as `(Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`.
fn format_indexed_path_part(index: &IndexTypeRef, key: &IndexEntryKey) -> String {
    match key {
        IndexEntryKey::Named(variant) => format!("{index}#{variant}"),
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
    indexed: &IndexedValue<Verdicts>,
    keys: &[ExpectedFailKey],
) -> AssertResult {
    let paths = collect_failing_paths(indexed);
    if paths.is_empty() {
        return AssertResult::Pass;
    }
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

/// Check indexed verdicts (possibly multi-dimensional).
///
/// For single-index: `Bool[Mode]` — entries are single verdicts.
/// For multi-index: `Bool[Phase, Maneuver]` — entries are nested indexed verdicts.
///
/// Single-index failure message example:
///   `failed at Mode#Boost`
/// Multi-index failure message example:
///   `failed at (Phase#Launch, Maneuver#Correction), (Phase#Cruise, Maneuver#Insertion)`
fn check_indexed_assert(indexed: &IndexedValue<Verdicts>) -> AssertResult {
    let paths = collect_failing_paths(indexed);
    if paths.is_empty() {
        return AssertResult::Pass;
    }
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

/// Recursively collect failing variant paths from indexed verdicts.
///
/// Each path is a `Vec<(IndexTypeRef, IndexEntryKey)>` of index/variant pairs
/// from outermost to innermost.
fn collect_failing_paths(
    indexed: &IndexedValue<Verdicts>,
) -> Vec<Vec<(IndexTypeRef, IndexEntryKey)>> {
    let index_name = indexed.index();
    let mut paths = Vec::new();
    for (variant, value) in indexed.iter() {
        let key = (index_name.clone(), variant.clone());
        match value {
            Verdicts::Single(true) => {}
            Verdicts::Single(false) => {
                paths.push(vec![key]);
            }
            Verdicts::Indexed(inner) => {
                // Recurse into nested dimension, prepending current variant to each path
                for mut inner_path in collect_failing_paths(inner) {
                    inner_path.insert(0, key.clone());
                    paths.push(inner_path);
                }
            }
        }
    }
    paths
}

/// Evaluate a single assert body and return an `AssertResult`.
fn evaluate_assert_body<'t>(
    body: AssertionOperands<'t>,
    evaluate_expression: &mut impl FnMut(Scoped<'t, Expr>) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    match body {
        AssertionOperands::Condition(body_expr) => {
            match evaluate_expression(body_expr).map(Verdicts::try_from_value) {
                Ok(Ok(Verdicts::Single(true))) => AssertResult::Pass,
                Ok(Ok(Verdicts::Single(false))) => AssertResult::Fail {
                    message: "assertion evaluated to false".to_string(),
                },
                Ok(Ok(Verdicts::Indexed(indexed))) => check_indexed_assert(&indexed),
                Ok(Err(invariant)) => invariant_result(&invariant),
                Err(error) => evaluation_error(error),
            }
        }
        AssertionOperands::Tolerance {
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
fn evaluate_tolerance_assert<'t>(
    actual: Scoped<'t, Expr>,
    expected: Scoped<'t, Expr>,
    tolerance: Scoped<'t, Expr>,
    evaluate_expression: &mut impl FnMut(Scoped<'t, Expr>) -> Result<RuntimeValue, GraphcalError>,
) -> AssertResult {
    let (actual_val, expected_val, tolerance_val) =
        match eval_tolerance_operands(actual, expected, tolerance, evaluate_expression) {
            Ok(operands) => operands,
            Err(result) => return result,
        };
    match eval_tolerance_tree(&actual_val, &expected_val, &tolerance_val) {
        Err(error) => error.result(),
        Ok((_, failures)) if failures.is_empty() => AssertResult::Pass,
        Ok((_, failures)) => AssertResult::Fail {
            message: format_tolerance_failures(&failures),
        },
    }
}

/// Evaluate the three operand expressions of a tolerance assertion.
///
/// Returns the operands read as (indexed) quantities, or the
/// `AssertResult::Error` to report.
fn eval_tolerance_operands<'t>(
    actual: Scoped<'t, Expr>,
    expected: Scoped<'t, Expr>,
    tolerance: Scoped<'t, Expr>,
    evaluate_expression: &mut impl FnMut(Scoped<'t, Expr>) -> Result<RuntimeValue, GraphcalError>,
) -> Result<(Measured, Measured, Measured), AssertResult> {
    let mut operand = |expr: Scoped<'t, Expr>, role: &str| {
        evaluate_expression(expr)
            .map_err(evaluation_error)
            .and_then(|value| {
                Measured::try_from_value(value, role)
                    .map_err(|invariant| invariant_result(&invariant))
            })
    };
    Ok((
        operand(actual, "actual")?,
        operand(expected, "expected")?,
        operand(tolerance, "tolerance")?,
    ))
}

/// A failing key of a tolerance assertion, with its numeric detail.
struct ToleranceFailure {
    /// Index path from outermost to innermost axis; empty for an unindexed
    /// assertion.
    path: Vec<(IndexTypeRef, IndexEntryKey)>,
    /// `actual X, expected Y +/- T, off by D`.
    detail: String,
}

/// Why a tolerance assertion could not be decided.
enum ToleranceError {
    /// A tolerance computed at runtime is negative.
    NegativeTolerance(String),
    /// The operands' shapes contradict their checked types.
    Invariant(Invariant),
}

impl ToleranceError {
    fn result(self) -> AssertResult {
        match self {
            Self::NegativeTolerance(tolerance) => AssertResult::Error {
                message: format!("tolerance must be non-negative, got {tolerance}"),
            },
            Self::Invariant(invariant) => invariant_result(&invariant),
        }
    }
}

/// Walk a tolerance assertion's operands element-wise, producing the per-key
/// verdicts (mirroring `actual`'s index structure) plus the detail for every
/// failing key.
fn eval_tolerance_tree(
    actual: &Measured,
    expected: &Measured,
    tolerance: &Measured,
) -> Result<(Verdicts, Vec<ToleranceFailure>), ToleranceError> {
    let mut failures = Vec::new();
    let mut path = Vec::new();
    let tree = tolerance_tree_inner(actual, expected, tolerance, &mut path, &mut failures)?;
    Ok((tree, failures))
}

fn tolerance_tree_inner(
    actual: &Measured,
    expected: &Measured,
    tolerance: &Measured,
    path: &mut Vec<(IndexTypeRef, IndexEntryKey)>,
    failures: &mut Vec<ToleranceFailure>,
) -> Result<Verdicts, ToleranceError> {
    let actual_val = match actual {
        Measured::Indexed(indexed) => {
            let index_name = indexed.index();
            let checked = indexed.try_map_ref(|variant, actual_entry| {
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
                result
            })?;
            return Ok(Verdicts::Indexed(checked));
        }
        Measured::Single(value) => *value,
    };
    let scalar = |operand: &Measured, role: &str| match operand {
        Measured::Single(value) => Ok(*value),
        Measured::Indexed(_) => Err(ToleranceError::Invariant(Invariant::violated(
            format_args!("tolerance {role} has more axes than actual"),
        ))),
    };
    let expected_val = scalar(expected, "expected")?;
    let tolerance_val = scalar(tolerance, "tolerance")?;
    let tol_display = format!("{tolerance_val}");

    // A negative tolerance makes the assertion unsatisfiable (even an
    // exact match fails). Statically-known negatives are rejected at
    // check time (#815); this guards tolerances computed at runtime.
    if tolerance_val < 0.0 {
        return Err(ToleranceError::NegativeTolerance(tol_display));
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
    Ok(Verdicts::Single(ok))
}

/// Select the entry of a broadcastable tolerance operand for one key of
/// `actual`'s axis: indexed operands index per key (their axes were checked
/// statically), unindexed operands broadcast unchanged.
fn tolerance_entry_or_broadcast<'a>(
    operand: &'a Measured,
    axis: &IndexTypeRef,
    variant: &IndexEntryKey,
) -> Result<&'a Measured, ToleranceError> {
    match operand {
        Measured::Indexed(indexed) => {
            let index_name = indexed.index();
            index_name
                .matches_ref(axis)
                .then(|| indexed.get(variant))
                .flatten()
                .ok_or_else(|| {
                    ToleranceError::Invariant(Invariant::violated(format_args!(
                        "tolerance assertion operand over `{index_name}` has no entry `{}` of `{axis}`",
                        format_indexed_path_part(axis, variant)
                    )))
                })
        }
        Measured::Single(_) => Ok(operand),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_read_only_bool_verdicts() {
        assert_eq!(
            Verdicts::try_from_value(RuntimeValue::Bool(true)).unwrap(),
            Verdicts::Single(true)
        );
        let indexed = IndexedValue::finite_for_test(vec![
            RuntimeValue::Bool(true),
            RuntimeValue::Bool(false),
        ]);
        let Verdicts::Indexed(verdicts) =
            Verdicts::try_from_value(RuntimeValue::Indexed(indexed)).unwrap()
        else {
            panic!("an indexed condition reads as indexed verdicts");
        };
        assert_eq!(collect_failing_paths(&verdicts).len(), 1);
        assert!(Verdicts::try_from_value(RuntimeValue::Int(1)).is_err());
        assert!(
            Verdicts::try_from_value(RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![
                RuntimeValue::Int(1)
            ])))
            .is_err()
        );
    }

    #[test]
    fn tolerance_operands_read_only_quantities() {
        let quantity = |value| RuntimeValue::quantity(value).unwrap();
        assert!(matches!(
            Measured::try_from_value(quantity(1.0), "actual"),
            Ok(Measured::Single(value)) if value.to_bits() == 1.0_f64.to_bits()
        ));
        assert!(Measured::try_from_value(RuntimeValue::Bool(true), "actual").is_err());
        let (verdicts, failures) = eval_tolerance_tree(
            &Measured::Single(1.0),
            &Measured::Single(1.5),
            &Measured::Single(0.1),
        )
        .ok()
        .unwrap();
        assert_eq!(verdicts, Verdicts::Single(false));
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            eval_tolerance_tree(
                &Measured::Single(1.0),
                &Measured::Single(1.0),
                &Measured::Single(-1.0),
            ),
            Err(ToleranceError::NegativeTolerance(_))
        ));
    }
}
