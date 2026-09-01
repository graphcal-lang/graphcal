use graphcal_compiler::builtin::{AggregationFn, BuiltinFnName};
use graphcal_compiler::registry::runtime_value::{RuntimeValue, RuntimeValueError};
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use indexmap::IndexMap;
use thiserror::Error;

use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// A completed indexed value violated the language's non-empty invariant.
    #[error(
        "{function}() received an empty completed Indexed value, violating the non-empty invariant"
    )]
    EmptyInput { function: BuiltinFnName },
    /// Rank checking should prevent `count()` from seeing nested indexed entries.
    #[error("count() received a multi-axis Indexed value after rank-one type checking")]
    MultiAxisCount,
    /// `argmin`/`argmax` construct axis keys and are routed through the
    /// extremum-key evaluation, never through the value-only kernel.
    #[error("{function}() must be evaluated through extremum-key routing")]
    ExtremumRouting { function: BuiltinFnName },
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
        matches!(
            self,
            Self::EmptyInput { .. }
                | Self::MultiAxisCount
                | Self::CountOutOfRange { .. }
                | Self::ExtremumRouting { .. }
        )
    }
}

/// Evaluate an aggregation function over indexed entries.
pub(super) fn aggregate_indexed_values(
    kind: AggregationFn,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<RuntimeValue, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyInput {
            function: kind.builtin_name(),
        });
    }

    match kind {
        AggregationFn::Sum => aggregate_sum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Product => aggregate_product(entries).map(RuntimeValue::Quantity),
        AggregationFn::Minimum => aggregate_minimum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Maximum => aggregate_maximum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        AggregationFn::RootSumSquare => {
            aggregate_root_sum_square(entries).map(RuntimeValue::Quantity)
        }
        AggregationFn::Count => aggregate_count(entries).map(RuntimeValue::Int),
        AggregationFn::Argmin | AggregationFn::Argmax => Err(AggregationError::ExtremumRouting {
            function: kind.builtin_name(),
        }),
    }
}

/// Entry key of the extremum element, resolving ties to the first entry in
/// index order (entries iterate in canonical index order).
pub(super) fn extremum_entry_key(
    kind: AggregationFn,
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<IndexEntryKey, AggregationError> {
    if entries.is_empty() {
        return Err(AggregationError::EmptyInput {
            function: kind.builtin_name(),
        });
    }
    let context = match kind {
        AggregationFn::Argmin => "argmin element",
        AggregationFn::Argmax => "argmax element",
        _ => {
            return Err(AggregationError::ExtremumRouting {
                function: kind.builtin_name(),
            });
        }
    };
    let mut best: Option<(&IndexEntryKey, f64)> = None;
    for (key, value) in entries {
        let quantity = quantity_entry(value, context)?;
        let better = match (&best, kind) {
            (None, _) => true,
            (Some((_, incumbent)), AggregationFn::Argmax) => quantity > *incumbent,
            (Some((_, incumbent)), _) => quantity < *incumbent,
        };
        if better {
            best = Some((key, quantity));
        }
    }
    match best {
        Some((key, _)) => Ok(key.clone()),
        None => Err(AggregationError::EmptyInput {
            function: kind.builtin_name(),
        }),
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

fn aggregate_product(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<f64, AggregationError> {
    entries.values().try_fold(1.0_f64, |product, value| {
        let value = quantity_entry(value, "product element")?;
        let result = product * value;
        if product != 0.0 && value != 0.0 {
            numeric::computed_nonzero_quantity(result, "product()").map_err(AggregationError::from)
        } else {
            numeric::computed_finite_quantity(result, "product()").map_err(AggregationError::from)
        }
    })
}

fn aggregate_root_sum_square(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<f64, AggregationError> {
    let values = entries
        .values()
        .map(|value| quantity_entry(value, "rss element"))
        .collect::<Result<Vec<_>, _>>()?;
    numeric::root_sum_square(values, "rss()").map_err(AggregationError::from)
}

fn aggregate_minimum(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<f64, AggregationError> {
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
    let values = entries
        .values()
        .map(|value| quantity_entry(value, "mean element"))
        .collect::<Result<Vec<_>, _>>()?;
    numeric::scaled_mean(&values, "mean()").map_err(AggregationError::from)
}

fn aggregate_count(
    entries: &IndexMap<IndexEntryKey, RuntimeValue>,
) -> Result<i64, AggregationError> {
    if entries
        .values()
        .any(|value| matches!(value, RuntimeValue::Indexed { .. }))
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
    fn every_aggregation_rejects_empty_completed_indexed_values_as_internal() {
        let entries = IndexMap::new();
        for function in [
            AggregationFn::Sum,
            AggregationFn::Product,
            AggregationFn::Minimum,
            AggregationFn::Maximum,
            AggregationFn::Mean,
            AggregationFn::RootSumSquare,
            AggregationFn::Count,
        ] {
            let error = aggregate_indexed_values(function, &entries).unwrap_err();
            assert!(matches!(
                &error,
                AggregationError::EmptyInput { function: found }
                    if *found == function.builtin_name()
            ));
            assert!(error.is_internal_invariant());
        }
    }

    #[test]
    fn product_and_rss_evaluate_with_numerical_checks() {
        let entries = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Quantity(3.0)),
            (IndexEntryKey::position(1), RuntimeValue::Quantity(4.0)),
        ]);
        assert!(matches!(
            aggregate_indexed_values(AggregationFn::Product, &entries),
            Ok(RuntimeValue::Quantity(12.0))
        ));
        assert!(matches!(
            aggregate_indexed_values(AggregationFn::RootSumSquare, &entries),
            Ok(RuntimeValue::Quantity(5.0))
        ));

        let large = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Quantity(1.0e308)),
            (IndexEntryKey::position(1), RuntimeValue::Quantity(1.0e308)),
        ]);
        let RuntimeValue::Quantity(rss) =
            aggregate_indexed_values(AggregationFn::RootSumSquare, &large).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(rss.is_finite());

        let small = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Quantity(1.0e-300)),
            (IndexEntryKey::position(1), RuntimeValue::Quantity(1.0e-300)),
        ]);
        let RuntimeValue::Quantity(small_rss) =
            aggregate_indexed_values(AggregationFn::RootSumSquare, &small).unwrap()
        else {
            panic!("rss must return a quantity");
        };
        assert!(small_rss > 0.0);

        let overflowing_product = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Quantity(f64::MAX)),
            (IndexEntryKey::position(1), RuntimeValue::Quantity(2.0)),
        ]);
        assert!(matches!(
            aggregate_indexed_values(AggregationFn::Product, &overflowing_product),
            Err(AggregationError::Quantity(
                numeric::QuantityValidationError::InfiniteResult { .. }
            ))
        ));
    }

    #[test]
    fn mean_avoids_overflow_in_a_representable_result() {
        let entries = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Quantity(1.0e308)),
            (IndexEntryKey::position(1), RuntimeValue::Quantity(1.0e308)),
        ]);
        let RuntimeValue::Quantity(mean) =
            aggregate_indexed_values(AggregationFn::Mean, &entries).unwrap()
        else {
            panic!("mean must return a quantity");
        };
        assert!((mean / 1.0e308 - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn count_accepts_non_quantity_entries_and_returns_int() {
        let entries = IndexMap::from([
            (IndexEntryKey::position(0), RuntimeValue::Bool(false)),
            (IndexEntryKey::position(1), RuntimeValue::Bool(true)),
        ]);
        assert!(matches!(
            aggregate_indexed_values(AggregationFn::Count, &entries),
            Ok(RuntimeValue::Int(2))
        ));
    }

    #[test]
    fn count_defensively_rejects_nested_indexed_entries() {
        use graphcal_compiler::registry::declared_type::IndexTypeRef;
        use graphcal_compiler::registry::index::FiniteIndex;

        let inner = RuntimeValue::Indexed {
            index_name: IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(1).unwrap()),
            entries: IndexMap::from([(IndexEntryKey::position(0), RuntimeValue::Quantity(1.0))]),
        };
        let entries = IndexMap::from([(IndexEntryKey::position(0), inner)]);
        let error = aggregate_indexed_values(AggregationFn::Count, &entries).unwrap_err();
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
