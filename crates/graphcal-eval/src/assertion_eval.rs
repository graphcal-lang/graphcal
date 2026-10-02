//! Assertion semantics over an expression-evaluation callback, independent of frame adapters.
//!
//! A checked assertion condition is a `Bool` or a `Bool` indexed by its axes,
//! and tolerance operands are quantities (optionally indexed). Their values
//! are read into [`Verdicts`] and [`Measured`] trees at one place each; a
//! value of any other shape contradicts the checked types and is reported as
//! a violated invariant, never interpreted.

/// Assertion results evaluated here name the declarations a blocked
/// assertion waits on by runtime identity; the output assembly renames them.
type AssertResult =
    crate::eval::types::AssertResult<graphcal_compiler::resolved_name::ResolvedDeclName>;
use crate::invariant::Invariant;
use crate::runtime_value::{IndexedValue, RuntimeValue};
use graphcal_compiler::assertion_expectation::{ExpectedFail, ExpectedFailKey};
use graphcal_compiler::cancellation::Cancelled;
use graphcal_compiler::hir::expr::{AssertBody, Expr};
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic::checked_type::IndexTypeRef;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::evaluation_unit::AssertionOperands;
use thiserror::Error;

/// The value of a checked assertion operand of an (indexed) scalar type: a
/// leaf of that type, or one such operand per key of an axis.
#[derive(Debug, Clone, PartialEq)]
enum Leaves<T> {
    /// An unindexed operand.
    Single(T),
    /// An operand indexed by one axis (entries may be indexed further).
    Indexed(IndexedValue<Self>),
}

/// The value of a checked assertion condition.
type Verdicts = Leaves<bool>;

/// The value of a checked tolerance-assertion operand.
type Measured = Leaves<f64>;

/// Read an operand checked as `expected` with `read`, which returns the
/// part of the value that contradicts the checked type: the one place an
/// assertion operand of another shape is reported.
fn read_operand<R>(
    value: RuntimeValue,
    expected: &dyn std::fmt::Display,
    read: impl FnOnce(RuntimeValue) -> Result<R, RuntimeValue>,
) -> Result<R, Invariant> {
    read(value).map_err(|other| {
        Invariant::violated(format_args!("{expected} evaluated to {}", other.describe()))
    })
}

impl<T> Leaves<T> {
    /// Read (indexed) leaves, each read by `leaf`; returns the part of
    /// `value` of another shape.
    fn read_tree(
        value: RuntimeValue,
        leaf: &impl Fn(RuntimeValue) -> Result<T, RuntimeValue>,
    ) -> Result<Self, RuntimeValue> {
        match value {
            RuntimeValue::Indexed(indexed) => indexed
                .try_map(|_, entry| Self::read_tree(entry, leaf))
                .map(Self::Indexed),
            value => leaf(value).map(Self::Single),
        }
    }

    /// Read an operand checked as `expected` (indexed) leaves, each read by
    /// `leaf`; any other shape contradicts the operand's checked type.
    fn read(
        value: RuntimeValue,
        expected: &dyn std::fmt::Display,
        leaf: &impl Fn(RuntimeValue) -> Result<T, RuntimeValue>,
    ) -> Result<Self, Invariant> {
        read_operand(value, expected, |value| Self::read_tree(value, leaf))
    }
}

/// A condition's leaf: a `Bool` verdict.
fn verdict(value: RuntimeValue) -> Result<bool, RuntimeValue> {
    match value {
        RuntimeValue::Bool(verdict) => Ok(verdict),
        other => Err(other),
    }
}

impl Verdicts {
    /// Read a condition's value.
    fn try_from_value(value: RuntimeValue) -> Result<Self, Invariant> {
        Self::read(value, &"assertion condition checked as Bool", &verdict)
    }

    /// Read the value of a condition checked as an indexed `Bool`.
    fn try_indexed_from_value(value: RuntimeValue) -> Result<IndexedValue<Self>, Invariant> {
        read_operand(
            value,
            &"assertion condition checked as an indexed Bool",
            |value| match value {
                RuntimeValue::Indexed(indexed) => {
                    indexed.try_map(|_, entry| Self::read_tree(entry, &verdict))
                }
                other => Err(other),
            },
        )
    }
}

impl Measured {
    /// Read a tolerance operand's value.
    fn try_from_value(value: RuntimeValue, role: &str) -> Result<Self, Invariant> {
        Self::read(
            value,
            &format_args!("tolerance {role} checked as a quantity"),
            &|value| match value {
                RuntimeValue::Quantity(value) => Ok(value.get()),
                other => Err(other),
            },
        )
    }
}

/// The result of an assertion whose evaluation ran to completion; a cancelled
/// assertion has no result.
type Evaluated = Result<AssertResult, Cancelled>;

/// The result reporting a failed operand evaluation, or the cancellation that
/// interrupted it.
fn evaluation_error(error: Outcome<SemanticError>) -> Evaluated {
    match error {
        Outcome::Cancelled => Err(Cancelled),
        Outcome::Failed(SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason, .. }),
            ..
        })) if reason.is_incomplete() => Ok(AssertResult::Blocked { reason }),
        Outcome::Failed(error) => Ok(AssertResult::Error {
            message: error.to_string(),
        }),
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
///
/// # Errors
///
/// Returns [`Cancelled`] when an operand evaluation was cancelled.
pub fn evaluate_assert_with_expected_fail<'t>(
    body: Scoped<'t, AssertBody>,
    ef: Option<&ExpectedFail>,
    evaluate_expression: &mut impl FnMut(
        Scoped<'t, Expr>,
    ) -> Result<RuntimeValue, Outcome<SemanticError>>,
) -> Evaluated {
    let body = body.operands();
    Ok(match ef {
        None => evaluate_assert_body(body, evaluate_expression)?,
        Some(ExpectedFail::All) => {
            let result = evaluate_assert_body(body, evaluate_expression)?;
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
            // Checking admits per-variant `#[expected_fail]` only on an
            // indexed assertion.
            let verdicts = match body {
                AssertionOperands::Condition(body_expr) => match evaluate_expression(body_expr) {
                    Ok(value) => match Verdicts::try_indexed_from_value(value) {
                        Ok(verdicts) => verdicts,
                        Err(invariant) => return Ok(invariant_result(&invariant)),
                    },
                    Err(error) => return evaluation_error(error),
                },
                AssertionOperands::Tolerance {
                    actual,
                    expected,
                    tolerance,
                } => {
                    let gauges = match eval_tolerance_operands(
                        actual,
                        expected,
                        tolerance,
                        evaluate_expression,
                        |gauges| match gauges {
                            Gauges::Indexed(gauges) => Ok(gauges),
                            Gauges::Single(_) => Err(Misaligned::Unindexed),
                        },
                    )? {
                        Ok(gauges) => gauges,
                        Err(result) => return Ok(result),
                    };
                    match gauges.try_map_ref(|variant, entry| {
                        let mut path = vec![(gauges.index().clone(), variant.clone())];
                        tolerance_tree_inner(entry, &mut path, &mut Vec::new())
                    }) {
                        Ok(verdicts) => verdicts,
                        Err(error) => return Ok(error.result()),
                    }
                }
            };
            let inverted = invert_indexed_variants(&verdicts, keys.as_slice());
            check_indexed_assert_with_expected_fail(&inverted, keys.as_slice())
        }
    })
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
                    .filter(|key| key.first().matches_entry(index_name, variant))
                    .filter_map(|key| NonEmpty::try_from_vec(key.split_first().1.to_vec()).ok())
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
    evaluate_expression: &mut impl FnMut(
        Scoped<'t, Expr>,
    ) -> Result<RuntimeValue, Outcome<SemanticError>>,
) -> Evaluated {
    match body {
        AssertionOperands::Condition(body_expr) => {
            match evaluate_expression(body_expr).map(Verdicts::try_from_value) {
                Ok(Ok(Verdicts::Single(true))) => Ok(AssertResult::Pass),
                Ok(Ok(Verdicts::Single(false))) => Ok(AssertResult::Fail {
                    message: "assertion evaluated to false".to_string(),
                }),
                Ok(Ok(Verdicts::Indexed(indexed))) => Ok(check_indexed_assert(&indexed)),
                Ok(Err(invariant)) => Ok(invariant_result(&invariant)),
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
    evaluate_expression: &mut impl FnMut(
        Scoped<'t, Expr>,
    ) -> Result<RuntimeValue, Outcome<SemanticError>>,
) -> Evaluated {
    let gauges =
        match eval_tolerance_operands(actual, expected, tolerance, evaluate_expression, Ok)? {
            Ok(gauges) => gauges,
            Err(result) => return Ok(result),
        };
    Ok(match eval_tolerance_tree(&gauges) {
        Err(error) => error.result(),
        Ok((_, failures)) if failures.is_empty() => AssertResult::Pass,
        Ok((_, failures)) => AssertResult::Fail {
            message: format_tolerance_failures(&failures),
        },
    })
}

/// Evaluate the three operand expressions of a tolerance assertion.
///
/// Returns the operands read as (indexed) quantities and aligned on
/// `actual`'s axes, or the `AssertResult::Error` to report; the outer error
/// is cancellation.
fn eval_tolerance_operands<'t, G>(
    actual: Scoped<'t, Expr>,
    expected: Scoped<'t, Expr>,
    tolerance: Scoped<'t, Expr>,
    evaluate_expression: &mut impl FnMut(
        Scoped<'t, Expr>,
    ) -> Result<RuntimeValue, Outcome<SemanticError>>,
    shape: impl FnOnce(Gauges) -> Result<G, Misaligned>,
) -> Result<Result<G, AssertResult>, Cancelled> {
    let mut operand =
        |expr: Scoped<'t, Expr>, role: &str| match evaluate_expression(expr) {
            Ok(value) => Ok(Measured::try_from_value(value, role)
                .map_err(|invariant| invariant_result(&invariant))),
            Err(error) => evaluation_error(error).map(Err),
        };
    let actual = match operand(actual, "actual")? {
        Ok(actual) => actual,
        Err(result) => return Ok(Err(result)),
    };
    let expected = match operand(expected, "expected")? {
        Ok(expected) => expected,
        Err(result) => return Ok(Err(result)),
    };
    let tolerance = match operand(tolerance, "tolerance")? {
        Ok(tolerance) => tolerance,
        Err(result) => return Ok(Err(result)),
    };
    // Checking proved `expected` and `tolerance` each unindexed or indexed by
    // a prefix of `actual`'s axes; operands that do not align contradict
    // their checked types.
    Ok(align(actual, &expected, &tolerance)
        .and_then(shape)
        .map_err(|misaligned| invariant_result(&Invariant::violated(misaligned))))
}

/// The three operands of a tolerance assertion at one key of `actual`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Gauge {
    actual: f64,
    expected: f64,
    tolerance: f64,
}

/// Tolerance operands aligned on `actual`'s axes.
type Gauges = Leaves<Gauge>;

/// Why tolerance operands do not align with `actual`'s axes.
#[derive(Debug, Error)]
enum Misaligned {
    #[error("per-variant #[expected_fail(...)] reached a non-indexed tolerance assertion")]
    Unindexed,
    #[error("tolerance {role} has more axes than actual")]
    ExtraAxes { role: &'static str },
    #[error("tolerance assertion operand over `{operand}` has no entry `{entry}` of `{axis}`")]
    MissingEntry {
        operand: IndexTypeRef,
        axis: IndexTypeRef,
        entry: IndexEntryKey,
    },
}

/// Align `expected` and `tolerance` on the axes of `actual`: an unindexed
/// operand broadcasts to every key, an indexed one is indexed per key.
fn align(
    actual: Measured,
    expected: &Measured,
    tolerance: &Measured,
) -> Result<Gauges, Misaligned> {
    match actual {
        Measured::Indexed(indexed) => {
            let axis = indexed.index().clone();
            indexed
                .try_map(|variant, actual_entry| {
                    align(
                        actual_entry,
                        entry_or_broadcast(expected, &axis, variant)?,
                        entry_or_broadcast(tolerance, &axis, variant)?,
                    )
                })
                .map(Gauges::Indexed)
        }
        Measured::Single(actual) => Ok(Gauges::Single(Gauge {
            actual,
            expected: scalar(expected, "expected")?,
            tolerance: scalar(tolerance, "tolerance")?,
        })),
    }
}

/// The value of an operand at a leaf of `actual`.
const fn scalar(operand: &Measured, role: &'static str) -> Result<f64, Misaligned> {
    match operand {
        Measured::Single(value) => Ok(*value),
        Measured::Indexed(_) => Err(Misaligned::ExtraAxes { role }),
    }
}

/// Select the entry of a broadcastable tolerance operand for one key of
/// `actual`'s axis: indexed operands index per key, unindexed operands
/// broadcast unchanged.
fn entry_or_broadcast<'a>(
    operand: &'a Measured,
    axis: &IndexTypeRef,
    variant: &IndexEntryKey,
) -> Result<&'a Measured, Misaligned> {
    match operand {
        Measured::Indexed(indexed) => indexed
            .index()
            .matches_ref(axis)
            .then(|| indexed.get(variant))
            .flatten()
            .ok_or_else(|| Misaligned::MissingEntry {
                operand: indexed.index().clone(),
                axis: axis.clone(),
                entry: variant.clone(),
            }),
        Measured::Single(_) => Ok(operand),
    }
}

/// A failing key of a tolerance assertion, with its numeric detail.
struct ToleranceFailure {
    /// Index path from outermost to innermost axis; empty for an unindexed
    /// assertion.
    path: Vec<(IndexTypeRef, IndexEntryKey)>,
    /// `actual X, expected Y +/- T, off by D`.
    detail: String,
}

/// A tolerance computed at runtime is negative: the assertion cannot be
/// decided.
struct NegativeTolerance(String);

impl NegativeTolerance {
    fn result(self) -> AssertResult {
        AssertResult::Error {
            message: format!("tolerance must be non-negative, got {}", self.0),
        }
    }
}

/// Walk aligned tolerance operands, producing the per-key verdicts
/// (mirroring `actual`'s index structure) plus the detail for every failing
/// key.
fn eval_tolerance_tree(
    gauges: &Gauges,
) -> Result<(Verdicts, Vec<ToleranceFailure>), NegativeTolerance> {
    let mut failures = Vec::new();
    let mut path = Vec::new();
    let tree = tolerance_tree_inner(gauges, &mut path, &mut failures)?;
    Ok((tree, failures))
}

fn tolerance_tree_inner(
    gauges: &Gauges,
    path: &mut Vec<(IndexTypeRef, IndexEntryKey)>,
    failures: &mut Vec<ToleranceFailure>,
) -> Result<Verdicts, NegativeTolerance> {
    let Gauge {
        actual: actual_val,
        expected: expected_val,
        tolerance: tolerance_val,
    } = match gauges {
        Gauges::Indexed(indexed) => {
            let index_name = indexed.index();
            let checked = indexed.try_map_ref(|variant, entry| {
                path.push((index_name.clone(), variant.clone()));
                let result = tolerance_tree_inner(entry, path, failures);
                path.pop();
                result
            })?;
            return Ok(Verdicts::Indexed(checked));
        }
        Gauges::Single(gauge) => *gauge,
    };
    let tol_display = format!("{tolerance_val}");

    // A negative tolerance makes the assertion unsatisfiable (even an
    // exact match fails). Statically-known negatives are rejected at
    // check time (#815); this guards tolerances computed at runtime.
    if tolerance_val < 0.0 {
        return Err(NegativeTolerance(tol_display));
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
        let gauges = align(
            Measured::Single(1.0),
            &Measured::Single(1.5),
            &Measured::Single(0.1),
        )
        .unwrap();
        let (verdicts, failures) = eval_tolerance_tree(&gauges).ok().unwrap();
        assert_eq!(verdicts, Verdicts::Single(false));
        assert_eq!(failures.len(), 1);
        let negative = align(
            Measured::Single(1.0),
            &Measured::Single(1.0),
            &Measured::Single(-1.0),
        )
        .unwrap();
        assert!(eval_tolerance_tree(&negative).is_err());
    }

    #[test]
    fn tolerance_operands_broadcast_onto_the_actual_axes() {
        let measured = |values: &[f64]| {
            Measured::Indexed(IndexedValue::finite_for_test(
                values.iter().copied().map(Measured::Single).collect(),
            ))
        };
        let gauges = align(
            measured(&[1.0, 2.0]),
            &measured(&[1.0, 3.0]),
            &Measured::Single(0.5),
        )
        .unwrap();
        let (verdicts, failures) = eval_tolerance_tree(&gauges).ok().unwrap();
        let Verdicts::Indexed(verdicts) = verdicts else {
            panic!("indexed operands give indexed verdicts");
        };
        assert_eq!(
            verdicts.values().as_slice(),
            [Verdicts::Single(true), Verdicts::Single(false)]
        );
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            align(
                Measured::Single(1.0),
                &measured(&[1.0]),
                &Measured::Single(0.5)
            ),
            Err(Misaligned::ExtraAxes { role: "expected" })
        ));
        assert!(matches!(
            align(
                measured(&[1.0, 2.0]),
                &Measured::Single(1.0),
                &measured(&[0.5])
            ),
            Err(Misaligned::MissingEntry { .. })
        ));
    }

    #[test]
    fn per_variant_conditions_must_be_indexed() {
        assert!(Verdicts::try_indexed_from_value(RuntimeValue::Bool(true)).is_err());
        let indexed = IndexedValue::finite_for_test(vec![RuntimeValue::Bool(false)]);
        assert_eq!(
            Verdicts::try_indexed_from_value(RuntimeValue::Indexed(indexed))
                .unwrap()
                .values()
                .as_slice(),
            [Verdicts::Single(false)]
        );
    }
}
