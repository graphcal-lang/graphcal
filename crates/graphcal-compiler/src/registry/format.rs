//! Formatting utilities for values and unit expressions.
//!
//! Consolidates numeric and unit formatting helpers that are used across the
//! CLI, LSP, and internal evaluation pipeline.

use crate::dimension::Rational;
use thiserror::Error;

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

/// Render a unit/dimension exponent suffix: `^2` for integers,
/// `^(1/2)` for rationals (the parenthesized form is re-parseable).
#[must_use]
pub fn format_exponent(exp: Rational) -> String {
    if exp.is_integer() {
        format!("^{}", exp.num())
    } else {
        format!("^({}/{})", exp.num(), exp.den())
    }
}

/// Failure while exactly accumulating canonical unit-label exponents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CanonicalUnitFormatError {
    /// The exact wide rational accumulator exceeded its internal representation.
    #[error("canonical unit exponent accumulation overflowed")]
    ExponentOverflow,
}

/// Exact display-only rational wide enough to render every individual
/// [`Rational`] magnitude, including `i32::MIN`, without changing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WideRational {
    numerator: i128,
    denominator: i128,
}

impl WideRational {
    const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };

    fn from_rational(value: Rational) -> Self {
        Self {
            numerator: i128::from(value.num()),
            denominator: i128::from(value.den()),
        }
    }

    fn checked_neg(self) -> Result<Self, CanonicalUnitFormatError> {
        Ok(Self {
            numerator: self
                .numerator
                .checked_neg()
                .ok_or(CanonicalUnitFormatError::ExponentOverflow)?,
            denominator: self.denominator,
        })
    }

    fn checked_add(self, other: Self) -> Result<Self, CanonicalUnitFormatError> {
        let common = gcd128(
            self.denominator.unsigned_abs(),
            other.denominator.unsigned_abs(),
        );
        let common =
            i128::try_from(common).map_err(|_| CanonicalUnitFormatError::ExponentOverflow)?;
        let self_scale = other
            .denominator
            .checked_div(common)
            .ok_or(CanonicalUnitFormatError::ExponentOverflow)?;
        let other_scale = self
            .denominator
            .checked_div(common)
            .ok_or(CanonicalUnitFormatError::ExponentOverflow)?;
        let numerator = self
            .numerator
            .checked_mul(self_scale)
            .and_then(|left| {
                other
                    .numerator
                    .checked_mul(other_scale)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or(CanonicalUnitFormatError::ExponentOverflow)?;
        let denominator = self
            .denominator
            .checked_mul(self_scale)
            .ok_or(CanonicalUnitFormatError::ExponentOverflow)?;
        Self::try_new(numerator, denominator)
    }

    fn try_new(numerator: i128, denominator: i128) -> Result<Self, CanonicalUnitFormatError> {
        if numerator == 0 {
            return Ok(Self::ZERO);
        }
        let divisor = gcd128(numerator.unsigned_abs(), denominator.unsigned_abs());
        let divisor =
            i128::try_from(divisor).map_err(|_| CanonicalUnitFormatError::ExponentOverflow)?;
        Ok(Self {
            numerator: numerator
                .checked_div(divisor)
                .ok_or(CanonicalUnitFormatError::ExponentOverflow)?,
            denominator: denominator
                .checked_div(divisor)
                .ok_or(CanonicalUnitFormatError::ExponentOverflow)?,
        })
    }

    const fn is_zero(self) -> bool {
        self.numerator == 0
    }

    fn factor(&self, name: &str, magnitude: bool) -> String {
        let numerator = if magnitude {
            self.numerator.unsigned_abs().to_string()
        } else {
            self.numerator.to_string()
        };
        if self.numerator.unsigned_abs() == 1 && self.denominator == 1 {
            name.to_string()
        } else if self.denominator == 1 {
            format!("{name}^{numerator}")
        } else {
            format!("{name}^({numerator}/{})", self.denominator)
        }
    }
}

fn gcd128(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd128(b, a % b) }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/h`, `kg * m / s^2`
///
/// If `parenthesize_multi_denom` is true, multi-term denominators are wrapped in parentheses:
/// `m / (s * kg)` instead of `m / s * kg`.
#[must_use]
pub fn format_unit_expr_with_config(
    expr: &crate::syntax::ast::UnitExpr,
    parenthesize_multi_denom: bool,
) -> String {
    format_unit_terms_with_config(
        expr.terms
            .iter()
            .map(|item| (item.op, item.name.value.to_string(), item.effective_power())),
        parenthesize_multi_denom,
    )
}

/// Format already-selected unit term spellings.
///
/// This is the representation-independent boundary used by syntax and HIR
/// callers without making either semantic layer depend on the other.
#[must_use]
pub fn format_unit_terms_with_config(
    terms: impl IntoIterator<Item = (crate::syntax::ast::MulDivOp, String, Rational)>,
    parenthesize_multi_denom: bool,
) -> String {
    use crate::syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for (op, name, power) in terms {
        let mut part = name;
        if power != Rational::ONE {
            part = format!("{part}{}", format_exponent(power));
        }
        match op {
            MulDivOp::Mul => numerator.push(part),
            MulDivOp::Div => denominator.push(part),
        }
    }

    if denominator.is_empty() {
        numerator.join(" * ")
    } else if numerator.len() == 1 && denominator.len() == 1 {
        format!("{}/{}", numerator[0], denominator[0])
    } else {
        let num = numerator.join(" * ");
        let den = denominator.join(" * ");
        if parenthesize_multi_denom && denominator.len() > 1 {
            format!("{num} / ({den})")
        } else {
            format!("{num}/{den}")
        }
    }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/h`, `kg * m / s^2`
#[must_use]
pub fn format_unit_expr(expr: &crate::syntax::ast::UnitExpr) -> String {
    format_unit_expr_with_config(expr, false)
}

/// Format a `UnitExpr` in canonical normalized form for display labels.
///
/// Combines repeated unit names into a single term with the summed exponent
/// (positive in the numerator, negative in the denominator), drops any units
/// whose exponents cancel to zero, and sorts both numerator and denominator
/// alphabetically so the result is order-independent.
///
/// Issue #577: the non-canonical `format_unit_expr` rendered `m/s/s` as
/// `m/s * s`, which is mathematically `m`. Display labels for computed values
/// must not lie about the engineering units, so the eval pipeline routes
/// through this function instead.
pub fn format_unit_expr_canonical(
    expr: &crate::syntax::ast::UnitExpr,
) -> Result<String, CanonicalUnitFormatError> {
    format_unit_terms_canonical(
        expr.terms
            .iter()
            .map(|item| (item.op, item.name.value.to_string(), item.effective_power())),
    )
}

/// Normalize and format already-selected unit term spellings.
pub fn format_unit_terms_canonical(
    terms: impl IntoIterator<Item = (crate::syntax::ast::MulDivOp, String, Rational)>,
) -> Result<String, CanonicalUnitFormatError> {
    use crate::syntax::ast::MulDivOp;
    use std::collections::BTreeMap;

    let exponents = terms.into_iter().try_fold(
        BTreeMap::<String, WideRational>::new(),
        |mut exponents, (op, name, power)| {
            let power = WideRational::from_rational(power);
            let signed = match op {
                MulDivOp::Mul => power,
                MulDivOp::Div => power.checked_neg()?,
            };
            let updated = exponents
                .get(&name)
                .copied()
                .unwrap_or(WideRational::ZERO)
                .checked_add(signed)?;
            if updated.is_zero() {
                exponents.remove(&name);
            } else {
                exponents.insert(name, updated);
            }
            Ok(exponents)
        },
    )?;

    let (numerator, denominator): (Vec<_>, Vec<_>) = exponents
        .iter()
        .partition(|(_, exponent)| exponent.numerator.is_positive());
    let numerator = numerator
        .into_iter()
        .map(|(name, exponent)| exponent.factor(name, false))
        .collect::<Vec<_>>();
    let denominator = denominator
        .into_iter()
        .map(|(name, exponent)| exponent.factor(name, true))
        .collect::<Vec<_>>();

    Ok(match (numerator.is_empty(), denominator.is_empty()) {
        (true, true) => String::new(),
        (false, true) => numerator.join(" * "),
        (true, false) => format!("1/{}", denominator.join(" * ")),
        (false, false) => {
            let num = numerator.join(" * ");
            let den = denominator.join(" * ");
            if denominator.len() == 1 {
                format!("{num}/{den}")
            } else {
                format!("{num} / ({den})")
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{MulDivOp, UnitExpr, UnitExprItem};
    use crate::syntax::dimension::{UnitName, UnitRef};
    use crate::syntax::span::Span;
    use crate::syntax::span::Spanned;

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

    fn unit_term(op: MulDivOp, name: &str, power: Option<i32>) -> UnitExprItem {
        UnitExprItem {
            op,
            name: Spanned::new(
                UnitRef::local(UnitName::expect_valid(name)),
                Span::new(0, 0),
            ),
            power: power.map(Rational::from),
        }
    }

    fn unit_expr(terms: Vec<UnitExprItem>) -> UnitExpr {
        UnitExpr {
            terms,
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn canonical_combines_repeated_denominator_terms() {
        // Issue #577: `m/s/s` previously rendered as `m/s * s` (≡ `m`).
        let expr = unit_expr(vec![
            unit_term(MulDivOp::Mul, "m", None),
            unit_term(MulDivOp::Div, "s", None),
            unit_term(MulDivOp::Div, "s", None),
        ]);
        assert_eq!(format_unit_expr_canonical(&expr).unwrap(), "m/s^2");
    }

    #[test]
    fn canonical_parenthesizes_multi_denominator() {
        // `kg * m^2 / A / s^3` must render with the denominator grouped so the
        // string parses back to the same dimensional monomial.
        let expr = unit_expr(vec![
            unit_term(MulDivOp::Mul, "kg", None),
            unit_term(MulDivOp::Mul, "m", Some(2)),
            unit_term(MulDivOp::Div, "A", None),
            unit_term(MulDivOp::Div, "s", Some(3)),
        ]);
        assert_eq!(
            format_unit_expr_canonical(&expr).unwrap(),
            "kg * m^2 / (A * s^3)"
        );
    }

    #[test]
    fn canonical_cancels_to_dimensionless() {
        // `s/s` cancels to nothing.
        let expr = unit_expr(vec![
            unit_term(MulDivOp::Mul, "s", None),
            unit_term(MulDivOp::Div, "s", None),
        ]);
        assert_eq!(format_unit_expr_canonical(&expr).unwrap(), "");
    }

    #[test]
    fn canonical_preserves_min_i32_exponent_magnitude() {
        let terms = [(MulDivOp::Mul, "m".to_string(), Rational::from(i32::MIN))];

        assert_eq!(
            format_unit_terms_canonical(terms).unwrap(),
            "1/m^2147483648"
        );
    }

    #[test]
    fn canonical_reports_wide_accumulator_overflow() {
        let denominators = [
            i32::MAX,
            2_147_483_629,
            2_147_483_587,
            2_147_483_579,
            2_147_483_563,
        ];
        let terms = denominators.into_iter().map(|denominator| {
            (
                MulDivOp::Mul,
                "m".to_string(),
                Rational::try_new(1, denominator).unwrap(),
            )
        });

        assert_eq!(
            format_unit_terms_canonical(terms),
            Err(CanonicalUnitFormatError::ExponentOverflow)
        );
    }
}
