//! Pure runtime-value checks against resolved domain constraints.
//!
//! Used both at compile time (validating const values in `exec_plan`) and at
//! evaluation time (validating param/node values in `eval::runtime`).

use crate::eval_expr::RuntimeValue;
use graphcal_compiler::tir::typed::ResolvedDomainConstraint;

/// A domain-constraint violation with a human-readable message.
///
/// The `message` field is safe to embed into a diagnostic verbatim. It includes
/// the relevant bound (min/max) with display units already substituted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct DomainViolation {
    pub message: String,
}

impl DomainViolation {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Check a runtime value against a resolved domain constraint.
///
/// Returns `Ok(())` if the value satisfies the constraint, or a
/// [`DomainViolation`] describing the first violation (including the variant
/// prefix for indexed values).
///
/// # Errors
///
/// Returns [`DomainViolation`] when any scalar sub-value falls outside the
/// declared bounds.
pub fn check_domain_constraint(
    rv: &RuntimeValue,
    constraint: &ResolvedDomainConstraint,
) -> Result<(), DomainViolation> {
    match rv {
        RuntimeValue::Scalar(si_value) => check_scalar_constraint(*si_value, constraint),
        RuntimeValue::Int(i) => check_int_constraint(*i, constraint),
        RuntimeValue::Indexed { entries, .. } => {
            for (variant, entry_rv) in entries {
                if let Err(violation) = check_domain_constraint(entry_rv, constraint) {
                    return Err(DomainViolation::new(format!(
                        "at {variant}: {}",
                        violation.message
                    )));
                }
            }
            Ok(())
        }
        // Bool, Label, Struct, Datetime, RangeLabel: no constraint checking
        _ => Ok(()),
    }
}

const MAX_EXACT_F64_INT: u64 = 1_u64 << f64::MANTISSA_DIGITS;

fn check_int_constraint(
    value: i64,
    constraint: &ResolvedDomainConstraint,
) -> Result<(), DomainViolation> {
    if (constraint.min.is_some() || constraint.max.is_some())
        && value.unsigned_abs() > MAX_EXACT_F64_INT
    {
        return Err(DomainViolation::new(format!(
            "integer {value} is too large for exact domain-bound comparison"
        )));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer magnitude is checked to be exactly representable before casting"
    )]
    let scalar = value as f64;
    check_scalar_constraint(scalar, constraint)
}

/// Check a scalar SI value against min/max bounds.
fn check_scalar_constraint(
    si_value: f64,
    constraint: &ResolvedDomainConstraint,
) -> Result<(), DomainViolation> {
    if let Some(min) = constraint.min
        && si_value < min
    {
        let min_display = constraint.min_display.as_deref().unwrap_or("?");
        return Err(DomainViolation::new(format!(
            "below minimum ({min_display})"
        )));
    }
    if let Some(max) = constraint.max
        && si_value > max
    {
        let max_display = constraint.max_display.as_deref().unwrap_or("?");
        return Err(DomainViolation::new(format!(
            "above maximum ({max_display})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::syntax::span::Span;

    fn max_constraint(max: f64) -> ResolvedDomainConstraint {
        ResolvedDomainConstraint {
            min: None,
            max: Some(max),
            min_display: None,
            max_display: Some(max.to_string()),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn rejects_int_too_large_for_exact_domain_comparison() {
        let err = check_domain_constraint(
            &RuntimeValue::Int((1_i64 << f64::MANTISSA_DIGITS) + 1),
            &max_constraint(f64::MAX),
        )
        .unwrap_err();
        assert!(
            err.message
                .contains("too large for exact domain-bound comparison")
        );
    }
}
