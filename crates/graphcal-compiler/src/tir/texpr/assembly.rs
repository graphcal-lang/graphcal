//! Bottom-up assembly of typed trees while one checking pass records nodes.
//!
//! Inference records every expression after its children, so each checked
//! node is assembled from the typed children already recorded for it. The
//! mapping from a HIR node to its typed form is the only place a [`TExpr`] is
//! built from inference results, and it rejects any shape the checker did not
//! prove: a missing or contextual child in a value position, a constructor
//! without its application, a match arm without its target, or a static
//! position that belongs to no selector of the node.
#![expect(
    clippy::vec_box,
    reason = "operand lists are the boxed children the operation families own"
)]

use std::collections::HashMap;

use thiserror::Error;

use crate::expression_id::ExprId;
use crate::hir::expr::{ConstRef, Expr, ExprKind, IndexArg, MatchPattern};
use crate::resolved_name::ResolvedConstructorName;
use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::syntax::ast::{BinOp, PowerExponent, UnaryOp};
use crate::syntax::span::Spanned;
use crate::tir::static_index::StaticIndexRequirement;

use super::call_targets::CallTargets;
use super::model::{
    ContextualLiteral, CoordinateSearch, DatetimeLiteral, ExternArgKind, StaticPosition, TArg,
    TConstRef, TConstructorArm, TContextual, TExpr, TExprKind, TExternArg, TFieldInit, TIndexArg,
    TKeyForm, TLabelArm, TMapEntry, TMatchArms, TParamBinding,
};
use super::nominal::{ConstructorApplication, ConstructorMatch};
use super::operators::{
    ArithOp, BExpr, CExpr, ComplexPart, DExpr, EqualityOp, IExpr, IntArithOp, LinearAlgebraCall,
    OrderedOperands, OrderingOp, QExpr, ScaleOp, ShiftOp,
};
use crate::builtin::{BuiltinFn, ComplexFn, ConversionFn, DatetimeConstructorFn, DatetimeFn};
use crate::function_signature::FunctionParam;
use crate::hir::expr::FunctionRef;

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
    #[error("expression {0:?} has no operation for its checked operand types")]
    UncheckedOperands(ExprId),
}

/// The node-specific facts checking established for one value expression.
pub struct NodeFacts<'a> {
    pub constructor: Option<&'a ConstructorApplication<Symbolic>>,
    /// The declared parameters of the plugin function a call node calls.
    pub extern_params: Option<&'a [FunctionParam]>,
    pub constructor_matches: &'a HashMap<ResolvedConstructorName, ConstructorMatch>,
    pub static_indexes: &'a [StaticIndexRequirement],
}

/// Typed nodes recorded so far whose parent has not been recorded yet.
///
/// After a successful pass only the checked roots remain. The DAGs the
/// recorded call nodes target are numbered by the pass's [`CallTargets`].
#[derive(Debug, Default)]
pub struct PendingNodes {
    nodes: HashMap<ExprId, TArg<Symbolic>>,
    calls: CallTargets,
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

    /// Every node no parent has claimed: the trees of the roots checked,
    /// with the call targets their call nodes are numbered by.
    pub fn into_roots(self) -> (HashMap<ExprId, super::model::TBody<Symbolic>>, CallTargets) {
        let roots = self
            .nodes
            .into_iter()
            .map(|(id, node)| {
                let body = match node {
                    TArg::Value(value) => super::model::TBody::Value(value),
                    TArg::Contextual(literal) => super::model::TBody::Contextual(literal),
                };
                (id, body)
            })
            .collect();
        (roots, self.calls)
    }

    /// The call targets the recorded call nodes are numbered by.
    pub(super) fn into_calls(self) -> CallTargets {
        self.calls
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
            ExprKind::FnCall { callee, args } => call(
                &callee.value,
                args.iter()
                    .map(|arg| self.take_arg(expr, arg))
                    .collect::<Result<_, _>>()?,
                facts.extern_params,
            )
            .ok_or_else(|| AssemblyError::UncheckedOperands(id()))?,
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
                        IndexArg::Expr(operand) => {
                            let position = positions.take(operand.id());
                            let operand = self.take_boxed(expr, operand)?;
                            match (operand.ty(), position) {
                                (CheckedType::Key(_), None) => TIndexArg::Key(operand),
                                (CheckedType::Int, Some(position)) => {
                                    TIndexArg::Position { operand, position }
                                }
                                _ => return Err(AssemblyError::UncheckedOperands(id())),
                            }
                        }
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
            } => {
                use crate::syntax::ast::KeyFormKind;
                let form = match (kind, positions.take(arg.id())) {
                    (KeyFormKind::Static, Some(position)) => TKeyForm::Static(position),
                    (KeyFormKind::Fin, None) => TKeyForm::Fin,
                    (KeyFormKind::Floor, None) => TKeyForm::Search(CoordinateSearch::Floor),
                    (KeyFormKind::Ceil, None) => TKeyForm::Search(CoordinateSearch::Ceil),
                    (KeyFormKind::Nearest, None) => TKeyForm::Search(CoordinateSearch::Nearest),
                    _ => return Err(AssemblyError::UncheckedOperands(id())),
                };
                TExprKind::Key {
                    form,
                    axis: axis.clone(),
                    arg: self.take_boxed(expr, arg)?,
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.take_boxed(expr, scrutinee)?;
                let mut labels = Vec::new();
                let mut constructors = Vec::new();
                for arm in arms {
                    match &arm.pattern {
                        MatchPattern::Constructor {
                            constructor,
                            bindings,
                            ..
                        } => constructors.push(TConstructorArm {
                            target: facts
                                .constructor_matches
                                .get(&constructor.value)
                                .cloned()
                                .ok_or_else(|| AssemblyError::MissingMatchTarget(id()))?,
                            bindings: bindings.clone(),
                            body: self.take_value(expr, &arm.body)?,
                            span: arm.span,
                        }),
                        MatchPattern::IndexLabel { variant, .. } => labels.push(TLabelArm {
                            label: variant.clone(),
                            body: self.take_value(expr, &arm.body)?,
                            span: arm.span,
                        }),
                    }
                }
                let arms = match (scrutinee.ty(), labels.is_empty(), constructors.is_empty()) {
                    (CheckedType::Key(_), _, true) => TMatchArms::Labels(labels),
                    (CheckedType::Struct(..), true, _) => TMatchArms::Constructors(constructors),
                    _ => return Err(AssemblyError::UncheckedOperands(id())),
                };
                TExprKind::Match { scrutinee, arms }
            }
            ExprKind::DagCall {
                target,
                args,
                static_bindings,
                output,
            } => TExprKind::DagCall {
                slot: self.calls.intern(target.value.clone()),
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

/// The typed form of a call of `callee` with checked `args`, if checking
/// admits their types.
fn call(
    callee: &FunctionRef,
    args: Vec<TArg<Symbolic>>,
    extern_params: Option<&[FunctionParam]>,
) -> Option<TExprKind<Symbolic>> {
    let function = match callee {
        FunctionRef::External(function) => {
            let params = extern_params?;
            let args = values(args)?;
            if args.len() != params.len() {
                return None;
            }
            return Some(TExprKind::Extern {
                function: function.clone(),
                args: params
                    .iter()
                    .zip(args)
                    .map(|(param, arg)| TExternArg {
                        kind: ExternArgKind::for_param(param),
                        value: *arg,
                    })
                    .collect(),
            });
        }
        FunctionRef::Epoch { scale } => {
            let [TArg::Contextual(civil)] = args.as_slice() else {
                return None;
            };
            let ContextualLiteral::CivilDateTime(civil) = civil.literal() else {
                return None;
            };
            return Some(TExprKind::DatetimeLiteral(DatetimeLiteral::Epoch {
                civil: *civil,
                scale: scale.value,
            }));
        }
        FunctionRef::Builtin(builtin) => builtin.function(),
    };
    if function == BuiltinFn::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Datetime)) {
        return datetime_literal(&args).map(TExprKind::DatetimeLiteral);
    }
    builtin_call(function, values(args)?)
}

/// The value arguments of a call, if none is a contextual literal.
fn values(args: Vec<TArg<Symbolic>>) -> Option<Vec<Box<TExpr<Symbolic>>>> {
    args.into_iter()
        .map(|arg| match arg {
            TArg::Value(value) => Some(value),
            TArg::Contextual(_) => None,
        })
        .collect()
}

/// The datetime `datetime(literal)` or `datetime(literal, timezone)` builds.
fn datetime_literal(args: &[TArg<Symbolic>]) -> Option<DatetimeLiteral> {
    match args {
        [TArg::Contextual(datetime)] => match datetime.literal() {
            ContextualLiteral::OffsetDateTime(datetime) => Some(DatetimeLiteral::Offset(*datetime)),
            _ => None,
        },
        [TArg::Contextual(datetime), TArg::Contextual(time_zone)] => {
            match (datetime.literal(), time_zone.literal()) {
                (ContextualLiteral::ZonedDateTime(datetime), ContextualLiteral::TimeZone(zone))
                    if datetime.time_zone() == zone =>
                {
                    Some(DatetimeLiteral::Zoned(datetime.clone()))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The single argument of a unary built-in.
fn single(args: Vec<Box<TExpr<Symbolic>>>) -> Option<Box<TExpr<Symbolic>>> {
    let [arg] = <[_; 1]>::try_from(args).ok()?;
    Some(arg)
}

/// The two arguments of a binary built-in.
fn pair(args: Vec<Box<TExpr<Symbolic>>>) -> Option<[Box<TExpr<Symbolic>>; 2]> {
    <[_; 2]>::try_from(args).ok()
}

/// The value category of a checked argument, as built-ins dispatch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Quantity,
    Complex,
    Int,
    Datetime,
    Key,
    Indexed,
    Other,
}

const fn category(arg: &TExpr<Symbolic>) -> Category {
    match arg.ty() {
        CheckedType::Quantity(_) => Category::Quantity,
        CheckedType::Complex(_) => Category::Complex,
        CheckedType::Int => Category::Int,
        CheckedType::Datetime(_) => Category::Datetime,
        CheckedType::Key(_) => Category::Key,
        CheckedType::Indexed { .. } => Category::Indexed,
        CheckedType::Bool | CheckedType::Struct(..) => Category::Other,
    }
}

/// The typed form of a value built-in call, if checking admits its argument
/// types.
fn builtin_call(
    function: BuiltinFn,
    args: Vec<Box<TExpr<Symbolic>>>,
) -> Option<TExprKind<Symbolic>> {
    Some(match function {
        BuiltinFn::Scalar(function) => {
            if !args.iter().all(|arg| category(arg) == Category::Quantity) {
                return None;
            }
            TExprKind::Quantity(QExpr::Scalar { function, args })
        }
        BuiltinFn::Complex(function) => complex_call(function, args)?,
        BuiltinFn::Aggregation(function) => {
            let arg = single(args)?;
            if category(&arg) != Category::Indexed {
                return None;
            }
            TExprKind::Aggregate { function, arg }
        }
        BuiltinFn::LinearAlgebra(function) => {
            if !args.iter().all(|arg| category(arg) == Category::Indexed) {
                return None;
            }
            TExprKind::LinearAlgebra(LinearAlgebraCall::try_new(function, args)?)
        }
        BuiltinFn::Conversion(conversion) => {
            let arg = single(args)?;
            match (conversion, category(&arg)) {
                (ConversionFn::ToFloat, Category::Int) => TExprKind::Quantity(QExpr::FromInt(arg)),
                (ConversionFn::ToInt, Category::Key) => TExprKind::Int(IExpr::FinPosition(arg)),
                (ConversionFn::ToInt, Category::Quantity) => {
                    TExprKind::Int(IExpr::FromQuantity(arg))
                }
                (ConversionFn::Coord, Category::Key) => TExprKind::Quantity(QExpr::Coordinate(arg)),
                _ => return None,
            }
        }
        BuiltinFn::Datetime(function) => {
            let arg = single(args)?;
            match (function, category(&arg)) {
                (DatetimeFn::ScaleConversion(conversion), Category::Datetime) => {
                    TExprKind::Datetime(DExpr::ToScale { conversion, arg })
                }
                (DatetimeFn::Field(field), Category::Datetime) => {
                    TExprKind::Int(IExpr::DatetimeField { field, arg })
                }
                (DatetimeFn::FromNumeric(function), Category::Quantity) => {
                    TExprKind::Datetime(DExpr::FromQuantity { function, arg })
                }
                (DatetimeFn::FromNumeric(function), Category::Int) => {
                    TExprKind::Datetime(DExpr::FromInt { function, arg })
                }
                (DatetimeFn::ToNumeric(function), Category::Datetime) => {
                    TExprKind::Quantity(QExpr::FromDatetime { function, arg })
                }
                _ => return None,
            }
        }
    })
}

/// The typed form of a complex built-in call, if checking admits its
/// argument types.
fn complex_call(
    function: ComplexFn,
    args: Vec<Box<TExpr<Symbolic>>>,
) -> Option<TExprKind<Symbolic>> {
    let part = |part, arg| TExprKind::Quantity(QExpr::ComplexPart { part, arg });
    Some(match function {
        ComplexFn::Rectangular | ComplexFn::Polar => {
            let [first, second] = pair(args)?;
            if category(&first) != Category::Quantity || category(&second) != Category::Quantity {
                return None;
            }
            TExprKind::Complex(if function == ComplexFn::Rectangular {
                CExpr::Rectangular {
                    re: first,
                    im: second,
                }
            } else {
                CExpr::Polar {
                    magnitude: first,
                    phase: second,
                }
            })
        }
        _ => {
            let arg = single(args)?;
            match (function, category(&arg)) {
                (ComplexFn::ToComplex, Category::Quantity) => {
                    TExprKind::Complex(CExpr::FromReal(arg))
                }
                (ComplexFn::Real, Category::Complex) => part(ComplexPart::Real, arg),
                (ComplexFn::Imaginary, Category::Complex) => part(ComplexPart::Imaginary, arg),
                (ComplexFn::Phase, Category::Complex) => part(ComplexPart::Phase, arg),
                (ComplexFn::Absolute, Category::Complex) => part(ComplexPart::Magnitude, arg),
                (ComplexFn::Absolute, Category::Quantity) => TExprKind::Quantity(QExpr::Abs(arg)),
                (ComplexFn::Conjugate, Category::Complex) => {
                    TExprKind::Complex(CExpr::Conjugate(arg))
                }
                (ComplexFn::Exponential, Category::Complex) => TExprKind::Complex(CExpr::Exp(arg)),
                (ComplexFn::Exponential, Category::Quantity) => {
                    TExprKind::Quantity(QExpr::Exp(arg))
                }
                _ => return None,
            }
        }
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
    use crate::semantic::checked_type::IndexTypeRef;
    use crate::semantic::time_scale::TimeScale;

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

    fn arg(ty: CheckedType<Symbolic>) -> Box<TExpr<Symbolic>> {
        let mut ids = crate::expression_id::ExprIds::default();
        Box::new(TExpr::new(
            ids.allocate().unwrap(),
            crate::syntax::span::Span::new(0, 1),
            ty,
            TExprKind::Bool(BExpr::Literal(true)),
        ))
    }

    fn builtin(function: BuiltinFn, args: &[CheckedType<Symbolic>]) -> Option<String> {
        // The node's variant and the first name inside it.
        builtin_call(function, args.iter().cloned().map(arg).collect()).map(|kind| {
            format!("{kind:?}")
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .take(2)
                .collect::<Vec<_>>()
                .join("::")
        })
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one case per built-in family and overload"
    )]
    fn built_in_calls_select_their_typed_operation() {
        use crate::builtin::{
            AggregationFn, DatetimeField, DatetimeFromNumericFn, DatetimeToNumericFn,
            LinearAlgebraFn, ScalarFn, TimeScaleConversionFn, ValueAggregation,
        };
        let indexed = CheckedType::Indexed {
            element: Box::new(quantity()),
            index: IndexTypeRef::from_finite_index_form(crate::nat::NatPolyForm::from_constant(2))
                .unwrap(),
        };
        let cases: Vec<(BuiltinFn, Vec<CheckedType<Symbolic>>, &str)> = vec![
            (
                BuiltinFn::Scalar(ScalarFn::Sqrt),
                vec![quantity()],
                "Quantity::Scalar",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Rectangular),
                vec![quantity(), quantity()],
                "Complex::Rectangular",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Polar),
                vec![quantity(), quantity()],
                "Complex::Polar",
            ),
            (
                BuiltinFn::Complex(ComplexFn::ToComplex),
                vec![quantity()],
                "Complex::FromReal",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Real),
                vec![complex()],
                "Quantity::ComplexPart",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Absolute),
                vec![complex()],
                "Quantity::ComplexPart",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Absolute),
                vec![quantity()],
                "Quantity::Abs",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Conjugate),
                vec![complex()],
                "Complex::Conjugate",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Exponential),
                vec![complex()],
                "Complex::Exp",
            ),
            (
                BuiltinFn::Complex(ComplexFn::Exponential),
                vec![quantity()],
                "Quantity::Exp",
            ),
            (
                BuiltinFn::Aggregation(AggregationFn::Value(ValueAggregation::Sum)),
                vec![indexed.clone()],
                "Aggregate::function",
            ),
            (
                BuiltinFn::LinearAlgebra(LinearAlgebraFn::Dot),
                vec![indexed.clone(), indexed],
                "LinearAlgebra::Dot",
            ),
            (
                BuiltinFn::Conversion(ConversionFn::ToFloat),
                vec![CheckedType::Int],
                "Quantity::FromInt",
            ),
            (
                BuiltinFn::Conversion(ConversionFn::ToInt),
                vec![key()],
                "Int::FinPosition",
            ),
            (
                BuiltinFn::Conversion(ConversionFn::ToInt),
                vec![quantity()],
                "Int::FromQuantity",
            ),
            (
                BuiltinFn::Conversion(ConversionFn::Coord),
                vec![key()],
                "Quantity::Coordinate",
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::Field(DatetimeField::Year)),
                vec![datetime()],
                "Int::DatetimeField",
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::FromNumeric(DatetimeFromNumericFn::Jd)),
                vec![quantity()],
                "Datetime::FromQuantity",
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::FromNumeric(DatetimeFromNumericFn::Unix)),
                vec![CheckedType::Int],
                "Datetime::FromInt",
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::ToNumeric(DatetimeToNumericFn::Mjd)),
                vec![datetime()],
                "Quantity::FromDatetime",
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::ScaleConversion(TimeScaleConversionFn::ALL[0])),
                vec![datetime()],
                "Datetime::ToScale",
            ),
        ];
        for (function, args, expected) in cases {
            assert_eq!(
                builtin(function, &args).as_deref(),
                Some(expected),
                "{function:?} {args:?}"
            );
        }
    }

    #[test]
    fn built_in_calls_reject_unchecked_argument_types() {
        use crate::builtin::{AggregationFn, ScalarFn, ValueAggregation};
        let cases: Vec<(BuiltinFn, Vec<CheckedType<Symbolic>>)> = vec![
            (BuiltinFn::Scalar(ScalarFn::Sqrt), vec![CheckedType::Int]),
            (BuiltinFn::Complex(ComplexFn::Rectangular), vec![quantity()]),
            (
                BuiltinFn::Complex(ComplexFn::Polar),
                vec![complex(), quantity()],
            ),
            (BuiltinFn::Complex(ComplexFn::Real), vec![quantity()]),
            (BuiltinFn::Complex(ComplexFn::Conjugate), vec![quantity()]),
            (BuiltinFn::Complex(ComplexFn::ToComplex), vec![complex()]),
            (
                BuiltinFn::Aggregation(AggregationFn::Value(ValueAggregation::Sum)),
                vec![quantity()],
            ),
            (
                BuiltinFn::Conversion(ConversionFn::ToFloat),
                vec![quantity()],
            ),
            (BuiltinFn::Conversion(ConversionFn::Coord), vec![quantity()]),
            (
                BuiltinFn::Conversion(ConversionFn::ToInt),
                vec![quantity(), quantity()],
            ),
            (
                BuiltinFn::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Epoch)),
                vec![datetime()],
            ),
        ];
        for (function, args) in cases {
            assert_eq!(builtin(function, &args), None, "{function:?} {args:?}");
        }
    }

    fn contextual(literal: ContextualLiteral) -> TArg<Symbolic> {
        let mut ids = crate::expression_id::ExprIds::default();
        TArg::Contextual(TContextual::new(
            ids.allocate().unwrap(),
            crate::syntax::span::Span::new(0, 1),
            literal,
        ))
    }

    fn datetime_constructor() -> FunctionRef {
        let crate::builtin::BuiltinApplication::ScaleFree(builtin) =
            BuiltinFn::Datetime(DatetimeFn::Constructor(DatetimeConstructorFn::Datetime))
                .application()
        else {
            panic!("datetime() is applied without a time scale");
        };
        FunctionRef::Builtin(builtin)
    }

    fn offset() -> ContextualLiteral {
        ContextualLiteral::OffsetDateTime(
            crate::datetime_literal::OffsetDateTimeLiteral::parse("2026-01-01T00:00:00Z").unwrap(),
        )
    }

    fn civil() -> crate::datetime_literal::CivilDateTimeLiteral {
        crate::datetime_literal::CivilDateTimeLiteral::parse("2026-01-01T09:00:00").unwrap()
    }

    fn zone(name: &str) -> crate::semantic::time_zone::IanaTimeZoneId {
        crate::semantic::time_zone::TimeZoneRegistry::bundled()
            .parse_iana_id(name)
            .unwrap()
    }

    fn zoned(name: &str) -> ContextualLiteral {
        ContextualLiteral::ZonedDateTime(
            crate::datetime_literal::ZonedDateTimeLiteral::resolve(
                civil(),
                zone(name),
                &crate::semantic::time_zone::TimeZoneRegistry::bundled(),
            )
            .unwrap(),
        )
    }

    #[test]
    fn extern_calls_pair_each_argument_with_its_declared_parameter() {
        use crate::function_signature::{
            DimMonomial, FunctionParam, FunctionSignature, ParamKind, ScalarValueKind,
        };
        use crate::syntax::function_name::{FnName, FnParamName};
        use crate::syntax::index_name::IndexVarName;
        use crate::syntax::non_empty::NonEmpty;

        let index = IndexVarName::expect_valid("I");
        let param = |name: &str, kind| FunctionParam {
            name: FnParamName::expect_valid(name),
            kind,
        };
        let signature = FunctionSignature::try_new(
            Vec::new(),
            vec![index.clone()],
            vec![
                param("flag", ParamKind::Scalar(ScalarValueKind::Bool)),
                param("count", ParamKind::Scalar(ScalarValueKind::Int)),
                param(
                    "level",
                    ParamKind::Scalar(ScalarValueKind::Quantity(DimMonomial::fixed(
                        Dimension::dimensionless(),
                    ))),
                ),
                param(
                    "xs",
                    ParamKind::Indexed {
                        element: ScalarValueKind::Int,
                        indexes: NonEmpty::singleton(index),
                    },
                ),
            ],
            ParamKind::Scalar(ScalarValueKind::Bool).into(),
        )
        .unwrap();
        let callee = FunctionRef::External(crate::hir::expr::ExternFnRef {
            plugin: crate::plugin_identity::PluginIdentity::Host(
                crate::syntax::plugin::PluginPath::new("graphcal:test"),
            ),
            alias: crate::syntax::module_name::ModuleAliasName::expect_valid("test"),
            name: FnName::expect_valid("f"),
        });
        let args = |count: usize| {
            (0..count)
                .map(|_| TArg::Value(arg(quantity())))
                .collect::<Vec<_>>()
        };

        let Some(TExprKind::Extern { args: built, .. }) =
            call(&callee, args(4), Some(signature.params()))
        else {
            panic!("a checked extern call builds its node");
        };
        let kinds = built.iter().map(|arg| &arg.kind).collect::<Vec<_>>();
        assert!(matches!(
            kinds.as_slice(),
            [
                ExternArgKind::Bool { .. },
                ExternArgKind::Int { .. },
                ExternArgKind::Quantity { .. },
                ExternArgKind::Indexed {
                    element: ScalarValueKind::Int,
                    indexes,
                    ..
                },
            ] if indexes.len() == 1
        ));
        assert_eq!(
            kinds
                .iter()
                .map(|kind| kind.param().as_str())
                .collect::<Vec<_>>(),
            ["flag", "count", "level", "xs"]
        );

        // Without its declared parameters, with another argument count, or
        // with a contextual argument, the call is not a checked extern call.
        assert!(call(&callee, args(4), None).is_none());
        assert!(call(&callee, args(3), Some(signature.params())).is_none());
        let mut contextual_args = args(3);
        contextual_args.push(contextual(ContextualLiteral::String("x".to_owned())));
        assert!(call(&callee, contextual_args, Some(signature.params())).is_none());
    }

    #[test]
    fn datetime_constructors_build_their_parsed_literal() {
        assert!(matches!(
            call(&datetime_constructor(), vec![contextual(offset())], None),
            Some(TExprKind::DatetimeLiteral(DatetimeLiteral::Offset(_)))
        ));
        let Some(TExprKind::DatetimeLiteral(DatetimeLiteral::Zoned(datetime))) = call(
            &datetime_constructor(),
            vec![
                contextual(zoned("Asia/Tokyo")),
                contextual(ContextualLiteral::TimeZone(zone("Asia/Tokyo"))),
            ],
            None,
        ) else {
            panic!("a zoned literal in its own timezone builds a zoned datetime");
        };
        assert_eq!(datetime.time_zone(), &zone("Asia/Tokyo"));
        let scale = TimeScale::ALL[1];
        let epoch = FunctionRef::Epoch {
            scale: Spanned::new(scale, crate::syntax::span::Span::new(0, 1)),
        };
        assert!(matches!(
            call(&epoch, vec![contextual(ContextualLiteral::CivilDateTime(civil()))], None),
            Some(TExprKind::DatetimeLiteral(DatetimeLiteral::Epoch { scale: built, .. }))
                if built == scale
        ));
    }

    #[test]
    fn datetime_constructors_reject_unchecked_arguments() {
        let constructor = datetime_constructor();
        let time_zone = || contextual(ContextualLiteral::TimeZone(zone("Asia/Tokyo")));
        let rejected = [
            // A zoned literal resolved in another timezone than its argument.
            vec![
                contextual(zoned("Europe/Paris")),
                contextual(ContextualLiteral::TimeZone(zone("Asia/Tokyo"))),
            ],
            // A zoned literal without its timezone argument.
            vec![contextual(zoned("Asia/Tokyo"))],
            // An offset literal with a timezone argument.
            vec![contextual(offset()), time_zone()],
            // A literal of another kind.
            vec![contextual(ContextualLiteral::String("2026".to_owned()))],
            vec![contextual(ContextualLiteral::CivilDateTime(civil()))],
            // The timezone first.
            vec![time_zone(), contextual(zoned("Asia/Tokyo"))],
            // A value argument, or none.
            vec![TArg::Value(arg(datetime()))],
            Vec::new(),
        ];
        for args in rejected {
            assert!(call(&constructor, args, None).is_none());
        }
        let epoch = FunctionRef::Epoch {
            scale: Spanned::new(TimeScale::ALL[0], crate::syntax::span::Span::new(0, 1)),
        };
        assert!(call(&epoch, vec![contextual(offset())], None).is_none());
        assert!(call(&epoch, vec![TArg::Value(arg(datetime()))], None).is_none());
        assert!(
            call(
                &epoch,
                vec![
                    contextual(ContextualLiteral::CivilDateTime(civil())),
                    contextual(ContextualLiteral::CivilDateTime(civil())),
                ],
                None,
            )
            .is_none()
        );
    }
}
