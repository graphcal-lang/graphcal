//! Exact power-of-two scaling of finite binary64 values.
//!
//! A finite nonzero `f64` is `significand * 2^exponent` with a significand
//! magnitude in `[1, 2)`. Kernels that must not overflow or underflow in an
//! intermediate step (residual checks, products of many factors) keep the
//! exponent separately and scale back once, with a single rounding.

/// A finite nonzero value as `significand * 2^exponent`, with
/// `1 <= |significand| < 2`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Normalized {
    pub(super) significand: f64,
    pub(super) exponent: i32,
}

/// The number of explicit significand bits of binary64.
const SIGNIFICAND_BITS: u32 = 52;
const EXPONENT_BIAS: i32 = 1023;
/// The exponent of the smallest normal power of two.
const MIN_NORMAL_EXPONENT: i32 = -1022;
/// The exponent of the smallest subnormal power of two.
const MIN_SUBNORMAL_EXPONENT: i32 = -1074;
const MAX_EXPONENT: i32 = 1023;

impl Normalized {
    /// The decomposition of `value`; `None` for zero or a non-finite value.
    pub(super) fn of(value: f64) -> Option<Self> {
        if value == 0.0 || !value.is_finite() {
            return None;
        }
        // A subnormal is first scaled exactly into the normal range.
        let (value, offset) = if value.abs() < f64::MIN_POSITIVE {
            (value * power_of_two_unchecked(64), -64)
        } else {
            (value, 0)
        };
        let bits = value.to_bits();
        let biased = i32::try_from((bits >> SIGNIFICAND_BITS) & 0x7ff).ok()?;
        let significand_bits =
            (bits & !(0x7ff_u64 << SIGNIFICAND_BITS)) | (1023_u64 << SIGNIFICAND_BITS);
        Some(Self {
            significand: f64::from_bits(significand_bits),
            exponent: biased.saturating_sub(EXPONENT_BIAS).saturating_add(offset),
        })
    }
}

/// `2^exponent` for an exponent of a normal power of two.
fn power_of_two_unchecked(exponent: i32) -> f64 {
    let biased = exponent
        .saturating_add(EXPONENT_BIAS)
        .clamp(1, 2046)
        .cast_unsigned();
    f64::from_bits(u64::from(biased) << SIGNIFICAND_BITS)
}

/// `value * 2^exponent`, correctly rounded (one rounding) to binary64: an
/// infinity beyond the range and a signed zero below half the smallest
/// subnormal.
pub(super) fn scaled_by_power_of_two(value: f64, exponent: i64) -> f64 {
    let Some(Normalized {
        significand,
        exponent: own,
    }) = Normalized::of(value)
    else {
        return value;
    };
    let target = i64::from(own).saturating_add(exponent);
    if target > i64::from(MAX_EXPONENT) {
        return f64::INFINITY.copysign(value);
    }
    if target >= i64::from(MIN_NORMAL_EXPONENT) {
        // Normal and exact.
        return significand * power_of_two_unchecked(narrow(target));
    }
    // |result| < 2^(target + 1): at most half the smallest subnormal.
    if target < i64::from(MIN_SUBNORMAL_EXPONENT).saturating_sub(1) {
        return 0.0_f64.copysign(value);
    }
    // Shift exactly onto the subnormal grid's unit, then round once.
    let lifted = significand
        * power_of_two_unchecked(narrow(
            target.saturating_sub(i64::from(MIN_SUBNORMAL_EXPONENT)),
        ));
    lifted * f64::from_bits(1)
}

/// An exponent already checked to lie within the binary64 range.
fn narrow(exponent: i64) -> i32 {
    i32::try_from(exponent).unwrap_or(if exponent < 0 { i32::MIN } else { i32::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decomposes_normal_and_subnormal_values() {
        assert_eq!(
            Normalized::of(12.0),
            Some(Normalized {
                significand: 1.5,
                exponent: 3
            })
        );
        assert_eq!(
            Normalized::of(-0.75),
            Some(Normalized {
                significand: -1.5,
                exponent: -1
            })
        );
        assert_eq!(
            Normalized::of(f64::from_bits(1)),
            Some(Normalized {
                significand: 1.0,
                exponent: -1074
            })
        );
        assert_eq!(
            Normalized::of(f64::MAX).map(|value| value.exponent),
            Some(1023)
        );
        assert_eq!(Normalized::of(0.0), None);
        assert_eq!(Normalized::of(f64::INFINITY), None);
    }

    #[test]
    fn scaling_rounds_once_across_the_whole_range() {
        assert_eq!(scaled_by_power_of_two(1.5, 3), 12.0);
        assert_eq!(
            scaled_by_power_of_two(1.0e308, -2000),
            1.0e308 * 2.0_f64.powi(-1000) * 2.0_f64.powi(-1000)
        );
        assert_eq!(scaled_by_power_of_two(1.0, -1074), f64::from_bits(1));
        // 1.5 * 2^-1074 is a tie between 1 and 2 units: round to even (2).
        assert_eq!(scaled_by_power_of_two(1.5, -1074), f64::from_bits(2));
        assert_eq!(scaled_by_power_of_two(1.0, -1075), 0.0);
        assert_eq!(scaled_by_power_of_two(1.5, -1075), f64::from_bits(1));
        assert_eq!(
            scaled_by_power_of_two(-1.0, -5000).to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(scaled_by_power_of_two(f64::from_bits(1), 1074), 1.0);
        assert_eq!(scaled_by_power_of_two(1.0, 1024), f64::INFINITY);
        assert_eq!(scaled_by_power_of_two(-1.0, i64::MAX), f64::NEG_INFINITY);
        assert_eq!(scaled_by_power_of_two(0.0, 10), 0.0);
    }
}
