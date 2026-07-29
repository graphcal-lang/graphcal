//! Exact rational values preserved from source syntax.
//!
//! Value-level power expressions need a wider exact representation than the
//! dimension algebra: an exact exponent can be meaningful for a dimensionless
//! base even when it does not fit the dimension model's `i32` exponents. This
//! type keeps the source value reduced and exact until dimensional analysis
//! decides whether narrowing is required.

use std::fmt;

use thiserror::Error;

/// A reduced exact rational with a positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExactRational {
    numerator: i64,
    denominator: i64,
}

impl ExactRational {
    /// Construct a reduced rational.
    ///
    /// # Errors
    ///
    /// Returns [`ExactRationalError::ZeroDenominator`] for a zero denominator,
    /// or [`ExactRationalError::Overflow`] when sign normalization cannot fit
    /// back into `i64`.
    pub fn try_new(numerator: i64, denominator: i64) -> Result<Self, ExactRationalError> {
        Self::try_new_i128(i128::from(numerator), i128::from(denominator))
    }

    /// Construct from widened parser arithmetic, reducing before narrowing.
    pub(crate) fn try_new_i128(
        numerator: i128,
        denominator: i128,
    ) -> Result<Self, ExactRationalError> {
        if denominator == 0 {
            return Err(ExactRationalError::ZeroDenominator);
        }
        if numerator == 0 {
            return Ok(Self::from_integer(0));
        }

        let divisor = gcd_u128(numerator.unsigned_abs(), denominator.unsigned_abs());
        let divisor = i128::try_from(divisor).map_err(|_| ExactRationalError::Overflow)?;
        let mut reduced_numerator = numerator / divisor;
        let mut reduced_denominator = denominator / divisor;
        if reduced_denominator < 0 {
            reduced_numerator = reduced_numerator
                .checked_neg()
                .ok_or(ExactRationalError::Overflow)?;
            reduced_denominator = reduced_denominator
                .checked_neg()
                .ok_or(ExactRationalError::Overflow)?;
        }

        Ok(Self {
            numerator: i64::try_from(reduced_numerator)
                .map_err(|_| ExactRationalError::Overflow)?,
            denominator: i64::try_from(reduced_denominator)
                .map_err(|_| ExactRationalError::Overflow)?,
        })
    }

    /// Construct an integer as an exact rational.
    #[must_use]
    pub const fn from_integer(value: i64) -> Self {
        Self {
            numerator: value,
            denominator: 1,
        }
    }

    /// Numerator of the reduced value.
    #[must_use]
    pub const fn numerator(self) -> i64 {
        self.numerator
    }

    /// Positive denominator of the reduced value.
    #[must_use]
    pub const fn denominator(self) -> i64 {
        self.denominator
    }

    /// Whether the value is an integer.
    #[must_use]
    pub const fn is_integer(self) -> bool {
        self.denominator == 1
    }

    /// Negate this value exactly.
    ///
    /// # Errors
    ///
    /// Returns [`ExactRationalError::Overflow`] for `i64::MIN`.
    pub fn checked_neg(self) -> Result<Self, ExactRationalError> {
        Ok(Self {
            numerator: self
                .numerator
                .checked_neg()
                .ok_or(ExactRationalError::Overflow)?,
            denominator: self.denominator,
        })
    }

    /// Convert to the binary64 approximation used for runtime arithmetic.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "the exact metadata is retained separately; binary64 is the runtime quantity representation"
    )]
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    /// Raise a binary64 value to this exact rational power while preserving
    /// real odd-root semantics for negative bases.
    ///
    /// # Errors
    ///
    /// Returns [`ExactPowerError::EvenRootOfNegative`] when a negative base
    /// has an even reduced denominator and therefore no real result.
    pub fn pow_f64(self, base: f64) -> Result<f64, ExactPowerError> {
        if self.is_integer()
            && let Ok(integer) = i32::try_from(self.numerator)
        {
            return Ok(base.powi(integer));
        }
        if base >= 0.0 {
            return Ok(base.powf(self.as_f64()));
        }
        if self.denominator % 2 == 0 {
            return Err(ExactPowerError::EvenRootOfNegative);
        }
        let magnitude = (-base).powf(self.as_f64());
        Ok(if self.numerator % 2 == 0 {
            magnitude
        } else {
            -magnitude
        })
    }
}

impl fmt::Display for ExactRational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_integer() {
            write!(f, "{}", self.numerator)
        } else {
            write!(f, "{}/{}", self.numerator, self.denominator)
        }
    }
}

/// Failure to construct or transform an [`ExactRational`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExactRationalError {
    /// The denominator was zero.
    #[error("denominator must not be zero")]
    ZeroDenominator,
    /// The normalized result did not fit in `i64`.
    #[error("exact rational overflowed i64")]
    Overflow,
}

/// A real-valued exact power could not be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExactPowerError {
    /// A negative real has no real even-denominator root.
    #[error("a negative base has no real value for an exact exponent with an even denominator")]
    EvenRootOfNegative,
}

const fn gcd_u128(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd_u128(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_normalizes_sign() {
        assert_eq!(
            ExactRational::try_new(6, -4).unwrap(),
            ExactRational {
                numerator: -3,
                denominator: 2,
            }
        );
        assert_eq!(
            ExactRational::try_new(0, -9).unwrap(),
            ExactRational::from_integer(0)
        );
    }

    #[test]
    fn rejects_zero_denominator() {
        assert_eq!(
            ExactRational::try_new(1, 0),
            Err(ExactRationalError::ZeroDenominator)
        );
    }
}
