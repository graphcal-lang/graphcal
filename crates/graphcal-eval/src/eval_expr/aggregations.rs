//! Aggregations over the entries of a rank-one indexed value.
//!
//! Every operand arrives read as its checked type (see [`super::operations`]):
//! the quantity aggregations and the extremum keys receive quantities, and
//! `count()` receives entries that are not indexed further.

use std::num::NonZeroUsize;

use crate::invariant::{Failure, Invariant};
use crate::runtime_value::{IndexedValue, KeyValue, RuntimeValue};
use graphcal_compiler::builtin::{KeyAggregation, ValueAggregation};
use graphcal_compiler::cancellation::Cancelled;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::outcome::Outcome;
use thiserror::Error;

use super::exact_binary_sum::ExactBinarySum;
use super::kernel_checkpoint::KernelCheckpoint;
use super::numeric;

/// Error produced by pure aggregation evaluation.
#[derive(Debug, Error)]
pub(super) enum AggregationError {
    /// An input quantity or computed aggregate was non-finite.
    #[error(transparent)]
    Quantity(#[from] numeric::QuantityValidationError),
}

/// Why an aggregation stopped: cancellation or a user-facing failure.
type AggregationStop = Outcome<AggregationError>;

fn quantity_error(error: numeric::QuantityValidationError) -> AggregationStop {
    Outcome::Failed(AggregationError::from(error))
}

/// Evaluate an aggregation function over the quantities of an indexed value,
/// stepping `control` once per entry.
pub(super) fn aggregate_quantities(
    kind: ValueAggregation,
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<RuntimeValue, Outcome<Failure<AggregationError>>> {
    let value = match kind {
        ValueAggregation::Sum => aggregate_sum(entries, control),
        ValueAggregation::Product => aggregate_product(entries, control),
        ValueAggregation::Minimum => aggregate_minimum(entries, control),
        ValueAggregation::Maximum => aggregate_maximum(entries, control),
        ValueAggregation::Mean => aggregate_mean(entries, control),
        ValueAggregation::RootSumSquare => aggregate_root_sum_square(entries, control),
        ValueAggregation::Count => {
            return count(entries.len())
                .map(RuntimeValue::Int)
                .map_err(Into::into);
        }
    };
    value
        .map(RuntimeValue::Quantity)
        .map_err(|stop| stop.map_failed(Failure::Error))
}

/// `count()` of a rank-one indexed value: its number of entries, whatever
/// they are.
pub(super) fn count_entries<V>(indexed: &IndexedValue<V>) -> Result<RuntimeValue, Invariant> {
    count(indexed.values().len()).map(RuntimeValue::Int)
}

/// Key of the extremum element, resolving ties to the first entry in index
/// order (entries iterate in canonical index order). Only the winning key is
/// built.
pub(super) fn extremum_key(
    kind: KeyAggregation,
    indexed: &IndexedValue<FiniteQuantity>,
    control: &mut KernelCheckpoint<'_>,
) -> Result<KeyValue, Outcome<Invariant>> {
    let (first, rest) = indexed.values().split_first();
    let (position, _) =
        rest.iter()
            .zip(1..)
            .try_fold((0, *first), |incumbent, (quantity, position)| {
                control.step()?;
                let better = match kind {
                    KeyAggregation::Argmax => *quantity > incumbent.1,
                    KeyAggregation::Argmin => *quantity < incumbent.1,
                };
                Ok::<_, Cancelled>(if better {
                    (position, *quantity)
                } else {
                    incumbent
                })
            })?;
    KeyValue::at(indexed.axis().clone(), position).ok_or_else(|| {
        Outcome::Failed(Invariant::violated(format_args!(
            "extremum position {position} is not a key of its axis"
        )))
    })
}

fn aggregate_sum(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    // The raw total may overflow midway and still be reported once at the end.
    let total = entries.iter().try_fold(0.0_f64, |acc, value| {
        control.step()?;
        Ok::<_, Cancelled>(acc + value.get())
    })?;
    numeric::computed_finite_quantity(total, "sum()").map_err(quantity_error)
}

fn aggregate_product(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    entries
        .iter()
        .try_fold(FiniteQuantity::ONE, |product, value| {
            control.step()?;
            product.checked_mul(*value).map_err(|error| {
                quantity_error(numeric::QuantityValidationError::from_arithmetic(
                    error,
                    "product()",
                ))
            })
        })
}

fn aggregate_root_sum_square(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    entries
        .iter()
        .try_fold(numeric::RootSumSquare::new(), |mut accumulator, value| {
            control.step()?;
            accumulator
                .add(value.get(), "rss()")
                .map_err(quantity_error)?;
            Ok::<_, AggregationStop>(accumulator)
        })?
        .finish("rss()")
        .map_err(quantity_error)
}

fn aggregate_minimum(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    let minimum = entries.iter().try_fold(f64::INFINITY, |acc, value| {
        control.step()?;
        Ok::<_, Cancelled>(acc.min(value.get()))
    })?;
    numeric::computed_finite_quantity(minimum, "minimum()").map_err(quantity_error)
}

fn aggregate_maximum(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    let maximum = entries.iter().try_fold(f64::NEG_INFINITY, |acc, value| {
        control.step()?;
        Ok::<_, Cancelled>(acc.max(value.get()))
    })?;
    numeric::computed_finite_quantity(maximum, "maximum()").map_err(quantity_error)
}

/// Average the exact binary64 inputs, rounding only the final quotient.
///
/// A common floating scale can erase a small term before cancellation reveals
/// it. The exact sum retains that term and also avoids overflowing a
/// representable mean's intermediate sum. This is not decimal reinterpretation.
fn aggregate_mean(
    entries: &[FiniteQuantity],
    control: &mut KernelCheckpoint<'_>,
) -> Result<FiniteQuantity, AggregationStop> {
    let Some(count) = NonZeroUsize::new(entries.len()) else {
        return Err(quantity_error(
            numeric::QuantityValidationError::NanResult {
                context: "mean()".to_string(),
            },
        ));
    };
    let mut sum = ExactBinarySum::new();
    for entry in entries {
        control.step()?;
        sum.add(*entry);
    }
    numeric::rounded_rational(&sum.mean(count), "mean()").map_err(quantity_error)
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
    use graphcal_compiler::cancellation::CancellationToken;

    fn quantities(values: &[f64]) -> IndexedValue<FiniteQuantity> {
        IndexedValue::finite_for_test(
            values
                .iter()
                .map(|value| FiniteQuantity::try_new(*value).unwrap())
                .collect(),
        )
    }

    const UNBOUNDED: CancellationToken = CancellationToken::unbounded();

    fn aggregate(
        kind: ValueAggregation,
        values: &[f64],
    ) -> Result<RuntimeValue, Failure<AggregationError>> {
        aggregate_quantities(
            kind,
            quantities(values).values().as_slice(),
            &mut KernelCheckpoint::new(&UNBOUNDED),
        )
        .map_err(|outcome| match outcome {
            Outcome::Failed(failure) => failure,
            Outcome::Cancelled => panic!("an unbounded aggregation cannot be cancelled"),
        })
    }

    fn extremum(kind: KeyAggregation, entries: &IndexedValue<FiniteQuantity>) -> usize {
        extremum_key(kind, entries, &mut KernelCheckpoint::new(&UNBOUNDED))
            .unwrap()
            .position()
    }

    #[test]
    fn extremum_keys_prefer_the_first_of_tied_entries() {
        let entries = quantities(&[2.0, 1.0, 3.0, 1.0, 3.0]);
        assert_eq!(extremum(KeyAggregation::Argmin, &entries), 1);
        assert_eq!(extremum(KeyAggregation::Argmax, &entries), 2);
        let single = quantities(&[5.0]);
        assert_eq!(extremum(KeyAggregation::Argmax, &single), 0);
    }

    #[test]
    fn aggregations_observe_cancellation() {
        let source = graphcal_compiler::cancellation::CancellationSource::new();
        source.cancel();
        let token = source.token();
        let entries = quantities(&[1.0, 2.0]);
        for kind in [
            ValueAggregation::Sum,
            ValueAggregation::Product,
            ValueAggregation::Minimum,
            ValueAggregation::Maximum,
            ValueAggregation::Mean,
            ValueAggregation::RootSumSquare,
        ] {
            assert!(
                matches!(
                    aggregate_quantities(
                        kind,
                        entries.values().as_slice(),
                        &mut KernelCheckpoint::new(&token),
                    ),
                    Err(Outcome::Cancelled)
                ),
                "{kind:?} ignored cancellation"
            );
        }
        for kind in [KeyAggregation::Argmin, KeyAggregation::Argmax] {
            assert_eq!(
                extremum_key(kind, &entries, &mut KernelCheckpoint::new(&token)),
                Err(Outcome::Cancelled)
            );
        }
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

    /// The bits of `mean()` over `values`.
    fn mean_bits(values: &[f64]) -> u64 {
        let RuntimeValue::Quantity(mean) = aggregate(ValueAggregation::Mean, values).unwrap()
        else {
            panic!("mean must return a quantity");
        };
        mean.get().to_bits()
    }

    /// The `mean()` this accumulator replaced: one exact rational addition
    /// (with its gcd) per term, then one rounded division.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "arbitrary-precision rationals cannot overflow; the count is non-zero"
    )]
    fn reference_mean_bits(values: &[f64]) -> u64 {
        use num_rational::BigRational;
        use num_traits::ToPrimitive;
        let total = values.iter().fold(BigRational::default(), |total, value| {
            total + BigRational::from_float(*value).unwrap()
        });
        let count = BigRational::from_integer(values.len().into());
        (total / count).to_f64().unwrap().to_bits()
    }

    proptest::proptest! {
        #[test]
        fn mean_matches_independent_bounded_integer_reference(
            integers in proptest::collection::vec(-1_000_000_i32..=1_000_000, 1..32),
        ) {
            // Both integer accumulation and conversion are exact in this range;
            // the independent reference performs one binary64 division.
            let total: i32 = integers.iter().sum();
            let count = i32::try_from(integers.len()).unwrap();
            let values = integers.into_iter().map(f64::from).collect::<Vec<_>>();
            proptest::prop_assert_eq!(
                mean_bits(&values),
                (f64::from(total) / f64::from(count)).to_bits()
            );
        }

        /// The scaled-integer accumulator leaves every `mean()` bit-identical
        /// to the per-term rational sum it replaced (#2251).
        #[test]
        fn mean_is_bit_identical_to_the_per_term_rational_sum(
            values in proptest::collection::vec(
                proptest::num::f64::NORMAL
                    | proptest::num::f64::SUBNORMAL
                    | proptest::num::f64::ZERO
                    | proptest::num::f64::POSITIVE
                    | proptest::num::f64::NEGATIVE,
                1..48,
            ),
        ) {
            let values = values
                .into_iter()
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            proptest::prop_assume!(!values.is_empty());
            // A mean whose rounding overflows is an error in both.
            let reference = f64::from_bits(reference_mean_bits(&values));
            proptest::prop_assume!(reference.is_finite());
            proptest::prop_assert_eq!(mean_bits(&values), reference.to_bits());
        }
    }

    #[test]
    fn mean_is_bit_identical_on_extreme_alternating_magnitudes() {
        let values = (0..4_096)
            .map(|index| if index % 2 == 0 { 1.0e300 } else { 3.0e-300 })
            .chain([f64::MAX, -f64::MAX, f64::from_bits(1), -0.0])
            .collect::<Vec<_>>();
        assert_eq!(mean_bits(&values), reference_mean_bits(&values));
    }

    #[test]
    fn cancellation_preserves_a_small_representable_mean_in_every_order() {
        for values in [
            [1.0e308, 1.0e-100, -1.0e308],
            [1.0e-100, -1.0e308, 1.0e308],
            [-1.0e308, 1.0e308, 1.0e-100],
            [1.0e308, -1.0e308, 1.0e-100],
            [-1.0e308, 1.0e-100, 1.0e308],
            [1.0e-100, 1.0e308, -1.0e308],
        ] {
            assert_eq!(mean_bits(&values), (1.0e-100_f64 / 3.0).to_bits());
        }
        assert_eq!(mean_bits(&[f64::MAX, f64::MAX]), f64::MAX.to_bits());
        assert_eq!(
            mean_bits(&[f64::MAX, f64::MAX, 1.0e-100, -f64::MAX, -f64::MAX]),
            (1.0e-100_f64 / 5.0).to_bits()
        );
    }

    #[test]
    fn mean_rounds_subnormals_once_and_rejects_an_empty_input() {
        let tiny = f64::from_bits(1);
        for value in [tiny, -tiny, f64::MIN_POSITIVE, f64::MAX] {
            assert_eq!(mean_bits(&[value, value]), value.to_bits());
        }
        assert_eq!(mean_bits(&[tiny, f64::from_bits(2)]), 2);
        assert_eq!(mean_bits(&[tiny, -tiny]), 0.0_f64.to_bits());
        assert!(matches!(
            aggregate_quantities(
                ValueAggregation::Mean,
                &[],
                &mut KernelCheckpoint::new(&UNBOUNDED),
            ),
            Err(Outcome::Failed(Failure::Error(_)))
        ));
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
