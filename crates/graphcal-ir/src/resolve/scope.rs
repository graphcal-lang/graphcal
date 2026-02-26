use std::collections::HashSet;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_registry::error::GraphcalError;
use graphcal_syntax::ast::{Expr, ExprKind, FnBody, FnDecl};

/// Check that an expression contains no `@` references (for const expressions).
pub(super) fn check_no_graph_refs(
    expr: &Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            Err(GraphcalError::GraphRefInConst {
                name: ident.value.clone(),
                src: src.clone(),
                span: expr.span.into(),
            })
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs(lhs, src)?;
            check_no_graph_refs(rhs, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs(operand, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                check_no_graph_refs(arg, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_graph_refs(condition, src)?;
            check_no_graph_refs(then_branch, src)?;
            check_no_graph_refs(else_branch, src)
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => check_no_graph_refs(inner, src),
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_graph_refs(&stmt.value, src)?;
            }
            check_no_graph_refs(expr, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_graph_refs(expr, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs(val, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                check_no_graph_refs(&entry.value, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_graph_refs(body, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_graph_refs(source, src)?;
            check_no_graph_refs(init, src)?;
            check_no_graph_refs(body, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_no_graph_refs(init, src)?;
            check_no_graph_refs(body, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_graph_refs(scrutinee, src)?;
            for arm in arms {
                check_no_graph_refs(&arm.body, src)?;
            }
            Ok(())
        }
    }
}

/// Check that a function body contains no `@` references (purity enforcement).
pub(super) fn check_no_graph_refs_in_fn(
    fn_decl: &FnDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let check = |expr: &Expr| -> Result<(), GraphcalError> {
        check_no_graph_refs_in_fn_expr(expr, fn_decl.name.value.as_str(), src)
    };
    match &fn_decl.body {
        FnBody::Short(expr) => check(expr),
        FnBody::Block { stmts, expr } => {
            for stmt in stmts {
                check(&stmt.value)?;
            }
            check(expr)
        }
    }
}

fn check_no_graph_refs_in_fn_expr(
    expr: &Expr,
    fn_name: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            Err(GraphcalError::GraphRefInFn {
                name: ident.value.clone(),
                src: src.clone(),
                span: expr.span.into(),
                help: format!("pass `{fn_name}` as a function parameter instead"),
            })
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs_in_fn_expr(lhs, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(rhs, fn_name, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs_in_fn_expr(operand, fn_name, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                check_no_graph_refs_in_fn_expr(arg, fn_name, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_graph_refs_in_fn_expr(condition, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(then_branch, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(else_branch, fn_name, src)
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => {
            check_no_graph_refs_in_fn_expr(inner, fn_name, src)
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_graph_refs_in_fn_expr(&stmt.value, fn_name, src)?;
            }
            check_no_graph_refs_in_fn_expr(expr, fn_name, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_graph_refs_in_fn_expr(expr, fn_name, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs_in_fn_expr(val, fn_name, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                check_no_graph_refs_in_fn_expr(&entry.value, fn_name, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_graph_refs_in_fn_expr(body, fn_name, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_graph_refs_in_fn_expr(source, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(init, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(body, fn_name, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_no_graph_refs_in_fn_expr(init, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(body, fn_name, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_graph_refs_in_fn_expr(scrutinee, fn_name, src)?;
            for arm in arms {
                check_no_graph_refs_in_fn_expr(&arm.body, fn_name, src)?;
            }
            Ok(())
        }
    }
}

/// Check that an expression does not reference any assert name via `@`.
///
/// Assert declarations are leaf nodes — they cannot be referenced by other declarations.
pub(super) fn check_no_assert_graph_refs(
    expr: &Expr,
    assert_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            if assert_names.contains(ident.value.as_str()) {
                return Err(GraphcalError::GraphRefToAssert {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            Ok(())
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_assert_graph_refs(lhs, assert_names, src)?;
            check_no_assert_graph_refs(rhs, assert_names, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_assert_graph_refs(operand, assert_names, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                check_no_assert_graph_refs(arg, assert_names, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_assert_graph_refs(condition, assert_names, src)?;
            check_no_assert_graph_refs(then_branch, assert_names, src)?;
            check_no_assert_graph_refs(else_branch, assert_names, src)
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => {
            check_no_assert_graph_refs(inner, assert_names, src)
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_assert_graph_refs(&stmt.value, assert_names, src)?;
            }
            check_no_assert_graph_refs(expr, assert_names, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_assert_graph_refs(expr, assert_names, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_assert_graph_refs(val, assert_names, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                check_no_assert_graph_refs(&entry.value, assert_names, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_assert_graph_refs(body, assert_names, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_assert_graph_refs(source, assert_names, src)?;
            check_no_assert_graph_refs(init, assert_names, src)?;
            check_no_assert_graph_refs(body, assert_names, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_no_assert_graph_refs(init, assert_names, src)?;
            check_no_assert_graph_refs(body, assert_names, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_assert_graph_refs(scrutinee, assert_names, src)?;
            for arm in arms {
                check_no_assert_graph_refs(&arm.body, assert_names, src)?;
            }
            Ok(())
        }
    }
}
