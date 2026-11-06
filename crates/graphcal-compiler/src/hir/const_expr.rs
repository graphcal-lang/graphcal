//! Compile-time constant expressions and their pure evaluator.
//!
//! Unit-scale definitions (`const unit km: Length = 1000 m;`), coordinate
//! bounds (`range(0.0 s, 1.0 s, step: 0.1 s)`), and `linspace` point counts
//! are evaluated while the unit and index tables are built, before any
//! runtime value exists. The trees in this module are the checked form of
//! those positions. Lowering (`crate::hir::const_lower`) types every operator
//! with the shared rules of [`crate::tir::operator_typing`], so a tree
//! here is sorted like a checked node body: [`ConstInt`] holds an `Int`
//! value, [`ConstReal`] holds a real quantity, and an `Int` that scales a
//! quantity becomes an explicit [`ConstRealKind::FromInt`] node.
//!
//! Evaluation uses the checked operations node evaluation uses: the `Int`
//! kernel of [`crate::semantic::int_arith`], the checked finite arithmetic of
//! [`FiniteQuantity`], and the exact `Int` to binary64 conversion. The
//! position marker `P` makes the constructs a position rejects
//! unrepresentable (a quantity literal in a unit scale).

use std::convert::Infallible;

use thiserror::Error;

use crate::builtin::{BuiltinConst, IntDivisionFn};
use crate::dimension::Dimension;
use crate::display::unit_label::format_unit_expr_with_config;
use crate::exact_rational::ExactRational;
use crate::finite_value::{
    ExactI64ToF64Error, ExactPowerFailure, FiniteArithmeticError, FiniteQuantity,
};
use crate::hir::types::NatExpr;
use crate::ratio::ExactPowerError;
use crate::semantic::index_def::{
    CoordinateDisplayUnit, CoordinateIndexData, CoordinateIndexError,
};
use crate::semantic::int_arith::{self, IntArithError};
use crate::semantic::int_division::{self, IntDivisionError};
use crate::semantic::unit_scale::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitResolveError,
};
use crate::syntax::ast::UnitExpr;
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;
use crate::tir::texpr::operators::{ArithOp, IntArithOp, IntConversion};

/// A syntactic position that admits a constant expression.
pub trait ConstPosition: std::fmt::Debug + Clone + Copy + PartialEq {
    /// Witness that quantity literals (`1.0 m`) are admitted: `()` when they
    /// are and [`Infallible`] when they are not.
    type Quantity: std::fmt::Debug + Clone + Copy + PartialEq;
    /// The witness, when the position admits quantity literals.
    const QUANTITY: Option<Self::Quantity>;
}

/// The scale expression of a unit definition: dimensionless arithmetic over
/// numbers, integers, and built-in constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitScalePosition {}

impl ConstPosition for UnitScalePosition {
    type Quantity = Infallible;
    const QUANTITY: Option<Infallible> = None;
}

/// A coordinate bound or step: arithmetic over quantities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatePosition {}

impl ConstPosition for CoordinatePosition {
    type Quantity = ();
    const QUANTITY: Option<()> = Some(());
}

/// A unit literal resolved against the static unit registry.
///
/// `spelling` keeps the source unit expression for coordinate display labels;
/// `dimension` and `scale` are the resolved semantic facts.
#[derive(Debug, Clone)]
pub struct ConstUnit {
    spelling: UnitExpr,
    dimension: Dimension,
    scale: PositiveFiniteScale,
}

impl ConstUnit {
    /// Pair a unit spelling with its resolved dimension and static scale.
    #[must_use]
    pub(crate) const fn new(
        spelling: UnitExpr,
        dimension: Dimension,
        scale: PositiveFiniteScale,
    ) -> Self {
        Self {
            spelling,
            dimension,
            scale,
        }
    }

    /// The dimension the unit measures.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    fn display_unit(&self) -> CoordinateDisplayUnit {
        CoordinateDisplayUnit {
            label: Some(format_unit_expr_with_config(&self.spelling, true)),
            scale: self.scale,
        }
    }
}

/// One `Int`-valued constant node.
#[derive(Debug, Clone)]
pub enum ConstIntKind {
    /// An `Int` literal.
    Literal(i64),
    /// Negation.
    Neg(Box<ConstInt>),
    /// `+ - * %` on two `Int` values.
    Arith {
        op: IntArithOp,
        lhs: Box<ConstInt>,
        rhs: Box<ConstInt>,
    },
    /// A `div_*` built-in: the quotient rounded in the direction it names.
    Division {
        function: IntDivisionFn,
        dividend: Box<ConstInt>,
        divisor: Box<ConstInt>,
    },
    /// A power with an `Int` exponent.
    Power {
        base: Box<ConstInt>,
        exponent: Box<ConstInt>,
    },
}

/// A checked `Int`-valued constant expression.
#[derive(Debug, Clone)]
pub struct ConstInt {
    kind: ConstIntKind,
    span: Span,
}

/// One real-quantity-valued constant node.
#[derive(Debug, Clone)]
pub enum ConstRealKind<P: ConstPosition> {
    /// A dimensionless numeric literal.
    Number(FiniteQuantity),
    /// A built-in constant such as `PI`.
    Builtin(BuiltinConst),
    /// A quantity literal (`1.5 km`), admitted only where `P::Quantity` is
    /// inhabited.
    Quantity {
        value: FiniteQuantity,
        unit: ConstUnit,
        admitted: P::Quantity,
    },
    /// An `Int` converted exactly: an explicit `to_float(n)`, or an `Int`
    /// operand of `*` or `/` whose other operand is a quantity.
    FromInt {
        conversion: IntConversion,
        operand: Box<ConstInt>,
    },
    /// Negation.
    Neg(Box<ConstReal<P>>),
    /// `+ - * /` on two real quantities.
    Arith {
        op: ArithOp,
        lhs: Box<ConstReal<P>>,
        rhs: Box<ConstReal<P>>,
    },
    /// A power with an exact source exponent (`2`, `-2`, `(3/2)`), evaluated
    /// with real odd-root semantics.
    ExactPower {
        base: Box<ConstReal<P>>,
        exponent: ExactRational,
    },
    /// A dimensionless base raised to a dimensionless exponent expression.
    Power {
        base: Box<ConstReal<P>>,
        exponent: Box<ConstReal<P>>,
    },
}

/// A checked real-quantity-valued constant expression for position `P`.
#[derive(Debug, Clone)]
pub struct ConstReal<P: ConstPosition> {
    kind: ConstRealKind<P>,
    span: Span,
}

/// Why a constant expression could not be lowered or evaluated.
///
/// The messages are the user-facing diagnostic text; [`Self::span`] is the
/// source range they blame. Typing errors are not here: lowering reports
/// them with the diagnostics of the shared operator rules.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConstExprError {
    /// A unit-scale name is not a built-in constant.
    #[error(
        "unknown constant `{}` in scale expression; only built-in constants (PI, E, TAU, SQRT2, LN2, LN10) are supported",
        path.value
    )]
    UnknownScaleConstant { path: Spanned<NamePath> },
    /// A unit scale contained a construct outside the constant sublanguage.
    #[error(
        "scale expression must be a constant expression (numbers, built-in constants, arithmetic, `to_float`, and `div_*`)"
    )]
    NonConstantScale { span: Span },
    /// A coordinate name is not a built-in constant.
    #[error(
        "coordinate expression must be statically evaluable; `{}` is not a built-in constant",
        path.value
    )]
    UnknownCoordinateConstant { path: Spanned<NamePath> },
    /// A coordinate contained a construct outside the constant sublanguage.
    #[error(
        "coordinate arguments must be statically evaluable; only literals, built-in constants, arithmetic, `to_float`, and `div_*` are supported"
    )]
    NonStaticCoordinate { span: Span },
    /// A numeric literal was not finite.
    #[error("numeric literal must be finite, got {value}")]
    NonFiniteLiteral { value: f64, span: Span },
    /// A unit literal could not be resolved to a static unit.
    #[error("unit resolution failed")]
    Unit { error: UnitResolveError, span: Span },
    /// Checked `Int` arithmetic failed.
    #[error("{error}")]
    Int { error: IntArithError, span: Span },
    /// A `div_*` built-in has no `Int` quotient.
    #[error("{error}")]
    IntDivision { error: IntDivisionError, span: Span },
    /// A `to_float` argument has no exact binary64 value.
    #[error("to_float() argument: {error}")]
    ToFloat {
        error: ExactI64ToF64Error,
        span: Span,
    },
    /// An `Int` operand of `*` or `/` has no exact binary64 value.
    #[error("Int operand of `*` or `/` with a quantity: {error}")]
    IntScalar {
        error: ExactI64ToF64Error,
        span: Span,
    },
    /// An `Int` unit scale has no exact binary64 value.
    #[error("Int unit scale: {error}")]
    IntUnitScale {
        error: ExactI64ToF64Error,
        span: Span,
    },
    /// Checked quantity arithmetic failed.
    #[error("constant arithmetic failed: {error}")]
    Arithmetic {
        error: FiniteArithmeticError,
        span: Span,
    },
    /// An exact power has no real value.
    #[error("{error}")]
    ExactPower { error: ExactPowerError, span: Span },
    /// The final unit-scale value was not positive and finite.
    #[error("unit scale expression {error}")]
    InvalidUnitScale {
        error: PositiveFiniteScaleError,
        span: Span,
    },
    /// A `linspace` point count referenced a name instead of a constant.
    #[error("linspace point count must be statically known; `{name}` is not a constant Nat")]
    NonConstantNat { name: GenericParamName, span: Span },
    /// A `linspace` point-count sum overflowed `u64`.
    #[error("linspace point-count addition overflow")]
    NatAdditionOverflow { span: Span },
    /// A `linspace` point-count product overflowed `u64`.
    #[error("linspace point-count multiplication overflow")]
    NatMultiplicationOverflow { span: Span },
}

impl ConstExprError {
    /// Source range blamed by this error.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::UnknownScaleConstant { path } | Self::UnknownCoordinateConstant { path } => {
                path.span
            }
            Self::NonConstantScale { span }
            | Self::NonStaticCoordinate { span }
            | Self::NonFiniteLiteral { span, .. }
            | Self::Unit { span, .. }
            | Self::Int { span, .. }
            | Self::IntDivision { span, .. }
            | Self::ToFloat { span, .. }
            | Self::IntScalar { span, .. }
            | Self::IntUnitScale { span, .. }
            | Self::Arithmetic { span, .. }
            | Self::ExactPower { span, .. }
            | Self::InvalidUnitScale { span, .. }
            | Self::NonConstantNat { span, .. }
            | Self::NatAdditionOverflow { span }
            | Self::NatMultiplicationOverflow { span } => *span,
        }
    }
}

impl ConstInt {
    /// Build a node. Lowering is the only producer.
    #[must_use]
    pub(crate) const fn new(kind: ConstIntKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The node shape.
    #[must_use]
    pub const fn kind(&self) -> &ConstIntKind {
        &self.kind
    }

    /// Source span of the whole node.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Evaluate this expression with checked `Int` arithmetic, operands
    /// left to right.
    ///
    /// # Errors
    ///
    /// Returns [`ConstExprError::Int`] when an operation has no `Int` result.
    pub fn evaluate(&self) -> Result<i64, ConstExprError> {
        let result = match &self.kind {
            ConstIntKind::Literal(value) => Ok(*value),
            ConstIntKind::Neg(operand) => int_arith::negate(operand.evaluate()?),
            ConstIntKind::Arith { op, lhs, rhs } => {
                let lhs = lhs.evaluate()?;
                int_arith::arith(*op, lhs, rhs.evaluate()?)
            }
            ConstIntKind::Division {
                function,
                dividend,
                divisor,
            } => {
                let dividend = dividend.evaluate()?;
                return int_division::divide(*function, dividend, divisor.evaluate()?).map_err(
                    |error| ConstExprError::IntDivision {
                        error,
                        span: self.span,
                    },
                );
            }
            ConstIntKind::Power { base, exponent } => {
                let base = base.evaluate()?;
                int_arith::power(base, exponent.evaluate()?)
            }
        };
        result.map_err(|error| ConstExprError::Int {
            error,
            span: self.span,
        })
    }
}

impl<P: ConstPosition> ConstReal<P> {
    /// Build a node. Lowering is the only producer.
    #[must_use]
    pub(crate) const fn new(kind: ConstRealKind<P>, span: Span) -> Self {
        Self { kind, span }
    }

    /// The node shape.
    #[must_use]
    pub const fn kind(&self) -> &ConstRealKind<P> {
        &self.kind
    }

    /// Source span of the whole node.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Evaluate this expression to its SI value.
    ///
    /// Operands are evaluated left to right, and every step is the checked
    /// binary64 operation node evaluation applies, so the result is
    /// bit-identical to the same expression in a node body.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] when an operation has no finite result,
    /// an exact power has no real value, or an `Int` has no exact binary64
    /// value.
    pub fn evaluate(&self) -> Result<FiniteQuantity, ConstExprError> {
        let arithmetic = |error| ConstExprError::Arithmetic {
            error,
            span: self.span,
        };
        match &self.kind {
            ConstRealKind::Number(value) => Ok(*value),
            ConstRealKind::Builtin(constant) => FiniteQuantity::try_new(constant.value())
                .map_err(|_| arithmetic(FiniteArithmeticError::Infinite)),
            ConstRealKind::Quantity { value, unit, .. } => {
                FiniteQuantity::try_new(value.get() * unit.scale.get())
                    .map_err(|_| arithmetic(FiniteArithmeticError::Infinite))
            }
            ConstRealKind::FromInt {
                conversion,
                operand,
            } => FiniteQuantity::from_exact_int(operand.evaluate()?).map_err(|error| {
                let span = operand.span;
                match conversion {
                    IntConversion::ToFloat => ConstExprError::ToFloat { error, span },
                    IntConversion::Scaling => ConstExprError::IntScalar { error, span },
                }
            }),
            ConstRealKind::Neg(operand) => Ok(operand.evaluate()?.negated()),
            ConstRealKind::Arith { op, lhs, rhs } => {
                let lhs = lhs.evaluate()?;
                let rhs = rhs.evaluate()?;
                match op {
                    ArithOp::Add => lhs.checked_add(rhs),
                    ArithOp::Sub => lhs.checked_sub(rhs),
                    ArithOp::Mul => lhs.checked_mul(rhs),
                    ArithOp::Div => lhs.checked_div(rhs),
                }
                .map_err(arithmetic)
            }
            ConstRealKind::ExactPower { base, exponent } => base
                .evaluate()?
                .checked_exact_power(*exponent)
                .map_err(|error| match error {
                    ExactPowerFailure::NoRealValue(error) => ConstExprError::ExactPower {
                        error,
                        span: self.span,
                    },
                    ExactPowerFailure::Arithmetic(error) => arithmetic(error),
                }),
            ConstRealKind::Power { base, exponent } => {
                let base = base.evaluate()?;
                base.checked_powf(exponent.evaluate()?).map_err(arithmetic)
            }
        }
    }
}

/// The checked scale expression of a static unit definition: an `Int` or a
/// dimensionless quantity.
#[derive(Debug, Clone)]
pub enum UnitScaleExpr {
    /// An `Int` scale, converted exactly.
    Int(ConstInt),
    /// A dimensionless real scale.
    Real(ConstReal<UnitScalePosition>),
}

impl UnitScaleExpr {
    /// Evaluate the scale factor this expression contributes to its unit.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] when evaluation fails or the value is not
    /// positive and finite.
    pub fn evaluate(&self) -> Result<PositiveFiniteScale, ConstExprError> {
        let (value, span) = match self {
            Self::Int(expr) => (
                FiniteQuantity::from_exact_int(expr.evaluate()?).map_err(|error| {
                    ConstExprError::IntUnitScale {
                        error,
                        span: expr.span,
                    }
                })?,
                expr.span,
            ),
            Self::Real(expr) => (expr.evaluate()?, expr.span),
        };
        PositiveFiniteScale::new(value.get())
            .map_err(|error| ConstExprError::InvalidUnitScale { error, span })
    }
}

/// A checked coordinate bound or step: a real quantity of `dimension`.
#[derive(Debug, Clone)]
pub struct CoordinateExpr {
    expr: ConstReal<CoordinatePosition>,
    dimension: Dimension,
}

impl CoordinateExpr {
    /// Pair a checked tree with the dimension its typing derived.
    #[must_use]
    pub(crate) const fn new(expr: ConstReal<CoordinatePosition>, dimension: Dimension) -> Self {
        Self { expr, dimension }
    }

    /// The dimension of the coordinate value.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// Evaluate the coordinate value in SI.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] when evaluation fails.
    pub fn evaluate(&self) -> Result<FiniteQuantity, ConstExprError> {
        self.expr.evaluate()
    }

    /// The display unit of a coordinate axis whose start is this expression:
    /// the unit of a (possibly negated) quantity literal, or SI otherwise.
    #[must_use]
    pub fn display_unit(&self) -> CoordinateDisplayUnit {
        fn literal_unit(expr: &ConstReal<CoordinatePosition>) -> CoordinateDisplayUnit {
            match &expr.kind {
                ConstRealKind::Quantity { unit, .. } => unit.display_unit(),
                ConstRealKind::Neg(operand) => literal_unit(operand),
                ConstRealKind::Number(_)
                | ConstRealKind::Builtin(_)
                | ConstRealKind::FromInt { .. }
                | ConstRealKind::Arith { .. }
                | ConstRealKind::ExactPower { .. }
                | ConstRealKind::Power { .. } => CoordinateDisplayUnit::SI,
            }
        }
        literal_unit(&self.expr)
    }
}

/// Evaluate a static natural-number expression.
///
/// A static count is closed: lowering (`lower_static_nat_expr`) already
/// rejected every name.
///
/// # Errors
///
/// Returns a [`ConstExprError`] on `u64` overflow.
pub fn evaluate_static_nat(expr: &NatExpr) -> Result<u64, ConstExprError> {
    match expr {
        NatExpr::Literal(value, _) => Ok(*value),
        NatExpr::Add(operands, span) => operands.iter().try_fold(0_u64, |sum, operand| {
            sum.checked_add(evaluate_static_nat(operand)?)
                .ok_or(ConstExprError::NatAdditionOverflow { span: *span })
        }),
        NatExpr::Mul(operands, span) => operands.iter().try_fold(1_u64, |product, operand| {
            product
                .checked_mul(evaluate_static_nat(operand)?)
                .ok_or(ConstExprError::NatMultiplicationOverflow { span: *span })
        }),
    }
}

/// A resolved coordinate-axis declaration.
#[derive(Debug, Clone)]
pub enum CoordinateAxisExpr {
    /// `range(start, end, step: step)`.
    Range {
        start: CoordinateExpr,
        end: CoordinateExpr,
        step: CoordinateExpr,
    },
    /// `linspace(start, end, points: points)`.
    Linspace {
        start: CoordinateExpr,
        end: CoordinateExpr,
        points: NatExpr,
    },
}

/// Why a coordinate axis could not be evaluated.
#[derive(Debug, Clone, PartialEq)]
pub enum CoordinateAxisError {
    /// One argument failed to evaluate.
    Expr(ConstExprError),
    /// `range` arguments disagree on their dimension.
    RangeDimensionMismatch {
        start: Dimension,
        end: Dimension,
        step: Dimension,
    },
    /// `linspace` endpoints disagree on their dimension.
    LinspaceDimensionMismatch { start: Dimension, end: Dimension },
    /// The evaluated arguments do not describe a valid axis.
    Invalid {
        error: CoordinateIndexError,
        /// The `linspace` point-count span when the failure concerns it.
        point_count: Option<Span>,
    },
}

impl From<ConstExprError> for CoordinateAxisError {
    fn from(error: ConstExprError) -> Self {
        Self::Expr(error)
    }
}

impl CoordinateAxisExpr {
    /// Evaluate the axis arguments and build the validated coordinate data.
    ///
    /// # Errors
    ///
    /// Returns a [`CoordinateAxisError`] for mismatched argument dimensions,
    /// a failing argument, or an invalid axis.
    pub fn evaluate(&self) -> Result<CoordinateIndexData, CoordinateAxisError> {
        match self {
            Self::Range { start, end, step } => {
                if start.dimension != end.dimension || start.dimension != step.dimension {
                    return Err(CoordinateAxisError::RangeDimensionMismatch {
                        start: start.dimension.clone(),
                        end: end.dimension.clone(),
                        step: step.dimension.clone(),
                    });
                }
                CoordinateIndexData::try_range(
                    start.evaluate()?.get(),
                    end.evaluate()?.get(),
                    step.evaluate()?.get(),
                    start.dimension.clone(),
                    start.display_unit(),
                )
                .map_err(|error| CoordinateAxisError::Invalid {
                    error,
                    point_count: None,
                })
            }
            Self::Linspace { start, end, points } => {
                if start.dimension != end.dimension {
                    return Err(CoordinateAxisError::LinspaceDimensionMismatch {
                        start: start.dimension.clone(),
                        end: end.dimension.clone(),
                    });
                }
                let start_value = start.evaluate()?.get();
                let end_value = end.evaluate()?.get();
                let point_count = evaluate_static_nat(points)?;
                CoordinateIndexData::try_linspace(
                    start_value,
                    end_value,
                    point_count,
                    start.dimension.clone(),
                    start.display_unit(),
                )
                .map_err(|error| CoordinateAxisError::Invalid {
                    point_count: error.concerns_point_count().then(|| points.span()),
                    error,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::non_empty::AtLeastTwo;

    fn span() -> Span {
        Span::new(0, 1)
    }

    fn int(kind: ConstIntKind) -> ConstInt {
        ConstInt::new(kind, span())
    }

    fn real<P: ConstPosition>(kind: ConstRealKind<P>) -> ConstReal<P> {
        ConstReal::new(kind, span())
    }

    fn number<P: ConstPosition>(value: f64) -> ConstReal<P> {
        real(ConstRealKind::Number(
            FiniteQuantity::try_new(value).unwrap(),
        ))
    }

    fn boxed<T>(value: T) -> Box<T> {
        Box::new(value)
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the expected value replays the source-order binary64 operations"
    )]
    fn unit_scale_arithmetic_is_source_ordered_binary64() {
        // (PI / 180) * 2.0 - 1.0 + 3.0 ^ 2.0
        let expr = real::<UnitScalePosition>(ConstRealKind::Arith {
            op: ArithOp::Add,
            lhs: boxed(real(ConstRealKind::Arith {
                op: ArithOp::Sub,
                lhs: boxed(real(ConstRealKind::Arith {
                    op: ArithOp::Mul,
                    lhs: boxed(real(ConstRealKind::Arith {
                        op: ArithOp::Div,
                        lhs: boxed(real(ConstRealKind::Builtin(BuiltinConst::Pi))),
                        rhs: boxed(real(ConstRealKind::FromInt {
                            conversion: IntConversion::Scaling,
                            operand: boxed(int(ConstIntKind::Literal(180))),
                        })),
                    })),
                    rhs: boxed(number(2.0)),
                })),
                rhs: boxed(number(1.0)),
            })),
            rhs: boxed(real(ConstRealKind::Power {
                base: boxed(number(3.0)),
                exponent: boxed(number(2.0)),
            })),
        });
        let expected = std::f64::consts::PI / 180.0 * 2.0 - 1.0 + 3.0_f64.powf(2.0);
        let actual = UnitScaleExpr::Real(expr).evaluate().unwrap().get();
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    #[test]
    fn int_unit_scales_convert_exactly() {
        let thousand = UnitScaleExpr::Int(int(ConstIntKind::Literal(1000)));
        assert_eq!(
            thousand.evaluate().unwrap().get().to_bits(),
            1000.0_f64.to_bits()
        );
        let inexact = UnitScaleExpr::Int(int(ConstIntKind::Literal((1 << 53) + 1)));
        assert!(matches!(
            inexact.evaluate(),
            Err(ConstExprError::IntUnitScale { .. })
        ));
        let scaled = UnitScaleExpr::Real(real(ConstRealKind::Arith {
            op: ArithOp::Mul,
            lhs: boxed(real(ConstRealKind::FromInt {
                conversion: IntConversion::Scaling,
                operand: boxed(int(ConstIntKind::Literal((1 << 53) + 1))),
            })),
            rhs: boxed(number(1.0)),
        }));
        assert!(matches!(
            scaled.evaluate(),
            Err(ConstExprError::IntScalar { .. })
        ));
    }

    #[test]
    fn int_arithmetic_is_checked() {
        let overflow = int(ConstIntKind::Power {
            base: boxed(int(ConstIntKind::Literal(2))),
            exponent: boxed(int(ConstIntKind::Literal(63))),
        });
        assert!(matches!(
            overflow.evaluate(),
            Err(ConstExprError::Int {
                error: IntArithError::Overflow,
                ..
            })
        ));
        let remainder = int(ConstIntKind::Arith {
            op: IntArithOp::Mod,
            lhs: boxed(int(ConstIntKind::Literal(7))),
            rhs: boxed(int(ConstIntKind::Neg(boxed(int(ConstIntKind::Literal(3)))))),
        });
        assert_eq!(remainder.evaluate(), Ok(1));
    }

    #[test]
    fn unit_scale_rejects_non_positive_results_and_non_finite_steps() {
        let negative = UnitScaleExpr::Real(real(ConstRealKind::Neg(boxed(number(2.0)))));
        assert!(matches!(
            negative.evaluate(),
            Err(ConstExprError::InvalidUnitScale {
                error: PositiveFiniteScaleError::NonPositive { .. },
                ..
            })
        ));
        let infinite = UnitScaleExpr::Real(real(ConstRealKind::Arith {
            op: ArithOp::Mul,
            lhs: boxed(number(1e308)),
            rhs: boxed(number(10.0)),
        }));
        assert!(matches!(
            infinite.evaluate(),
            Err(ConstExprError::Arithmetic {
                error: FiniteArithmeticError::Infinite,
                ..
            })
        ));
    }

    #[test]
    fn exact_power_of_negative_base_with_even_denominator_fails() {
        let expr = UnitScaleExpr::Real(real(ConstRealKind::ExactPower {
            base: boxed(number(-4.0)),
            exponent: ExactRational::try_new(1, 2).unwrap(),
        }));
        assert!(matches!(
            expr.evaluate(),
            Err(ConstExprError::ExactPower {
                error: ExactPowerError::EvenRootOfNegative,
                ..
            })
        ));
        let cube = UnitScaleExpr::Real(real(ConstRealKind::ExactPower {
            base: boxed(number(2.0)),
            exponent: ExactRational::try_new(3, 1).unwrap(),
        }));
        assert_eq!(cube.evaluate().unwrap().get().to_bits(), 8.0_f64.to_bits());
    }

    fn metre(value: f64, scale: PositiveFiniteScale) -> ConstReal<CoordinatePosition> {
        let length = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ));
        real(ConstRealKind::Quantity {
            value: FiniteQuantity::try_new(value).unwrap(),
            unit: ConstUnit::new(
                UnitExpr {
                    terms: Vec::new(),
                    span: span(),
                },
                length,
                scale,
            ),
            admitted: (),
        })
    }

    #[test]
    fn display_unit_follows_negation_to_a_quantity_literal() {
        let plain = CoordinateExpr::new(number(1.0), Dimension::dimensionless());
        assert_eq!(plain.display_unit(), CoordinateDisplayUnit::SI);
        let two = PositiveFiniteScale::new(2.0).unwrap();
        let negated = CoordinateExpr::new(
            real(ConstRealKind::Neg(boxed(metre(1.0, two)))),
            Dimension::dimensionless(),
        );
        assert_eq!(negated.display_unit().scale, two);
        assert!(negated.display_unit().label.is_some());
        assert_eq!(negated.evaluate().unwrap().get(), -2.0);
    }

    #[test]
    fn static_nat_folds_with_overflow_checks() {
        let sum = NatExpr::Add(
            AtLeastTwo::new(NatExpr::Literal(2, span()), NatExpr::Literal(3, span())),
            span(),
        );
        let product = NatExpr::Mul(AtLeastTwo::new(sum, NatExpr::Literal(4, span())), span());
        assert_eq!(evaluate_static_nat(&product), Ok(20));
        let overflow = NatExpr::Add(
            AtLeastTwo::new(
                NatExpr::Literal(u64::MAX, span()),
                NatExpr::Literal(1, span()),
            ),
            span(),
        );
        assert!(matches!(
            evaluate_static_nat(&overflow),
            Err(ConstExprError::NatAdditionOverflow { .. })
        ));
        let product_overflow = NatExpr::Mul(
            AtLeastTwo::new(
                NatExpr::Literal(u64::MAX, span()),
                NatExpr::Literal(2, span()),
            ),
            span(),
        );
        assert!(matches!(
            evaluate_static_nat(&product_overflow),
            Err(ConstExprError::NatMultiplicationOverflow { .. })
        ));
    }

    #[test]
    fn coordinate_axes_check_dimensions_and_blame_the_point_count() {
        let dimensionless = |value| CoordinateExpr::new(number(value), Dimension::dimensionless());
        let axis = CoordinateAxisExpr::Linspace {
            start: dimensionless(0.0),
            end: dimensionless(1.0),
            points: NatExpr::Literal(0, Span::new(5, 1)),
        };
        assert!(matches!(
            axis.evaluate(),
            Err(CoordinateAxisError::Invalid {
                error: CoordinateIndexError::EmptyLinspace,
                point_count: Some(span),
            }) if span == Span::new(5, 1)
        ));
        let range = CoordinateAxisExpr::Range {
            start: dimensionless(0.0),
            end: dimensionless(1.0),
            step: dimensionless(0.5),
        };
        assert_eq!(range.evaluate().unwrap().cardinality(), 3);
        let length = metre(1.0, PositiveFiniteScale::ONE);
        let mismatch = CoordinateAxisExpr::Linspace {
            start: dimensionless(0.0),
            end: CoordinateExpr::new(
                length,
                Dimension::dimensionless()
                    .checked_mul(&Dimension::base(crate::dimension::BaseDimId::Prelude(
                        crate::dimension::PreludeBaseDimension::Length,
                    )))
                    .unwrap(),
            ),
            points: NatExpr::Literal(2, span()),
        };
        assert!(matches!(
            mismatch.evaluate(),
            Err(CoordinateAxisError::LinspaceDimensionMismatch { .. })
        ));
    }
}
