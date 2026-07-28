use super::builtin_call::AggregationFn;
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueError};
use graphcal_compiler::syntax::index_name::IndexVariantName;
use indexmap::IndexMap;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// An indexed entry was not quantity-like.
    #[error(transparent)]
    ElementType(#[from] RuntimeValueError),
    /// `min()` / `max()` have no identity element.
    #[error("{function}() over an empty Indexed value is undefined")]
    EmptyExtremum { function: &'static str },
    /// `mean()` has no identity element.
    #[error("mean() over an empty Indexed value is undefined")]
    EmptyMean,
    /// An input quantity or computed aggregate was non-finite.
    #[error(transparent)]
    Quantity(#[from] numeric::QuantityValidationError),
}

/// Evaluate an aggregation function over indexed entries.
pub(super) fn aggregate_indexed_quantities(
    kind: AggregationFn,
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<RuntimeValue, AggregationError> {
    match kind {
        AggregationFn::Sum => aggregate_sum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Min => aggregate_min(entries).map(RuntimeValue::Quantity),
        AggregationFn::Max => aggregate_max(entries).map(RuntimeValue::Quantity),
        AggregationFn::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        AggregationFn::Count => aggregate_count(entries).map(RuntimeValue::Quantity),
    }
}

fn quantity_entry(value: &RuntimeValue, context: &'static str) -> Result<f64, AggregationError> {
    let quantity = value.expect_quantity(context)?;
    numeric::finite_quantity(quantity, context).map_err(AggregationError::from)
}

fn aggregate_sum(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    let total =
        entries
            .values()
            .try_fold(0.0_f64, |acc, value| -> Result<f64, AggregationError> {
                Ok(acc + quantity_entry(value, "sum element")?)
            })?;
    numeric::computed_finite_quantity(total, "sum()").map_err(AggregationError::from)
}

fn aggregate_min(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyExtremum { function: "min" });
    }
    let min = entries.values().try_fold(
        f64::INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.min(quantity_entry(value, "min element")?))
        },
    )?;
    numeric::computed_finite_quantity(min, "min()").map_err(AggregationError::from)
}

fn aggregate_max(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyExtremum { function: "max" });
    }
    let max = entries.values().try_fold(
        f64::NEG_INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.max(quantity_entry(value, "max element")?))
        },
    )?;
    numeric::computed_finite_quantity(max, "max()").map_err(AggregationError::from)
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
                Ok(acc + quantity_entry(value, "mean element")?)
            })?;
    numeric::computed_finite_quantity(total / n, "mean()").map_err(AggregationError::from)
}

fn aggregate_count(
    entries: &IndexMap<IndexVariantName, RuntimeValue>,
) -> Result<f64, AggregationError> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "indexed collection length fits in f64"
    )]
    let count = entries.len() as f64;
    numeric::computed_finite_quantity(count, "count()").map_err(AggregationError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_min_max_report_empty_collection() {
        let entries = IndexMap::new();
        assert!(matches!(
            aggregate_min(&entries),
            Err(AggregationError::EmptyExtremum { function: "min" })
        ));
        assert!(matches!(
            aggregate_max(&entries),
            Err(AggregationError::EmptyExtremum { function: "max" })
        ));
    }
}
