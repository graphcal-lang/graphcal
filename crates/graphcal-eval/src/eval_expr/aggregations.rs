//! Aggregations over the entries of a rank-one indexed value.
//!
//! Every operand arrives read as its checked type (see [`super::operations`]):
//! the quantity aggregations and the extremum keys receive quantities, and
//! `count()` receives entries that are not indexed further.

use crate::invariant::{Failure, Invariant};
use crate::runtime_value::{IndexedValue, KeyValue, RuntimeValue};
use graphcal_compiler::builtin::{KeyAggregation, ValueAggregation};
use graphcal_compiler::finite_value::FiniteQuantity;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// An input quantity or computed aggregate was non-finite.
    #[error(transparent)]
    Quantity(#[from] numeric::QuantityValidationError),
}

/// Evaluate an aggregation function over the quantities of an indexed value.
pub(super) fn aggregate_quantities(
    kind: ValueAggregation,
    entries: &[FiniteQuantity],
) -> Result<RuntimeValue, Failure<AggregationError>> {
    let value = match kind {
        ValueAggregation::Sum => aggregate_sum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Product => aggregate_product(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Minimum => aggregate_minimum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Maximum => aggregate_maximum(entries).map(RuntimeValue::Quantity),
        ValueAggregation::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        ValueAggregation::RootSumSquare => {
            aggregate_root_sum_square(entries).map(RuntimeValue::Quantity)
        }
        ValueAggregation::Count => {
            return count(entries.len())
                .map(RuntimeValue::Int)
                .map_err(Failure::Invariant);
        }
    };
    value.map_err(Failure::Error)
}

/// `count()` of a rank-one indexed value: its number of entries, whatever
/// they are.
pub(super) fn count_entries<V>(indexed: &IndexedValue<V>) -> Result<RuntimeValue, Invariant> {
    count(indexed.values().len()).map(RuntimeValue::Int)
}

/// Key of the extremum element, resolving ties to the first entry in index
/// order (entries iterate in canonical index order).
pub(super) fn extremum_key(
    kind: KeyAggregation,
    indexed: &IndexedValue<FiniteQuantity>,
) -> KeyValue {
    let keys = KeyValue::all(indexed.axis());
    let (first_key, rest_keys) = keys.split_first();
    let (first_value, rest_values) = indexed.values().split_first();
    let (key, _) = rest_keys.iter().zip(rest_values).fold(
        (first_key, *first_value),
        |incumbent, (key, quantity)| {
            let better = match kind {
                KeyAggregation::Argmax => *quantity > incumbent.1,
                KeyAggregation::Argmin => *quantity < incumbent.1,
            };
            if better { (key, *quantity) } else { incumbent }
        },
    );
    key.clone()
}

fn aggregate_sum(entries: &[FiniteQuantity]) -> Result<FiniteQuantity, AggregationError> {
    // The raw total may overflow midway and still be reported once at the end.
    let total = entries.iter().fold(0.0_f64, |acc, value| acc + value.get());
    numeric::computed_finite_quantity(total, "sum()").map_err(AggregationError::from)
}

fn aggregate_product(entries: &[FiniteQuantity]) -> Result<FiniteQuantity, AggregationError> {
    entries
        .iter()
        .try_fold(FiniteQuantity::ONE, |product, value| {
            product.checked_mul(*value).map_err(|error| {
                AggregationError::from(numeric::QuantityValidationError::from_arithmetic(
                    error,
                    "product()",
                ))
            })
        })
}

fn aggregate_root_sum_square(
    entries: &[FiniteQuantity],
) -> Result<FiniteQuantity, AggregationError> {
    let values = entries.iter().map(|value| value.get()).collect::<Vec<_>>();
    numeric::root_sum_square(values, "rss()").map_err(AggregationError::from)
}

fn aggregate_minimum(entries: &[FiniteQuantity]) -> Result<FiniteQuantity, AggregationError> {
    let minimum = entries
        .iter()
        .fold(f64::INFINITY, |acc, value| acc.min(value.get()));
    numeric::computed_finite_quantity(minimum, "minimum()").map_err(AggregationError::from)
}

fn aggregate_maximum(entries: &[FiniteQuantity]) -> Result<FiniteQuantity, AggregationError> {
    let maximum = entries
        .iter()
        .fold(f64::NEG_INFINITY, |acc, value| acc.max(value.get()));
    numeric::computed_finite_quantity(maximum, "maximum()").map_err(AggregationError::from)
}

fn aggregate_mean(entries: &[FiniteQuantity]) -> Result<FiniteQuantity, AggregationError> {
    let values = entries.iter().map(|value| value.get()).collect::<Vec<_>>();
    numeric::exact_mean(&values, "mean()").map_err(AggregationError::from)
}

/// No in-memory axis has more entries than `Int` can count.
fn count(count: usize) -> Result<i64, Invariant> {
    i64::try_from(count).map_err(|_| {
        Invariant::violated(format_args!(
            "count() cardinality {count} cannot be represented as Int"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantities(values: &[f64]) -> IndexedValue<FiniteQuantity> {
        IndexedValue::finite_for_test(
            values
                .iter()
                .map(|value| FiniteQuantity::try_new(*value).unwrap())
                .collect(),
        )
    }

    fn aggregate(
        kind: ValueAggregation,
        values: &[f64],
    ) -> Result<RuntimeValue, Failure<AggregationError>> {
        aggregate_quantities(kind, quantities(values).values().as_slice())
    }

    #[test]
    fn extremum_keys_prefer_the_first_of_tied_entries() {
        let entries = quantities(&[2.0, 1.0, 3.0, 1.0, 3.0]);
        assert_eq!(extremum_key(KeyAggregation::Argmin, &entries).position(), 1);
        assert_eq!(extremum_key(KeyAggregation::Argmax, &entries).position(), 2);
        let single = quantities(&[5.0]);
        assert_eq!(extremum_key(KeyAggregation::Argmax, &single).position(), 0);
    }

    #[test]
    fn sums_and_extrema_aggregate_every_entry() {
        assert!(matches!(
            aggregate(ValueAggregation::Sum, &[1.0, 2.0, 3.5]),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 6.5_f64.to_bits()
        ));
        assert!(matches!(
            aggregate(ValueAggregation::Minimum, &[2.0, -1.0, 3.0]),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == (-1.0_f64).to_bits()
        ));
        assert!(matches!(
            aggregate(ValueAggregation::Maximum, &[2.0, -1.0, 3.0]),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 3.0_f64.to_bits()
        ));
        assert!(matches!(
            aggregate(ValueAggregation::Sum, &[f64::MAX, f64::MAX]),
            Err(Failure::Error(AggregationError::Quantity(_)))
        ));
        assert!(matches!(
            aggregate(ValueAggregation::Count, &[1.0, 2.0]),
            Ok(RuntimeValue::Int(2))
        ));
    }

    #[test]
    fn product_and_rss_evaluate_with_numerical_checks() {
        assert!(matches!(
            aggregate(ValueAggregation::Product, &[3.0, 4.0]),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 12.0_f64.to_bits()
        ));
        assert!(matches!(
            aggregate(ValueAggregation::RootSumSquare, &[3.0, 4.0]),
            Ok(RuntimeValue::Quantity(value)) if value.get().to_bits() == 5.0_f64.to_bits()
        ));
        let RuntimeValue::Quantity(rss) =
            aggregate(ValueAggregation::RootSumSquare, &[1.0e308, 1.0e308]).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(rss.get().is_finite());
        let RuntimeValue::Quantity(small_rss) =
            aggregate(ValueAggregation::RootSumSquare, &[1.0e-300, 1.0e-300]).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(small_rss.get() > 0.0);
        assert!(matches!(
            aggregate(ValueAggregation::Product, &[f64::MAX, 2.0]),
            Err(Failure::Error(AggregationError::Quantity(
                numeric::QuantityValidationError::InfiniteResult { .. }
            )))
        ));
    }

    #[test]
    fn mean_avoids_overflow_in_a_representable_result() {
        let RuntimeValue::Quantity(mean) =
            aggregate(ValueAggregation::Mean, &[1.0e308, 1.0e308]).unwrap()
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
        assert!(matches!(count_entries(&entries), Ok(RuntimeValue::Int(2))));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn count_checks_conversion_to_int() {
        let too_large = usize::try_from(i64::MAX).unwrap() + 1;
        assert_eq!(
            count(too_large).unwrap_err().to_string(),
            format!("count() cardinality {too_large} cannot be represented as Int")
        );
    }
}
