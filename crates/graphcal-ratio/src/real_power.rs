//! Real-valued powers with an exact rational exponent.

use thiserror::Error;

use crate::ratio::Ratio;

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
    /// Every exponent, integer or not, goes through the same `powf` kernel,
    /// so the result does not depend on how the exponent was written (a
    /// literal `3` and a runtime `Int` 3 agree bit for bit). Repeated
    /// multiplication (`powi`) is deliberately avoided: it is not correctly
    /// rounded, can underflow falsely, and its precision is
    /// platform-dependent. The sign of a negative base comes from the exact
    /// numerator's parity, never from the rounded binary64 exponent.
    ///
    /// # Errors
    ///
    /// Returns [`ExactPowerError::EvenRootOfNegative`] when a negative base
    /// has an even reduced denominator and therefore no real result.
    pub fn pow_f64(self, base: f64) -> Result<f64, ExactPowerError> {
        if !base.is_sign_negative() {
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
        let third = Ratio::<i64>::try_new(1, 3).unwrap();
        assert!((third.pow_f64(-8.0).unwrap() + 2.0).abs() < 1e-12);
        let half = Ratio::<i64>::try_new(1, 2).unwrap();
        assert_eq!(half.pow_f64(-4.0), Err(ExactPowerError::EvenRootOfNegative));
        assert_eq!(Ratio::<i64>::integer(3).unwrap().pow_f64(-2.0), Ok(-8.0));
    }

    #[test]
    fn integer_powers_match_powf_bit_for_bit() {
        let base = 0.701_546_366_168_601_9_f64;
        for exponent in [-7_i64, -2, -1, 0, 1, 2, 3, 4, 17, 100_000_000] {
            let power = Ratio::<i64>::integer(exponent).unwrap();
            #[expect(clippy::cast_precision_loss, reason = "small test exponents")]
            let expected = base.powf(exponent as f64);
            assert_eq!(power.pow_f64(base).unwrap().to_bits(), expected.to_bits());
        }
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the reference is deliberately `powf`, the kernel every power uses"
    )]
    fn integer_powers_do_not_underflow_falsely() {
        let power = Ratio::<i64>::integer(-1070).unwrap();
        assert_eq!(power.pow_f64(2.0), Ok(2.0_f64.powf(-1070.0)));
        assert!(power.pow_f64(2.0).unwrap() > 0.0);
        let power = Ratio::<i64>::integer(-2).unwrap();
        assert!(power.pow_f64(1.0e160).unwrap() > 0.0);
    }

    #[test]
    fn negative_base_sign_follows_exact_numerator_parity() {
        // 2^53 + 1 is odd but rounds to the even binary64 2^53.
        let odd = Ratio::<i64>::integer((1_i64 << 53) + 1).unwrap();
        assert_eq!(odd.pow_f64(-1.0), Ok(-1.0));
        let even = Ratio::<i64>::integer(1_i64 << 53).unwrap();
        assert_eq!(even.pow_f64(-1.0), Ok(1.0));
        assert_eq!(Ratio::<i64>::integer(3).unwrap().pow_f64(-0.0), Ok(-0.0));
    }
}
