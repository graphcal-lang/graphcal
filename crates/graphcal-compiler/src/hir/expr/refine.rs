//! Rebuild an HIR tree under another [`Completeness`].
//!
//! Changing completeness changes the type of every node, so the tree is
//! rebuilt. Nodes are visited in pre-order (a node before its children, the
//! children in the structural order of [`super::visit_expr_children`]), so the
//! first failure a [`Refinement`] reports is the first one in source order.

use super::model::Completeness;
use super::model::{
    AssertBody, ConstRef, Expr, ExprKind, FieldInit, IndexArg, MapEntry, MatchArm, ParamBinding,
    ResolvedUnitExpr, ResolvedUnitExprItem,
};
use crate::syntax::span::Spanned;

/// Node-level policy for [`refine_expr`].
pub trait Refinement<A: Completeness, B: Completeness> {
    type Failure;

    /// Translate one node identity. Called for a node before its children.
    fn id(&mut self, id: A::Id) -> Result<B::Id, Self::Failure>;

    /// Translate an error node. This is where a refinement to a tree without
    /// error nodes rejects the input.
    fn error_node(&mut self, error: A::Error) -> Result<B::Error, Self::Failure>;

    /// Translate how a declaration reference names its target.
    fn decl_ref(&mut self, reference: A::DeclRef) -> B::DeclRef;

    /// Translate how a unit reference names its unit.
    fn unit_ref(&mut self, reference: A::UnitRef) -> B::UnitRef;
}

/// Rebuild one expression tree under completeness `B`.
pub fn refine_expr<A, B, R>(expr: Expr<A>, refinement: &mut R) -> Result<Expr<B>, R::Failure>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    crate::stack::with_stack_growth(|| {
        let Expr { kind, span, id } = expr;
        let id = refinement.id(id)?;
        let kind = refine_kind(*kind, refinement)?;
        Ok(Expr {
            kind: Box::new(kind),
            span,
            id,
        })
    })
}

/// Rebuild every operand of an assertion body, in operand order.
pub fn refine_assert_body<A, B, R>(
    body: AssertBody<A>,
    refinement: &mut R,
) -> Result<AssertBody<B>, R::Failure>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    Ok(match body {
        AssertBody::Expr(expr) => AssertBody::Expr(refine_box(expr, refinement)?),
        AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => AssertBody::Tolerance {
            actual: refine_box(actual, refinement)?,
            expected: refine_box(expected, refinement)?,
            tolerance: refine_box(tolerance, refinement)?,
        },
    })
}

#[expect(
    clippy::boxed_local,
    reason = "moves a boxed child out of the consumed parent node"
)]
fn refine_box<A, B, R>(expr: Box<Expr<A>>, refinement: &mut R) -> Result<Box<Expr<B>>, R::Failure>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    refine_expr(*expr, refinement).map(Box::new)
}

fn refine_all<A, B, R>(exprs: Vec<Expr<A>>, refinement: &mut R) -> Result<Vec<Expr<B>>, R::Failure>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    exprs
        .into_iter()
        .map(|expr| refine_expr(expr, refinement))
        .collect()
}

fn refine_unit_expr<A, B, R>(
    unit: ResolvedUnitExpr<A::UnitRef>,
    refinement: &mut R,
) -> ResolvedUnitExpr<B::UnitRef>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    ResolvedUnitExpr {
        terms: unit
            .terms
            .into_iter()
            .map(
                |ResolvedUnitExprItem { op, name, power }| ResolvedUnitExprItem {
                    op,
                    name: Spanned::new(refinement.unit_ref(name.value), name.span),
                    power,
                },
            )
            .collect(),
        span: unit.span,
    }
}

#[expect(clippy::too_many_lines, reason = "exhaustive ExprKind rebuild")]
fn refine_kind<A, B, R>(kind: ExprKind<A>, refinement: &mut R) -> Result<ExprKind<B>, R::Failure>
where
    A: Completeness,
    B: Completeness,
    R: Refinement<A, B>,
{
    Ok(match kind {
        ExprKind::Error(error) => ExprKind::Error(refinement.error_node(error)?),
        ExprKind::Number(value) => ExprKind::Number(value),
        ExprKind::Integer(value) => ExprKind::Integer(value),
        ExprKind::Bool(value) => ExprKind::Bool(value),
        ExprKind::StringLiteral(value) => ExprKind::StringLiteral(value),
        ExprKind::OffsetDateTimeLiteral(value) => ExprKind::OffsetDateTimeLiteral(value),
        ExprKind::CivilDateTimeLiteral(value) => ExprKind::CivilDateTimeLiteral(value),
        ExprKind::EpochLiteral(value) => ExprKind::EpochLiteral(value),
        ExprKind::ZonedDateTimeLiteral(value) => ExprKind::ZonedDateTimeLiteral(value),
        ExprKind::IanaTimeZoneLiteral(value) => ExprKind::IanaTimeZoneLiteral(value),
        ExprKind::TypeSystemRef(value) => ExprKind::TypeSystemRef(value),
        ExprKind::GraphRef(Spanned { value, span }) => {
            ExprKind::GraphRef(Spanned::new(refinement.decl_ref(value), span))
        }
        ExprKind::ConstRef(Spanned { value, span }) => ExprKind::ConstRef(Spanned::new(
            match value {
                ConstRef::Decl(reference) => ConstRef::Decl(refinement.decl_ref(reference)),
                ConstRef::Constructor(constructor) => ConstRef::Constructor(constructor),
                ConstRef::Builtin(builtin) => ConstRef::Builtin(builtin),
            },
            span,
        )),
        ExprKind::LocalRef(value) => ExprKind::LocalRef(value),
        ExprKind::VariantLiteral(value) => ExprKind::VariantLiteral(value),
        ExprKind::QuantityLiteral { value, unit } => ExprKind::QuantityLiteral {
            value,
            unit: refine_unit_expr(unit, refinement),
        },
        ExprKind::BinOp { op, lhs, rhs } => ExprKind::BinOp {
            op,
            lhs: refine_box(lhs, refinement)?,
            rhs: refine_box(rhs, refinement)?,
        },
        ExprKind::UnaryOp { op, operand } => ExprKind::UnaryOp {
            op,
            operand: refine_box(operand, refinement)?,
        },
        ExprKind::FnCall { callee, args } => ExprKind::FnCall {
            callee,
            args: refine_all(args, refinement)?,
        },
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => ExprKind::If {
            condition: refine_box(condition, refinement)?,
            then_branch: refine_box(then_branch, refinement)?,
            else_branch: refine_box(else_branch, refinement)?,
        },
        ExprKind::Convert { expr, target } => ExprKind::Convert {
            expr: refine_box(expr, refinement)?,
            target: refine_unit_expr(target, refinement),
        },
        ExprKind::DisplayTimezone { expr, timezone } => ExprKind::DisplayTimezone {
            expr: refine_box(expr, refinement)?,
            timezone,
        },
        ExprKind::FieldAccess { expr, field } => ExprKind::FieldAccess {
            expr: refine_box(expr, refinement)?,
            field,
        },
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields: fields
                .into_iter()
                .map(|FieldInit { name, value }| {
                    Ok(FieldInit {
                        name,
                        value: refine_expr(value, refinement)?,
                    })
                })
                .collect::<Result<_, _>>()?,
        },
        ExprKind::MapLiteral { entries } => ExprKind::MapLiteral {
            entries: entries
                .into_iter()
                .map(|MapEntry { keys, value }| {
                    Ok(MapEntry {
                        keys,
                        value: refine_expr(value, refinement)?,
                    })
                })
                .collect::<Result<_, _>>()?,
        },
        ExprKind::ForComp { bindings, body } => ExprKind::ForComp {
            bindings,
            body: refine_box(body, refinement)?,
        },
        ExprKind::IndexAccess { expr, args } => {
            let expr = refine_box(expr, refinement)?;
            ExprKind::IndexAccess {
                expr,
                args: args.try_map(|arg| {
                    Ok(match arg {
                        IndexArg::Variant(variant) => IndexArg::Variant(variant),
                        IndexArg::Var(local) => IndexArg::Var(local),
                        IndexArg::Expr(expr) => IndexArg::Expr(refine_box(expr, refinement)?),
                    })
                })?,
            }
        }
        ExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => ExprKind::Scan {
            source: refine_box(source, refinement)?,
            init: refine_box(init, refinement)?,
            acc,
            val,
            body: refine_box(body, refinement)?,
        },
        ExprKind::Unfold {
            recurrence,
            init,
            body,
        } => ExprKind::Unfold {
            recurrence,
            init: refine_box(init, refinement)?,
            body: refine_box(body, refinement)?,
        },
        ExprKind::KeyForm {
            kind,
            axis,
            axis_span,
            arg,
        } => ExprKind::KeyForm {
            kind,
            axis,
            axis_span,
            arg: refine_box(arg, refinement)?,
        },
        ExprKind::Match { scrutinee, arms } => ExprKind::Match {
            scrutinee: refine_box(scrutinee, refinement)?,
            arms: arms
                .into_iter()
                .map(
                    |MatchArm {
                         pattern,
                         body,
                         span,
                     }| {
                        Ok(MatchArm {
                            pattern,
                            body: refine_expr(body, refinement)?,
                            span,
                        })
                    },
                )
                .collect::<Result<_, _>>()?,
        },
        ExprKind::DagCall {
            target,
            args,
            static_bindings,
            output,
        } => ExprKind::DagCall {
            target,
            args: args
                .into_iter()
                .map(|ParamBinding { target, value }| {
                    Ok(ParamBinding {
                        target,
                        value: refine_expr(value, refinement)?,
                    })
                })
                .collect::<Result<_, _>>()?,
            static_bindings,
            output,
        },
    })
}
