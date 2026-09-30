//! Typed operations: each checked operation evaluated by the kernel its node
//! selects.
//!
//! Checking recorded the operation an operator's operand types select, so
//! every operand here is read as the type its checked node has; a value of
//! another shape is a violated invariant, reported at one place
//! ([`Operands`]), and never an evaluation error.

use graphcal_compiler::complex_value::ComplexValue;
use graphcal_compiler::finite_value::FiniteQuantity;
use graphcal_compiler::registry::checked_type::IndexTypeRef;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::operators::{
    BExpr, CExpr, DExpr, EqualityOp, IExpr, OrderedOperands, QExpr, ShiftOp,
};
use graphcal_compiler::tir::typed::scoped_node::ScopedNode;

use crate::invariant::{Failure, Invariant};
use crate::runtime_value::{IndexAxis, KeyValue, RuntimeValue};

use super::EvalSession;
use super::arithmetic::apply_ordering;

/// Evaluates the operands of typed operations of one tree, reading each as
/// the type its checked node has.
pub(super) struct Operands<'o, 't> {
    evaluate: &'o dyn Fn(ScopedNode<'t>) -> Result<RuntimeValue, GraphcalError>,
    ctx: &'o EvalSession<'o>,
}

impl<'o, 't> Operands<'o, 't> {
    pub(super) fn new(
        evaluate: &'o dyn Fn(ScopedNode<'t>) -> Result<RuntimeValue, GraphcalError>,
        ctx: &'o EvalSession<'o>,
    ) -> Self {
        Self { evaluate, ctx }
    }

    /// The value of `node`, read as `extract` selects; any other shape
    /// contradicts the node's checked type.
    fn read<T>(
        &self,
        node: ScopedNode<'t>,
        expected: &str,
        extract: impl FnOnce(RuntimeValue) -> Result<T, RuntimeValue>,
    ) -> Result<T, GraphcalError> {
        extract((self.evaluate)(node)?).map_err(|other| {
            self.ctx.failure_error(
                Failure::<std::convert::Infallible>::Invariant(Invariant::violated(format_args!(
                    "operand checked as {expected} evaluated to {}",
                    other.describe()
                ))),
                node.span(),
            )
        })
    }

    fn value(&self, node: ScopedNode<'t>) -> Result<RuntimeValue, GraphcalError> {
        (self.evaluate)(node)
    }

    fn quantity(&self, node: ScopedNode<'t>) -> Result<FiniteQuantity, GraphcalError> {
        self.read(node, "a quantity", |value| match value {
            RuntimeValue::Quantity(value) => Ok(value),
            other => Err(other),
        })
    }

    fn int(&self, node: ScopedNode<'t>) -> Result<i64, GraphcalError> {
        self.read(node, "an Int", |value| match value {
            RuntimeValue::Int(value) => Ok(value),
            other => Err(other),
        })
    }

    pub(super) fn bool(&self, node: ScopedNode<'t>) -> Result<bool, GraphcalError> {
        self.read(node, "a Bool", |value| match value {
            RuntimeValue::Bool(value) => Ok(value),
            other => Err(other),
        })
    }

    fn complex(&self, node: ScopedNode<'t>) -> Result<ComplexValue, GraphcalError> {
        self.read(node, "a complex quantity", |value| match value {
            RuntimeValue::Complex(value) => Ok(value),
            other => Err(other),
        })
    }

    fn datetime(&self, node: ScopedNode<'t>) -> Result<hifitime::Epoch, GraphcalError> {
        self.read(node, "a Datetime", |value| match value {
            RuntimeValue::Datetime(value) => Ok(value),
            other => Err(other),
        })
    }

    fn key(&self, node: ScopedNode<'t>) -> Result<KeyValue, GraphcalError> {
        self.read(node, "a key", |value| match value {
            RuntimeValue::Key(value) => Ok(value),
            other => Err(other),
        })
    }
}

/// Evaluate an operation whose result is a real quantity.
pub(super) fn quantity<'t>(
    operation: &QExpr<ScopedNode<'t>>,
    span: Span,
    operands: &Operands<'_, 't>,
) -> Result<FiniteQuantity, GraphcalError> {
    let ctx = operands.ctx;
    match *operation {
        QExpr::Number(value) => super::numeric::finite_quantity(value, "numeric literal")
            .map_err(|error| ctx.eval_error(error.to_string(), span)),
        QExpr::Constant(constant) => {
            super::numeric::finite_quantity(constant.value(), "built-in constant")
                .map_err(|error| ctx.eval_error(error.to_string(), span))
        }
        QExpr::Arith { op, lhs, rhs } => {
            let lhs = operands.quantity(lhs)?;
            let rhs = operands.quantity(rhs)?;
            super::arithmetic::quantity_arith(op, lhs, rhs, ctx, span)
        }
        QExpr::ExactPower { base, exponent, .. } => {
            let base = operands.quantity(base)?;
            super::arithmetic::eval_exact_quantity_power(base, exponent, ctx, span)
        }
        QExpr::Power { base, exponent } => {
            let base = operands.quantity(base)?;
            let exponent = operands.quantity(exponent)?;
            super::arithmetic::quantity_power(base, exponent, ctx, span)
        }
        QExpr::Neg(operand) => Ok(operands.quantity(operand)?.negated()),
        QExpr::DatetimeDifference { lhs, rhs } => {
            let lhs = operands.datetime(lhs)?;
            let rhs = operands.datetime(rhs)?;
            let seconds = super::datetime::checked_epoch_difference_seconds(lhs, rhs)
                .map_err(|error| ctx.eval_error(error.to_string(), span))?;
            super::numeric::finite_quantity(seconds, "datetime difference")
                .map_err(|error| ctx.eval_error(error.to_string(), span))
        }
    }
}

/// Evaluate an operation whose result is an `Int`.
pub(super) fn int<'t>(
    operation: &IExpr<ScopedNode<'t>>,
    span: Span,
    operands: &Operands<'_, 't>,
) -> Result<i64, GraphcalError> {
    let ctx = operands.ctx;
    match *operation {
        IExpr::Literal(value) => Ok(value),
        IExpr::Arith { op, lhs, rhs } => {
            let lhs = operands.int(lhs)?;
            let rhs = operands.int(rhs)?;
            super::arithmetic::int_arith(op, lhs, rhs, ctx, span)
        }
        IExpr::ExactPower { base, exponent, .. } => {
            let base = operands.int(base)?;
            super::arithmetic::int_power(base, exponent, ctx, span)
        }
        IExpr::Power { base, exponent } => {
            let base = operands.int(base)?;
            let exponent = operands.int(exponent)?;
            super::arithmetic::int_power(base, exponent, ctx, span)
        }
        IExpr::Neg(operand) => operands
            .int(operand)?
            .checked_neg()
            .ok_or_else(|| ctx.eval_error("integer negation overflow", span)),
    }
}

/// Evaluate an operation whose result is a `Bool`.
///
/// Both operands of `and` and `or` are always evaluated.
pub(super) fn boolean<'t>(
    operation: &BExpr<ScopedNode<'t>>,
    operands: &Operands<'_, 't>,
) -> Result<bool, GraphcalError> {
    Ok(match *operation {
        BExpr::Literal(value) => value,
        BExpr::Not(operand) => !operands.bool(operand)?,
        BExpr::And { lhs, rhs } => {
            let lhs = operands.bool(lhs)?;
            let rhs = operands.bool(rhs)?;
            lhs && rhs
        }
        BExpr::Or { lhs, rhs } => {
            let lhs = operands.bool(lhs)?;
            let rhs = operands.bool(rhs)?;
            lhs || rhs
        }
        BExpr::Equality { op, lhs, rhs } => {
            let lhs = operands.value(lhs)?;
            let rhs = operands.value(rhs)?;
            (lhs == rhs) == (op == EqualityOp::Eq)
        }
        BExpr::Ordering {
            op,
            operands: kind,
            lhs,
            rhs,
        } => match kind {
            OrderedOperands::Quantity => {
                let lhs = operands.quantity(lhs)?;
                let rhs = operands.quantity(rhs)?;
                apply_ordering(op, &lhs, &rhs)
            }
            OrderedOperands::Int => {
                let lhs = operands.int(lhs)?;
                let rhs = operands.int(rhs)?;
                apply_ordering(op, &lhs, &rhs)
            }
            OrderedOperands::Datetime => {
                let lhs = operands.datetime(lhs)?;
                let rhs = operands.datetime(rhs)?;
                apply_ordering(op, &lhs, &rhs)
            }
        },
    })
}

/// Evaluate an operation whose result is a complex quantity.
pub(super) fn complex<'t>(
    operation: &CExpr<ScopedNode<'t>>,
    span: Span,
    operands: &Operands<'_, 't>,
) -> Result<ComplexValue, GraphcalError> {
    let result = match *operation {
        CExpr::Arith { op, lhs, rhs } => {
            let lhs = operands.complex(lhs)?;
            let rhs = operands.complex(rhs)?;
            super::complex::arith(op, lhs, rhs)
        }
        CExpr::ScaleRight {
            op,
            complex,
            scalar,
        } => {
            let complex = operands.complex(complex)?;
            let scalar = operands.quantity(scalar)?;
            super::complex::scale_right(op, complex, scalar)
        }
        CExpr::ScaleLeft {
            op,
            scalar,
            complex,
        } => {
            let scalar = operands.quantity(scalar)?;
            let complex = operands.complex(complex)?;
            super::complex::scale_left(op, scalar, complex)
        }
        CExpr::Neg(operand) => return Ok(operands.complex(operand)?.negated()),
    };
    result.map_err(|error| operands.ctx.eval_error(error.to_string(), span))
}

/// Evaluate an operation whose result is a datetime.
pub(super) fn datetime<'t>(
    operation: &DExpr<ScopedNode<'t>>,
    span: Span,
    operands: &Operands<'_, 't>,
) -> Result<hifitime::Epoch, GraphcalError> {
    let result = match *operation {
        DExpr::Shift {
            op,
            datetime,
            duration,
        } => {
            let datetime = operands.datetime(datetime)?;
            let seconds = operands.quantity(duration)?.get();
            match op {
                ShiftOp::Add => super::datetime::checked_epoch_add_seconds(datetime, seconds),
                ShiftOp::Sub => super::datetime::checked_epoch_subtract_seconds(datetime, seconds),
            }
        }
        DExpr::ShiftAfter { duration, datetime } => {
            let seconds = operands.quantity(duration)?.get();
            let datetime = operands.datetime(datetime)?;
            super::datetime::checked_epoch_add_seconds(datetime, seconds)
        }
    };
    result.map_err(|error| operands.ctx.eval_error(error.to_string(), span))
}

/// Evaluate `k + c` on a `Fin` key: the key at position `k + c` of the wider
/// target axis `Fin(N + c)`, which the checker derived from the static addend.
pub(super) fn key_shift<'t>(
    target: &IndexTypeRef,
    key: ScopedNode<'t>,
    addend: ScopedNode<'t>,
    span: Span,
    operands: &Operands<'_, 't>,
) -> Result<KeyValue, GraphcalError> {
    let ctx = operands.ctx;
    let key = operands.key(key)?;
    let addend = operands.int(addend)?;
    let axis = IndexAxis::resolve(ctx.tir, target).ok_or_else(|| {
        ctx.internal_error(
            format!("key axis `{target}` has no concrete definition"),
            span,
        )
    })?;
    usize::try_from(addend)
        .ok()
        .and_then(|addend| key.position().checked_add(addend))
        .and_then(|position| KeyValue::at(axis, position))
        .ok_or_else(|| {
            ctx.internal_error(
                format!("key shifted by {addend} left its checked axis `{target}`"),
                span,
            )
        })
}
