//! The structural index that a `Fin(...)` binding value names.
//!
//! An `index` binding (`include lib(index I: Fin(2 * 3))` or
//! `@lib(index I: Fin(2 * 3))::n`) has no generic scope, so its cardinality
//! must be a closed Nat expression. Both binding forms evaluate it here, so
//! they accept the same values and report the same diagnostics.

use crate::semantic::index_def::{FiniteIndex, IndexCardinalityError};
use crate::semantic_error::index::IndexError;
use crate::syntax::ast::NatExpr;
use crate::syntax::index_name::IndexName;
use crate::syntax::nat_eval::{ClosedNat, NatArithmetic};
use crate::syntax::span::Span;

/// Why a `Fin(...)` binding value names no structural index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiniteIndexBindingErrorKind {
    /// The cardinality names a variable; there is no generic scope to bind it.
    UnknownName { name: IndexName },
    /// A closed sum or product in the cardinality leaves `u64`.
    Overflow { op: NatArithmetic },
    /// The closed cardinality is not a valid structural index size.
    InvalidCardinality { error: IndexCardinalityError },
}

/// A rejected `Fin(...)` binding value, located at its offending span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiniteIndexBindingError {
    pub kind: FiniteIndexBindingErrorKind,
    pub span: Span,
}

impl FiniteIndexBindingError {
    /// The index diagnostic this failure is reported as.
    #[must_use]
    pub fn index_error(&self) -> IndexError {
        match &self.kind {
            FiniteIndexBindingErrorKind::UnknownName { name } => IndexError::UnknownIndex {
                name: name.clone().into(),
            },
            FiniteIndexBindingErrorKind::Overflow { op } => {
                IndexError::fin_cardinality_overflow(*op)
            }
            FiniteIndexBindingErrorKind::InvalidCardinality { error } => {
                IndexError::InvalidFiniteIndexCardinality { error: *error }
            }
        }
    }
}

/// Evaluate the cardinality of a `Fin(...)` binding value written at
/// `value_span`.
///
/// # Errors
///
/// Returns an error located at the first variable for an open cardinality,
/// at the overflowing sum or product for an overflow, and at `value_span`
/// for a closed cardinality that is not a valid structural index size.
pub fn finite_index_binding(
    cardinality: &NatExpr,
    value_span: Span,
) -> Result<FiniteIndex, FiniteIndexBindingError> {
    let value = match cardinality.closed_value() {
        Ok(ClosedNat::Value(value)) => value,
        Ok(ClosedNat::Open { first_var }) => {
            return Err(FiniteIndexBindingError {
                kind: FiniteIndexBindingErrorKind::UnknownName {
                    name: IndexName::classify(first_var.name.atom().clone()),
                },
                span: first_var.span,
            });
        }
        Err(overflow) => {
            return Err(FiniteIndexBindingError {
                kind: FiniteIndexBindingErrorKind::Overflow { op: overflow.op },
                span: overflow.span,
            });
        }
    };
    FiniteIndex::try_from_u64(value).map_err(|error| FiniteIndexBindingError {
        kind: FiniteIndexBindingErrorKind::InvalidCardinality { error },
        span: value_span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::index_def::MAX_INDEX_CARDINALITY;
    use crate::syntax::ast::Ident;
    use crate::syntax::non_empty::AtLeastTwo;

    const VALUE_SPAN: Span = Span::new(0, 20);

    fn lit(value: u64) -> NatExpr {
        NatExpr::Literal(value, Span::new(4, 1))
    }

    fn var(name: &str, offset: usize) -> NatExpr {
        NatExpr::Var(Ident {
            name: crate::syntax::token::SourceIdentifier::parse(name).unwrap(),
            span: Span::new(offset, 1),
        })
    }

    fn add(lhs: NatExpr, rhs: NatExpr, offset: usize) -> NatExpr {
        NatExpr::Add(AtLeastTwo::new(lhs, rhs), Span::new(offset, 3))
    }

    fn mul(lhs: NatExpr, rhs: NatExpr, offset: usize) -> NatExpr {
        NatExpr::Mul(AtLeastTwo::new(lhs, rhs), Span::new(offset, 3))
    }

    #[test]
    fn closed_cardinality_names_its_finite_index() {
        assert_eq!(
            finite_index_binding(&add(lit(1), mul(lit(2), lit(3), 8), 4), VALUE_SPAN),
            Ok(FiniteIndex::try_from_u64(7).unwrap())
        );
    }

    #[test]
    fn open_cardinality_is_an_unknown_index_at_its_first_variable() {
        let error = finite_index_binding(&add(lit(1), var("N", 8), 4), VALUE_SPAN).unwrap_err();
        assert_eq!(error.span, Span::new(8, 1));
        assert!(matches!(
            error.kind,
            FiniteIndexBindingErrorKind::UnknownName { ref name } if name.atom().as_str() == "N"
        ));
        assert!(matches!(
            error.index_error(),
            IndexError::UnknownIndex { .. }
        ));
    }

    #[test]
    fn overflow_is_located_at_the_overflowing_operation() {
        let error = finite_index_binding(&mul(lit(u64::MAX), lit(2), 6), VALUE_SPAN).unwrap_err();
        assert_eq!(
            error,
            FiniteIndexBindingError {
                kind: FiniteIndexBindingErrorKind::Overflow {
                    op: NatArithmetic::Multiplication,
                },
                span: Span::new(6, 3),
            }
        );
        assert!(matches!(
            error.index_error(),
            IndexError::FinCardinalityMultiplicationOverflow
        ));
    }

    #[test]
    fn invalid_closed_cardinality_is_located_at_the_value() {
        for cardinality in [0, MAX_INDEX_CARDINALITY as u64 + 1] {
            let error = finite_index_binding(&lit(cardinality), VALUE_SPAN).unwrap_err();
            assert_eq!(error.span, VALUE_SPAN);
            assert!(matches!(
                error.index_error(),
                IndexError::InvalidFiniteIndexCardinality { .. }
            ));
        }
    }
}
