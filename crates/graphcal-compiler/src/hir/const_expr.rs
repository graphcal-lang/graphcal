//! Compile-time constant expressions and their single pure evaluator.
//!
//! Unit-scale definitions (`const unit km: Length = 1000 m;`), coordinate
//! bounds (`range(0.0 s, 1.0 s, step: 0.1 s)`), and `linspace` point counts
//! admit a closed arithmetic sublanguage that registry construction evaluates
//! before any runtime value exists. The trees in this module are the resolved
//! (HIR) form of those positions: built-in constants are typed
//! [`BuiltinConst`]s, unit literals carry their resolved dimension and static
//! scale, and the position marker `P` makes the constructs a position rejects
//! unrepresentable (an `Int` literal in a coordinate, a quantity literal in a
//! unit scale, a power in a coordinate).
//!
//! Evaluation is one generic fold ([`ConstExpr::evaluate`]); the position
//! entry points turn its result directly into the registry's invariant types:
//! [`UnitScaleExpr::evaluate`] yields a [`PositiveFiniteScale`] and
//! [`CoordinateAxisExpr::evaluate`] yields a validated
//! [`CoordinateIndexData`].

use std::convert::Infallible;

use thiserror::Error;

use crate::builtin::BuiltinConst;
use crate::dimension::Dimension;
use crate::display::unit_label::format_unit_expr_with_config;
use crate::exact_rational::{ExactPowerError, ExactRational};
use crate::hir::types::NatExpr;
use crate::semantic::index_def::{
    CoordinateDisplayUnit, CoordinateIndexData, CoordinateIndexError,
};
use crate::semantic::unit_scale::{
    PositiveFiniteScale, PositiveFiniteScaleError, UnitResolveError,
};
use crate::syntax::ast::{BinOp, UnitExpr};
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::GenericParamName;

/// A syntactic position that admits a constant expression.
///
/// Each associated type is `()` when the position admits the construct and
/// [`Infallible`] when it does not, so a lowered tree can only contain the
/// constructs its position accepts.
pub trait ConstPosition: std::fmt::Debug + Clone + Copy + PartialEq {
    /// Witness that `Int` literals are admitted.
    type Integer: std::fmt::Debug + Clone + Copy + PartialEq;
    /// Witness that quantity literals (`1.0 m`) are admitted.
    type Quantity: std::fmt::Debug + Clone + Copy + PartialEq;
    /// Witness that powers (`x ^ y`) are admitted.
    type Power: std::fmt::Debug + Clone + Copy + PartialEq;
    /// Whether every intermediate value must be finite.
    ///
    /// Unit scales validate only their final value; coordinates reject a
    /// non-finite value at the node that produced it.
    const FINITE_STEPS: bool;
}

/// The scale expression of a unit definition: dimensionless arithmetic over
/// numbers, integers, and built-in constants, including powers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitScalePosition {}

impl ConstPosition for UnitScalePosition {
    type Integer = ();
    type Quantity = Infallible;
    type Power = ();
    const FINITE_STEPS: bool = false;
}

/// A coordinate bound or step: finite quantity arithmetic without `Int`
/// values or powers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatePosition {}

impl ConstPosition for CoordinatePosition {
    type Integer = Infallible;
    type Quantity = ();
    type Power = Infallible;
    const FINITE_STEPS: bool = true;
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

    fn display_unit(&self) -> CoordinateDisplayUnit {
        CoordinateDisplayUnit {
            label: Some(format_unit_expr_with_config(&self.spelling, true)),
            scale: self.scale,
        }
    }
}

/// Arithmetic operators shared by every constant position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// The exponent of a constant power.
#[derive(Debug, Clone)]
pub enum ConstExponent<P: ConstPosition> {
    /// An exact source exponent (`2`, `-2`, `(3/2)`), evaluated with real
    /// odd-root semantics.
    Exact(ExactRational),
    /// Any other exponent, evaluated as a binary64 value.
    Evaluated(Box<ConstExpr<P>>),
}

/// One constant-expression node.
#[derive(Debug, Clone)]
pub enum ConstExprKind<P: ConstPosition> {
    /// A numeric literal. Coordinate lowering admits only finite literals.
    Number(f64),
    /// An `Int` literal, admitted only where `P::Integer` is inhabited.
    Integer(i64, P::Integer),
    /// A built-in constant such as `PI`.
    Builtin(BuiltinConst),
    /// A quantity literal (`1.5 km`), admitted only where `P::Quantity` is
    /// inhabited.
    Quantity {
        value: f64,
        unit: ConstUnit,
        admitted: P::Quantity,
    },
    /// Negation.
    Neg(Box<ConstExpr<P>>),
    /// Binary arithmetic.
    Arith {
        op: ConstArithOp,
        lhs: Box<ConstExpr<P>>,
        rhs: Box<ConstExpr<P>>,
    },
    /// A power, admitted only where `P::Power` is inhabited.
    Power {
        base: Box<ConstExpr<P>>,
        exponent: ConstExponent<P>,
        admitted: P::Power,
    },
}

/// A resolved constant expression for position `P`.
#[derive(Debug, Clone)]
pub struct ConstExpr<P: ConstPosition> {
    kind: ConstExprKind<P>,
    span: Span,
}

/// The value of a constant expression: an SI magnitude and its dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstQuantity {
    pub value: f64,
    pub dimension: Dimension,
}

impl ConstQuantity {
    const fn dimensionless(value: f64) -> Self {
        Self {
            value,
            dimension: Dimension::dimensionless(),
        }
    }
}

/// Why a constant expression could not be lowered or evaluated.
///
/// The messages are the user-facing diagnostic text; [`Self::span`] is the
/// source range they blame.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConstExprError {
    /// A unit-scale name is not a built-in constant.
    #[error(
        "unknown constant `{}` in scale expression; only built-in constants (PI, E, TAU, SQRT2, LN2, LN10) are supported",
        path.value
    )]
    UnknownScaleConstant { path: Spanned<NamePath> },
    /// A unit scale used a non-arithmetic operator.
    #[error(
        "unsupported operator `{op:?}` in scale expression; only `+`, `-`, `*`, `/`, `^` are allowed"
    )]
    UnsupportedScaleOperator { op: BinOp, span: Span },
    /// A unit scale contained a construct outside the constant sublanguage.
    #[error("scale expression must be a constant expression (numbers, PI, E, and arithmetic)")]
    NonConstantScale { span: Span },
    /// A coordinate name is not a built-in constant.
    #[error(
        "coordinate expression must be statically evaluable; `{}` is not a built-in constant",
        path.value
    )]
    UnknownCoordinateConstant { path: Spanned<NamePath> },
    /// A coordinate used an operator other than `+`, `-`, `*`, `/`.
    #[error("coordinate arguments support only static quantity arithmetic")]
    UnsupportedCoordinateOperator { span: Span },
    /// A coordinate contained an `Int` value or a runtime construct.
    #[error(
        "coordinate arguments must be statically evaluable quantities; Int values and runtime expressions are not supported"
    )]
    NonStaticCoordinate { span: Span },
    /// A coordinate literal or intermediate value was not finite.
    #[error("coordinate expression must evaluate to a finite quantity, got {value}")]
    NonFiniteCoordinate { value: f64, span: Span },
    /// Coordinate addition or subtraction mixed dimensions.
    #[error("addition or subtraction in a coordinate expression requires matching dimensions")]
    CoordinateDimensionMismatch { span: Span },
    /// A unit literal could not be resolved to a static unit.
    #[error("unit resolution failed")]
    Unit { error: UnitResolveError, span: Span },
    /// Dimension exponent arithmetic overflowed.
    #[error("dimension exponent overflow")]
    DimensionOverflow { span: Span },
    /// An exact power has no real value.
    #[error("{error}")]
    ExactPower { error: ExactPowerError, span: Span },
    /// A power was applied to a dimensioned base.
    #[error("powers in constant expressions require a dimensionless base")]
    DimensionedPower { span: Span },
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
            Self::UnsupportedScaleOperator { span, .. }
            | Self::NonConstantScale { span }
            | Self::UnsupportedCoordinateOperator { span }
            | Self::NonStaticCoordinate { span }
            | Self::NonFiniteCoordinate { span, .. }
            | Self::CoordinateDimensionMismatch { span }
            | Self::Unit { span, .. }
            | Self::DimensionOverflow { span }
            | Self::ExactPower { span, .. }
            | Self::DimensionedPower { span }
            | Self::InvalidUnitScale { span, .. }
            | Self::NonConstantNat { span, .. }
            | Self::NatAdditionOverflow { span }
            | Self::NatMultiplicationOverflow { span } => *span,
        }
    }
}

impl<P: ConstPosition> ConstExpr<P> {
    /// Build a node. Lowering is the only producer.
    #[must_use]
    pub(crate) const fn new(kind: ConstExprKind<P>, span: Span) -> Self {
        Self { kind, span }
    }

    /// The node shape.
    #[must_use]
    pub const fn kind(&self) -> &ConstExprKind<P> {
        &self.kind
    }

    /// Source span of the whole node.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Evaluate this expression to its SI value and dimension.
    ///
    /// Operands are evaluated left to right and every arithmetic step uses
    /// plain binary64 operations, so the result is bit-identical to the
    /// source-order computation.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] for mismatched or overflowing dimensions,
    /// an exact power without a real value, or (where `P::FINITE_STEPS`) a
    /// non-finite intermediate value.
    pub fn evaluate(&self) -> Result<ConstQuantity, ConstExprError> {
        let quantity = match &self.kind {
            ConstExprKind::Number(value) => ConstQuantity::dimensionless(*value),
            #[expect(clippy::cast_precision_loss, reason = "unit scale constant expression")]
            ConstExprKind::Integer(value, _) => ConstQuantity::dimensionless(*value as f64),
            ConstExprKind::Builtin(constant) => ConstQuantity::dimensionless(constant.value()),
            ConstExprKind::Quantity { value, unit, .. } => ConstQuantity {
                value: *value * unit.scale.get(),
                dimension: unit.dimension.clone(),
            },
            ConstExprKind::Neg(operand) => {
                let operand = operand.evaluate()?;
                ConstQuantity {
                    value: -operand.value,
                    dimension: operand.dimension,
                }
            }
            ConstExprKind::Arith { op, lhs, rhs } => {
                let lhs = lhs.evaluate()?;
                let rhs = rhs.evaluate()?;
                self.arith(*op, lhs, &rhs)?
            }
            ConstExprKind::Power { base, exponent, .. } => {
                let base = base.evaluate()?;
                let value = match exponent {
                    ConstExponent::Exact(exponent) => {
                        exponent.pow_f64(base.value).map_err(|error| {
                            ConstExprError::ExactPower {
                                error,
                                span: self.span,
                            }
                        })?
                    }
                    ConstExponent::Evaluated(exponent) => {
                        base.value.powf(exponent.evaluate()?.value)
                    }
                };
                if !base.dimension.is_dimensionless() {
                    return Err(ConstExprError::DimensionedPower { span: self.span });
                }
                ConstQuantity::dimensionless(value)
            }
        };
        if P::FINITE_STEPS && !quantity.value.is_finite() {
            return Err(ConstExprError::NonFiniteCoordinate {
                value: quantity.value,
                span: self.span,
            });
        }
        Ok(quantity)
    }

    fn arith(
        &self,
        op: ConstArithOp,
        lhs: ConstQuantity,
        rhs: &ConstQuantity,
    ) -> Result<ConstQuantity, ConstExprError> {
        let overflow = |_| ConstExprError::DimensionOverflow { span: self.span };
        match op {
            ConstArithOp::Add | ConstArithOp::Sub => {
                if lhs.dimension != rhs.dimension {
                    return Err(ConstExprError::CoordinateDimensionMismatch { span: self.span });
                }
                let value = if op == ConstArithOp::Add {
                    lhs.value + rhs.value
                } else {
                    lhs.value - rhs.value
                };
                Ok(ConstQuantity {
                    value,
                    dimension: lhs.dimension,
                })
            }
            ConstArithOp::Mul => Ok(ConstQuantity {
                value: lhs.value * rhs.value,
                dimension: lhs
                    .dimension
                    .checked_mul(&rhs.dimension)
                    .map_err(overflow)?,
            }),
            ConstArithOp::Div => Ok(ConstQuantity {
                value: lhs.value / rhs.value,
                dimension: lhs
                    .dimension
                    .checked_div(&rhs.dimension)
                    .map_err(overflow)?,
            }),
        }
    }
}

/// The resolved scale expression of a static unit definition.
#[derive(Debug, Clone)]
pub struct UnitScaleExpr(ConstExpr<UnitScalePosition>);

impl UnitScaleExpr {
    /// Wrap a lowered unit-scale tree.
    #[must_use]
    pub(crate) const fn new(expr: ConstExpr<UnitScalePosition>) -> Self {
        Self(expr)
    }

    /// Evaluate the scale factor this expression contributes to its unit.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] when evaluation fails or the value is not
    /// positive and finite.
    pub fn evaluate(&self) -> Result<PositiveFiniteScale, ConstExprError> {
        let value = self.0.evaluate()?.value;
        PositiveFiniteScale::new(value).map_err(|error| ConstExprError::InvalidUnitScale {
            error,
            span: self.0.span,
        })
    }
}

/// A resolved coordinate bound or step.
pub type CoordinateExpr = ConstExpr<CoordinatePosition>;

impl CoordinateExpr {
    /// The display unit of a coordinate axis whose start is this expression:
    /// the unit of a (possibly negated) quantity literal, or SI otherwise.
    #[must_use]
    pub fn display_unit(&self) -> CoordinateDisplayUnit {
        match &self.kind {
            ConstExprKind::Quantity { unit, .. } => unit.display_unit(),
            ConstExprKind::Neg(operand) => operand.display_unit(),
            ConstExprKind::Number(_)
            | ConstExprKind::Integer(..)
            | ConstExprKind::Builtin(_)
            | ConstExprKind::Arith { .. }
            | ConstExprKind::Power { .. } => CoordinateDisplayUnit::SI,
        }
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
    /// Returns a [`CoordinateAxisError`] for a failing argument, mismatched
    /// argument dimensions, or an invalid axis.
    pub fn evaluate(&self) -> Result<CoordinateIndexData, CoordinateAxisError> {
        match self {
            Self::Range { start, end, step } => {
                let start_value = start.evaluate()?;
                let end_value = end.evaluate()?;
                let step_value = step.evaluate()?;
                if start_value.dimension != end_value.dimension
                    || start_value.dimension != step_value.dimension
                {
                    return Err(CoordinateAxisError::RangeDimensionMismatch {
                        start: start_value.dimension,
                        end: end_value.dimension,
                        step: step_value.dimension,
                    });
                }
                CoordinateIndexData::try_range(
                    start_value.value,
                    end_value.value,
                    step_value.value,
                    start_value.dimension,
                    start.display_unit(),
                )
                .map_err(|error| CoordinateAxisError::Invalid {
                    error,
                    point_count: None,
                })
            }
            Self::Linspace { start, end, points } => {
                let start_value = start.evaluate()?;
                let end_value = end.evaluate()?;
                if start_value.dimension != end_value.dimension {
                    return Err(CoordinateAxisError::LinspaceDimensionMismatch {
                        start: start_value.dimension,
                        end: end_value.dimension,
                    });
                }
                let point_count = evaluate_static_nat(points)?;
                CoordinateIndexData::try_linspace(
                    start_value.value,
                    end_value.value,
                    point_count,
                    start_value.dimension,
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

    fn scale(kind: ConstExprKind<UnitScalePosition>) -> ConstExpr<UnitScalePosition> {
        ConstExpr::new(kind, span())
    }

    fn coordinate(kind: ConstExprKind<CoordinatePosition>) -> CoordinateExpr {
        ConstExpr::new(kind, span())
    }

    fn boxed<P: ConstPosition>(expr: ConstExpr<P>) -> Box<ConstExpr<P>> {
        Box::new(expr)
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the expected value replays the source-order binary64 operations"
    )]
    fn unit_scale_arithmetic_is_source_ordered_binary64() {
        // (PI / 180) * 2 - 1 + 3 ^ 2
        let expr = scale(ConstExprKind::Arith {
            op: ConstArithOp::Add,
            lhs: boxed(scale(ConstExprKind::Arith {
                op: ConstArithOp::Sub,
                lhs: boxed(scale(ConstExprKind::Arith {
                    op: ConstArithOp::Mul,
                    lhs: boxed(scale(ConstExprKind::Arith {
                        op: ConstArithOp::Div,
                        lhs: boxed(scale(ConstExprKind::Builtin(BuiltinConst::Pi))),
                        rhs: boxed(scale(ConstExprKind::Integer(180, ()))),
                    })),
                    rhs: boxed(scale(ConstExprKind::Number(2.0))),
                })),
                rhs: boxed(scale(ConstExprKind::Number(1.0))),
            })),
            rhs: boxed(scale(ConstExprKind::Power {
                base: boxed(scale(ConstExprKind::Number(3.0))),
                exponent: ConstExponent::Evaluated(boxed(scale(ConstExprKind::Number(2.0)))),
                admitted: (),
            })),
        });
        let expected = std::f64::consts::PI / 180.0 * 2.0 - 1.0 + 3.0_f64.powf(2.0);
        let actual = UnitScaleExpr::new(expr).evaluate().unwrap().get();
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    #[test]
    fn unit_scale_rejects_non_positive_and_non_finite_results() {
        let negative = UnitScaleExpr::new(scale(ConstExprKind::Neg(boxed(scale(
            ConstExprKind::Number(2.0),
        )))));
        assert!(matches!(
            negative.evaluate(),
            Err(ConstExprError::InvalidUnitScale {
                error: PositiveFiniteScaleError::NonPositive { .. },
                ..
            })
        ));
        // Intermediate infinities are not rejected for unit scales; only the
        // final value is validated.
        let infinite = UnitScaleExpr::new(scale(ConstExprKind::Arith {
            op: ConstArithOp::Mul,
            lhs: boxed(scale(ConstExprKind::Number(1e308))),
            rhs: boxed(scale(ConstExprKind::Number(10.0))),
        }));
        assert!(matches!(
            infinite.evaluate(),
            Err(ConstExprError::InvalidUnitScale {
                error: PositiveFiniteScaleError::NonFinite { .. },
                ..
            })
        ));
    }

    #[test]
    fn exact_power_of_negative_base_with_even_denominator_fails() {
        let expr = UnitScaleExpr::new(scale(ConstExprKind::Power {
            base: boxed(scale(ConstExprKind::Number(-4.0))),
            exponent: ConstExponent::Exact(ExactRational::try_new(1, 2).unwrap()),
            admitted: (),
        }));
        assert!(matches!(
            expr.evaluate(),
            Err(ConstExprError::ExactPower {
                error: ExactPowerError::EvenRootOfNegative,
                ..
            })
        ));
        let cube = UnitScaleExpr::new(scale(ConstExprKind::Power {
            base: boxed(scale(ConstExprKind::Number(2.0))),
            exponent: ConstExponent::Exact(ExactRational::try_new(3, 1).unwrap()),
            admitted: (),
        }));
        assert_eq!(cube.evaluate().unwrap().get().to_bits(), 8.0_f64.to_bits());
    }

    #[test]
    fn coordinate_steps_must_stay_finite() {
        let expr = coordinate(ConstExprKind::Arith {
            op: ConstArithOp::Mul,
            lhs: boxed(coordinate(ConstExprKind::Number(1e308))),
            rhs: boxed(coordinate(ConstExprKind::Number(10.0))),
        });
        assert!(matches!(
            expr.evaluate(),
            Err(ConstExprError::NonFiniteCoordinate { value, .. }) if value.is_infinite()
        ));
    }

    #[test]
    fn coordinate_addition_requires_matching_dimensions() {
        let length = Dimension::base(crate::dimension::BaseDimId::Prelude(
            crate::dimension::PreludeBaseDimension::Length,
        ));
        let unit = ConstUnit::new(
            UnitExpr {
                terms: Vec::new(),
                span: span(),
            },
            length.clone(),
            PositiveFiniteScale::ONE,
        );
        let metre = coordinate(ConstExprKind::Quantity {
            value: 1.0,
            unit,
            admitted: (),
        });
        let mismatch = coordinate(ConstExprKind::Arith {
            op: ConstArithOp::Sub,
            lhs: boxed(metre.clone()),
            rhs: boxed(coordinate(ConstExprKind::Number(1.0))),
        });
        assert!(matches!(
            mismatch.evaluate(),
            Err(ConstExprError::CoordinateDimensionMismatch { .. })
        ));
        let area = coordinate(ConstExprKind::Arith {
            op: ConstArithOp::Mul,
            lhs: boxed(metre.clone()),
            rhs: boxed(metre),
        });
        assert_eq!(
            area.evaluate().unwrap().dimension,
            length.clone().checked_mul(&length).unwrap()
        );
    }

    #[test]
    fn display_unit_follows_negation_to_a_quantity_literal() {
        let plain = coordinate(ConstExprKind::Number(1.0));
        assert_eq!(plain.display_unit(), CoordinateDisplayUnit::SI);
        let two = PositiveFiniteScale::new(2.0).unwrap();
        let unit = ConstUnit::new(
            UnitExpr {
                terms: Vec::new(),
                span: span(),
            },
            Dimension::dimensionless(),
            two,
        );
        let negated = coordinate(ConstExprKind::Neg(boxed(coordinate(
            ConstExprKind::Quantity {
                value: 1.0,
                unit,
                admitted: (),
            },
        ))));
        assert_eq!(negated.display_unit().scale, two);
        assert!(negated.display_unit().label.is_some());
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
    fn linspace_blames_the_point_count() {
        let axis = CoordinateAxisExpr::Linspace {
            start: coordinate(ConstExprKind::Number(0.0)),
            end: coordinate(ConstExprKind::Number(1.0)),
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
            start: coordinate(ConstExprKind::Number(0.0)),
            end: coordinate(ConstExprKind::Number(1.0)),
            step: coordinate(ConstExprKind::Number(0.5)),
        };
        assert_eq!(range.evaluate().unwrap().cardinality(), 3);
    }
}
