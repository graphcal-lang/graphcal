//! Correctly rounded sums of two binary64 products.
//!
//! `a * b + c * d` is the kernel of complex multiplication and of the
//! components of `cross()`. Evaluating it term by term rounds each product
//! first: a product can overflow before a cancellation brings the result back
//! into range, or underflow to zero although the exact result is not zero.
//! Here the products are summed exactly and rounded once, so the result is
//! finite whenever the exact value is representable, and a nonzero exact
//! value that rounds to zero is reported as an underflow.

use graphcal_compiler::finite_value::{FiniteArithmeticError, FiniteQuantity};
use num_traits::{ToPrimitive, Zero};

use super::exact_binary_sum::ExactBinarySum;

/// A product `lhs * rhs` of two finite quantities, one term of a
/// [`product_sum`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Product {
    pub(super) lhs: FiniteQuantity,
    pub(super) rhs: FiniteQuantity,
}

impl Product {
    pub(super) const fn new(lhs: FiniteQuantity, rhs: FiniteQuantity) -> Self {
        Self { lhs, rhs }
    }

    /// The same product with the opposite sign (exact).
    pub(super) const fn negated(self) -> Self {
        Self {
            lhs: self.lhs.negated(),
            rhs: self.rhs,
        }
    }

    /// Whether the binary64 product is a negative zero.
    fn is_negative_zero(self) -> bool {
        let (lhs, rhs) = (self.lhs.get(), self.rhs.get());
        (lhs == 0.0 || rhs == 0.0) && (lhs.is_sign_negative() != rhs.is_sign_negative())
    }
}

/// `first + second`, the exact sum of two exact products rounded once to
/// binary64.
///
/// An exact zero takes the IEEE 754 sign of adding the two binary64
/// products: negative only when both are negative zeros.
///
/// # Errors
///
/// Returns [`FiniteArithmeticError::Infinite`] when the exact sum rounds
/// beyond the binary64 range and [`FiniteArithmeticError::Underflow`] when a
/// nonzero exact sum rounds to zero.
pub(super) fn product_sum(
    first: Product,
    second: Product,
) -> Result<FiniteQuantity, FiniteArithmeticError> {
    let mut sum = ExactBinarySum::new();
    sum.add_product(first.lhs, first.rhs);
    sum.add_product(second.lhs, second.rhs);
    let exact = sum.total();
    if exact.is_zero() {
        return Ok(if first.is_negative_zero() && second.is_negative_zero() {
            FiniteQuantity::ZERO.negated()
        } else {
            FiniteQuantity::ZERO
        });
    }
    let rounded = exact.to_f64().ok_or(FiniteArithmeticError::Infinite)?;
    if rounded == 0.0 {
        return Err(FiniteArithmeticError::Underflow);
    }
    FiniteQuantity::try_new(rounded).map_err(|_| FiniteArithmeticError::Infinite)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(value: f64) -> FiniteQuantity {
        FiniteQuantity::try_new(value).unwrap()
    }

    fn sum(a: f64, b: f64, c: f64, d: f64) -> Result<f64, FiniteArithmeticError> {
        product_sum(Product::new(q(a), q(b)), Product::new(q(c), q(d))).map(FiniteQuantity::get)
    }

    #[test]
    fn overflowing_products_that_cancel_into_range_are_finite() {
        // 1.4e154^2 - 6e153^2 ~ 1.6e308, though 1.4e154^2 alone overflows.
        let result = sum(1.4e154, 1.4e154, -6.0e153, 6.0e153).unwrap();
        assert!((result / 1.6e308 - 1.0).abs() < 1.0e-15);
        assert_eq!(sum(f64::MAX, 2.0, -f64::MAX, 1.5), Ok(f64::MAX / 2.0));
    }

    #[test]
    fn the_result_is_rounded_once() {
        // 0.1 * 0.1 - 0.01 is not zero exactly; term-by-term rounding gives a
        // different value.
        let exact = sum(0.1, 0.1, -0.01, 1.0).unwrap();
        let fused = 0.1_f64.mul_add(0.1, -0.01);
        assert_eq!(exact.to_bits(), fused.to_bits());
    }

    #[test]
    fn range_violations_are_errors() {
        assert_eq!(
            sum(f64::MAX, 2.0, 1.0, 1.0),
            Err(FiniteArithmeticError::Infinite)
        );
        assert_eq!(
            sum(1.0e-300, 1.0e-300, 0.0, 1.0),
            Err(FiniteArithmeticError::Underflow)
        );
    }

    #[test]
    fn exact_zeros_follow_ieee_signs() {
        assert_eq!(
            sum(1.0, 2.0, -2.0, 1.0).unwrap().to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(
            sum(-0.0, 1.0, 0.0, -1.0).unwrap().to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(
            sum(-0.0, 1.0, 0.0, 1.0).unwrap().to_bits(),
            0.0_f64.to_bits()
        );
        let negated = Product::new(q(0.0), q(1.0)).negated();
        assert_eq!(
            product_sum(negated, negated).unwrap().get().to_bits(),
            (-0.0_f64).to_bits()
        );
    }
}
