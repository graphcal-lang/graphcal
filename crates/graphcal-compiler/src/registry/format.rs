//! Formatting utilities for values and unit expressions.
//!
//! Consolidates numeric and unit formatting helpers that are used across the
//! CLI, LSP, and internal evaluation pipeline.

/// Format a numeric value for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
use crate::dimension::Rational;
use thiserror::Error;
#[must_use]
pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let abs = value.abs();
    if value.fract() == 0.0 && abs < 1e15 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "value.abs() < 1e15 guarantees it fits in i64"
        )]
        let int_val = value as i64;
        format!("{int_val}")
    } else if abs < 5e-7 {
        format_scientific(value)
    } else {
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

fn format_scientific(value: f64) -> String {
    let formatted = format!("{value:.6e}");
    let Some((mantissa, exponent)) = formatted.split_once('e') else {
        return formatted;
    };
    let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
    let exponent = exponent
        .parse::<i32>()
        .map_or_else(|_| exponent.to_string(), |n| n.to_string());
    format!("{mantissa}e{exponent}")
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
