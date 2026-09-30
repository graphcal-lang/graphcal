//! Runtime numeric values whose finite-value invariant has been validated.

use thiserror::Error;

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
    use crate::finite_value::FiniteQuantity;

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
}
