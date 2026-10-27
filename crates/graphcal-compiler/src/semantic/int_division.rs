//! Integer division with the rounding direction its built-in names.
//!
//! The quotient is exact over the whole `i64` range: rounding is the only
//! policy applied, and a quotient `i64` cannot hold is an error rather than a
//! wrapped value. Checking and evaluation share this kernel, so a quotient
//! folded at compile time always equals the one computed at runtime.

use crate::builtin::IntDivisionFn;

/// Why an integer division has no `Int` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntDivisionError {
    /// The divisor is zero.
    DivisionByZero,
    /// The quotient does not fit in `i64` (`i64::MIN` divided by `-1`).
    Overflow,
}

/// Divide `dividend` by `divisor`, rounding the quotient in the direction
/// `function` names.
///
/// # Errors
///
/// Returns [`IntDivisionError::DivisionByZero`] for a zero divisor and
/// [`IntDivisionError::Overflow`] when the quotient does not fit in `i64`.
pub const fn divide(
    function: IntDivisionFn,
    dividend: i64,
    divisor: i64,
) -> Result<i64, IntDivisionError> {
    if divisor == 0 {
        return Err(IntDivisionError::DivisionByZero);
    }
    let Some(truncated) = dividend.checked_div(divisor) else {
        return Err(IntDivisionError::Overflow);
    };
    let exact = matches!(dividend.checked_rem(divisor), Some(0));
    let negative = (dividend < 0) != (divisor < 0);
    // An inexact quotient moves one step away from zero for floor (negative
    // quotients) and ceil (positive ones). The step cannot overflow, since
    // the truncated quotient is smaller in magnitude than the dividend.
    let rounded = match function {
        IntDivisionFn::Floor if !exact && negative => truncated.checked_sub(1),
        IntDivisionFn::Ceil if !exact && !negative => truncated.checked_add(1),
        IntDivisionFn::Trunc | IntDivisionFn::Floor | IntDivisionFn::Ceil => Some(truncated),
    };
    match rounded {
        Some(quotient) => Ok(quotient),
        None => Err(IntDivisionError::Overflow),
    }
}

#[cfg(test)]
mod tests {
    use super::{IntDivisionError, divide};
    use crate::builtin::IntDivisionFn;

    #[test]
    fn each_function_rounds_in_its_direction() {
        // (dividend, divisor, trunc, floor, ceil)
        let cases = [
            (7, 2, 3, 3, 4),
            (-7, 2, -3, -4, -3),
            (7, -2, -3, -4, -3),
            (-7, -2, 3, 3, 4),
            (6, 3, 2, 2, 2),
            (-6, 3, -2, -2, -2),
            (0, 5, 0, 0, 0),
            (0, -5, 0, 0, 0),
            (1, 2, 0, 0, 1),
            (-1, 2, 0, -1, 0),
            (i64::MAX, 2, i64::MAX / 2, i64::MAX / 2, i64::MAX / 2 + 1),
            (i64::MIN, 2, i64::MIN / 2, i64::MIN / 2, i64::MIN / 2),
            (
                i64::MIN + 1,
                2,
                i64::MIN / 2 + 1,
                i64::MIN / 2,
                i64::MIN / 2 + 1,
            ),
            (i64::MIN, 1, i64::MIN, i64::MIN, i64::MIN),
            (i64::MAX, -1, -i64::MAX, -i64::MAX, -i64::MAX),
            (i64::MIN, i64::MAX, -1, -2, -1),
            (i64::MAX, i64::MIN, 0, -1, 0),
        ];
        for (dividend, divisor, trunc, floor, ceil) in cases {
            for (function, expected) in [
                (IntDivisionFn::Trunc, trunc),
                (IntDivisionFn::Floor, floor),
                (IntDivisionFn::Ceil, ceil),
            ] {
                assert_eq!(
                    divide(function, dividend, divisor),
                    Ok(expected),
                    "{function}({dividend}, {divisor})"
                );
            }
        }
    }

    #[test]
    fn zero_divisor_and_unrepresentable_quotient_are_errors() {
        for function in IntDivisionFn::all() {
            for dividend in [0, 1, -1, i64::MIN, i64::MAX] {
                assert_eq!(
                    divide(function, dividend, 0),
                    Err(IntDivisionError::DivisionByZero),
                    "{function}({dividend}, 0)"
                );
            }
            assert_eq!(
                divide(function, i64::MIN, -1),
                Err(IntDivisionError::Overflow),
                "{function}"
            );
        }
    }

    #[test]
    fn quotients_bracket_the_real_quotient() {
        for dividend in -20_i64..=20 {
            for divisor in (-6_i64..=6).filter(|divisor| *divisor != 0) {
                let floor = divide(IntDivisionFn::Floor, dividend, divisor).unwrap();
                let ceil = divide(IntDivisionFn::Ceil, dividend, divisor).unwrap();
                let trunc = divide(IntDivisionFn::Trunc, dividend, divisor).unwrap();
                let exact = dividend % divisor == 0;
                // floor(q) * |b| <= a * sign(b) < (floor(q) + 1) * |b|
                let scaled = dividend * divisor.signum();
                assert!(floor * divisor.abs() <= scaled);
                assert!(scaled < (floor + 1) * divisor.abs());
                assert_eq!(ceil, if exact { floor } else { floor + 1 });
                assert_eq!(trunc, if floor < 0 { ceil } else { floor });
            }
        }
    }
}
