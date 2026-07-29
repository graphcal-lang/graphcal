use super::builtin_call::AggregationFn;
use graphcal_compiler::builtin::BuiltinFnName;
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueError};
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use indexmap::IndexMap;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// An indexed entry was not quantity-like.
    #[error(transparent)]
    ElementType(#[from] RuntimeValueError),
    /// `minimum()` / `maximum()` have no identity element.
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
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<RuntimeValue, AggregationError> {
    match kind {
        AggregationFn::Sum => aggregate_sum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Minimum => aggregate_minimum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Maximum => aggregate_maximum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        AggregationFn::Count => aggregate_count(entries).map(RuntimeValue::Quantity),
    }
}

fn quantity_entry(value: &RuntimeValue, context: &'static str) -> Result<f64, AggregationError> {
    let quantity = value.expect_quantity(context)?;
    numeric::finite_quantity(quantity, context).map_err(AggregationError::from)
}

fn aggregate_sum(entries: &IndexMap<IndexEntryKey, RuntimeValue>) -> Result<f64, AggregationError> {
    let total =
        entries
            .values()
            .try_fold(0.0_f64, |acc, value| -> Result<f64, AggregationError> {
                Ok(acc + quantity_entry(value, "sum element")?)
            })?;
    numeric::computed_finite_quantity(total, "sum()").map_err(AggregationError::from)
}

fn aggregate_minimum(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<f64, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyExtremum {
            function: BuiltinFnName::Minimum.as_str(),
        });
    }
    let minimum = entries.values().try_fold(
        f64::INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.min(quantity_entry(value, "minimum element")?))
        },
    )?;
    numeric::computed_finite_quantity(minimum, "minimum()").map_err(AggregationError::from)
}

fn aggregate_maximum(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<f64, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyExtremum {
            function: BuiltinFnName::Maximum.as_str(),
        });
    }
    let maximum = entries.values().try_fold(
        f64::NEG_INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.max(quantity_entry(value, "maximum element")?))
        },
    )?;
    numeric::computed_finite_quantity(maximum, "maximum()").map_err(AggregationError::from)
}

fn aggregate_mean(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
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
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
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
    fn empty_minimum_maximum_report_empty_collection() {
        let entries = IndexMap::new();
        assert!(matches!(
            aggregate_minimum(&entries),
            Err(AggregationError::EmptyExtremum {
                function: "minimum"
            })
        ));
        assert!(matches!(
            aggregate_maximum(&entries),
            Err(AggregationError::EmptyExtremum {
                function: "maximum"
            })
        ));
    }
}
