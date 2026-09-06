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

impl FiniteQuantity {
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
}

#[cfg(test)]
mod tests {
    use crate::complex_value::ComplexValue;
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
    fn complex_value_validates_both_components() {
        let value = ComplexValue::try_new(-0.0, f64::from_bits(1)).unwrap();
        assert_eq!(value.re().to_bits(), (-0.0_f64).to_bits());
        assert_eq!(value.im().to_bits(), 1);
        assert!(ComplexValue::try_new(f64::NAN, 0.0).is_err());
        assert!(ComplexValue::try_new(0.0, f64::INFINITY).is_err());
    }
}
