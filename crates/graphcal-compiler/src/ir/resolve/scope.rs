use std::collections::HashSet;
use std::sync::Arc;

use miette::NamedSource;

use crate::registry::error::GraphcalError;
use crate::syntax::ast::{Expr, ExprKind, FnBody, FnDecl, IndexArg, MapEntry, MatchArm};
use crate::syntax::visitor::ExprVisitor;

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

/// Visitor that checks for variant literals in non-rebindable contexts.
///
/// Variant literals (e.g., `Phase::Design`) cannot appear in node, const, assert,
/// fn, or plot expressions because they prevent the file from being reused as a
/// library with a different index definition. Users must extract variant-specific
/// logic into `param` declarations instead.
///
/// This checker catches variant literals in four forms:
/// 1. `ExprKind::VariantLiteral` — standalone variant references
/// 2. `IndexArg::Variant` in `IndexAccess` — e.g., `@cost[Phase::Design]`
/// 3. `MapEntryKey` in map/table literals — e.g., `{ Phase::Design: 1.0 }`
/// 4. `MatchPattern.qualified_index` in match arms — e.g., `Phase::Design => ...`
struct VariantLiteralChecker<'a> {
    context: &'a str,
    src: &'a NamedSource<Arc<String>>,
}

impl VariantLiteralChecker<'_> {
    fn make_error(
        &self,
        index: &impl std::fmt::Display,
        variant: &impl std::fmt::Display,
        span: crate::syntax::span::Span,
    ) -> GraphcalError {
        GraphcalError::VariantLiteralInNonRebindable {
            index: index.to_string(),
            variant: variant.to_string(),
            context: self.context.to_string(),
            src: self.src.clone(),
            span: span.into(),
        }
    }
}

impl ExprVisitor for VariantLiteralChecker<'_> {
    type Error = GraphcalError;

    fn visit_leaf(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::VariantLiteral { index, variant } = &expr.kind {
            return Err(self.make_error(&index.value, &variant.value, expr.span));
        }
        Ok(())
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        // Check IndexAccess args for variant literals before recursing into inner expr.
        if let ExprKind::IndexAccess { args, .. } = &expr.kind {
            for arg in args {
                if let IndexArg::Variant { index, variant } = arg {
                    return Err(self.make_error(&index.value, &variant.value, expr.span));
                }
            }
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(&mut self, _expr: &Expr, entries: &[MapEntry]) -> Result<(), Self::Error> {
        for entry in entries {
            if let Some(key) = entry.keys.first() {
                return Err(self.make_error(&key.index.value, &key.variant.value, key.index.span));
            }
            self.visit_expr(&entry.value)?;
        }
        Ok(())
    }

    fn visit_match(
        &mut self,
        _expr: &Expr,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<(), Self::Error> {
        self.visit_expr(scrutinee)?;
        for arm in arms {
            if let Some(qi) = &arm.pattern.qualified_index {
                return Err(self.make_error(
                    &qi.value,
                    &arm.pattern.variant_name.value,
                    arm.pattern.span,
                ));
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }
}

/// Check that an expression contains no variant literals (for non-rebindable contexts).
///
/// Variant literals are banned in node, const, assert, fn, and plot expressions
/// to ensure files can be reused as libraries with different index definitions.
pub(super) fn check_no_variant_literals(
    expr: &Expr,
    context: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut checker = VariantLiteralChecker { context, src };
    checker.visit_expr(expr)
}
