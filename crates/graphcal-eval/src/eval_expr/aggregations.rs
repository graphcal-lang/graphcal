use crate::runtime_value::{IndexedValue, KeyValue, RuntimeValue, RuntimeValueError};
use graphcal_compiler::builtin::{KeyAggregation, ValueAggregation};
use graphcal_compiler::finite_value::FiniteQuantity;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// Rank checking should prevent `count()` from seeing nested indexed entries.
    #[error("count() received a multi-axis Indexed value after rank-one type checking")]
    MultiAxisCount,
    /// A runtime cardinality could not be represented by Graphcal's `Int` type.
    #[error("count() cardinality {count} cannot be represented as Int")]
    CountOutOfRange { count: usize },
    /// An indexed entry was not quantity-like.
    #[error(transparent)]
    ElementType(#[from] RuntimeValueError),
    /// An input quantity or computed aggregate was non-finite.
    #[error(transparent)]
    Quantity(#[from] numeric::QuantityValidationError),
}

impl AggregationError {
    /// Whether this error represents a violation of an invariant enforced before evaluation.
    #[must_use]
    pub(super) const fn is_internal_invariant(&self) -> bool {
        matches!(self, Self::MultiAxisCount | Self::CountOutOfRange { .. })
    }
}

/// Evaluate an aggregation function over indexed entries.
pub(super) fn aggregate_indexed_values(
    kind: ValueAggregation,
    indexed: &IndexedValue<RuntimeValue>,
) -> Result<RuntimeValue, AggregationError> {
    let entries = indexed.values().as_slice();
    match kind {
        ValueAggregation::Sum => aggregate_sum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Product => aggregate_product(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Minimum => aggregate_minimum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Maximum => aggregate_maximum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        ValueAggregation::RootSumSquare => {
            aggregate_root_sum_square(entries).map(RuntimeValue::Quantity)
        }
        ValueAggregation::Count => aggregate_count(entries).map(RuntimeValue::Int),
    }
}

/// Key of the extremum element, resolving ties to the first entry in index
/// order (entries iterate in canonical index order).
pub(super) fn extremum_key(
    kind: KeyAggregation,
    indexed: &IndexedValue<RuntimeValue>,
) -> Result<KeyValue, AggregationError> {
    let context = match kind {
        KeyAggregation::Argmin => "argmin element",
        KeyAggregation::Argmax => "argmax element",
    };
    let keys = KeyValue::all(indexed.axis());
    let (first_key, rest_keys) = keys.split_first();
    let (first_value, rest_values) = indexed.values().split_first();
    let first = (first_key, quantity_entry(first_value, context)?);
    let (key, _) = rest_keys.iter().zip(rest_values).try_fold(
        first,
        |incumbent, (key, value)| -> Result<_, AggregationError> {
            let quantity = quantity_entry(value, context)?;
            let better = match kind {
                KeyAggregation::Argmax => quantity > incumbent.1,
                KeyAggregation::Argmin => quantity < incumbent.1,
            };
            Ok(if better { (key, quantity) } else { incumbent })
        },
    )?;
    Ok(key.clone())
}

fn quantity_entry(
    value: &RuntimeValue,
    context: &'static str,
) -> Result<FiniteQuantity, AggregationError> {
    Ok(value.expect_quantity(context)?)
}

fn aggregate_sum(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    // The raw total may overflow midway and still be reported once at the end.
    let total =
        entries
            .iter()
            .try_fold(0.0_f64, |acc, value| -> Result<f64, AggregationError> {
                Ok(acc + quantity_entry(value, "sum element")?.get())
            })?;
    numeric::computed_finite_quantity(total, "sum()").map_err(AggregationError::from)
}

fn aggregate_product(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    entries
        .iter()
        .try_fold(FiniteQuantity::ONE, |product, value| {
            let value = quantity_entry(value, "product element")?;
            product.checked_mul(value).map_err(|error| {
                AggregationError::from(numeric::QuantityValidationError::from_arithmetic(
                    error,
                    "product()",
                ))
            })
        })
}

fn aggregate_root_sum_square(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    let values = entries
        .iter()
        .map(|value| quantity_entry(value, "rss element").map(FiniteQuantity::get))
        .collect::<Result<Vec<_>, _>>()?;
    numeric::root_sum_square(values, "rss()").map_err(AggregationError::from)
}

fn aggregate_minimum(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    let minimum = entries.iter().try_fold(
        f64::INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.min(quantity_entry(value, "minimum element")?.get()))
        },
    )?;
    numeric::computed_finite_quantity(minimum, "minimum()").map_err(AggregationError::from)
}

fn aggregate_maximum(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    let maximum = entries.iter().try_fold(
        f64::NEG_INFINITY,
        |acc, value| -> Result<f64, AggregationError> {
            Ok(acc.max(quantity_entry(value, "maximum element")?.get()))
        },
    )?;
    numeric::computed_finite_quantity(maximum, "maximum()").map_err(AggregationError::from)
}

fn aggregate_mean(entries: &[RuntimeValue]) -> Result<FiniteQuantity, AggregationError> {
    let values = entries
        .iter()
        .map(|value| quantity_entry(value, "mean element").map(FiniteQuantity::get))
        .collect::<Result<Vec<_>, _>>()?;
    numeric::exact_mean(&values, "mean()").map_err(AggregationError::from)
}

fn aggregate_count(entries: &[RuntimeValue]) -> Result<i64, AggregationError> {
    if entries
        .iter()
        .any(|value| matches!(value, RuntimeValue::Indexed(_)))
    {
        return Err(AggregationError::MultiAxisCount);
    }
    checked_count(entries.len())
}

fn checked_count(count: usize) -> Result<i64, AggregationError> {
    i64::try_from(count).map_err(|_| AggregationError::CountOutOfRange { count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extremum_keys_prefer_the_first_of_tied_entries() {
        let entries = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(2.0).unwrap(),
            RuntimeValue::quantity(1.0).unwrap(),
            RuntimeValue::quantity(3.0).unwrap(),
            RuntimeValue::quantity(1.0).unwrap(),
            RuntimeValue::quantity(3.0).unwrap(),
        ]);
        assert_eq!(
            extremum_key(KeyAggregation::Argmin, &entries)
                .unwrap()
                .position(),
            1
        );
        assert_eq!(
            extremum_key(KeyAggregation::Argmax, &entries)
                .unwrap()
                .position(),
            2
        );
        let single = IndexedValue::finite_for_test(vec![RuntimeValue::quantity(5.0).unwrap()]);
        assert_eq!(
            extremum_key(KeyAggregation::Argmax, &single)
                .unwrap()
                .position(),
            0
        );
        let non_quantity = IndexedValue::finite_for_test(vec![RuntimeValue::Bool(true)]);
        assert!(matches!(
            extremum_key(KeyAggregation::Argmin, &non_quantity),
            Err(AggregationError::ElementType(_))
        ));
    }

    #[test]
    fn product_and_rss_evaluate_with_numerical_checks() {
        let entries = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(3.0).unwrap(),
            RuntimeValue::quantity(4.0).unwrap(),
        ]);
        assert!(matches!(
            aggregate_indexed_values(ValueAggregation::Product, &entries),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 12.0_f64.to_bits()
        ));
        assert!(matches!(
            aggregate_indexed_values(ValueAggregation::RootSumSquare, &entries),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 5.0_f64.to_bits()
        ));

        let large = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(1.0e308).unwrap(),
            RuntimeValue::quantity(1.0e308).unwrap(),
        ]);
        let RuntimeValue::Quantity(rss) =
            aggregate_indexed_values(ValueAggregation::RootSumSquare, &large).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(rss.get().is_finite());

        let small = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(1.0e-300).unwrap(),
            RuntimeValue::quantity(1.0e-300).unwrap(),
        ]);
        let RuntimeValue::Quantity(small_rss) =
            aggregate_indexed_values(ValueAggregation::RootSumSquare, &small).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(small_rss.get() > 0.0);

        let overflowing_product = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(f64::MAX).unwrap(),
            RuntimeValue::quantity(2.0).unwrap(),
        ]);
        assert!(matches!(
            aggregate_indexed_values(ValueAggregation::Product, &overflowing_product),
            Err(AggregationError::Quantity(
                numeric::QuantityValidationError::InfiniteResult { .. }
            ))
        ));
    }

    #[test]
    fn mean_avoids_overflow_in_a_representable_result() {
        let entries = IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(1.0e308).unwrap(),
            RuntimeValue::quantity(1.0e308).unwrap(),
        ]);
        let RuntimeValue::Quantity(mean) =
            aggregate_indexed_values(ValueAggregation::Mean, &entries).unwrap()
        else {
            panic!("mean must return a quantity");
        };
        assert!((mean.get() / 1.0e308 - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn count_accepts_non_quantity_entries_and_returns_int() {
        let entries = IndexedValue::finite_for_test(vec![
            RuntimeValue::Bool(false),
            RuntimeValue::Bool(true),
        ]);
        assert!(matches!(
            aggregate_indexed_values(ValueAggregation::Count, &entries),
            Ok(RuntimeValue::Int(2))
        ));
    }

    #[test]
    fn count_defensively_rejects_nested_indexed_entries() {
        let inner = RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![
            RuntimeValue::quantity(1.0).unwrap(),
        ]));
        let entries = IndexedValue::finite_for_test(vec![inner]);
        let error = aggregate_indexed_values(ValueAggregation::Count, &entries).unwrap_err();
        assert!(matches!(&error, AggregationError::MultiAxisCount));
        assert!(error.is_internal_invariant());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn count_checks_conversion_to_int() {
        let too_large = usize::try_from(i64::MAX).unwrap() + 1;
        assert!(matches!(
            checked_count(too_large),
            Err(AggregationError::CountOutOfRange { count }) if count == too_large
        ));
    }
}
