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
        #[expect(
            clippy::cast_precision_loss,
            reason = "domain bound comparison on small integers"
        )]
        RuntimeValue::Int(i) => check_scalar_constraint(*i as f64, constraint),
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
