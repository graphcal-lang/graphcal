//! Cartesian representation of a validated complex numeric value.

use thiserror::Error;

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
/// Construction validates both components. Numerical kernels that need raw
/// workspaces should use their own private carrier and validate at the runtime
/// value boundary.
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
    re: f64,
    im: f64,
}

impl ComplexValue {
    /// Validate and construct Cartesian components.
    pub const fn try_new(re: f64, im: f64) -> Result<Self, ComplexValueError> {
        if re.is_finite() && im.is_finite() {
            Ok(Self { re, im })
        } else {
            Err(ComplexValueError { re, im })
        }
    }

    #[must_use]
    pub const fn re(self) -> f64 {
        self.re
    }

    #[must_use]
    pub const fn im(self) -> f64 {
        self.im
    }
}
