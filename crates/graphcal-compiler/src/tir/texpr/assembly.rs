//! Bottom-up assembly of typed trees while one checking pass records nodes.
//!
//! Inference records every expression after its children, so each checked
//! node is assembled from the typed children already recorded for it. The
//! mapping from a HIR node to its typed form is the only place a [`TExpr`] is
//! built from inference results, and it rejects any shape the checker did not
//! prove: a missing or contextual child in a value position, a constructor
//! without its application, a match arm without its target, or a static
//! position that belongs to no selector of the node.

use std::collections::HashMap;

use thiserror::Error;

use crate::expression_id::ExprId;
use crate::hir::expr::{ConstRef, Expr, ExprKind, IndexArg, MatchPattern};
use crate::registry::checked_type::{CheckedType, Symbolic};
use crate::resolved_name::ResolvedConstructorName;
use crate::syntax::ast::{BinOp, PowerExponent, UnaryOp};
use crate::syntax::span::Spanned;
use crate::tir::static_index::StaticIndexRequirement;

use super::model::{
    ContextualLiteral, StaticPosition, TArg, TConstRef, TContextual, TExpr, TExprKind, TFieldInit,
    TIndexArg, TMapEntry, TMatchArm, TMatchPattern, TParamBinding,
};
use super::nominal::{ConstructorApplication, ConstructorMatch};
use super::operators::{
    ArithOp, BExpr, CExpr, DExpr, EqualityOp, IExpr, IntArithOp, OrderedOperands, OrderingOp,
    QExpr, ScaleOp, ShiftOp,
};

/// Why a checked node could not be assembled into a typed tree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssemblyError {
    #[error("expression checked more than once in one checking pass: {0:?}")]
    CheckedTwice(ExprId),
    #[error("checked expression {parent:?} has an unchecked child {child:?}")]
    UncheckedChild { parent: ExprId, child: ExprId },
    #[error("contextual literal {0:?} is used as a value")]
    ContextualValue(ExprId),
    #[error("expression {0:?} is not a contextual literal")]
    NotContextual(ExprId),
    #[error("constructor {0:?} has no checked application")]
    MissingApplication(ExprId),
    #[error("expression {0:?} carries a constructor application it does not apply")]
    UnappliedApplication(ExprId),
    #[error("a constructor arm of match {0:?} has no checked target")]
    MissingMatchTarget(ExprId),
    #[error("a static position of {0:?} belongs to none of its selectors")]
    UnplacedStaticPosition(ExprId),
    #[error("operator {0:?} has no operation for its checked operand types")]
    UncheckedOperands(ExprId),
}

/// The node-specific facts checking established for one value expression.
pub struct NodeFacts<'a> {
    pub constructor: Option<&'a ConstructorApplication<Symbolic>>,
    pub constructor_matches: &'a HashMap<ResolvedConstructorName, ConstructorMatch>,
    pub static_indexes: &'a [StaticIndexRequirement],
}

/// Typed nodes recorded so far whose parent has not been recorded yet.
///
/// After a successful pass only the checked roots remain.
#[derive(Debug, Default)]
pub struct PendingNodes {
    nodes: HashMap<ExprId, TArg<Symbolic>>,
}

impl PendingNodes {
    /// Record a checked value expression, consuming its recorded children.
    pub fn record_value(
        &mut self,
        expr: &Expr,
        ty: CheckedType<Symbolic>,
        facts: &NodeFacts<'_>,
    ) -> Result<(), AssemblyError> {
        let kind = self.assemble(expr, facts)?;
        self.insert(
            expr.id(),
            TArg::Value(Box::new(TExpr::new(expr.id().clone(), expr.span, ty, kind))),
        )
    }

    /// Record a contextual literal accepted by its consumer.
    pub fn record_contextual(&mut self, expr: &Expr) -> Result<(), AssemblyError> {
        let literal = match expr.kind() {
            ExprKind::StringLiteral(value) => ContextualLiteral::String(value.clone()),
            ExprKind::OffsetDateTimeLiteral(value) => ContextualLiteral::OffsetDateTime(*value),
            ExprKind::CivilDateTimeLiteral(value) => ContextualLiteral::CivilDateTime(*value),
            ExprKind::ZonedDateTimeLiteral(value) => {
                ContextualLiteral::ZonedDateTime(value.clone())
            }
            ExprKind::IanaTimeZoneLiteral(value) => ContextualLiteral::TimeZone(value.clone()),
            _ => return Err(AssemblyError::NotContextual(expr.id().clone())),
        };
        self.insert(
            expr.id(),
            TArg::Contextual(TContextual::new(expr.id().clone(), expr.span, literal)),
        )
    }

    /// Take the typed node of a checked root out of the pending set.
    pub fn take_root(&mut self, root: &ExprId) -> Option<TArg<Symbolic>> {
        self.nodes.remove(root)
    }

    /// Any recorded node that no parent and no root has claimed.
    pub fn unclaimed(&self) -> Option<&ExprId> {
        self.nodes.keys().next()
    }

    /// Every node no parent has claimed: the trees of the roots checked.
    pub fn into_roots(self) -> HashMap<ExprId, super::model::TBody<Symbolic>> {
        self.nodes
            .into_iter()
            .map(|(id, node)| {
                let body = match node {
                    TArg::Value(value) => super::model::TBody::Value(value),
                    TArg::Contextual(literal) => super::model::TBody::Contextual(literal),
                };
                (id, body)
            })
            .collect()
    }

    fn insert(&mut self, id: &ExprId, node: TArg<Symbolic>) -> Result<(), AssemblyError> {
        match self.nodes.entry(id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(node);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(AssemblyError::CheckedTwice(id.clone()))
            }
        }
    }

    fn take_arg(&mut self, parent: &Expr, child: &Expr) -> Result<TArg<Symbolic>, AssemblyError> {
        self.nodes
            .remove(child.id())
            .ok_or_else(|| AssemblyError::UncheckedChild {
                parent: parent.id().clone(),
                child: child.id().clone(),
            })
    }

    fn take_boxed(
        &mut self,
        parent: &Expr,
        child: &Expr,
    ) -> Result<Box<TExpr<Symbolic>>, AssemblyError> {
        match self.take_arg(parent, child)? {
            TArg::Value(value) => Ok(value),
            TArg::Contextual(literal) => Err(AssemblyError::ContextualValue(literal.id().clone())),
        }
    }

    fn take_value(
        &mut self,
        parent: &Expr,
        child: &Expr,
    ) -> Result<TExpr<Symbolic>, AssemblyError> {
        self.take_boxed(parent, child).map(|value| *value)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive mapping from each HIR expression kind to its typed form"
    )]
    fn assemble(
        &mut self,
        expr: &Expr,
        facts: &NodeFacts<'_>,
    ) -> Result<TExprKind<Symbolic>, AssemblyError> {
        let id = || expr.id().clone();
        let mut positions = StaticPositions::new(facts.static_indexes);
        let application = || {
            facts
                .constructor
                .cloned()
                .ok_or_else(|| AssemblyError::MissingApplication(id()))
        };
        let applies_constructor = matches!(
            expr.kind(),
            ExprKind::ConstructorCall { .. }
                | ExprKind::ConstRef(Spanned {
                    value: ConstRef::Constructor(_),
                    ..
                })
        );
        if facts.constructor.is_some() && !applies_constructor {
            return Err(AssemblyError::UnappliedApplication(id()));
        }
        let kind = match expr.kind() {
            ExprKind::Error(no_error) => no_error.absurd(),
            ExprKind::Number(value) => TExprKind::Quantity(QExpr::Number(*value)),
            ExprKind::Integer(value) => TExprKind::Int(IExpr::Literal(*value)),
            ExprKind::Bool(value) => TExprKind::Bool(BExpr::Literal(*value)),
            ExprKind::StringLiteral(_)
            | ExprKind::OffsetDateTimeLiteral(_)
            | ExprKind::CivilDateTimeLiteral(_)
            | ExprKind::ZonedDateTimeLiteral(_)
            | ExprKind::IanaTimeZoneLiteral(_)
            | ExprKind::TypeSystemRef(_) => return Err(AssemblyError::ContextualValue(id())),
            ExprKind::QuantityLiteral { value, unit } => TExprKind::QuantityLiteral {
                value: *value,
                unit: unit.clone(),
            },
            ExprKind::VariantLiteral(variant) => TExprKind::Variant(variant.clone()),
            ExprKind::GraphRef(target) => TExprKind::GraphRef(target.clone()),
            ExprKind::ConstRef(target) => match &target.value {
                ConstRef::Decl(declaration) => TExprKind::Const(Spanned::new(
                    TConstRef::Decl(declaration.clone()),
                    target.span,
                )),
                ConstRef::Builtin(constant) => TExprKind::Quantity(QExpr::Constant(*constant)),
                ConstRef::Constructor(_) => TExprKind::Const(Spanned::new(
                    TConstRef::Constructor(application()?),
                    target.span,
                )),
            },
            ExprKind::LocalRef(local) => TExprKind::Local(local.clone()),
            ExprKind::BinOp { op, lhs, rhs } => binary(
                *op,
                self.take_boxed(expr, lhs)?,
                self.take_boxed(expr, rhs)?,
            )
            .ok_or_else(|| AssemblyError::UncheckedOperands(id()))?,
            ExprKind::UnaryOp { op, operand } => unary(*op, self.take_boxed(expr, operand)?)
                .ok_or_else(|| AssemblyError::UncheckedOperands(id()))?,
            ExprKind::FnCall { callee, args } => TExprKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|arg| self.take_arg(expr, arg))
                    .collect::<Result<_, _>>()?,
            },
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => TExprKind::If {
                condition: self.take_boxed(expr, condition)?,
                then_branch: self.take_boxed(expr, then_branch)?,
                else_branch: self.take_boxed(expr, else_branch)?,
            },
            ExprKind::Convert {
                expr: inner,
                target,
            } => TExprKind::Convert {
                expr: self.take_boxed(expr, inner)?,
                target: target.clone(),
            },
            ExprKind::DisplayTimezone {
                expr: inner,
                timezone,
            } => TExprKind::DisplayTimezone {
                expr: self.take_boxed(expr, inner)?,
                timezone: timezone.clone(),
            },
            ExprKind::FieldAccess { expr: inner, field } => TExprKind::Field {
                expr: self.take_boxed(expr, inner)?,
                field: field.clone(),
            },
            ExprKind::ConstructorCall { fields, .. } => TExprKind::Construct {
                application: application()?,
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(TFieldInit {
                            name: field.name.value.clone(),
                            value: self.take_value(expr, &field.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::MapLiteral { entries } => TExprKind::Map {
                entries: entries
                    .iter()
                    .map(|entry| {
                        Ok(TMapEntry {
                            keys: entry.keys.clone(),
                            value: self.take_value(expr, &entry.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::ForComp { bindings, body } => TExprKind::For {
                bindings: bindings.clone(),
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::IndexAccess { expr: inner, args } => TExprKind::Index {
                expr: self.take_boxed(expr, inner)?,
                args: args.try_map_ref(|arg| {
                    Ok::<_, AssemblyError>(match arg {
                        IndexArg::Variant(variant) => TIndexArg::Variant(variant.clone()),
                        IndexArg::Var(local) => TIndexArg::Var(local.clone()),
                        IndexArg::Expr(operand) => TIndexArg::Expr {
                            static_position: positions.take(operand.id()),
                            operand: self.take_boxed(expr, operand)?,
                        },
                    })
                })?,
            },
            ExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => TExprKind::Scan {
                source: self.take_boxed(expr, source)?,
                init: self.take_boxed(expr, init)?,
                acc: acc.clone(),
                val: val.clone(),
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::Unfold {
                recurrence,
                init,
                body,
            } => TExprKind::Unfold {
                recurrence: recurrence.clone(),
                init: self.take_boxed(expr, init)?,
                body: self.take_boxed(expr, body)?,
            },
            ExprKind::KeyForm {
                kind, axis, arg, ..
            } => TExprKind::Key {
                kind: *kind,
                axis: axis.clone(),
                static_position: positions.take(arg.id()),
                arg: self.take_boxed(expr, arg)?,
            },
            ExprKind::Match { scrutinee, arms } => TExprKind::Match {
                scrutinee: self.take_boxed(expr, scrutinee)?,
                arms: arms
                    .iter()
                    .map(|arm| {
                        let pattern = match &arm.pattern {
                            MatchPattern::Constructor {
                                constructor,
                                bindings,
                                ..
                            } => TMatchPattern::Constructor {
                                target: facts
                                    .constructor_matches
                                    .get(&constructor.value)
                                    .cloned()
                                    .ok_or_else(|| AssemblyError::MissingMatchTarget(id()))?,
                                bindings: bindings.clone(),
                            },
                            MatchPattern::IndexLabel { variant, .. } => {
                                TMatchPattern::IndexLabel(variant.clone())
                            }
                        };
                        Ok(TMatchArm {
                            pattern,
                            body: self.take_value(expr, &arm.body)?,
                            span: arm.span,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
            },
            ExprKind::DagCall {
                target,
                args,
                static_bindings,
                output,
            } => TExprKind::DagCall {
                target: target.clone(),
                args: args
                    .iter()
                    .map(|binding| {
                        Ok(TParamBinding {
                            target: binding.target.value.clone(),
                            value: self.take_value(expr, &binding.value)?,
                        })
                    })
                    .collect::<Result<_, AssemblyError>>()?,
                static_bindings: static_bindings.clone(),
                output: output.clone(),
            },
        };
        if positions.all_placed() {
            Ok(kind)
        } else {
            Err(AssemblyError::UnplacedStaticPosition(id()))
        }
    }
}

/// The operation a binary operator selects for its operand types, as its
/// checked result.
#[derive(Debug, Clone, Copy)]
enum BinaryOperation {
    And,
    Or,
    Equality(EqualityOp),
    Ordering(OrderingOp, OrderedOperands),
    KeyShift,
    Int(IntArithOp),
    IntExactPower(i64),
    IntPower,
    Quantity(ArithOp),
    QuantityExactPower(crate::exact_rational::ExactRational),
    QuantityPower,
    DatetimeDifference,
    Complex(ArithOp),
    ScaleRight(ScaleOp),
    ScaleLeft(ScaleOp),
    Shift(ShiftOp),
    ShiftAfter,
}

impl BinaryOperation {
    /// The operation `op` selects for operands of types `lhs` and `rhs`, if
    /// checking admits them.
    fn select(op: BinOp, lhs: &CheckedType<Symbolic>, rhs: &CheckedType<Symbolic>) -> Option<Self> {
        use CheckedType as T;
        let arith = match op {
            BinOp::Add => Some(ArithOp::Add),
            BinOp::Sub => Some(ArithOp::Sub),
            BinOp::Mul => Some(ArithOp::Mul),
            BinOp::Div => Some(ArithOp::Div),
            _ => None,
        };
        let scale = match op {
            BinOp::Mul => Some(ScaleOp::Mul),
            BinOp::Div => Some(ScaleOp::Div),
            _ => None,
        };
        let unindexed = |ty: &CheckedType<Symbolic>| !matches!(ty, T::Indexed { .. });
        Some(match (op, lhs, rhs) {
            (BinOp::And, T::Bool, T::Bool) => Self::And,
            (BinOp::Or, T::Bool, T::Bool) => Self::Or,
            (BinOp::Eq, lhs, rhs) if unindexed(lhs) && unindexed(rhs) => {
                Self::Equality(EqualityOp::Eq)
            }
            (BinOp::Ne, lhs, rhs) if unindexed(lhs) && unindexed(rhs) => {
                Self::Equality(EqualityOp::Ne)
            }
            (BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge, lhs, rhs) => {
                let ordering = match op {
                    BinOp::Lt => OrderingOp::Lt,
                    BinOp::Gt => OrderingOp::Gt,
                    BinOp::Le => OrderingOp::Le,
                    _ => OrderingOp::Ge,
                };
                let operands = match (lhs, rhs) {
                    (T::Quantity(_), T::Quantity(_)) => OrderedOperands::Quantity,
                    (T::Int, T::Int) => OrderedOperands::Int,
                    (T::Datetime(_), T::Datetime(_)) => OrderedOperands::Datetime,
                    _ => return None,
                };
                Self::Ordering(ordering, operands)
            }
            (BinOp::Add, T::Key(_), T::Int) => Self::KeyShift,
            (BinOp::Mod, T::Int, T::Int) => Self::Int(IntArithOp::Mod),
            (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, T::Int, T::Int) => {
                Self::Int(match arith? {
                    ArithOp::Add => IntArithOp::Add,
                    ArithOp::Sub => IntArithOp::Sub,
                    ArithOp::Mul => IntArithOp::Mul,
                    ArithOp::Div => IntArithOp::Div,
                })
            }
            (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, T::Quantity(_), T::Quantity(_)) => {
                Self::Quantity(arith?)
            }
            (BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div, T::Complex(_), T::Complex(_)) => {
                Self::Complex(arith?)
            }
            (BinOp::Mul | BinOp::Div, T::Complex(_), T::Quantity(_)) => Self::ScaleRight(scale?),
            (BinOp::Mul | BinOp::Div, T::Quantity(_), T::Complex(_)) => Self::ScaleLeft(scale?),
            (BinOp::Sub, T::Datetime(_), T::Datetime(_)) => Self::DatetimeDifference,
            (BinOp::Add, T::Datetime(_), T::Quantity(_)) => Self::Shift(ShiftOp::Add),
            (BinOp::Sub, T::Datetime(_), T::Quantity(_)) => Self::Shift(ShiftOp::Sub),
            (BinOp::Add, T::Quantity(_), T::Datetime(_)) => Self::ShiftAfter,
            (BinOp::Pow(PowerExponent::Exact(exponent)), T::Int, _) if exponent.is_integer() => {
                Self::IntExactPower(exponent.num())
            }
            (BinOp::Pow(PowerExponent::Runtime), T::Int, T::Int) => Self::IntPower,
            (BinOp::Pow(PowerExponent::Exact(exponent)), T::Quantity(_), _) => {
                Self::QuantityExactPower(exponent)
            }
            (
                BinOp::Pow(PowerExponent::FloatSyntax { .. } | PowerExponent::Runtime),
                T::Quantity(_),
                T::Quantity(_),
            ) => Self::QuantityPower,
            _ => return None,
        })
    }
}

/// The typed form of `lhs op rhs`, if checking admits its operand types.
fn binary(
    op: BinOp,
    lhs: Box<TExpr<Symbolic>>,
    rhs: Box<TExpr<Symbolic>>,
) -> Option<TExprKind<Symbolic>> {
    Some(match BinaryOperation::select(op, lhs.ty(), rhs.ty())? {
        BinaryOperation::And => TExprKind::Bool(BExpr::And { lhs, rhs }),
        BinaryOperation::Or => TExprKind::Bool(BExpr::Or { lhs, rhs }),
        BinaryOperation::Equality(op) => TExprKind::Bool(BExpr::Equality { op, lhs, rhs }),
        BinaryOperation::Ordering(op, operands) => TExprKind::Bool(BExpr::Ordering {
            op,
            operands,
            lhs,
            rhs,
        }),
        BinaryOperation::KeyShift => TExprKind::KeyShift {
            key: lhs,
            addend: rhs,
        },
        BinaryOperation::Int(op) => TExprKind::Int(IExpr::Arith { op, lhs, rhs }),
        BinaryOperation::IntExactPower(exponent) => TExprKind::Int(IExpr::ExactPower {
            base: lhs,
            exponent,
            exponent_expr: rhs,
        }),
        BinaryOperation::IntPower => TExprKind::Int(IExpr::Power {
            base: lhs,
            exponent: rhs,
        }),
        BinaryOperation::Quantity(op) => TExprKind::Quantity(QExpr::Arith { op, lhs, rhs }),
        BinaryOperation::QuantityExactPower(exponent) => TExprKind::Quantity(QExpr::ExactPower {
            base: lhs,
            exponent,
            exponent_expr: rhs,
        }),
        BinaryOperation::QuantityPower => TExprKind::Quantity(QExpr::Power {
            base: lhs,
            exponent: rhs,
        }),
        BinaryOperation::DatetimeDifference => {
            TExprKind::Quantity(QExpr::DatetimeDifference { lhs, rhs })
        }
        BinaryOperation::Complex(op) => TExprKind::Complex(CExpr::Arith { op, lhs, rhs }),
        BinaryOperation::ScaleRight(op) => TExprKind::Complex(CExpr::ScaleRight {
            op,
            complex: lhs,
            scalar: rhs,
        }),
        BinaryOperation::ScaleLeft(op) => TExprKind::Complex(CExpr::ScaleLeft {
            op,
            scalar: lhs,
            complex: rhs,
        }),
        BinaryOperation::Shift(op) => TExprKind::Datetime(DExpr::Shift {
            op,
            datetime: lhs,
            duration: rhs,
        }),
        BinaryOperation::ShiftAfter => TExprKind::Datetime(DExpr::ShiftAfter {
            duration: lhs,
            datetime: rhs,
        }),
    })
}

/// The typed form of `op operand`, if checking admits its operand type.
fn unary(op: UnaryOp, operand: Box<TExpr<Symbolic>>) -> Option<TExprKind<Symbolic>> {
    Some(match (op, operand.ty()) {
        (UnaryOp::Not, CheckedType::Bool) => TExprKind::Bool(BExpr::Not(operand)),
        (UnaryOp::Neg, CheckedType::Quantity(_)) => TExprKind::Quantity(QExpr::Neg(operand)),
        (UnaryOp::Neg, CheckedType::Int) => TExprKind::Int(IExpr::Neg(operand)),
        (UnaryOp::Neg, CheckedType::Complex(_)) => TExprKind::Complex(CExpr::Neg(operand)),
        _ => return None,
    })
}

/// The static index proofs of one node, placed on the selectors they prove.
struct StaticPositions<'a> {
    requirements: &'a [StaticIndexRequirement],
    placed: usize,
}

impl<'a> StaticPositions<'a> {
    const fn new(requirements: &'a [StaticIndexRequirement]) -> Self {
        Self {
            requirements,
            placed: 0,
        }
    }

    /// The proof for `operand`: requirements are retained in selector order.
    fn take(&mut self, operand: &ExprId) -> Option<StaticPosition<Symbolic>> {
        let requirement = self.requirements.get(self.placed)?;
        (requirement.operand == *operand).then(|| {
            self.placed = self.placed.saturating_add(1);
            StaticPosition {
                axis: requirement.axis.clone(),
                position: requirement.position,
                usage: requirement.usage,
            }
        })
    }

    const fn all_placed(&self) -> bool {
        self.placed == self.requirements.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Dimension;
    use crate::registry::checked_type::IndexTypeRef;
    use crate::registry::time_scale::TimeScale;

    fn quantity() -> CheckedType<Symbolic> {
        CheckedType::Quantity(Dimension::dimensionless())
    }

    fn complex() -> CheckedType<Symbolic> {
        CheckedType::Complex(Dimension::dimensionless())
    }

    fn datetime() -> CheckedType<Symbolic> {
        CheckedType::Datetime(TimeScale::ALL[0])
    }

    fn key() -> CheckedType<Symbolic> {
        CheckedType::Key(
            IndexTypeRef::from_finite_index_form(crate::nat::NatPolyForm::from_constant(3))
                .unwrap(),
        )
    }

    fn select(
        op: BinOp,
        lhs: &CheckedType<Symbolic>,
        rhs: &CheckedType<Symbolic>,
    ) -> Option<String> {
        BinaryOperation::select(op, lhs, rhs).map(|operation| format!("{operation:?}"))
    }

    #[test]
    fn operand_types_select_the_operation() {
        use CheckedType as T;
        let two = crate::exact_rational::ExactRational::integer(2).unwrap();
        let cases = [
            (BinOp::And, T::Bool, T::Bool, "And"),
            (BinOp::Or, T::Bool, T::Bool, "Or"),
            (BinOp::Eq, T::Bool, T::Bool, "Equality(Eq)"),
            (BinOp::Ne, datetime(), datetime(), "Equality(Ne)"),
            (BinOp::Lt, quantity(), quantity(), "Ordering(Lt, Quantity)"),
            (BinOp::Gt, T::Int, T::Int, "Ordering(Gt, Int)"),
            (BinOp::Le, datetime(), datetime(), "Ordering(Le, Datetime)"),
            (BinOp::Ge, quantity(), quantity(), "Ordering(Ge, Quantity)"),
            (BinOp::Add, key(), T::Int, "KeyShift"),
            (BinOp::Add, T::Int, T::Int, "Int(Add)"),
            (BinOp::Sub, T::Int, T::Int, "Int(Sub)"),
            (BinOp::Mul, T::Int, T::Int, "Int(Mul)"),
            (BinOp::Div, T::Int, T::Int, "Int(Div)"),
            (BinOp::Mod, T::Int, T::Int, "Int(Mod)"),
            (BinOp::Add, quantity(), quantity(), "Quantity(Add)"),
            (BinOp::Sub, quantity(), quantity(), "Quantity(Sub)"),
            (BinOp::Mul, quantity(), quantity(), "Quantity(Mul)"),
            (BinOp::Div, complex(), complex(), "Complex(Div)"),
            (BinOp::Mul, complex(), quantity(), "ScaleRight(Mul)"),
            (BinOp::Div, complex(), quantity(), "ScaleRight(Div)"),
            (BinOp::Mul, quantity(), complex(), "ScaleLeft(Mul)"),
            (BinOp::Div, quantity(), complex(), "ScaleLeft(Div)"),
            (BinOp::Sub, datetime(), datetime(), "DatetimeDifference"),
            (BinOp::Add, datetime(), quantity(), "Shift(Add)"),
            (BinOp::Sub, datetime(), quantity(), "Shift(Sub)"),
            (BinOp::Add, quantity(), datetime(), "ShiftAfter"),
            (
                BinOp::Pow(PowerExponent::Exact(two)),
                T::Int,
                T::Int,
                "IntExactPower(2)",
            ),
            (
                BinOp::Pow(PowerExponent::Runtime),
                T::Int,
                T::Int,
                "IntPower",
            ),
            (
                BinOp::Pow(PowerExponent::Runtime),
                quantity(),
                quantity(),
                "QuantityPower",
            ),
            (
                BinOp::Pow(PowerExponent::FloatSyntax { exact: None }),
                quantity(),
                quantity(),
                "QuantityPower",
            ),
        ];
        for (op, lhs, rhs, expected) in cases {
            assert_eq!(
                select(op, &lhs, &rhs).as_deref(),
                Some(expected),
                "{op:?} {lhs:?} {rhs:?}"
            );
        }
        let half = crate::exact_rational::ExactRational::try_new(1, 2).unwrap();
        assert!(matches!(
            BinaryOperation::select(
                BinOp::Pow(PowerExponent::Exact(half)),
                &quantity(),
                &quantity()
            ),
            Some(BinaryOperation::QuantityExactPower(exponent)) if exponent == half
        ));
    }

    #[test]
    fn unchecked_operand_types_select_no_operation() {
        use CheckedType as T;
        let half = crate::exact_rational::ExactRational::try_new(1, 2).unwrap();
        let indexed = T::Indexed {
            element: Box::new(T::Int),
            index: IndexTypeRef::from_finite_index_form(crate::nat::NatPolyForm::from_constant(2))
                .unwrap(),
        };
        let cases = [
            (BinOp::And, T::Bool, T::Int),
            (BinOp::Or, T::Int, T::Bool),
            (BinOp::Eq, indexed.clone(), indexed.clone()),
            (BinOp::Ne, T::Int, indexed),
            (BinOp::Lt, complex(), complex()),
            (BinOp::Lt, T::Int, quantity()),
            (BinOp::Mod, quantity(), quantity()),
            (BinOp::Mul, key(), T::Int),
            (BinOp::Add, complex(), quantity()),
            (BinOp::Add, datetime(), datetime()),
            (BinOp::Sub, quantity(), datetime()),
            (BinOp::Pow(PowerExponent::Exact(half)), T::Int, T::Int),
            (
                BinOp::Pow(PowerExponent::FloatSyntax { exact: None }),
                T::Int,
                T::Int,
            ),
        ];
        for (op, lhs, rhs) in cases {
            assert_eq!(select(op, &lhs, &rhs), None, "{op:?} {lhs:?} {rhs:?}");
        }
    }

    #[test]
    fn unary_operators_follow_the_operand_type() {
        let node = |ty| {
            let mut ids = crate::expression_id::ExprIds::default();
            Box::new(TExpr::new(
                ids.allocate().unwrap(),
                crate::syntax::span::Span::new(0, 1),
                ty,
                TExprKind::Bool(BExpr::Literal(true)),
            ))
        };
        assert!(matches!(
            unary(UnaryOp::Not, node(CheckedType::Bool)),
            Some(TExprKind::Bool(BExpr::Not(_)))
        ));
        assert!(matches!(
            unary(UnaryOp::Neg, node(quantity())),
            Some(TExprKind::Quantity(QExpr::Neg(_)))
        ));
        assert!(matches!(
            unary(UnaryOp::Neg, node(CheckedType::Int)),
            Some(TExprKind::Int(IExpr::Neg(_)))
        ));
        assert!(matches!(
            unary(UnaryOp::Neg, node(complex())),
            Some(TExprKind::Complex(CExpr::Neg(_)))
        ));
        assert!(unary(UnaryOp::Not, node(CheckedType::Int)).is_none());
        assert!(unary(UnaryOp::Neg, node(CheckedType::Bool)).is_none());
        assert!(unary(UnaryOp::Neg, node(datetime())).is_none());
    }
}
