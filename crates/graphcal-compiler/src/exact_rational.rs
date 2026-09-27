//! Exact rational values preserved from source syntax.
//!
//! Value-level power expressions need a wider exact representation than the
//! dimension algebra: an exact exponent can be meaningful for a dimensionless
//! base even when it does not fit the dimension model's `i32` exponents. This
//! type keeps the source value reduced and exact until dimensional analysis
//! decides whether narrowing is required.

use thiserror::Error;

use crate::ratio::Ratio;

/// A reduced exact rational with a positive denominator, in the symmetric
/// `i64` range.
pub type ExactRational = Ratio<i64>;

impl Ratio<i64> {
    /// Convert to the binary64 approximation used for runtime arithmetic.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "the exact metadata is retained separately; binary64 is the runtime quantity representation"
    )]
    pub fn as_f64(self) -> f64 {
        self.num() as f64 / self.den() as f64
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
            && let Ok(integer) = i32::try_from(self.num())
        {
            return Ok(base.powi(integer));
        }
        if base >= 0.0 {
            return Ok(base.powf(self.as_f64()));
        }
        if self.den() % 2 == 0 {
            return Err(ExactPowerError::EvenRootOfNegative);
        }
        let magnitude = (-base).powf(self.as_f64());
        Ok(if self.num() % 2 == 0 {
            magnitude
        } else {
            -magnitude
        })
    }
}

/// A real-valued exact power could not be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExactPowerError {
    /// A negative real has no real even-denominator root.
    #[error("a negative base has no real value for an exact exponent with an even denominator")]
    EvenRootOfNegative,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_roots_of_negative_bases_are_real() {
        let third = ExactRational::try_new(1, 3).unwrap();
        assert!((third.pow_f64(-8.0).unwrap() + 2.0).abs() < 1e-12);
        let half = ExactRational::try_new(1, 2).unwrap();
        assert_eq!(half.pow_f64(-4.0), Err(ExactPowerError::EvenRootOfNegative));
        assert_eq!(ExactRational::integer(3).unwrap().pow_f64(-2.0), Ok(-8.0));
    }
}
