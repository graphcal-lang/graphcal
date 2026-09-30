//! Display formatting of numeric values, shared by the CLI, LSP, and the
//! evaluation pipeline.

const DISPLAY_SIGNIFICANT_DIGITS: usize = 7;
const SCIENTIFIC_FRACTIONAL_DIGITS: usize = DISPLAY_SIGNIFICANT_DIGITS - 1;
const POSITIONAL_MIN_EXPONENT: i32 = -4;
const POSITIONAL_MAX_EXPONENT_EXCLUSIVE: i32 = 15;

/// Format a finite numeric value with up to seven significant digits.
///
/// Positional notation is used when the rounded decimal exponent is in
/// `-4..15`; values outside that readable window use scientific notation.
/// Trailing fractional zeros and redundant scientific-exponent characters are
/// omitted. Both positive and negative zero render as `0`.
#[must_use]
pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    if !value.is_finite() {
        return value.to_string();
    }

    // Round once in scientific notation so every magnitude receives the same
    // significant-digit policy. Select notation from the rounded exponent so
    // values that round across a cutoff render on the expected side of it.
    let rounded = format!("{value:.SCIENTIFIC_FRACTIONAL_DIGITS$e}");
    let Some((mantissa, exponent)) = rounded.split_once('e') else {
        return rounded;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return rounded;
    };

    if (POSITIONAL_MIN_EXPONENT..POSITIONAL_MAX_EXPONENT_EXCLUSIVE).contains(&exponent) {
        format_positional(mantissa, exponent)
    } else {
        format_scientific(mantissa, exponent)
    }
}

fn format_scientific(mantissa: &str, exponent: i32) -> String {
    let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
    format!("{mantissa}e{exponent}")
}

fn format_positional(mantissa: &str, exponent: i32) -> String {
    let (sign, unsigned_mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |unsigned| ("-", unsigned));
    let digits = unsigned_mantissa.replace('.', "");
    let decimal_point = exponent + 1;

    let mut unsigned = if decimal_point <= 0 {
        let Ok(leading_zeros) = usize::try_from(decimal_point.unsigned_abs()) else {
            return format_scientific(mantissa, exponent);
        };
        format!("0.{}{digits}", "0".repeat(leading_zeros))
    } else {
        let Ok(decimal_point) = usize::try_from(decimal_point) else {
            return format_scientific(mantissa, exponent);
        };
        if decimal_point >= digits.len() {
            format!("{digits}{}", "0".repeat(decimal_point - digits.len()))
        } else {
            format!("{}.{}", &digits[..decimal_point], &digits[decimal_point..])
        }
    };

    if unsigned.contains('.') {
        unsigned.truncate(unsigned.trim_end_matches('0').trim_end_matches('.').len());
    }
    format!("{sign}{unsigned}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::approx_constant,
        reason = "testing the exact display policy for the decimal value 3.14"
    )]
    fn numbers_use_consistent_significant_digits_across_magnitudes() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1200.0, "1200"),
            (-42.0, "-42"),
            (9.80665, "9.80665"),
            (3.14, "3.14"),
            (3138.128, "3138.128"),
            (9.876_543_21e-6, "9.876543e-6"),
            (1.234_567_89e-5, "1.234568e-5"),
            (0.000_123_456_789, "0.0001234568"),
            (6.022_140_76e23, "6.022141e23"),
            (1.0e30, "1e30"),
            (1.0e300, "1e300"),
            (1.602_176_634e-19, "1.602177e-19"),
            (f64::MIN_POSITIVE, "2.225074e-308"),
            (f64::MAX, "1.797693e308"),
        ];

        for (value, expected) in cases {
            assert_eq!(format_number(value), expected, "value: {value:?}");
        }
    }

    #[test]
    fn notation_uses_the_rounded_exponent_at_each_cutoff() {
        assert_eq!(format_number(1.0e-4), "0.0001");
        assert_eq!(format_number(9.999_999_99e-5), "0.0001");
        assert_eq!(format_number(999_999_999_999_999.0), "1e15");
        assert_eq!(format_number(1.0e15), "1e15");
    }

    #[test]
    fn non_finite_values_retain_standard_rust_spelling() {
        assert_eq!(format_number(f64::INFINITY), "inf");
        assert_eq!(format_number(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_number(f64::NAN), "NaN");
    }

    #[test]
    fn magnitude_sweep_preserves_the_display_precision_and_notation_window() {
        for exponent in -300..=300 {
            let value = 1.234_567_89 * 10.0_f64.powi(exponent);
            let formatted = format_number(value);
            let displayed = formatted
                .parse::<f64>()
                .unwrap_or_else(|error| panic!("failed to parse `{formatted}`: {error}"));
            let relative_error = ((displayed - value) / value).abs();

            assert!(
                relative_error <= 5.0e-7,
                "{value:?} rendered as `{formatted}` with relative error {relative_error}"
            );
            assert_eq!(
                formatted.contains('e'),
                !(POSITIONAL_MIN_EXPONENT..POSITIONAL_MAX_EXPONENT_EXCLUSIVE).contains(&exponent),
                "unexpected notation for exponent {exponent}: `{formatted}`"
            );
        }
    }
}
