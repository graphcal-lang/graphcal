//! Products of many binary64 factors that validate only the final result.
//!
//! Multiplying factor by factor in binary64 can overflow or underflow midway
//! although the final product is representable, so whether a product
//! succeeded depended on the order of its factors. This accumulator keeps a
//! sign, a significand in `[1, 2)` and a checked `i64` exponent, so no
//! intermediate leaves the range; only the final value is checked.
//!
//! Each step still rounds the significand product, so the last bits of the
//! result may depend on the factor order (about one ulp per factor).
//! Success or failure does not.

use graphcal_compiler::finite_value::{FiniteArithmeticError, FiniteQuantity};

use super::binary_scale::{Normalized, scaled_by_power_of_two};

/// The running product of finite factors.
#[derive(Debug, Clone, Copy)]
pub(super) struct RangeSafeProduct {
    state: State,
}

#[derive(Debug, Clone, Copy)]
enum State {
    /// `significand * 2^exponent`, with `1 <= |significand| < 2`.
    Nonzero { significand: f64, exponent: i64 },
    /// A zero factor was seen; `negative` tracks the IEEE sign of the zero.
    Zero { negative: bool },
}

impl RangeSafeProduct {
    /// The empty product, one.
    pub(super) const fn new() -> Self {
        Self {
            state: State::Nonzero {
                significand: 1.0,
                exponent: 0,
            },
        }
    }

    /// Multiply by `factor`.
    pub(super) fn multiply(&mut self, factor: FiniteQuantity) {
        let factor = factor.get();
        self.state = match (self.state, Normalized::of(factor)) {
            (State::Zero { negative }, _) => State::Zero {
                negative: negative != factor.is_sign_negative(),
            },
            (State::Nonzero { significand, .. }, None) => State::Zero {
                negative: significand.is_sign_negative() != factor.is_sign_negative(),
            },
            (
                State::Nonzero {
                    significand,
                    exponent,
                },
                Some(normalized),
            ) => {
                // Both magnitudes are in [1, 2), so the product is in [1, 4).
                let product = significand * normalized.significand;
                let (significand, carry) = if product.abs() >= 2.0 {
                    (product / 2.0, 1)
                } else {
                    (product, 0)
                };
                State::Nonzero {
                    significand,
                    // At most 2,100 per factor: an i64 cannot overflow for any
                    // in-memory number of factors, but saturate rather than wrap.
                    exponent: exponent
                        .saturating_add(i64::from(normalized.exponent))
                        .saturating_add(carry),
                }
            }
        };
    }

    /// The product, rounded once from the accumulated significand.
    ///
    /// # Errors
    ///
    /// Returns [`FiniteArithmeticError::Infinite`] when the product exceeds
    /// the binary64 range and [`FiniteArithmeticError::Underflow`] when a
    /// product of nonzero factors rounds to zero.
    pub(super) fn finish(self) -> Result<FiniteQuantity, FiniteArithmeticError> {
        match self.state {
            State::Zero { negative } => Ok(if negative {
                FiniteQuantity::ZERO.negated()
            } else {
                FiniteQuantity::ZERO
            }),
            State::Nonzero {
                significand,
                exponent,
            } => {
                let value = scaled_by_power_of_two(significand, exponent);
                if value == 0.0 {
                    Err(FiniteArithmeticError::Underflow)
                } else {
                    FiniteQuantity::try_new(value).map_err(|_| FiniteArithmeticError::Infinite)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(values: &[f64]) -> Result<f64, FiniteArithmeticError> {
        let mut product = RangeSafeProduct::new();
        for value in values {
            product.multiply(FiniteQuantity::try_new(*value).unwrap());
        }
        product.finish().map(FiniteQuantity::get)
    }

    #[test]
    fn success_does_not_depend_on_the_factor_order() {
        for order in [
            [1.0e-200, 1.0e-200, 1.0e200, 1.0e200],
            [1.0e200, 1.0e200, 1.0e-200, 1.0e-200],
            [1.0e-200, 1.0e200, 1.0e-200, 1.0e200],
        ] {
            let result = product(&order).unwrap();
            assert!(
                (result - 1.0).abs() <= 4.0 * f64::EPSILON,
                "{order:?}: {result}"
            );
        }
    }

    #[test]
    fn exact_products_are_exact() {
        assert_eq!(product(&[3.0, 4.0]), Ok(12.0));
        assert_eq!(product(&[-2.0, 0.5, -8.0]), Ok(8.0));
        assert_eq!(product(&[]), Ok(1.0));
        assert_eq!(product(&[f64::MAX, 0.5, 2.0]), Ok(f64::MAX));
        let tiny = f64::from_bits(1);
        assert_eq!(product(&[tiny, 2.0]), Ok(f64::from_bits(2)));
        assert_eq!(product(&[1.0e300, tiny]), Ok(1.0e300 * tiny));
    }

    #[test]
    fn only_the_final_result_is_validated() {
        assert_eq!(
            product(&[f64::MAX, 2.0]),
            Err(FiniteArithmeticError::Infinite)
        );
        assert_eq!(
            product(&[1.0e-200, 1.0e-200]),
            Err(FiniteArithmeticError::Underflow)
        );
        assert_eq!(
            product(&[f64::MAX, f64::MAX, 1.0e-300, 1.0e-300]).map(f64::is_finite),
            Ok(true)
        );
    }

    #[test]
    fn zero_factors_give_signed_zeros() {
        assert_eq!(
            product(&[f64::MAX, 0.0, f64::MAX]).unwrap().to_bits(),
            0.0_f64.to_bits()
        );
        assert_eq!(
            product(&[-3.0, 0.0]).unwrap().to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(product(&[-0.0, -1.0]).unwrap().to_bits(), 0.0_f64.to_bits());
    }
}
