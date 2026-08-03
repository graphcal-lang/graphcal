//! Pure dimension/type rules shared by the syntax-AST and HIR inference
//! engines.
//!
//! Both engines walk different expression representations but must apply
//! identical typing rules. Keeping the rules here as pure functions over
//! [`InferredType`] operands means a rule change lands once — the engines
//! had already drifted (HIR accepted `-` on Bool) when each carried its own
//! copy.

use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{BinOp, UnaryOp};
use crate::dimension::{BaseDimId, Dimension, PreludeBaseDimension, Rational};
use crate::exact_rational::ExactRational;
use crate::registry::error::GraphcalError;
use crate::registry::types::Registry;
use crate::syntax::ast::PowerExponent;
use crate::syntax::span::Span;

use super::super::InferredType;
use super::super::helpers::{expect_quantity, format_inferred_type};

/// A typed operand with the span diagnostics should point at.
pub(super) struct Operand {
    pub ty: InferredType,
    pub span: Span,
}

/// Require one comparison operand to be an unindexed value.
///
/// Comparisons deliberately follow arithmetic's no-broadcasting rule: callers
/// must use an explicit `for` comprehension and compare one element at a time.
fn comparison_operand_type<'a>(
    operand: &'a Operand,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a InferredType, GraphcalError> {
    match &operand.ty {
        InferredType::Indexed { .. } => Err(GraphcalError::IndexedComparisonOperand {
            found: format_inferred_type(&operand.ty, registry),
            src: src.clone(),
            span: operand.span.into(),
        }),
        ty => Ok(ty),
    }
}

fn exact_float_replacement(exact: Option<ExactRational>) -> Option<String> {
    let rational = Rational::try_from(exact?).ok()?;
    Some(if rational.is_integer() {
        rational.num().to_string()
    } else {
        format!("({}/{})", rational.num(), rational.den())
    })
}

/// The exact additive fragment of Fin-key arithmetic:
/// `k : Key<Fin(N)>` plus a static Nat constant `c` yields `Key<Fin(N + c)>`.
fn fin_key_additive_rule(
    op: BinOp,
    key_index: &super::super::InferredIndex,
    rhs: &Operand,
    rhs_const_int: Option<i64>,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let reject = |help: &str| {
        Err(GraphcalError::DimensionMismatch {
            expected: "a static Nat constant".to_string(),
            found: format_inferred_type(&rhs.ty, registry),
            help: help.to_string(),
            src: src.clone(),
            span: rhs.span.into(),
        })
    };
    let Some(bound) = key_index.finite_index_form() else {
        return reject(
            "named and coordinate keys have no arithmetic; only Fin-axis keys \
             carry the additive fragment `k + c`",
        );
    };
    if op != BinOp::Add {
        return reject(
            "key subtraction is fallible at 0 and excluded; restructure additively \
             or use to_int() and fin_key()",
        );
    }
    if !rhs.ty.is_int_like() {
        return reject("`k + c` takes an integer constant addend");
    }
    let Some(addend) = rhs_const_int else {
        return reject(
            "a runtime offset escapes any static bound; use `to_int(k) + e` and \
             re-enter with fin_key()",
        );
    };
    let Ok(addend) = u64::try_from(addend) else {
        return reject("`k + c` takes a non-negative static constant");
    };
    let shifted = bound
        .add(&crate::nat::NatPolyForm::from_constant(addend))
        .map_err(|err| GraphcalError::EvalError {
            message: err.to_string(),
            src: src.clone(),
            span: rhs.span.into(),
        })?;
    crate::tir::dim_check::InferredIndex::from_finite_index_form(shifted)
        .map(InferredType::Key)
        .map_err(|err| GraphcalError::EvalError {
            message: err.to_string(),
            src: src.clone(),
            span: rhs.span.into(),
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
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let lhs_type = &lhs.ty;
    let rhs_type = &rhs.ty;
    match op {
        // Logical operators: require Bool operands, return Bool
        BinOp::And | BinOp::Or => {
            if *lhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(lhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: lhs.span.into(),
                });
            }
            if *rhs_type != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(rhs_type, registry),
                    help: "boolean operators require Bool operands".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Equality: operands must be unindexed and have the same ValueType
        // (Int and Fin(N) are compatible).
        BinOp::Eq | BinOp::Ne => {
            let lhs_type = comparison_operand_type(lhs, registry, src)?;
            let rhs_type = comparison_operand_type(rhs, registry, src)?;
            if matches!(
                lhs_type,
                InferredType::NamedIndexCase(_) | InferredType::IndexArg(_)
            ) || matches!(
                rhs_type,
                InferredType::NamedIndexCase(_) | InferredType::IndexArg(_)
            ) {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "value expression".to_string(),
                    found: if matches!(
                        lhs_type,
                        InferredType::NamedIndexCase(_) | InferredType::IndexArg(_)
                    ) {
                        format_inferred_type(lhs_type, registry)
                    } else {
                        format_inferred_type(rhs_type, registry)
                    },
                    help: "named index labels are not values; use `match` for index case analysis"
                        .to_string(),
                    src: src.clone(),
                    span: if matches!(
                        lhs_type,
                        InferredType::NamedIndexCase(_) | InferredType::IndexArg(_)
                    ) {
                        lhs.span
                    } else {
                        rhs.span
                    }
                    .into(),
                });
            }
            if lhs_type == rhs_type || (lhs_type.is_int_like() && rhs_type.is_int_like()) {
                return Ok(InferredType::Bool);
            }
            if let (Some(lhs_dim), Some(rhs_dim)) =
                (lhs_type.quantity_dimension(), rhs_type.quantity_dimension())
                && lhs_dim == rhs_dim
            {
                return Ok(InferredType::Bool);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: format_inferred_type(lhs_type, registry),
                found: format_inferred_type(rhs_type, registry),
                help: "equality operands must have the same type".to_string(),
                src: src.clone(),
                span: rhs.span.into(),
            })
        }
        // Ordering comparisons require unindexed operands that are same-type
        // quantities, Int/Fin values, or same-scale Datetimes.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let lhs_type = comparison_operand_type(lhs, registry, src)?;
            let rhs_type = comparison_operand_type(rhs, registry, src)?;
            if matches!(lhs_type, InferredType::Complex(_))
                || matches!(rhs_type, InferredType::Complex(_))
            {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "an ordered real quantity, integer, or datetime".to_string(),
                    found: if matches!(lhs_type, InferredType::Complex(_)) {
                        format_inferred_type(lhs_type, registry)
                    } else {
                        format_inferred_type(rhs_type, registry)
                    },
                    help: "complex quantities are unordered; compare re(), im(), abs(), or phase() explicitly"
                        .to_string(),
                    src: src.clone(),
                    span: if matches!(lhs_type, InferredType::Complex(_)) {
                        lhs.span
                    } else {
                        rhs.span
                    }
                    .into(),
                });
            }
            if lhs_type.is_int_like() || rhs_type.is_int_like() {
                if !lhs_type.is_int_like() || !rhs_type.is_int_like() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(lhs_type, registry),
                        found: format_inferred_type(rhs_type, registry),
                        help: "comparison operands must have the same type".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            // Datetime comparisons: same time scale required
            if let InferredType::Datetime(ls) = lhs_type
                && let InferredType::Datetime(rs) = rhs_type
            {
                if ls != rs {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(lhs_type, registry),
                        found: format_inferred_type(rhs_type, registry),
                        help: "cannot compare datetimes with different time scales".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Bool);
            }
            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "comparison operands must have the same dimension".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        // Arithmetic operators: require matching numeric operands (Int or Quantity)
        BinOp::Add | BinOp::Sub => {
            // Additive Fin-key arithmetic: `k + c` with a static Nat constant
            // shifts the bound into the type — `Key<Fin(N)> + c : Key<Fin(N + c)>`
            // — exactly and infallibly. Everything else on keys is rejected:
            // subtraction is fallible at 0 and Nat itself has none; runtime
            // offsets escape any static bound.
            if let InferredType::Key(key_index) = lhs_type {
                return fin_key_additive_rule(op, key_index, rhs, rhs_const_int, registry, src);
            }
            if matches!(rhs_type, InferredType::Key(_)) {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(lhs_type, registry),
                    found: format_inferred_type(rhs_type, registry),
                    help: "Fin-key arithmetic is written key-first: `k + c` with a \
                           static Nat constant"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            match (lhs_type, rhs_type) {
                (InferredType::Complex(lhs_dim), InferredType::Complex(rhs_dim)) => {
                    if lhs_dim != rhs_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format_inferred_type(lhs_type, registry),
                            found: format_inferred_type(rhs_type, registry),
                            help: "complex operands of addition and subtraction must have the same dimension"
                                .to_string(),
                            src: src.clone(),
                            span: rhs.span.into(),
                        });
                    }
                    return Ok(InferredType::Complex(lhs_dim.clone()));
                }
                (InferredType::Complex(_), _) | (_, InferredType::Complex(_)) => {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format_inferred_type(lhs_type, registry),
                        found: format_inferred_type(rhs_type, registry),
                        help: "addition and subtraction do not implicitly promote real quantities; use to_complex()"
                            .to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                _ => {}
            }
            // Point-vs-vector rules for Datetime
            if let InferredType::Datetime(ls) = lhs_type {
                let time_dim = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
                if let InferredType::Datetime(rs) = rhs_type {
                    // Datetime - Datetime -> Quantity(Time)
                    if op == BinOp::Sub {
                        if ls != rs {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format_inferred_type(lhs_type, registry),
                                found: format_inferred_type(rhs_type, registry),
                                help: "cannot subtract datetimes with different time scales"
                                    .to_string(),
                                src: src.clone(),
                                span: rhs.span.into(),
                            });
                        }
                        return Ok(InferredType::Quantity(time_dim));
                    }
                    // Datetime + Datetime -> error
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Quantity(Time)".to_string(),
                        found: format_inferred_type(rhs_type, registry),
                        help: "cannot add two datetimes; did you mean to subtract?".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                // Datetime +/- Quantity(Time) -> Datetime
                let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
                if rhs_dim != time_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Time".to_string(),
                        found: registry.dimensions.format_dimension(&rhs_dim),
                        help: "can only add/subtract a Time duration to/from a Datetime"
                            .to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Ok(InferredType::Datetime(*ls));
            }
            if let InferredType::Datetime(rs) = rhs_type {
                // Quantity(Time) + Datetime -> Datetime (only for Add)
                if op == BinOp::Add {
                    let time_dim = Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time));
                    let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
                    if lhs_dim != time_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: "Time".to_string(),
                            found: registry.dimensions.format_dimension(&lhs_dim),
                            help: "can only add a Time duration to a Datetime".to_string(),
                            src: src.clone(),
                            span: lhs.span.into(),
                        });
                    }
                    return Ok(InferredType::Datetime(*rs));
                }
                // Quantity - Datetime -> error
                return Err(GraphcalError::DimensionMismatch {
                    expected: format_inferred_type(lhs_type, registry),
                    found: format_inferred_type(rhs_type, registry),
                    help: "cannot subtract a Datetime from a quantity".to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;
            let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
            if lhs_dim != rhs_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&lhs_dim),
                    found: registry.dimensions.format_dimension(&rhs_dim),
                    help: "operands of addition and subtraction must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }
            Ok(InferredType::Quantity(lhs_dim))
        }
        BinOp::Mul => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let (lhs_dim, lhs_complex) = match lhs_type {
                InferredType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(lhs_type, registry, src, lhs.span)?, false),
            };
            let (rhs_dim, rhs_complex) = match rhs_type {
                InferredType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(rhs_type, registry, src, rhs.span)?, false),
            };
            let dim =
                lhs_dim
                    .checked_mul(&rhs_dim)
                    .map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: expr_span.into(),
                    })?;
            if lhs_complex || rhs_complex {
                Ok(InferredType::Complex(dim))
            } else {
                Ok(InferredType::Quantity(dim))
            }
        }
        BinOp::Div => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            let (lhs_dim, lhs_complex) = match lhs_type {
                InferredType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(lhs_type, registry, src, lhs.span)?, false),
            };
            let (rhs_dim, rhs_complex) = match rhs_type {
                InferredType::Complex(dimension) => (dimension.clone(), true),
                _ => (expect_quantity(rhs_type, registry, src, rhs.span)?, false),
            };
            let dim =
                lhs_dim
                    .checked_div(&rhs_dim)
                    .map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: expr_span.into(),
                    })?;
            if lhs_complex || rhs_complex {
                Ok(InferredType::Complex(dim))
            } else {
                Ok(InferredType::Quantity(dim))
            }
        }
        BinOp::Mod => {
            if lhs_type.is_int_like() && rhs_type.is_int_like() {
                return Ok(InferredType::Int);
            }
            Err(GraphcalError::DimensionMismatch {
                expected: "Int".to_string(),
                found: format!(
                    "{} % {}",
                    format_inferred_type(lhs_type, registry),
                    format_inferred_type(rhs_type, registry)
                ),
                help: "modulo operator requires Int operands".to_string(),
                src: src.clone(),
                span: expr_span.into(),
            })
        }
        BinOp::Pow(exponent) => {
            // Int/Fin powers remain integer-only. Exact integer syntax is
            // preferred; right-associated constant Int chains retain their
            // existing checked constant folding.
            if lhs_type.is_int_like() {
                let int_exp = match exponent {
                    PowerExponent::Exact(exact) if exact.is_integer() => Some(exact.numerator()),
                    PowerExponent::Runtime => rhs_const_int,
                    PowerExponent::Exact(_) | PowerExponent::FloatSyntax { .. } => None,
                };
                if let Some(value) = int_exp {
                    if value >= 0 {
                        return Ok(InferredType::Int);
                    }
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "non-negative Int exponent".to_string(),
                        found: value.to_string(),
                        help: "integer power requires a non-negative exact integer exponent"
                            .to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
                return Err(GraphcalError::DimensionMismatch {
                    expected: "non-negative exact Int exponent".to_string(),
                    found: format_inferred_type(rhs_type, registry),
                    help: "integer power requires an exact integer exponent such as `2`"
                        .to_string(),
                    src: src.clone(),
                    span: rhs.span.into(),
                });
            }

            let lhs_dim = expect_quantity(lhs_type, registry, src, lhs.span)?;

            // Non-exact exponent expressions must still be dimensionless.
            if !matches!(exponent, PowerExponent::Exact(_)) {
                let rhs_dim = expect_quantity(rhs_type, registry, src, rhs.span)?;
                if !rhs_dim.is_dimensionless() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless exponent".to_string(),
                        found: registry.dimensions.format_dimension(&rhs_dim),
                        help: "the exponent of a power must be dimensionless".to_string(),
                        src: src.clone(),
                        span: rhs.span.into(),
                    });
                }
            }

            match exponent {
                PowerExponent::Exact(exact) => {
                    if lhs_dim.is_dimensionless() {
                        return Ok(InferredType::Quantity(Dimension::dimensionless()));
                    }
                    let rational = Rational::try_from(exact).map_err(|_| {
                        GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: rhs.span.into(),
                        }
                    })?;
                    let dim =
                        lhs_dim
                            .pow(rational)
                            .map_err(|_| GraphcalError::DimensionOverflow {
                                src: src.clone(),
                                span: expr_span.into(),
                            })?;
                    Ok(InferredType::Quantity(dim))
                }
                PowerExponent::FloatSyntax { exact } => {
                    if lhs_dim.is_dimensionless() {
                        return Ok(InferredType::Quantity(Dimension::dimensionless()));
                    }
                    let replacement = exact_float_replacement(exact);
                    let help = replacement.as_ref().map_or_else(
                        || {
                            "write the exponent as an exact integer or parenthesized rational"
                                .to_string()
                        },
                        |replacement| format!("replace the float exponent with `{replacement}`"),
                    );
                    Err(GraphcalError::FloatPowerExponent {
                        replacement,
                        help,
                        src: src.clone(),
                        span: rhs.span.into(),
                    })
                }
                PowerExponent::Runtime => {
                    if lhs_dim.is_dimensionless() {
                        Ok(InferredType::Quantity(Dimension::dimensionless()))
                    } else {
                        Err(GraphcalError::RuntimeExponentForDimensionedBase {
                            src: src.clone(),
                            span: rhs.span.into(),
                        })
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
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match op {
        UnaryOp::Not => {
            if operand.ty != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred_type(&operand.ty, registry),
                    help: "logical NOT requires a Bool operand".to_string(),
                    src: src.clone(),
                    span: operand.span.into(),
                });
            }
            Ok(InferredType::Bool)
        }
        UnaryOp::Neg => match &operand.ty {
            InferredType::Quantity(_) | InferredType::Complex(_) | InferredType::Int => {
                Ok(operand.ty.clone())
            }
            InferredType::CoordinateIndexLabel { dimension, .. } => {
                Ok(InferredType::Quantity(dimension.clone()))
            }
            InferredType::Fin(_) => Err(GraphcalError::DimensionMismatch {
                expected: "Int or Quantity".to_string(),
                found: format_inferred_type(&operand.ty, registry),
                help: "Fin(N loop variables are bounded natural indexes; convert explicitly before negating)"
                    .to_string(),
                src: src.clone(),
                span: operand.span.into(),
            }),
            other => Err(GraphcalError::DimensionMismatch {
                expected: "Int or Quantity".to_string(),
                found: format_inferred_type(other, registry),
                help: "negation requires a numeric quantity or Int operand".to_string(),
                src: src.clone(),
                span: operand.span.into(),
            }),
        }
    }
}

/// Typing rule for an `if`/`else` expression, given inferred parts.
pub(super) fn if_rule(
    cond: &Operand,
    then_branch: &Operand,
    else_branch: &Operand,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if cond.ty != InferredType::Bool {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Bool".to_string(),
            found: format_inferred_type(&cond.ty, registry),
            help: "if/else condition must be Bool".to_string(),
            src: src.clone(),
            span: cond.span.into(),
        });
    }
    if then_branch.ty != else_branch.ty {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&then_branch.ty, registry),
            found: format_inferred_type(&else_branch.ty, registry),
            help: "both branches of if/else must have the same dimension".to_string(),
            src: src.clone(),
            span: else_branch.span.into(),
        });
    }
    Ok(then_branch.ty.clone())
}

/// Resolve a canonical HIR unit expression's dimension.
pub(super) fn resolve_unit_dimension_or_diagnose(
    unit: &crate::hir::ResolvedUnitExpr,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    unit.terms
        .iter()
        .try_fold(Dimension::dimensionless(), |dimension, item| {
            let info = tir.unit_info(item.name.value.resolved()).ok_or_else(|| {
                GraphcalError::UnknownUnit {
                    name: item.name.value.spelling().clone(),
                    src: src.clone(),
                    span: item.name.span.into(),
                }
            })?;
            let exponent = item.power.unwrap_or(Rational::ONE);
            let term_dimension =
                info.dimension
                    .pow(exponent)
                    .map_err(|_| GraphcalError::DimensionOverflow {
                        src: src.clone(),
                        span: item.name.span.into(),
                    })?;
            let resolved = match item.op {
                crate::syntax::ast::MulDivOp::Mul => dimension.checked_mul(&term_dimension),
                crate::syntax::ast::MulDivOp::Div => dimension.checked_div(&term_dimension),
            };
            resolved.map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: item.name.span.into(),
            })
        })
}

/// Typing rule for `match` arms: all arms must have the same type, and at
/// least one arm must exist. `arm_body_span` maps an arm index to the span
/// of its body for diagnostics (the two engines carry different arm types).
pub(in crate::tir::dim_check) fn match_arms_rule(
    arm_types: &[InferredType],
    arm_body_span: impl Fn(usize) -> Span,
    expr_span: Span,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(first) = arm_types.first() else {
        return Err(GraphcalError::EvalError {
            message: "match expression has no arms".to_string(),
            src: src.clone(),
            span: expr_span.into(),
        });
    };
    for (i, arm_type) in arm_types.iter().enumerate().skip(1) {
        if arm_type != first {
            return Err(GraphcalError::DimensionMismatch {
                expected: format_inferred_type(first, registry),
                found: format_inferred_type(arm_type, registry),
                help: "all match arms must return the same type".to_string(),
                src: src.clone(),
                span: arm_body_span(i).into(),
            });
        }
    }
    Ok(first.clone())
}
