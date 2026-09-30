//! Cartesian representation of a validated complex numeric value.

use thiserror::Error;

use crate::finite_value::FiniteQuantity;

/// Failure to construct a complex value with finite components.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("complex value must have finite components, got ({re}, {im})")]
pub struct ComplexValueError {
    /// Rejected real component.
    pub re: f64,
    /// Rejected imaginary component.
    pub im: f64,
}

/// A complex number stored as Cartesian binary64 components.
///
/// Both components are [`FiniteQuantity`] values, so construction validates
/// them once and every accessor hands out finite components.
///
/// ```
/// use graphcal_compiler::complex_value::ComplexValue;
/// let value = ComplexValue::try_new(1.0, -0.0).unwrap();
/// assert_eq!(value.im().to_bits(), (-0.0_f64).to_bits());
/// ```
///
/// Both components must pass validation; fields cannot be initialized directly:
/// ```compile_fail
/// use graphcal_compiler::complex_value::ComplexValue;
/// let invalid = ComplexValue { re: f64::INFINITY, im: 0.0 };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexValue {
    re: FiniteQuantity,
    im: FiniteQuantity,
}

impl ComplexValue {
    /// Validate and construct Cartesian components.
    pub const fn try_new(re: f64, im: f64) -> Result<Self, ComplexValueError> {
        match (FiniteQuantity::try_new(re), FiniteQuantity::try_new(im)) {
            (Ok(re), Ok(im)) => Ok(Self { re, im }),
            _ => Err(ComplexValueError { re, im }),
        }
    }

    /// The complex value with finite Cartesian components `re` and `im`.
    #[must_use]
    pub const fn from_parts(re: FiniteQuantity, im: FiniteQuantity) -> Self {
        Self { re, im }
    }

    /// The real component as a finite quantity.
    #[must_use]
    pub const fn real_part(self) -> FiniteQuantity {
        self.re
    }

    /// The imaginary component as a finite quantity.
    #[must_use]
    pub const fn imaginary_part(self) -> FiniteQuantity {
        self.im
    }

    #[must_use]
    pub const fn re(self) -> f64 {
        self.re.get()
    }

    #[must_use]
    pub const fn im(self) -> f64 {
        self.im.get()
    }

    /// The complex conjugate, which stays finite.
    #[must_use]
    pub const fn conjugate(self) -> Self {
        Self {
            re: self.re,
            im: self.im.negated(),
        }
    }

    /// `-self`, which stays finite.
    #[must_use]
    pub const fn negated(self) -> Self {
        Self {
            re: self.re.negated(),
            im: self.im.negated(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::complex_value::ComplexValue;
    use crate::finite_value::FiniteQuantity;

    #[test]
    fn complex_value_validates_both_components() {
        let value = ComplexValue::try_new(-0.0, f64::from_bits(1)).unwrap();
        assert_eq!(value.re().to_bits(), (-0.0_f64).to_bits());
        assert_eq!(value.im().to_bits(), 1);
        assert!(ComplexValue::try_new(f64::NAN, 0.0).is_err());
        assert!(ComplexValue::try_new(0.0, f64::INFINITY).is_err());
    }

    #[test]
    fn finite_parts_build_and_transform_infallibly() {
        let q = |value| FiniteQuantity::try_new(value).unwrap();
        let value = ComplexValue::from_parts(q(1.0), q(-2.0));
        assert_eq!(value.real_part(), q(1.0));
        assert_eq!(value.imaginary_part(), q(-2.0));
        assert_eq!(value.conjugate(), ComplexValue::from_parts(q(1.0), q(2.0)));
        assert_eq!(value.negated(), ComplexValue::from_parts(q(-1.0), q(2.0)));
    }
}
