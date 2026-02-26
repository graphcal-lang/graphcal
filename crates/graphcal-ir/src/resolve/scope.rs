use std::collections::HashSet;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_registry::error::GraphcalError;
use graphcal_syntax::ast::{Expr, ExprKind, FnBody, FnDecl};
use graphcal_syntax::visitor::ExprVisitor;

/// Visitor that checks for graph references in const expressions.
struct NoGraphRefChecker<'a> {
    src: &'a NamedSource<Arc<String>>,
}

impl ExprVisitor for NoGraphRefChecker<'_> {
    type Error = GraphcalError;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            Err(GraphcalError::GraphRefInConst {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
            })
        } else {
            Ok(())
        }
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { name: ident, .. } = &expr.kind {
            Err(GraphcalError::GraphRefInConst {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
            })
        } else {
            Ok(())
        }
    }
}

/// Check that an expression contains no `@` references (for const expressions).
pub(super) fn check_no_graph_refs(
    expr: &Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut checker = NoGraphRefChecker { src };
    checker.visit_expr(expr)
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

/// Visitor that checks for graph references in function expressions.
struct NoGraphRefInFnChecker<'a> {
    fn_name: &'a str,
    src: &'a NamedSource<Arc<String>>,
}

impl ExprVisitor for NoGraphRefInFnChecker<'_> {
    type Error = GraphcalError;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind {
            Err(GraphcalError::GraphRefInFn {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
                help: format!("pass `{}` as a function parameter instead", self.fn_name),
            })
        } else {
            Ok(())
        }
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { name: ident, .. } = &expr.kind {
            Err(GraphcalError::GraphRefInFn {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
                help: format!("pass `{}` as a function parameter instead", self.fn_name),
            })
        } else {
            Ok(())
        }
    }
}

fn check_no_graph_refs_in_fn_expr(
    expr: &Expr,
    fn_name: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut checker = NoGraphRefInFnChecker { fn_name, src };
    checker.visit_expr(expr)
}

/// Visitor that checks for assert graph references.
struct NoAssertGraphRefChecker<'a> {
    assert_names: &'a HashSet<String>,
    src: &'a NamedSource<Arc<String>>,
}

impl ExprVisitor for NoAssertGraphRefChecker<'_> {
    type Error = GraphcalError;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind
            && self.assert_names.contains(ident.value.as_str())
        {
            return Err(GraphcalError::GraphRefToAssert {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
            });
        }
        Ok(())
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { name: ident, .. } = &expr.kind
            && self.assert_names.contains(ident.value.as_str())
        {
            return Err(GraphcalError::GraphRefToAssert {
                name: ident.value.clone(),
                src: self.src.clone(),
                span: expr.span.into(),
            });
        }
        Ok(())
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
    let mut checker = NoAssertGraphRefChecker { assert_names, src };
    checker.visit_expr(expr)
}
