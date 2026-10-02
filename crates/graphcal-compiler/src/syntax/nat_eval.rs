//! Evaluation of a type-level natural-number expression as written, before
//! any generic parameter is bound.
//!
//! A `Fin(...)` cardinality is a [`NatExpr`]; a closed one (no variable) has
//! one `u64` value. Callers decide what an open expression or an overflow
//! means for them (a diagnostic, or "not concrete").

use crate::syntax::ast::{Ident, NatExpr};
use crate::syntax::span::Span;

/// The value of a [`NatExpr`] that did not overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedNat<'a> {
    /// The expression has no variable.
    Value(u64),
    /// The expression names a variable; `first_var` is the first one in
    /// source order.
    Open { first_var: &'a Ident },
}

/// The arithmetic operation that overflowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatArithmetic {
    Addition,
    Multiplication,
}

/// A closed part of a [`NatExpr`] whose value leaves `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NatOverflow {
    pub op: NatArithmetic,
    /// The span of the sum or product that overflowed.
    pub span: Span,
}

impl NatExpr {
    /// Evaluate this expression, operands in source order.
    ///
    /// Every closed subexpression is evaluated, even in an open expression,
    /// so a closed part that overflows is reported however the variables
    /// are later bound. Once a sum or product has met a variable, it is not
    /// computed further.
    ///
    /// # Errors
    ///
    /// Returns the first sum or product, in evaluation order, whose closed
    /// value leaves `u64`.
    pub fn closed_value(&self) -> Result<ClosedNat<'_>, NatOverflow> {
        match self {
            Self::Literal(value, _) => Ok(ClosedNat::Value(*value)),
            Self::Var(ident) => Ok(ClosedNat::Open { first_var: ident }),
            Self::Add(operands, span) => fold(operands, 0, NatArithmetic::Addition, *span),
            Self::Mul(operands, span) => fold(operands, 1, NatArithmetic::Multiplication, *span),
        }
    }
}

/// Combine `operands` left to right with `op`, starting from `identity`.
fn fold(
    operands: &crate::syntax::non_empty::AtLeastTwo<NatExpr>,
    identity: u64,
    op: NatArithmetic,
    span: Span,
) -> Result<ClosedNat<'_>, NatOverflow> {
    operands
        .iter()
        .try_fold(ClosedNat::Value(identity), |acc, operand| {
            Ok(match (acc, operand.closed_value()?) {
                (ClosedNat::Value(acc), ClosedNat::Value(value)) => {
                    let combined = match op {
                        NatArithmetic::Addition => acc.checked_add(value),
                        NatArithmetic::Multiplication => acc.checked_mul(value),
                    };
                    ClosedNat::Value(combined.ok_or(NatOverflow { op, span })?)
                }
                (open @ ClosedNat::Open { .. }, _)
                | (ClosedNat::Value(_), open @ ClosedNat::Open { .. }) => open,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::non_empty::AtLeastTwo;

    fn at_least_two(operands: Vec<NatExpr>) -> AtLeastTwo<NatExpr> {
        let mut operands = operands.into_iter();
        let mut items = AtLeastTwo::new(operands.next().unwrap(), operands.next().unwrap());
        for operand in operands {
            items.push(operand);
        }
        items
    }

    fn lit(value: u64) -> NatExpr {
        NatExpr::Literal(value, Span::new(0, 1))
    }

    fn var(name: &str, offset: usize) -> NatExpr {
        NatExpr::Var(Ident {
            name: crate::syntax::token::SourceIdentifier::parse(name).unwrap(),
            span: Span::new(offset, 1),
        })
    }

    fn add(operands: Vec<NatExpr>, offset: usize) -> NatExpr {
        NatExpr::Add(at_least_two(operands), Span::new(offset, 3))
    }

    fn mul(operands: Vec<NatExpr>, offset: usize) -> NatExpr {
        NatExpr::Mul(at_least_two(operands), Span::new(offset, 3))
    }

    #[test]
    fn closed_expressions_have_a_value() {
        assert_eq!(
            add(vec![lit(2), mul(vec![lit(3), lit(4)], 5)], 0).closed_value(),
            Ok(ClosedNat::Value(14))
        );
    }

    #[test]
    fn open_expressions_name_their_first_variable() {
        let expr = add(vec![lit(1), var("N", 4), var("M", 8)], 0);
        let Ok(ClosedNat::Open { first_var }) = expr.closed_value() else {
            panic!("expected an open expression");
        };
        assert_eq!(first_var.span, Span::new(4, 1));
    }

    #[test]
    fn closed_parts_overflow_even_in_open_expressions() {
        let overflowing = mul(vec![lit(u64::MAX), lit(2)], 7);
        assert_eq!(
            add(vec![var("N", 0), overflowing], 2).closed_value(),
            Err(NatOverflow {
                op: NatArithmetic::Multiplication,
                span: Span::new(7, 3),
            })
        );
        assert_eq!(
            add(vec![lit(u64::MAX), lit(1), var("N", 0)], 9).closed_value(),
            Err(NatOverflow {
                op: NatArithmetic::Addition,
                span: Span::new(9, 3),
            })
        );
        // Once a sum is open, it is not computed further.
        assert!(matches!(
            add(vec![var("N", 0), lit(u64::MAX), lit(1)], 9).closed_value(),
            Ok(ClosedNat::Open { .. })
        ));
    }
}
