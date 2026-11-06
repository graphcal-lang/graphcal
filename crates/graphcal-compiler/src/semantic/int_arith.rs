//! Checked `Int` arithmetic.
//!
//! Every result is the exact mathematical value or an error; nothing wraps.
//! Evaluation, compile-time folding of static `Int` expressions, and
//! unit-scale and coordinate constants share this kernel, so a value folded
//! at compile time always equals the one computed at runtime.

use thiserror::Error;

use crate::tir::texpr::operators::IntArithOp;

/// Why an `Int` operation has no `Int` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IntArithError {
    /// The divisor of `%` is zero.
    #[error("integer modulo by zero")]
    ModuloByZero,
    /// The result does not fit in `i64`.
    #[error("integer arithmetic overflow")]
    Overflow,
    /// The negation does not fit in `i64` (`-i64::MIN`).
    #[error("integer negation overflow")]
    NegationOverflow,
    /// The exponent of an `Int` power is negative.
    #[error("integer exponent must be non-negative")]
    NegativeExponent,
    /// The exponent of an `Int` power does not fit in `u32`.
    #[error("integer exponent too large")]
    ExponentTooLarge,
}

/// Apply a binary `Int` operator.
///
/// # Errors
///
/// Returns [`IntArithError::ModuloByZero`] for `% 0` and
/// [`IntArithError::Overflow`] when the result does not fit in `i64`.
pub const fn arith(op: IntArithOp, lhs: i64, rhs: i64) -> Result<i64, IntArithError> {
    let result = match op {
        IntArithOp::Add => lhs.checked_add(rhs),
        IntArithOp::Sub => lhs.checked_sub(rhs),
        IntArithOp::Mul => lhs.checked_mul(rhs),
        IntArithOp::Mod => {
            if rhs == 0 {
                return Err(IntArithError::ModuloByZero);
            }
            // The mathematical remainder is zero for every dividend when the
            // divisor is -1. `checked_rem` nevertheless returns `None` for
            // `i64::MIN % -1` because the corresponding machine instruction
            // traps, so handle this total case explicitly.
            if rhs == -1 {
                Some(0)
            } else {
                lhs.checked_rem(rhs)
            }
        }
    };
    match result {
        Some(value) => Ok(value),
        None => Err(IntArithError::Overflow),
    }
}

/// Negate an `Int`.
///
/// # Errors
///
/// Returns [`IntArithError::NegationOverflow`] for `i64::MIN`.
pub const fn negate(value: i64) -> Result<i64, IntArithError> {
    match value.checked_neg() {
        Some(negated) => Ok(negated),
        None => Err(IntArithError::NegationOverflow),
    }
}

/// Raise an `Int` to a non-negative `Int` power.
///
/// # Errors
///
/// Returns [`IntArithError::NegativeExponent`] for a negative exponent,
/// [`IntArithError::ExponentTooLarge`] for one beyond `u32`, and
/// [`IntArithError::Overflow`] when the power does not fit in `i64`.
pub fn power(base: i64, exponent: i64) -> Result<i64, IntArithError> {
    if exponent < 0 {
        return Err(IntArithError::NegativeExponent);
    }
    let exponent = u32::try_from(exponent).map_err(|_| IntArithError::ExponentTooLarge)?;
    base.checked_pow(exponent).ok_or(IntArithError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::{IntArithError, arith, negate, power};
    use crate::tir::texpr::operators::IntArithOp;

    #[test]
    fn arithmetic_is_checked() {
        assert_eq!(arith(IntArithOp::Add, 2, 3), Ok(5));
        assert_eq!(arith(IntArithOp::Sub, 2, 3), Ok(-1));
        assert_eq!(arith(IntArithOp::Mul, -4, 3), Ok(-12));
        assert_eq!(arith(IntArithOp::Mod, -7, 3), Ok(-1));
        assert_eq!(
            arith(IntArithOp::Add, i64::MAX, 1),
            Err(IntArithError::Overflow)
        );
        assert_eq!(
            arith(IntArithOp::Mul, i64::MIN, -1),
            Err(IntArithError::Overflow)
        );
    }

    #[test]
    fn remainder_is_total_except_for_a_zero_divisor() {
        assert_eq!(arith(IntArithOp::Mod, i64::MIN, -1), Ok(0));
        assert_eq!(
            arith(IntArithOp::Mod, 1, 0),
            Err(IntArithError::ModuloByZero)
        );
    }

    #[test]
    fn negation_and_power_are_checked() {
        assert_eq!(negate(5), Ok(-5));
        assert_eq!(negate(i64::MIN), Err(IntArithError::NegationOverflow));
        assert_eq!(power(2, 10), Ok(1024));
        assert_eq!(power(2, -1), Err(IntArithError::NegativeExponent));
        assert_eq!(
            power(2, i64::from(u32::MAX) + 1),
            Err(IntArithError::ExponentTooLarge)
        );
        assert_eq!(power(2, 63), Err(IntArithError::Overflow));
    }
}
