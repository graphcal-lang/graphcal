//! Checked operations, typed by their operands.
//!
//! Checking accepts an operator only for particular operand types, and a
//! typed tree records the operation that combination selects: a real sum, an
//! integer quotient, a complex scaling, a datetime shift, and so on. An
//! evaluator selects each operation from its node, never from the shape of
//! the values its operands produce, and an operand combination checking
//! rejects has no representation.
//!
//! Each family is generic over how it holds its operands, `C`: a stored tree
//! owns boxed children, and a scoped traversal hands them out in the scope of
//! the tree that holds them.

use crate::builtin::BuiltinConst;
use crate::exact_rational::ExactRational;

/// A real or complex arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// An integer arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Scaling of a complex value by a real quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleOp {
    Mul,
    Div,
}

/// Moving a datetime by a duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftOp {
    Add,
    Sub,
}

/// Equality or inequality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqualityOp {
    Eq,
    Ne,
}

/// One of the four ordering comparisons (`<`, `>`, `<=`, `>=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingOp {
    Lt,
    Gt,
    Le,
    Ge,
}

/// The totally ordered type both operands of an ordering comparison have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderedOperands {
    Quantity,
    Int,
    Datetime,
}

/// An operation whose checked result is a real quantity.
#[derive(Debug, Clone)]
pub enum QExpr<C> {
    /// A dimensionless numeric literal.
    Number(f64),
    /// A built-in mathematical constant.
    Constant(BuiltinConst),
    /// `+ - * /` on two real quantities.
    Arith {
        op: ArithOp,
        lhs: C,
        rhs: C,
    },
    /// A power with an exact rational exponent; the exponent's syntax stays
    /// a child of the node.
    ExactPower {
        base: C,
        exponent: ExactRational,
        exponent_expr: C,
    },
    /// A dimensionless base raised to a dimensionless runtime exponent.
    Power {
        base: C,
        exponent: C,
    },
    Neg(C),
    /// `datetime - datetime`: the elapsed duration.
    DatetimeDifference {
        lhs: C,
        rhs: C,
    },
}

/// An operation whose checked result is an `Int`.
#[derive(Debug, Clone)]
pub enum IExpr<C> {
    Literal(i64),
    Arith {
        op: IntArithOp,
        lhs: C,
        rhs: C,
    },
    /// A power with an exact non-negative integer exponent; the exponent's
    /// syntax stays a child of the node.
    ExactPower {
        base: C,
        exponent: i64,
        exponent_expr: C,
    },
    /// A power with a runtime `Int` exponent.
    Power {
        base: C,
        exponent: C,
    },
    Neg(C),
}

/// An operation whose checked result is a `Bool`.
#[derive(Debug, Clone)]
pub enum BExpr<C> {
    Literal(bool),
    Not(C),
    And {
        lhs: C,
        rhs: C,
    },
    Or {
        lhs: C,
        rhs: C,
    },
    /// Structural equality of two unindexed values of one type.
    Equality {
        op: EqualityOp,
        lhs: C,
        rhs: C,
    },
    Ordering {
        op: OrderingOp,
        operands: OrderedOperands,
        lhs: C,
        rhs: C,
    },
}

/// An operation whose checked result is a complex quantity.
#[derive(Debug, Clone)]
pub enum CExpr<C> {
    /// `+ - * /` on two complex quantities.
    Arith {
        op: ArithOp,
        lhs: C,
        rhs: C,
    },
    /// `complex * real` or `complex / real`.
    ScaleRight {
        op: ScaleOp,
        complex: C,
        scalar: C,
    },
    /// `real * complex` or `real / complex`.
    ScaleLeft {
        op: ScaleOp,
        scalar: C,
        complex: C,
    },
    Neg(C),
}

/// An operation whose checked result is a datetime.
#[derive(Debug, Clone)]
pub enum DExpr<C> {
    /// `datetime + duration` or `datetime - duration`.
    Shift {
        op: ShiftOp,
        datetime: C,
        duration: C,
    },
    /// `duration + datetime`.
    ShiftAfter { duration: C, datetime: C },
}

impl<C> QExpr<C> {
    /// This operation with every operand mapped by `f`, in structural order.
    pub(crate) fn try_map<'a, D, E>(
        &'a self,
        mut f: impl FnMut(&'a C) -> Result<D, E>,
    ) -> Result<QExpr<D>, E> {
        Ok(match self {
            Self::Number(value) => QExpr::Number(*value),
            Self::Constant(constant) => QExpr::Constant(*constant),
            Self::Arith { op, lhs, rhs } => QExpr::Arith {
                op: *op,
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::ExactPower {
                base,
                exponent,
                exponent_expr,
            } => QExpr::ExactPower {
                base: f(base)?,
                exponent: *exponent,
                exponent_expr: f(exponent_expr)?,
            },
            Self::Power { base, exponent } => QExpr::Power {
                base: f(base)?,
                exponent: f(exponent)?,
            },
            Self::Neg(operand) => QExpr::Neg(f(operand)?),
            Self::DatetimeDifference { lhs, rhs } => QExpr::DatetimeDifference {
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
        })
    }

    /// Every operand, in structural order.
    pub(crate) fn operands(&self) -> Vec<&C> {
        match self {
            Self::Number(_) | Self::Constant(_) => Vec::new(),
            Self::Neg(operand) => vec![operand],
            Self::Arith { lhs, rhs, .. } | Self::DatetimeDifference { lhs, rhs } => vec![lhs, rhs],
            Self::ExactPower {
                base,
                exponent_expr,
                ..
            } => vec![base, exponent_expr],
            Self::Power { base, exponent } => vec![base, exponent],
        }
    }
}

impl<C> IExpr<C> {
    /// This operation with every operand mapped by `f`, in structural order.
    pub(crate) fn try_map<'a, D, E>(
        &'a self,
        mut f: impl FnMut(&'a C) -> Result<D, E>,
    ) -> Result<IExpr<D>, E> {
        Ok(match self {
            Self::Literal(value) => IExpr::Literal(*value),
            Self::Arith { op, lhs, rhs } => IExpr::Arith {
                op: *op,
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::ExactPower {
                base,
                exponent,
                exponent_expr,
            } => IExpr::ExactPower {
                base: f(base)?,
                exponent: *exponent,
                exponent_expr: f(exponent_expr)?,
            },
            Self::Power { base, exponent } => IExpr::Power {
                base: f(base)?,
                exponent: f(exponent)?,
            },
            Self::Neg(operand) => IExpr::Neg(f(operand)?),
        })
    }

    /// Every operand, in structural order.
    pub(crate) fn operands(&self) -> Vec<&C> {
        match self {
            Self::Literal(_) => Vec::new(),
            Self::Neg(operand) => vec![operand],
            Self::Arith { lhs, rhs, .. } => vec![lhs, rhs],
            Self::ExactPower {
                base,
                exponent_expr,
                ..
            } => vec![base, exponent_expr],
            Self::Power { base, exponent } => vec![base, exponent],
        }
    }
}

impl<C> BExpr<C> {
    /// This operation with every operand mapped by `f`, in structural order.
    pub(crate) fn try_map<'a, D, E>(
        &'a self,
        mut f: impl FnMut(&'a C) -> Result<D, E>,
    ) -> Result<BExpr<D>, E> {
        Ok(match self {
            Self::Literal(value) => BExpr::Literal(*value),
            Self::Not(operand) => BExpr::Not(f(operand)?),
            Self::And { lhs, rhs } => BExpr::And {
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::Or { lhs, rhs } => BExpr::Or {
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::Equality { op, lhs, rhs } => BExpr::Equality {
                op: *op,
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::Ordering {
                op,
                operands,
                lhs,
                rhs,
            } => BExpr::Ordering {
                op: *op,
                operands: *operands,
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
        })
    }

    /// Every operand, in structural order.
    pub(crate) fn operands(&self) -> Vec<&C> {
        match self {
            Self::Literal(_) => Vec::new(),
            Self::Not(operand) => vec![operand],
            Self::And { lhs, rhs }
            | Self::Or { lhs, rhs }
            | Self::Equality { lhs, rhs, .. }
            | Self::Ordering { lhs, rhs, .. } => vec![lhs, rhs],
        }
    }
}

impl<C> CExpr<C> {
    /// This operation with every operand mapped by `f`, in structural order.
    pub(crate) fn try_map<'a, D, E>(
        &'a self,
        mut f: impl FnMut(&'a C) -> Result<D, E>,
    ) -> Result<CExpr<D>, E> {
        Ok(match self {
            Self::Arith { op, lhs, rhs } => CExpr::Arith {
                op: *op,
                lhs: f(lhs)?,
                rhs: f(rhs)?,
            },
            Self::ScaleRight {
                op,
                complex,
                scalar,
            } => CExpr::ScaleRight {
                op: *op,
                complex: f(complex)?,
                scalar: f(scalar)?,
            },
            Self::ScaleLeft {
                op,
                scalar,
                complex,
            } => CExpr::ScaleLeft {
                op: *op,
                scalar: f(scalar)?,
                complex: f(complex)?,
            },
            Self::Neg(operand) => CExpr::Neg(f(operand)?),
        })
    }

    /// Every operand, in structural order.
    pub(crate) fn operands(&self) -> Vec<&C> {
        match self {
            Self::Neg(operand) => vec![operand],
            Self::Arith { lhs, rhs, .. } => vec![lhs, rhs],
            Self::ScaleRight {
                complex, scalar, ..
            } => vec![complex, scalar],
            Self::ScaleLeft {
                scalar, complex, ..
            } => vec![scalar, complex],
        }
    }
}

impl<C> DExpr<C> {
    /// This operation with every operand mapped by `f`, in structural order.
    pub(crate) fn try_map<'a, D, E>(
        &'a self,
        mut f: impl FnMut(&'a C) -> Result<D, E>,
    ) -> Result<DExpr<D>, E> {
        Ok(match self {
            Self::Shift {
                op,
                datetime,
                duration,
            } => DExpr::Shift {
                op: *op,
                datetime: f(datetime)?,
                duration: f(duration)?,
            },
            Self::ShiftAfter { duration, datetime } => DExpr::ShiftAfter {
                duration: f(duration)?,
                datetime: f(datetime)?,
            },
        })
    }

    /// Every operand, in structural order.
    pub(crate) fn operands(&self) -> Vec<&C> {
        match self {
            Self::Shift {
                datetime, duration, ..
            } => vec![datetime, duration],
            Self::ShiftAfter { duration, datetime } => vec![duration, datetime],
        }
    }
}
