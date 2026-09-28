//! Structural traversal of HIR expression trees and queries built on it.

use crate::resolved_name::ResolvedDeclName;
use std::collections::BTreeSet;

use crate::dag_id::DagId;
use crate::syntax::span::{Span, Spanned};

use super::completeness::Completeness;
use super::model::{ConstRef, Expr, ExprKind, ExternFnRef, FunctionRef, IndexArg};

/// Canonical declaration dependencies observed in one HIR expression tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExprDependencies {
    /// Runtime graph dependencies reached through `@name` references.
    pub graph_refs: BTreeSet<ResolvedDeclName>,
    /// Compile-time const dependencies reached through const-like value refs.
    pub(crate) const_refs: BTreeSet<ResolvedDeclName>,
}

/// Collect canonical declaration dependencies from an already-lowered HIR expression.
#[must_use]
pub fn collect_expr_dependencies<C: Completeness>(expr: &Expr<C>) -> ExprDependencies {
    let mut deps = ExprDependencies::default();
    collect_expr_dependencies_into(expr, &mut deps);
    deps
}

fn collect_expr_dependencies_into<C: Completeness>(expr: &Expr<C>, deps: &mut ExprDependencies) {
    visit_expr(expr, &mut |node| match node.kind() {
        ExprKind::GraphRef(target) => {
            deps.graph_refs.insert(target.value.clone());
        }
        ExprKind::ConstRef(Spanned {
            value: ConstRef::Decl(target),
            ..
        }) => {
            deps.const_refs.insert(target.clone());
        }
        _ => {}
    });
}

/// One exhaustive child inventory serves both inspection and construction-time walks.
macro_rules! expression_children {
    ($kind:expr, $iter:ident, $error_children:ident, $visitor:ident, [$($borrow:tt)*]) => {
        match $kind {
            ExprKind::Error(error) => C::$error_children(error).$iter().for_each(&mut *$visitor),
            ExprKind::Number(_) | ExprKind::Integer(_) | ExprKind::Bool(_)
            | ExprKind::StringLiteral(_) | ExprKind::OffsetDateTimeLiteral(_)
            | ExprKind::CivilDateTimeLiteral(_) | ExprKind::ZonedDateTimeLiteral(_)
            | ExprKind::IanaTimeZoneLiteral(_) | ExprKind::TypeSystemRef(_)
            | ExprKind::GraphRef(_) | ExprKind::ConstRef(_) | ExprKind::LocalRef(_)
            | ExprKind::QuantityLiteral { .. } | ExprKind::VariantLiteral(_) => {},
            ExprKind::BinOp { lhs, rhs, .. } => { $visitor(lhs); $visitor(rhs); }
            ExprKind::UnaryOp { operand, .. } | ExprKind::Convert { expr: operand, .. }
            | ExprKind::DisplayTimezone { expr: operand, .. } | ExprKind::FieldAccess { expr: operand, .. } => $visitor(operand),
            ExprKind::FnCall { args, .. } => args.$iter().for_each(&mut *$visitor),
            ExprKind::If { condition, then_branch, else_branch } => { $visitor(condition); $visitor(then_branch); $visitor(else_branch); }
            ExprKind::ConstructorCall { fields, .. } => fields.$iter().for_each(|field| $visitor($($borrow)* field.value)),
            ExprKind::MapLiteral { entries } => entries.$iter().for_each(|entry| $visitor($($borrow)* entry.value)),
            ExprKind::ForComp { body, .. } => $visitor(body),
            ExprKind::IndexAccess { expr, args } => {
                $visitor(expr);
                args.$iter().for_each(|arg| match arg { IndexArg::Expr(expr) => $visitor(expr), IndexArg::Variant(_) | IndexArg::Var(_) => {} });
            }
            ExprKind::Scan { source, init, body, .. } => { $visitor(source); $visitor(init); $visitor(body); }
            ExprKind::Unfold { init, body, .. } => { $visitor(init); $visitor(body); }
            ExprKind::KeyForm { arg, .. } => $visitor(arg),
            ExprKind::Match { scrutinee, arms } => std::iter::once($($borrow)* **scrutinee)
                .chain(arms.$iter().map(|arm| $($borrow)* arm.body))
                .for_each(&mut *$visitor),
            ExprKind::DagCall { args, .. } => args.$iter().for_each(|binding| $visitor($($borrow)* binding.value)),
        }
    };
}

/// Visit immediate expression children, in structural order.
pub fn visit_expr_children<'a, C: Completeness>(
    expr: &'a Expr<C>,
    visitor: &mut dyn FnMut(&'a Expr<C>),
) {
    expression_children!(expr.kind(), iter, error_children, visitor, [&]);
}

pub(super) fn visit_expr_children_mut<C: Completeness>(
    expr: &mut Expr<C>,
    visitor: &mut impl FnMut(&mut Expr<C>),
) {
    expression_children!(&mut *expr.kind, iter_mut, error_children_mut, visitor, [&mut]);
}

/// Visit all expression occurrences in pre-order. This does not model evaluation order.
pub fn visit_expr<'a, C: Completeness>(expr: &'a Expr<C>, visitor: &mut dyn FnMut(&'a Expr<C>)) {
    crate::stack::with_stack_growth(|| {
        visitor(expr);
        visit_expr_children(expr, &mut |child| visit_expr(child, visitor));
    });
}

/// Visit descendants before their parent, so dependency facts can be composed
/// in one pass without cloning an intermediate occurrence-ID ordering.
pub fn visit_expr_postorder<'a, C: Completeness>(
    expr: &'a Expr<C>,
    visitor: &mut dyn FnMut(&'a Expr<C>),
) {
    crate::stack::with_stack_growth(|| {
        visit_expr_children(expr, &mut |child| visit_expr_postorder(child, visitor));
        visitor(expr);
    });
}

/// Return the first DAG call in an HIR expression, if any.
#[must_use]
pub fn find_dag_call<C: Completeness>(expr: &Expr<C>) -> Option<(DagId, Span)> {
    let mut found = None;
    visit_expr(expr, &mut |candidate| {
        if found.is_none()
            && let ExprKind::DagCall { target, .. } = candidate.kind()
        {
            found = Some((target.value.clone(), candidate.span));
        }
    });
    found
}

/// Find the first extern (plugin) function call in an expression tree.
///
/// Extern functions are runtime-provided; contexts that evaluate without a
/// host function registry (const expressions, domain bounds, dynamic unit
/// scales) use this to reject them with a spanned diagnostic.
#[must_use]
pub fn find_extern_call<C: Completeness>(expr: &Expr<C>) -> Option<(&ExternFnRef, Span)> {
    let mut found = None;
    visit_expr(expr, &mut |candidate| {
        if found.is_none()
            && let ExprKind::FnCall { callee, .. } = candidate.kind()
            && let FunctionRef::External(function) = &callee.value
        {
            found = Some((function, callee.span));
        }
    });
    found
}
