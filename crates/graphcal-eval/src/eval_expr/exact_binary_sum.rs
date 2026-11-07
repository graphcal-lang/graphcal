//! Exact sums of finite binary64 values.
//!
//! Every finite `f64` is an integer multiple of a power of two, so a sum of
//! them is exactly `scaled * 2^exponent` for an integer `scaled`. Adding a
//! value only shifts and adds integers: nothing is rounded and no gcd is taken
//! per term. The total is reduced to a rational once, when it is read.

use std::num::NonZeroUsize;

use graphcal_compiler::finite_value::FiniteQuantity;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Zero};

/// A finite binary64 value as `significand * 2^exponent`, with an odd
/// non-zero significand.
struct Binary {
    significand: i64,
    exponent: i32,
}

impl Binary {
    /// The exact binary decomposition of `value`; `None` for a zero.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "a binary64 significand has at most 53 bits, so negating it and adding its trailing-zero count to its exponent cannot overflow"
    )]
    fn of(value: FiniteQuantity) -> Option<Self> {
        // Exact for normal and subnormal values: value = sign * mantissa * 2^exponent.
        let (mantissa, exponent, sign) = num_traits::Float::integer_decode(value.get());
        if mantissa == 0 {
            return None;
        }
        let trailing = mantissa.trailing_zeros();
        let magnitude = (mantissa >> trailing).cast_signed();
        Some(Self {
            significand: if sign < 0 { -magnitude } else { magnitude },
            exponent: i32::from(exponent) + trailing.cast_signed(),
        })
    }
}

/// The exact sum of finite binary64 values.
///
/// The sum is `scaled * 2^exponent`, where `exponent` is the smallest
/// exponent of any added term, so values of similar magnitude keep `scaled`
/// small.
#[derive(Debug, Clone, Default)]
pub(super) struct ExactBinarySum {
    scaled: BigInt,
    /// The exponent of `scaled`'s unit; `None` until a non-zero term is added.
    exponent: Option<i32>,
}

impl ExactBinarySum {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Add `value` exactly.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "arbitrary-precision integers cannot overflow, and the shifts are differences of binary64 exponents, within [0, 2045]"
    )]
    pub(super) fn add(&mut self, value: FiniteQuantity) {
        let Some(Binary {
            significand,
            exponent,
        }) = Binary::of(value)
        else {
            return;
        };
        let unit = match self.exponent {
            Some(unit) if unit <= exponent => unit,
            Some(unit) => {
                // A finer term: rescale the running sum to its unit.
                self.scaled <<= (unit - exponent).unsigned_abs();
                exponent
            }
            None => exponent,
        };
        self.exponent = Some(unit);
        self.scaled += BigInt::from(significand) << (exponent - unit).unsigned_abs();
    }

    /// The exact total as a reduced rational.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "arbitrary-precision integer shifts cannot overflow"
    )]
    pub(super) fn total(&self) -> BigRational {
        match self.exponent {
            None => BigRational::zero(),
            Some(exponent) if exponent >= 0 => {
                BigRational::from_integer(self.scaled.clone() << exponent.unsigned_abs())
            }
            Some(exponent) => BigRational::new(
                self.scaled.clone(),
                BigInt::one() << exponent.unsigned_abs(),
            ),
        }
    }

    /// The exact mean of `count` added values, as a reduced rational.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "arbitrary-precision rational division by a non-zero count cannot overflow or fail"
    )]
    pub(super) fn mean(&self, count: NonZeroUsize) -> BigRational {
        self.total() / BigRational::from_integer(count.get().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn finite(value: f64) -> FiniteQuantity {
        FiniteQuantity::try_new(value).unwrap()
    }

    /// The sum the accumulator replaces: one exact rational addition (with
    /// its gcd) per term.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "arbitrary-precision rationals cannot overflow; the count is non-zero"
    )]
    fn reference_total(values: &[f64]) -> BigRational {
        values.iter().fold(BigRational::zero(), |total, value| {
            total + BigRational::from_float(*value).unwrap()
        })
    }

    fn total(values: &[f64]) -> BigRational {
        let mut sum = ExactBinarySum::new();
        for value in values {
            sum.add(finite(*value));
        }
        sum.total()
    }

    #[test]
    fn decomposes_every_binary64_class_exactly() {
        let tiny = f64::from_bits(1);
        for value in [
            0.0,
            -0.0,
            1.0,
            -3.0,
            0.1,
            tiny,
            -tiny,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
        ] {
            assert_eq!(
                total(&[value]),
                BigRational::from_float(value).unwrap(),
                "{value:e}"
            );
        }
    }

    #[test]
    fn sums_across_the_whole_exponent_range_exactly() {
        let values = [f64::MAX, f64::from_bits(1), -f64::MAX, 1.0e300, 3.0e-300];
        assert_eq!(total(&values), reference_total(&values));
        assert_eq!(total(&[]), BigRational::zero());
        assert_eq!(total(&[f64::MAX, -f64::MAX]), BigRational::zero());
    }

    proptest! {
        #[test]
        fn matches_per_term_rational_addition(
            values in proptest::collection::vec(
                any::<f64>().prop_filter("finite", |value| value.is_finite()),
                0..48,
            ),
        ) {
            prop_assert_eq!(total(&values), reference_total(&values));
        }
    }
}
