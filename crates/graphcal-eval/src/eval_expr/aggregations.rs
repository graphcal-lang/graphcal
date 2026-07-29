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
    /// A completed indexed value violated the language's non-empty invariant.
    #[error(
        "{function}() received an empty completed Indexed value, violating the non-empty invariant"
    )]
    EmptyInput { function: BuiltinFnName },
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
        matches!(
            self,
            Self::EmptyInput { .. } | Self::MultiAxisCount | Self::CountOutOfRange { .. }
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
        AggregationFn::Minimum => aggregate_minimum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Maximum => aggregate_maximum(entries).map(RuntimeValue::Quantity),
        AggregationFn::Mean => aggregate_mean(entries).map(RuntimeValue::Quantity),
        AggregationFn::Count => aggregate_count(entries).map(RuntimeValue::Int),
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
            AggregationFn::Minimum,
            AggregationFn::Maximum,
            AggregationFn::Mean,
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
