//! Formatting utilities for values and unit expressions.
//!
//! Consolidates numeric and unit formatting helpers that are used across the
//! CLI, LSP, and internal evaluation pipeline.

/// Format a numeric value for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
use crate::dimension::Rational;
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
        .map_or(exponent.to_string(), |n| n.to_string());
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

/// Negate an exponent for display, saturating instead of overflowing.
fn negate_exponent(exp: Rational) -> Rational {
    Rational::try_new(exp.num().checked_neg().unwrap_or(i32::MAX), exp.den()).unwrap_or(exp)
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/hour`, `kg * m / s^2`
///
/// If `parenthesize_multi_denom` is true, multi-term denominators are wrapped in parentheses:
/// `m / (s * kg)` instead of `m / s * kg`.
#[must_use]
pub fn format_unit_expr_with_config(
    expr: &crate::syntax::ast::UnitExpr,
    parenthesize_multi_denom: bool,
) -> String {
    use crate::syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.value.to_string();
        if let Some(pow) = item.power
            && pow != Rational::ONE
        {
            part = format!("{part}{}", format_exponent(pow));
        }
        match item.op {
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
/// E.g., `m`, `km/hour`, `kg * m / s^2`
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
#[must_use]
pub fn format_unit_expr_canonical(expr: &crate::syntax::ast::UnitExpr) -> String {
    use crate::syntax::ast::MulDivOp;
    use std::collections::BTreeMap;

    let mut exponents: BTreeMap<crate::syntax::dimension::UnitRef, Rational> = BTreeMap::new();
    for item in &expr.terms {
        let pow = item.power.unwrap_or(Rational::ONE);
        let signed = match item.op {
            MulDivOp::Mul => pow,
            MulDivOp::Div => negate_exponent(pow),
        };
        let entry = exponents
            .entry(item.name.value.clone())
            .or_insert(Rational::ZERO);
        // Saturate on overflow: this is a display label, not a value.
        *entry = (*entry + signed).unwrap_or(*entry);
    }

    let render = |name: &crate::syntax::dimension::UnitRef, exp: Rational| -> String {
        if exp == Rational::ONE {
            name.to_string()
        } else {
            format!("{name}{}", format_exponent(exp))
        }
    };

    let mut numerator: Vec<String> = Vec::new();
    let mut denominator: Vec<String> = Vec::new();
    for (name, exp) in &exponents {
        match exp.num().cmp(&0) {
            std::cmp::Ordering::Greater => numerator.push(render(name, *exp)),
            std::cmp::Ordering::Less => denominator.push(render(name, negate_exponent(*exp))),
            std::cmp::Ordering::Equal => {}
        }
    }

    match (numerator.is_empty(), denominator.is_empty()) {
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
    }
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
        assert_eq!(format_unit_expr_canonical(&expr), "m/s^2");
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
        assert_eq!(format_unit_expr_canonical(&expr), "kg * m^2 / (A * s^3)");
    }

    #[test]
    fn canonical_cancels_to_dimensionless() {
        // `s/s` cancels to nothing.
        let expr = unit_expr(vec![
            unit_term(MulDivOp::Mul, "s", None),
            unit_term(MulDivOp::Div, "s", None),
        ]);
        assert_eq!(format_unit_expr_canonical(&expr), "");
    }
}
