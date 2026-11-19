//! Human-readable labels of unit expressions.

use thiserror::Error;

use crate::dimension::Rational;
use crate::ratio::{ExponentStyle, Ratio};

/// Failure while exactly accumulating canonical unit-label exponents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CanonicalUnitFormatError {
    /// The exact `i64` rational accumulator exceeded its range.
    #[error("canonical unit exponent accumulation overflowed")]
    ExponentOverflow,
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
            part = format!("{part}{}", power.fmt_exponent(ExponentStyle::Source));
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

/// Normalize and format already-selected unit term spellings.
pub fn format_unit_terms_canonical(
    terms: impl IntoIterator<Item = (crate::syntax::ast::MulDivOp, String, Rational)>,
) -> Result<String, CanonicalUnitFormatError> {
    use crate::syntax::ast::MulDivOp;
    use std::collections::BTreeMap;

    let exponents = terms.into_iter().try_fold(
        BTreeMap::<String, Ratio<i64>>::new(),
        |mut exponents, (op, name, power)| {
            let power = Ratio::<i64>::from(power);
            let signed = match op {
                MulDivOp::Mul => power,
                MulDivOp::Div => -power,
            };
            let updated = (exponents.get(&name).copied().unwrap_or(Ratio::ZERO) + signed)
                .map_err(|_| CanonicalUnitFormatError::ExponentOverflow)?;
            if updated.is_zero() {
                exponents.remove(&name);
            } else {
                exponents.insert(name, updated);
            }
            Ok(exponents)
        },
    )?;

    let factor = |name: &str, exponent: Ratio<i64>| {
        if exponent == Ratio::ONE {
            name.to_string()
        } else {
            format!("{name}{}", exponent.fmt_exponent(ExponentStyle::Source))
        }
    };
    let (numerator, denominator): (Vec<_>, Vec<_>) = exponents
        .iter()
        .partition(|(_, exponent)| exponent.is_positive());
    let numerator = numerator
        .into_iter()
        .map(|(name, exponent)| factor(name, *exponent))
        .collect::<Vec<_>>();
    let denominator = denominator
        .into_iter()
        .map(|(name, exponent)| factor(name, -*exponent))
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
    use crate::syntax::ast::MulDivOp;

    #[test]
    fn canonical_combines_repeated_denominator_terms() {
        // Issue #577: `m/s/s` previously rendered as `m/s * s` (≡ `m`).
        let terms = [
            (
                MulDivOp::Mul,
                "m".to_string(),
                Rational::integer(1).unwrap(),
            ),
            (
                MulDivOp::Div,
                "s".to_string(),
                Rational::integer(1).unwrap(),
            ),
            (
                MulDivOp::Div,
                "s".to_string(),
                Rational::integer(1).unwrap(),
            ),
        ];
        assert_eq!(format_unit_terms_canonical(terms).unwrap(), "m/s^2");
    }

    #[test]
    fn canonical_parenthesizes_multi_denominator() {
        // `kg * m^2 / A / s^3` must render with the denominator grouped so the
        // string parses back to the same dimensional monomial.
        let terms = [
            (
                MulDivOp::Mul,
                "kg".to_string(),
                Rational::integer(1).unwrap(),
            ),
            (
                MulDivOp::Mul,
                "m".to_string(),
                Rational::integer(2).unwrap(),
            ),
            (
                MulDivOp::Div,
                "A".to_string(),
                Rational::integer(1).unwrap(),
            ),
            (
                MulDivOp::Div,
                "s".to_string(),
                Rational::integer(3).unwrap(),
            ),
        ];
        assert_eq!(
            format_unit_terms_canonical(terms).unwrap(),
            "kg * m^2 / (A * s^3)"
        );
    }

    #[test]
    fn canonical_cancels_to_dimensionless() {
        // `s/s` cancels to nothing.
        let terms = [
            (
                MulDivOp::Mul,
                "s".to_string(),
                Rational::integer(1).unwrap(),
            ),
            (
                MulDivOp::Div,
                "s".to_string(),
                Rational::integer(1).unwrap(),
            ),
        ];
        assert_eq!(format_unit_terms_canonical(terms).unwrap(), "");
    }

    #[test]
    fn canonical_renders_extreme_denominator_magnitude() {
        let terms = [(
            MulDivOp::Div,
            "m".to_string(),
            Rational::integer(i32::MAX).unwrap(),
        )];

        assert_eq!(
            format_unit_terms_canonical(terms).unwrap(),
            "1/m^2147483647"
        );
    }

    #[test]
    fn canonical_reports_accumulator_overflow() {
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
