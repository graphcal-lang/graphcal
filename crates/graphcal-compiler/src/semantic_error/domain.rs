//! Diagnostics of declared value domains (min and max constraints).
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};

/// Diagnostics of declared value domains (min and max constraints).
#[derive(Debug, Clone, Error)]
pub enum DomainError {
    #[error("domain violation: `{name}` value {value} is {violation}")]
    DomainViolation {
        name: String,
        value: String,
        violation: String,
    },
    #[error(
        "domain bound dimension mismatch on `{name}`: type has dimension {type_dim}, but {bound_name} bound has dimension {bound_dim}"
    )]
    DomainDimensionMismatch {
        name: String,
        type_dim: String,
        bound_name: String,
        bound_dim: String,
    },
    #[error("domain constraint on `{name}`: min ({min}) exceeds max ({max})")]
    DomainMinExceedsMax {
        name: String,
        min: String,
        max: String,
    },
    #[error("domain constraints are not valid on `{type_kind}` types")]
    InvalidDomainTarget { type_kind: String },
    #[error("domain bound type mismatch on Int `{name}`: {bound_name} bound has type {bound_type}")]
    IntDomainBoundTypeMismatch {
        name: String,
        bound_name: String,
        bound_type: String,
    },
    #[error("domain constraints are not supported on generic type arguments")]
    GenericTypeArgDomainConstraint,
    #[error(
        "datetime domain bound type mismatch on `{name}`: target is {target_type}, but {bound_name} bound is {bound_type}"
    )]
    DatetimeDomainBoundTypeMismatch {
        name: String,
        target_type: String,
        bound_name: String,
        bound_type: String,
    },
}

impl DiagnosticKind for DomainError {
    fn code(&self) -> &'static str {
        match self {
            Self::DomainViolation { .. } => "graphcal::C001",
            Self::DomainDimensionMismatch { .. } => "graphcal::C002",
            Self::DomainMinExceedsMax { .. } => "graphcal::C003",
            Self::InvalidDomainTarget { .. } => "graphcal::C004",
            Self::IntDomainBoundTypeMismatch { .. } => "graphcal::C005",
            Self::GenericTypeArgDomainConstraint => "graphcal::C006",
            Self::DatetimeDomainBoundTypeMismatch { .. } => "graphcal::C007",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::DomainViolation { .. } => Some("value out of declared domain".to_owned()),
            Self::DomainDimensionMismatch { .. } => {
                Some("dimension mismatch in domain bound".to_owned())
            }
            Self::DomainMinExceedsMax { .. } => Some("min > max".to_owned()),
            Self::InvalidDomainTarget { .. } => Some("constraints not valid here".to_owned()),
            Self::IntDomainBoundTypeMismatch { .. } => {
                Some("Int bound must have type Int".to_owned())
            }
            Self::GenericTypeArgDomainConstraint => Some("constraint not allowed here".to_owned()),
            Self::DatetimeDomainBoundTypeMismatch { .. } => {
                Some("datetime bound has the wrong time scale or value type".to_owned())
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::DomainViolation { .. } => Some("the value must satisfy the domain constraints declared on the type".to_owned()),
            Self::DomainDimensionMismatch { .. } => Some("domain bounds must have the same dimension as the constrained type".to_owned()),
            Self::DomainMinExceedsMax { .. } => Some("the min bound must be less than or equal to the max bound".to_owned()),
            Self::InvalidDomainTarget { .. } => Some("domain constraints (min/max) are only valid on quantity, Int, and Datetime types".to_owned()),
            Self::IntDomainBoundTypeMismatch { .. } => Some("Int domain bounds must be Int so their full range is preserved exactly".to_owned()),
            Self::GenericTypeArgDomainConstraint => Some("put the constraint on the field in the struct definition, not on the generic type argument".to_owned()),
            Self::DatetimeDomainBoundTypeMismatch { .. } => Some("datetime bounds must have exactly the constrained Datetime<S> type; use an explicit time-scale conversion".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DomainViolation { .. }
            | Self::DomainDimensionMismatch { .. }
            | Self::DomainMinExceedsMax { .. }
            | Self::InvalidDomainTarget { .. }
            | Self::IntDomainBoundTypeMismatch { .. }
            | Self::GenericTypeArgDomainConstraint
            | Self::DatetimeDomainBoundTypeMismatch { .. } => Vec::new(),
        }
    }
}
