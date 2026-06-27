use graphcal_compiler::builtin::AggregationFn;
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueError};
use graphcal_compiler::syntax::index_name::IndexVariantName;
use indexmap::IndexMap;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// An indexed entry was not scalar-like.
    #[error(transparent)]
    ElementType(#[from] RuntimeValueError),
    /// `mean()` has no identity element.
    #[error("mean() over an empty Indexed value is undefined")]
    EmptyMean,
    /// An input scalar or computed aggregate was non-finite.
    #[error(transparent)]
    Scalar(#[from] numeric::ScalarValidationError),
}

/// Evaluate an aggregation function over indexed entries.
pub(super) fn aggregate_indexed_scalars(
    kind: AggregationFn,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<RuntimeValue, AggregationError> {
    match kind {
        AggregationFn::Sum => aggregate_sum(entries).map(RuntimeValue::Scalar),
        AggregationFn::Min => aggregate_min(entries).map(RuntimeValue::Scalar),
        AggregationFn::Max => aggregate_max(entries).map(RuntimeValue::Scalar),
        AggregationFn::Mean => aggregate_mean(entries).map(RuntimeValue::Scalar),
        AggregationFn::Count => aggregate_count(entries).map(RuntimeValue::Scalar),
    }
}

fn scalar_entry(value: &RuntimeValue, context: &'static str) -> Result<f64, AggregationError> {
    let scalar = value.expect_scalar(context)?;
    numeric::finite_scalar(scalar, context).map_err(AggregationError::from)
}

fn aggregate_sum(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    let total =
        entries
            .values()
            .try_fold(0.0_f64, |acc, value| -> Result<f64, AggregationError> {
                Ok(acc + scalar_entry(value, "sum element")?)
            })?;
    numeric::computed_finite_scalar(total, "sum()").map_err(AggregationError::from)
}

fn aggregate_min(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    let min = entries.values().try_fold(
        f64::INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.min(scalar_entry(value, "min element")?))
        },
    )?;
    numeric::computed_finite_scalar(min, "min()").map_err(AggregationError::from)
}

fn aggregate_max(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    let max = entries.values().try_fold(
        f64::NEG_INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.max(scalar_entry(value, "max element")?))
        },
    )?;
    numeric::computed_finite_scalar(max, "max()").map_err(AggregationError::from)
}

fn aggregate_mean(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyMean);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "indexed collection length fits in f64"
    )]
    let n = entries.len() as f64;
    let total =
        entries
            .values()
            .try_fold(0.0_f64, |acc, value| -> Result<f64, AggregationError> {
                Ok(acc + scalar_entry(value, "mean element")?)
            })?;
    numeric::computed_finite_scalar(total / n, "mean()").map_err(AggregationError::from)
}

fn aggregate_count(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "indexed collection length fits in f64"
    )]
    let count = entries.len() as f64;
    numeric::computed_finite_scalar(count, "count()").map_err(AggregationError::from)
}
