//! Pure dimension/type rules shared by the syntax-AST and HIR inference
//! engines.
//!
//! Both engines walk different expression representations but must apply
//! identical typing rules. Keeping the rules here as pure functions over
//! [`CheckedType`] operands means a rule change lands once — the engines
//! had already drifted (HIR accepted `-` on Bool) when each carried its own
//! copy.

use crate::desugar::desugared_ast::{BinOp, UnaryOp};
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension, Rational};
use crate::display::formatting_registry::FormattingRegistry;
use crate::exact_rational::ExactRational;
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{
    FinKeyArithmeticRule, MismatchOperand, MismatchRule, OperandExpectation,
};
use crate::semantic_error::index::IndexError;
use crate::semantic_error::structure::StructError;
use crate::source_id::SourceId;
use crate::syntax::ast::PowerExponent;
use crate::syntax::span::Span;

use super::super::helpers::{expect_quantity, format_checked_type};
use crate::semantic::checked_type::{CheckedType, Symbolic};

/// A typed operand with the span diagnostics should point at.
pub(super) struct Operand {
    pub ty: CheckedType<Symbolic>,
    pub span: Span,
}

/// Require one comparison operand to be an unindexed value.
///
/// Comparisons deliberately follow arithmetic's no-broadcasting rule: callers
/// must use an explicit `for` comprehension and compare one element at a time.
fn comparison_operand_type<'a>(
    operand: &'a Operand,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<&'a CheckedType<Symbolic>, SemanticError> {
    match &operand.ty {
        CheckedType::Indexed { .. } => Err(SemanticError::located(
            src,
            operand.span,
            DimensionError::IndexedComparisonOperand {
                found: format_checked_type(&operand.ty, registry),
            },
        )),
        ty => Ok(ty),
    }
}

fn exact_float_replacement(exact: Option<ExactRational>) -> Option<String> {
    let rational = Rational::try_from(exact?).ok()?;
    Some(rational.source_syntax().to_string())
}

/// The exact additive fragment of Fin-key arithmetic:
/// `k : Key<Fin(N)>` plus a static Nat constant `c` yields `Key<Fin(N + c)>`.
fn fin_key_additive_rule(
    op: BinOp,
    key_index: &crate::semantic::checked_type::IndexTypeRef<Symbolic>,
    rhs: &Operand,
    rhs_const_int: Option<i64>,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    let reject = |help: FinKeyArithmeticRule| {
        Err(SemanticError::located(
            src,
            rhs.span,
            DimensionError::DimensionMismatch {
                expected: Box::new(MismatchOperand::Expected(
                    OperandExpectation::StaticNatConstant,
                )),
                found: Box::new(MismatchOperand::Type(rhs.ty.spelling(&registry.dimensions))),
                help: Box::new(MismatchRule::FinKeyArithmetic(help)),
            },
        ))
    };
    let Some(bound) = key_index.finite_index_form() else {
        return reject(FinKeyArithmeticRule::NamedOrCoordinateKey);
    };
    if op != BinOp::Add {
        return reject(FinKeyArithmeticRule::Subtraction);
    }
    if rhs.ty != CheckedType::Int {
        return reject(FinKeyArithmeticRule::IntegerAddend);
    }
    let Some(addend) = rhs_const_int else {
        return reject(FinKeyArithmeticRule::RuntimeOffset);
    };
    let Ok(addend) = u64::try_from(addend) else {
        return reject(FinKeyArithmeticRule::NegativeAddend);
    };
    let shifted = bound
        .add(&crate::nat::NatPolyForm::from_constant(addend))
        .map_err(|err| {
            SemanticError::located(src, rhs.span, IndexError::NatOverflow { error: err })
        })?;
    crate::semantic::checked_type::IndexTypeRef::from_finite_index_form(shifted)
        .map(CheckedType::Key)
        .map_err(|err| {
            SemanticError::located(
                src,
                rhs.span,
                IndexError::InvalidFiniteIndexCardinality { error: err },
            )
        })
}

/// Typing rule for a binary operation, given already-inferred operands.
///
/// `rhs_const_int` is the right operand's constant-folded `Int` value for
/// runtime-classified `Int ^ Int` chains (issue #578) and for the additive
/// Fin-key rule. Exact literal and rational power syntax is carried directly
/// by [`BinOp::Pow`].
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive match over all BinOp variants"
)]
pub(super) fn binop_rule(
    expr_span: Span,
    op: BinOp,
    lhs: &Operand,
    rhs: &Operand,
    rhs_const_int: Option<i64>,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    let lhs_type = &lhs.ty;
    let rhs_type = &rhs.ty;
    match op {
        // Logical operators: require Bool operands, return Bool
        BinOp::And | BinOp::Or => {
            if *lhs_type != CheckedType::Bool {
                return Err(SemanticError::located(
                    src,
                    lhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(OperandExpectation::Bool)),
                        found: Box::new(MismatchOperand::Type(
                            lhs_type.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::BooleanOperands),
                    },
                ));
            }
            if *rhs_type != CheckedType::Bool {
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(OperandExpectation::Bool)),
                        found: Box::new(MismatchOperand::Type(
                            rhs_type.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::BooleanOperands),
                    },
                ));
            }
            Ok(CheckedType::Bool)
        }
        // Equality operands must be unindexed and have the same value type.
        BinOp::Eq | BinOp::Ne => {
            let lhs_type = comparison_operand_type(lhs, registry, src)?;
            let rhs_type = comparison_operand_type(rhs, registry, src)?;
            if lhs_type == rhs_type {
                return Ok(CheckedType::Bool);
            }
            if let (Some(lhs_dim), Some(rhs_dim)) =
                (lhs_type.quantity_dimension(), rhs_type.quantity_dimension())
                && lhs_dim == rhs_dim
            {
                return Ok(CheckedType::Bool);
            }
            Err(SemanticError::located(
                src,
                rhs.span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Type(
                        lhs_type.spelling(&registry.dimensions),
                    )),
                    found: Box::new(MismatchOperand::Type(
                        rhs_type.spelling(&registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::EqualitySameType),
                },
            ))
        }
        // Ordering comparisons require unindexed operands that are same-type
        // quantities, Int/Fin values, or same-scale Datetimes.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let lhs_type = comparison_operand_type(lhs, registry, src)?;
            let rhs_type = comparison_operand_type(rhs, registry, src)?;
            if matches!(lhs_type, CheckedType::Complex(_))
                || matches!(rhs_type, CheckedType::Complex(_))
            {
                return Err(SemanticError::located(
                    src,
                    if matches!(lhs_type, CheckedType::Complex(_)) {
                        lhs.span
                    } else {
                        rhs.span
                    },
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::OrderedQuantity,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            if matches!(lhs_type, CheckedType::Complex(_)) {
                                lhs_type
                            } else {
                                rhs_type
                            }
                            .spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ComplexUnordered),
                    },
                ));
            }
            if matches!(lhs_type, CheckedType::Int) || matches!(rhs_type, CheckedType::Int) {
                if !matches!(lhs_type, CheckedType::Int) || !matches!(rhs_type, CheckedType::Int) {
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Type(
                                lhs_type.spelling(&registry.dimensions),
                            )),
                            found: Box::new(MismatchOperand::Type(
                                rhs_type.spelling(&registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::ComparisonSameType),
                        },
                    ));
                }
                return Ok(CheckedType::Bool);
            }
            // Datetime comparisons: same time scale required
            if let CheckedType::Datetime(ls) = lhs_type
                && let CheckedType::Datetime(rs) = rhs_type
            {
                if ls != rs {
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Type(
                                lhs_type.spelling(&registry.dimensions),
                            )),
                            found: Box::new(MismatchOperand::Type(
                                rhs_type.spelling(&registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::DatetimeComparisonScales),
                        },
                    ));
                }
                return Ok(CheckedType::Bool);
            }
            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Dimension(
                            registry.dimensions.dimension_spelling(&lhs_dim),
                        )),
                        found: Box::new(MismatchOperand::Dimension(
                            registry.dimensions.dimension_spelling(&rhs_dim),
                        )),
                        help: Box::new(MismatchRule::ComparisonSameDimension),
                    },
                ));
            }
            Ok(CheckedType::Bool)
        }
        // Arithmetic operators: require matching numeric operands (Int or Quantity)
        BinOp::Add | BinOp::Sub => {
            // Additive Fin-key arithmetic: `k + c` with a static Nat constant
            // shifts the bound into the type — `Key<Fin(N)> + c : Key<Fin(N + c)>`
            // — exactly and infallibly. Everything else on keys is rejected:
            // subtraction is fallible at 0 and Nat itself has none; runtime
            // offsets escape any static bound.
            if let CheckedType::Key(key_index) = lhs_type {
                return fin_key_additive_rule(op, key_index, rhs, rhs_const_int, registry, src);
            }
            if matches!(rhs_type, CheckedType::Key(_)) {
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Type(
                            lhs_type.spelling(&registry.dimensions),
                        )),
                        found: Box::new(MismatchOperand::Type(
                            rhs_type.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::FinKeyArithmeticKeyFirst),
                    },
                ));
            }
            if matches!(lhs_type, CheckedType::Int) && matches!(rhs_type, CheckedType::Int) {
                return Ok(CheckedType::Int);
            }
            match (lhs_type, rhs_type) {
                (CheckedType::Complex(lhs_dim), CheckedType::Complex(rhs_dim)) => {
                    if lhs_dim != rhs_dim {
                        return Err(SemanticError::located(
                            src,
                            rhs.span,
                            DimensionError::DimensionMismatch {
                                expected: Box::new(MismatchOperand::Type(
                                    lhs_type.spelling(&registry.dimensions),
                                )),
                                found: Box::new(MismatchOperand::Type(
                                    rhs_type.spelling(&registry.dimensions),
                                )),
                                help: Box::new(MismatchRule::ComplexAdditionSameDimension),
                            },
                        ));
                    }
                    return Ok(CheckedType::Complex(lhs_dim.clone()));
                }
                (CheckedType::Complex(_), _) | (_, CheckedType::Complex(_)) => {
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Type(
                                lhs_type.spelling(&registry.dimensions),
                            )),
                            found: Box::new(MismatchOperand::Type(
                                rhs_type.spelling(&registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::NoImplicitComplexPromotion),
                        },
                    ));
                }
                _ => {}
            }
            // Point-vs-vector rules for Datetime
            if let CheckedType::Datetime(ls) = lhs_type {
                let time_dim = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
                if let CheckedType::Datetime(rs) = rhs_type {
                    // Datetime - Datetime -> Quantity(Time)
                    if op == BinOp::Sub {
                        if ls != rs {
                            return Err(SemanticError::located(
                                src,
                                rhs.span,
                                DimensionError::DimensionMismatch {
                                    expected: Box::new(MismatchOperand::Type(
                                        lhs_type.spelling(&registry.dimensions),
                                    )),
                                    found: Box::new(MismatchOperand::Type(
                                        rhs_type.spelling(&registry.dimensions),
                                    )),
                                    help: Box::new(MismatchRule::DatetimeSubtractionScales),
                                },
                            ));
                        }
                        return Ok(CheckedType::Quantity(time_dim));
                    }
                    // Datetime + Datetime -> error
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::TimeQuantity,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                rhs_type.spelling(&registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::DatetimeAddition),
                        },
                    ));
                }
                // Datetime +/- Quantity(Time) -> Datetime
                let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
                if rhs_dim != time_dim {
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(OperandExpectation::Time)),
                            found: Box::new(MismatchOperand::Dimension(
                                registry.dimensions.dimension_spelling(&rhs_dim),
                            )),
                            help: Box::new(MismatchRule::DurationAddSubtract),
                        },
                    ));
                }
                return Ok(CheckedType::Datetime(*ls));
            }
            if let CheckedType::Datetime(rs) = rhs_type {
                // Quantity(Time) + Datetime -> Datetime (only for Add)
                if op == BinOp::Add {
                    let time_dim = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
                    let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
                    if lhs_dim != time_dim {
                        return Err(SemanticError::located(
                            src,
                            lhs.span,
                            DimensionError::DimensionMismatch {
                                expected: Box::new(MismatchOperand::Expected(
                                    OperandExpectation::Time,
                                )),
                                found: Box::new(MismatchOperand::Dimension(
                                    registry.dimensions.dimension_spelling(&lhs_dim),
                                )),
                                help: Box::new(MismatchRule::DurationAdd),
                            },
                        ));
                    }
                    return Ok(CheckedType::Datetime(*rs));
                }
                // Quantity - Datetime -> error
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Type(
                            lhs_type.spelling(&registry.dimensions),
                        )),
                        found: Box::new(MismatchOperand::Type(
                            rhs_type.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::DatetimeFromQuantity),
                    },
                ));
            }
            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Dimension(
                            registry.dimensions.dimension_spelling(&lhs_dim),
                        )),
                        found: Box::new(MismatchOperand::Dimension(
                            registry.dimensions.dimension_spelling(&rhs_dim),
                        )),
                        help: Box::new(MismatchRule::AdditionSameDimension),
                    },
                ));
            }
            Ok(CheckedType::Quantity(lhs_dim))
        }
        BinOp::Mul => {
            if matches!(lhs_type, CheckedType::Int) && matches!(rhs_type, CheckedType::Int) {
                return Ok(CheckedType::Int);
            }
            let (lhs_dim, lhs_complex) = match lhs_type {
                CheckedType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(lhs_type, registry, src, lhs.span)?, false),
            };
            let (rhs_dim, rhs_complex) = match rhs_type {
                CheckedType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(rhs_type, registry, src, rhs.span)?, false),
            };
            let dim = lhs_dim.checked_mul(&rhs_dim).map_err(|_| {
                SemanticError::located(src, expr_span, DimensionError::DimensionOverflow)
            })?;
            if lhs_complex || rhs_complex {
                Ok(CheckedType::Complex(dim))
            } else {
                Ok(CheckedType::Quantity(dim))
            }
        }
        BinOp::Div => {
            if matches!(lhs_type, CheckedType::Int) && matches!(rhs_type, CheckedType::Int) {
                return Ok(CheckedType::Int);
            }
            let (lhs_dim, lhs_complex) = match lhs_type {
                CheckedType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(lhs_type, registry, src, lhs.span)?, false),
            };
            let (rhs_dim, rhs_complex) = match rhs_type {
                CheckedType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(rhs_type, registry, src, rhs.span)?, false),
            };
            let dim = lhs_dim.checked_div(&rhs_dim).map_err(|_| {
                SemanticError::located(src, expr_span, DimensionError::DimensionOverflow)
            })?;
            if lhs_complex || rhs_complex {
                Ok(CheckedType::Complex(dim))
            } else {
                Ok(CheckedType::Quantity(dim))
            }
        }
        BinOp::Mod => {
            if matches!(lhs_type, CheckedType::Int) && matches!(rhs_type, CheckedType::Int) {
                return Ok(CheckedType::Int);
            }
            Err(SemanticError::located(
                src,
                expr_span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Expected(OperandExpectation::Int)),
                    found: Box::new(MismatchOperand::ModuloOperands {
                        lhs: lhs_type.spelling(&registry.dimensions),
                        rhs: rhs_type.spelling(&registry.dimensions),
                    }),
                    help: Box::new(MismatchRule::ModuloInt),
                },
            ))
        }
        BinOp::Pow(exponent) => {
            // Int powers remain integer-only. Exact integer syntax is
            // preferred; right-associated constant Int chains retain their
            // existing checked constant folding.
            if matches!(lhs_type, CheckedType::Int) {
                let int_exp = match exponent {
                    PowerExponent::Exact(exact) if exact.is_integer() => Some(exact.num()),
                    PowerExponent::Runtime => rhs_const_int,
                    PowerExponent::Exact(_) | PowerExponent::FloatSyntax { .. } => None,
                };
                if let Some(value) = int_exp {
                    if value >= 0 {
                        return Ok(CheckedType::Int);
                    }
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::NonNegativeIntExponent,
                            )),
                            found: Box::new(MismatchOperand::IntExponent(value)),
                            help: Box::new(MismatchRule::NonNegativeExponent),
                        },
                    ));
                }
                return Err(SemanticError::located(
                    src,
                    rhs.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(
                            OperandExpectation::NonNegativeExactIntExponent,
                        )),
                        found: Box::new(MismatchOperand::Type(
                            rhs_type.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::ExactIntegerExponent),
                    },
                ));
            }

            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;

            // Non-exact exponent expressions must still be dimensionless.
            if !matches!(exponent, PowerExponent::Exact(_)) {
                let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
                if !rhs_dim.is_dimensionless() {
                    return Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::DimensionlessExponent,
                            )),
                            found: Box::new(MismatchOperand::Dimension(
                                registry.dimensions.dimension_spelling(&rhs_dim),
                            )),
                            help: Box::new(MismatchRule::DimensionlessExponent),
                        },
                    ));
                }
            }

            match exponent {
                PowerExponent::Exact(exact) => {
                    if lhs_dim.is_dimensionless() {
                        return Ok(CheckedType::Quantity(Dimension::dimensionless()));
                    }
                    let rational = Rational::try_from(exact).map_err(|_| {
                        SemanticError::located(src, rhs.span, DimensionError::DimensionOverflow)
                    })?;
                    let dim = lhs_dim.pow(rational).map_err(|_| {
                        SemanticError::located(src, expr_span, DimensionError::DimensionOverflow)
                    })?;
                    Ok(CheckedType::Quantity(dim))
                }
                PowerExponent::FloatSyntax { exact } => {
                    if lhs_dim.is_dimensionless() {
                        return Ok(CheckedType::Quantity(Dimension::dimensionless()));
                    }
                    let replacement = exact_float_replacement(exact);
                    let help = replacement.as_ref().map_or_else(
                        || {
                            "write the exponent as an exact integer or parenthesized rational"
                                .to_string()
                        },
                        |replacement| format!("replace the float exponent with `{replacement}`"),
                    );
                    Err(SemanticError::located(
                        src,
                        rhs.span,
                        DimensionError::FloatPowerExponent { replacement, help },
                    ))
                }
                PowerExponent::Runtime => {
                    if lhs_dim.is_dimensionless() {
                        Ok(CheckedType::Quantity(Dimension::dimensionless()))
                    } else {
                        Err(SemanticError::located(
                            src,
                            rhs.span,
                            DimensionError::RuntimeExponentForDimensionedBase,
                        ))
                    }
                }
            }
        }
    }
}

/// Typing rule for a unary operation, given the already-inferred operand.
pub(super) fn unary_rule(
    op: UnaryOp,
    operand: &Operand,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    match op {
        UnaryOp::Not => {
            if operand.ty != CheckedType::Bool {
                return Err(SemanticError::located(
                    src,
                    operand.span,
                    DimensionError::DimensionMismatch {
                        expected: Box::new(MismatchOperand::Expected(OperandExpectation::Bool)),
                        found: Box::new(MismatchOperand::Type(
                            operand.ty.spelling(&registry.dimensions),
                        )),
                        help: Box::new(MismatchRule::LogicalNot),
                    },
                ));
            }
            Ok(CheckedType::Bool)
        }
        UnaryOp::Neg => match &operand.ty {
            CheckedType::Quantity(_) | CheckedType::Complex(_) | CheckedType::Int => {
                Ok(operand.ty.clone())
            }
            other => Err(SemanticError::located(
                src,
                operand.span,
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Expected(
                        OperandExpectation::IntOrQuantity,
                    )),
                    found: Box::new(MismatchOperand::Type(other.spelling(&registry.dimensions))),
                    help: Box::new(MismatchRule::Negation),
                },
            )),
        },
    }
}

/// Typing rule for an `if`/`else` expression, given inferred parts.
pub(super) fn if_rule(
    cond: &Operand,
    then_branch: &Operand,
    else_branch: &Operand,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    if cond.ty != CheckedType::Bool {
        return Err(SemanticError::located(
            src,
            cond.span,
            DimensionError::DimensionMismatch {
                expected: Box::new(MismatchOperand::Expected(OperandExpectation::Bool)),
                found: Box::new(MismatchOperand::Type(
                    cond.ty.spelling(&registry.dimensions),
                )),
                help: Box::new(MismatchRule::IfConditionBool),
            },
        ));
    }
    if then_branch.ty != else_branch.ty {
        return Err(SemanticError::located(
            src,
            else_branch.span,
            DimensionError::DimensionMismatch {
                expected: Box::new(MismatchOperand::Type(
                    then_branch.ty.spelling(&registry.dimensions),
                )),
                found: Box::new(MismatchOperand::Type(
                    else_branch.ty.spelling(&registry.dimensions),
                )),
                help: Box::new(MismatchRule::IfBranchesSameDimension),
            },
        ));
    }
    Ok(then_branch.ty.clone())
}

/// Resolve a canonical HIR unit expression's dimension.
pub(in crate::tir::dim_check) fn resolve_unit_dimension_or_diagnose(
    unit: &crate::hir::expr::ResolvedUnitExpr,
    tir: &dyn crate::tir::typed::TirRead,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    unit.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |dimension, item| {
            let info = tir
                .unit_info(item.name.value.static_definition())
                .ok_or_else(|| {
                    SemanticError::located(
                        src,
                        item.name.span,
                        DimensionError::UnknownUnit {
                            name: item.name.value.spelling().clone(),
                        },
                    )
                })?;
            let exponent = item.power;
            let term_dimension = info.dimension.pow(exponent).map_err(|_| {
                SemanticError::located(src, item.name.span, DimensionError::DimensionOverflow)
            })?;
            let resolved = match item.op {
                crate::syntax::ast::MulDivOp::Mul => dimension.checked_mul(&term_dimension),
                crate::syntax::ast::MulDivOp::Div => dimension.checked_div(&term_dimension),
            };
            resolved.map_err(|_| {
                SemanticError::located(src, item.name.span, DimensionError::DimensionOverflow)
            })
        })
}

/// Typing rule for `match` arms: all arms must have the same type, and at
/// least one arm must exist. `arm_body_span` maps an arm index to the span
/// of its body for diagnostics (the two engines carry different arm types).
pub(in crate::tir::dim_check) fn match_arms_rule(
    arm_types: &[CheckedType<Symbolic>],
    arm_body_span: impl Fn(usize) -> Span,
    expr_span: Span,
    registry: &FormattingRegistry,
    src: SourceId,
) -> Result<CheckedType<Symbolic>, SemanticError> {
    let Some(first) = arm_types.first() else {
        return Err(SemanticError::located(
            src,
            expr_span,
            StructError::EmptyMatch,
        ));
    };
    for (i, arm_type) in arm_types.iter().enumerate().skip(1) {
        if arm_type != first {
            return Err(SemanticError::located(
                src,
                arm_body_span(i),
                DimensionError::DimensionMismatch {
                    expected: Box::new(MismatchOperand::Type(first.spelling(&registry.dimensions))),
                    found: Box::new(MismatchOperand::Type(
                        arm_type.spelling(&registry.dimensions),
                    )),
                    help: Box::new(MismatchRule::MatchArmsSameType),
                },
            ));
        }
    }
    Ok(first.clone())
}
