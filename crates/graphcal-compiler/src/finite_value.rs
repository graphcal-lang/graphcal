//! Runtime numeric values whose finite-value invariant has been validated.

use thiserror::Error;

use crate::exact_rational::ExactRational;
use crate::ratio::ExactPowerError;

/// A binary64 quantity proven to be finite.
///
/// ```
/// use graphcal_compiler::finite_value::FiniteQuantity;
/// let value = FiniteQuantity::try_new(-0.0).unwrap();
/// assert_eq!(value.get().to_bits(), (-0.0_f64).to_bits());
/// ```
///
/// Raw construction cannot bypass validation:
/// ```compile_fail
/// use graphcal_compiler::finite_value::FiniteQuantity;
/// let invalid = FiniteQuantity(f64::INFINITY);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FiniteQuantity(f64);

/// Failure to construct a semantic finite numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("quantity must be finite, got {value}")]
pub struct NonFiniteQuantity {
    /// The rejected binary64 payload.
    pub value: f64,
}

/// Failure of checked arithmetic on finite quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FiniteArithmeticError {
    /// The binary64 result overflowed to an infinity.
    #[error("result is infinite")]
    Infinite,
    /// A mathematically nonzero product or quotient rounded to zero.
    #[error("result underflowed to zero")]
    Underflow,
    /// The divisor was zero.
    #[error("division by zero")]
    DivisionByZero,
    /// A power of a negative base by a non-integer exponent has no real
    /// value.
    #[error("result is not a number")]
    NotANumber,
}

/// Failure of a checked power with an exact rational exponent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExactPowerFailure {
    /// The power has no real value.
    #[error(transparent)]
    NoRealValue(#[from] ExactPowerError),
    /// The real value is not a finite binary64 value.
    #[error(transparent)]
    Arithmetic(#[from] FiniteArithmeticError),
}

/// Failure to project an `i64` into binary64 without changing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("integer {value} cannot be represented exactly as binary64")]
pub struct ExactI64ToF64Error {
    /// The integer binary64 cannot hold.
    pub value: i64,
}

/// Convert an integer to binary64 only when the conversion is exact.
///
/// # Errors
///
/// Returns [`ExactI64ToF64Error`] when binary64 has no value equal to
/// `value` (magnitudes above 2^53 that are not multiples of a power of two).
pub const fn exact_i64_to_f64(value: i64) -> Result<f64, ExactI64ToF64Error> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the following wider-integer round trip proves whether this specific value is exact"
    )]
    let converted = value as f64;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "i128 contains every rounded binary64 value reachable from an i64"
    )]
    let round_trip = converted as i128;
    let original = value as i128;
    if round_trip == original {
        Ok(converted)
    } else {
        Err(ExactI64ToF64Error { value })
    }
}

/// Failure to convert a binary64 value to an `i64` without changing it.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum ExactF64ToI64Error {
    /// `i64` values are always finite.
    #[error("is not finite (got {value})")]
    NonFinite {
        /// The rejected binary64 value.
        value: f64,
    },
    /// Converting the value would require an implicit rounding policy.
    #[error("is not integer-valued (got {value})")]
    NonInteger {
        /// The rejected binary64 value.
        value: f64,
    },
    /// The value is integral but outside the `i64` range.
    #[error(
        "is outside the representable integer range ({}..={}) (got {value})",
        i64::MIN,
        i64::MAX
    )]
    OutOfRange {
        /// The rejected binary64 value.
        value: f64,
    },
}

/// Convert a binary64 value to an `i64` only when the conversion preserves it
/// exactly.
///
/// Rust's direct `as i64` cast truncates fractional values and saturates
/// values outside the `i64` range; explicit range and round-trip checks reject
/// both. Numeric equality deliberately accepts `-0.0` as the integer zero.
///
/// # Errors
///
/// Returns [`ExactF64ToI64Error`] naming the invariant `value` fails.
pub fn exact_f64_to_i64(value: f64) -> Result<i64, ExactF64ToI64Error> {
    if !value.is_finite() {
        return Err(ExactF64ToI64Error::NonFinite { value });
    }

    let lower_inclusive = -2.0_f64.powi(63);
    let upper_exclusive = 2.0_f64.powi(63);
    if value < lower_inclusive || value >= upper_exclusive {
        return Err(ExactF64ToI64Error::OutOfRange { value });
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the following binary64 round trip rejects values that would be truncated"
    )]
    let converted = value as i64;
    #[expect(
        clippy::cast_precision_loss,
        reason = "the binary64 round trip is the exactness check"
    )]
    let round_trip = converted as f64;
    #[expect(
        clippy::float_cmp,
        reason = "exactness requires equality; no tolerance or rounding policy is implicit"
    )]
    if round_trip != value {
        return Err(ExactF64ToI64Error::NonInteger { value });
    }

    Ok(converted)
}

impl FiniteQuantity {
    /// Positive zero.
    pub const ZERO: Self = Self(0.0);

    /// One.
    pub const ONE: Self = Self(1.0);

    /// Validate and retain a binary64 quantity.
    pub const fn try_new(value: f64) -> Result<Self, NonFiniteQuantity> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(NonFiniteQuantity { value })
        }
    }

    /// Return the validated binary64 payload.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// `-self`, which stays finite.
    #[must_use]
    pub const fn negated(self) -> Self {
        Self(-self.0)
    }

    /// `self + rhs`, when the sum is finite.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::Infinite`] on overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, FiniteArithmeticError> {
        Self::finite_result(self.0 + rhs.0)
    }

    /// `self - rhs`, when the difference is finite.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::Infinite`] on overflow.
    pub fn checked_sub(self, rhs: Self) -> Result<Self, FiniteArithmeticError> {
        Self::finite_result(self.0 - rhs.0)
    }

    /// `self * rhs`, when the product is finite and a product of nonzero
    /// factors did not round to zero.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::Infinite`] on overflow and
    /// [`FiniteArithmeticError::Underflow`] on complete underflow.
    pub fn checked_mul(self, rhs: Self) -> Result<Self, FiniteArithmeticError> {
        Self::nonzero_result(self.0 * rhs.0, self.0 != 0.0 && rhs.0 != 0.0)
    }

    /// `self / rhs`, when `rhs` is nonzero, the quotient is finite, and a
    /// nonzero dividend did not round to zero.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::DivisionByZero`] for a zero divisor,
    /// [`FiniteArithmeticError::Infinite`] on overflow, and
    /// [`FiniteArithmeticError::Underflow`] on complete underflow.
    pub fn checked_div(self, rhs: Self) -> Result<Self, FiniteArithmeticError> {
        if rhs.0 == 0.0 {
            return Err(FiniteArithmeticError::DivisionByZero);
        }
        Self::nonzero_result(self.0 / rhs.0, self.0 != 0.0)
    }

    /// The exact real value of an integer.
    ///
    /// # Errors
    ///
    /// Returns [`ExactI64ToF64Error`] when binary64 cannot hold `value`
    /// exactly.
    pub const fn from_exact_int(value: i64) -> Result<Self, ExactI64ToF64Error> {
        match exact_i64_to_f64(value) {
            // An exactly converted `i64` is always finite.
            Ok(converted) => Ok(Self(converted)),
            Err(error) => Err(error),
        }
    }

    /// `self ^ exponent` for a dimensionless runtime exponent, when the power
    /// is a real, finite value and a power of nonzero operands did not round
    /// to zero.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::NotANumber`] when the power has no
    /// real value, [`FiniteArithmeticError::Infinite`] on overflow, and
    /// [`FiniteArithmeticError::Underflow`] on complete underflow.
    pub fn checked_powf(self, exponent: Self) -> Result<Self, FiniteArithmeticError> {
        let result = self.0.powf(exponent.0);
        if result.is_nan() {
            return Err(FiniteArithmeticError::NotANumber);
        }
        Self::nonzero_result(result, self.0 != 0.0 && exponent.0 != 0.0)
    }

    /// `self ^ exponent` for an exact rational exponent, with real odd-root
    /// semantics for a negative base.
    ///
    /// # Errors
    ///
    /// Returns [`ExactPowerFailure::NoRealValue`] for an even root of a
    /// negative base, and [`ExactPowerFailure::Arithmetic`] when the power
    /// overflows or a nonzero base underflows to zero.
    pub fn checked_exact_power(self, exponent: ExactRational) -> Result<Self, ExactPowerFailure> {
        let result = exponent.pow_f64(self.0)?;
        if result.is_nan() {
            return Err(FiniteArithmeticError::NotANumber.into());
        }
        Ok(Self::nonzero_result(result, self.0 != 0.0)?)
    }

    /// Arithmetic on finite operands (with a nonzero divisor) never yields
    /// NaN, so a non-finite result is an overflow.
    const fn finite_result(value: f64) -> Result<Self, FiniteArithmeticError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(FiniteArithmeticError::Infinite)
        }
    }

    fn nonzero_result(
        value: f64,
        mathematically_nonzero: bool,
    ) -> Result<Self, FiniteArithmeticError> {
        let value = Self::finite_result(value)?;
        if mathematically_nonzero && value.0 == 0.0 {
            Err(FiniteArithmeticError::Underflow)
        } else {
            Ok(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::finite_value::{ExactF64ToI64Error, FiniteQuantity, exact_f64_to_i64};

    proptest::proptest! {
        #[test]
        fn accepted_binary64_payloads_are_preserved(bits in proptest::prelude::any::<u64>()) {
            let raw = f64::from_bits(bits);
            match FiniteQuantity::try_new(raw) {
                Ok(value) => {
                    proptest::prop_assert!(raw.is_finite());
                    proptest::prop_assert_eq!(value.get().to_bits(), bits);
                }
                Err(_) => proptest::prop_assert!(!raw.is_finite()),
            }
        }
    }

    #[test]
    fn finite_quantity_preserves_signed_zero_and_subnormals() {
        for value in [-0.0, f64::from_bits(1), -f64::from_bits(1), f64::MAX] {
            assert_eq!(
                FiniteQuantity::try_new(value).unwrap().get().to_bits(),
                value.to_bits()
            );
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(FiniteQuantity::try_new(value).is_err());
        }
    }

    #[test]
    fn checked_arithmetic_keeps_results_finite() {
        use crate::finite_value::FiniteArithmeticError;
        let q = |value| FiniteQuantity::try_new(value).unwrap();
        assert_eq!(q(1.5).checked_add(q(2.0)).unwrap(), q(3.5));
        assert_eq!(q(1.5).checked_sub(q(2.0)).unwrap(), q(-0.5));
        assert_eq!(q(1.5).checked_mul(q(2.0)).unwrap(), q(3.0));
        assert_eq!(q(3.0).checked_div(q(2.0)).unwrap(), q(1.5));
        assert_eq!(q(2.0).negated(), q(-2.0));
        assert_eq!(
            q(f64::MAX).checked_add(q(f64::MAX)),
            Err(FiniteArithmeticError::Infinite)
        );
        assert_eq!(
            q(-f64::MAX).checked_sub(q(f64::MAX)),
            Err(FiniteArithmeticError::Infinite)
        );
        assert_eq!(
            q(f64::MAX).checked_mul(q(2.0)),
            Err(FiniteArithmeticError::Infinite)
        );
        let tiny = q(f64::from_bits(1));
        assert_eq!(
            tiny.checked_mul(tiny),
            Err(FiniteArithmeticError::Underflow)
        );
        assert_eq!(
            tiny.checked_div(q(4.0)),
            Err(FiniteArithmeticError::Underflow)
        );
        assert_eq!(q(0.0).checked_mul(tiny).unwrap(), q(0.0));
        assert_eq!(q(0.0).checked_div(q(4.0)).unwrap(), q(0.0));
        assert_eq!(
            q(1.0).checked_div(q(0.0)),
            Err(FiniteArithmeticError::DivisionByZero)
        );
        assert_eq!(
            q(1.0).checked_div(q(-0.0)),
            Err(FiniteArithmeticError::DivisionByZero)
        );
        assert_eq!(
            q(f64::MAX).checked_div(tiny),
            Err(FiniteArithmeticError::Infinite)
        );
    }

    #[test]
    fn exact_conversion_accepts_integer_valued_inputs() {
        for (value, expected) in [
            (0.0, 0),
            (-0.0, 0),
            (42.0, 42),
            (-42.0, -42),
            (1.0e18, 1_000_000_000_000_000_000),
            (-2.0_f64.powi(63), i64::MIN),
        ] {
            assert_eq!(exact_f64_to_i64(value), Ok(expected));
        }
    }

    #[test]
    fn exact_conversion_rejects_fractional_inputs() {
        for value in [3.7, -3.7, f64::MIN_POSITIVE] {
            assert_eq!(
                exact_f64_to_i64(value),
                Err(ExactF64ToI64Error::NonInteger { value })
            );
        }
    }

    #[test]
    fn exact_conversion_rejects_non_finite_inputs() {
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(matches!(
                exact_f64_to_i64(value),
                Err(ExactF64ToI64Error::NonFinite { .. })
            ));
        }
    }

    #[test]
    fn exact_conversion_rejects_values_outside_int_range() {
        for value in [2.0_f64.powi(63), 1.0e20, -1.0e20] {
            assert_eq!(
                exact_f64_to_i64(value),
                Err(ExactF64ToI64Error::OutOfRange { value })
            );
        }
    }
}
