use std::borrow::Borrow;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;

use crate::registry::error::GraphcalError;
use crate::syntax::ast::{Expr, ExprKind, IndexArg, MapEntry, MatchArm};
use crate::syntax::span::Span;
use crate::syntax::visitor::ExprVisitor;
use miette::NamedSource;

// ---------------------------------------------------------------------------
// Graph-reference checkers
// ---------------------------------------------------------------------------

/// Generic visitor that rejects `@`-references whose names appear in a
/// forbidden set. The `make_error` closure produces the appropriate
/// [`GraphcalError`] variant for the use-site.
struct ForbiddenGraphRefChecker<'a, S, F> {
    forbidden: &'a HashSet<S>,
    src: &'a NamedSource<Arc<String>>,
    make_error: F,
}

impl<S, F> ExprVisitor for ForbiddenGraphRefChecker<'_, S, F>
where
    S: Eq + Hash + Borrow<str>,
    F: Fn(&str, &NamedSource<Arc<String>>, Span) -> GraphcalError,
{
    type Error = GraphcalError;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind
            && self.forbidden.contains(ident.value.as_str())
        {
            return Err((self.make_error)(ident.value.as_str(), self.src, expr.span));
        }
        Ok(())
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { name: ident, .. } = &expr.kind
            && self.forbidden.contains(ident.value.as_str())
        {
            return Err((self.make_error)(ident.value.as_str(), self.src, expr.span));
        }
        Ok(())
    }
}

/// Check that an expression contains no runtime `@` references (for const expressions).
///
/// Const node expressions may use `@other_const_node` but must not reference
/// runtime params or nodes.
pub(super) fn check_no_runtime_graph_refs(
    expr: &Expr,
    runtime_names: &HashSet<&str>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut checker = ForbiddenGraphRefChecker {
        forbidden: runtime_names,
        src,
        make_error: |name: &str, src: &NamedSource<Arc<String>>, span: Span| {
            GraphcalError::GraphRefInConst {
                name: name.into(),
                src: src.clone(),
                span: span.into(),
            }
        },
    };
    checker.visit_expr(expr)
}

/// Check that an expression does not reference any assert name via `@`.
///
/// Assert declarations are leaf nodes — they cannot be referenced by other declarations.
pub(super) fn check_no_assert_graph_refs(
    expr: &Expr,
    assert_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut checker = ForbiddenGraphRefChecker {
        forbidden: assert_names,
        src,
        make_error: |name: &str, src: &NamedSource<Arc<String>>, span: Span| {
            GraphcalError::GraphRefToAssert {
                name: name.into(),
                src: src.clone(),
                span: span.into(),
            }
        },
    };
    checker.visit_expr(expr)
}

// ---------------------------------------------------------------------------
// Variant-literal checkers
// ---------------------------------------------------------------------------

/// Generic visitor that rejects variant literals according to a caller-provided
/// predicate. The `check` closure inspects the index/variant pair and returns
/// `Err(GraphcalError)` when the variant literal is forbidden.
struct VariantLiteralChecker<'a, F> {
    check: F,
    src: &'a NamedSource<Arc<String>>,
}

impl<F> ExprVisitor for VariantLiteralChecker<'_, F>
where
    F: Fn(&str, &str, Span, &NamedSource<Arc<String>>) -> Result<(), GraphcalError>,
{
    type Error = GraphcalError;

    fn visit_leaf(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::VariantLiteral { index, variant } = &expr.kind {
            (self.check)(
                index.value.as_ref(),
                variant.value.as_ref(),
                expr.span,
                self.src,
            )?;
        }
        Ok(())
    }

    fn visit_single_child(&mut self, expr: &Expr, inner: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::IndexAccess { args, .. } = &expr.kind {
            for arg in args {
                if let IndexArg::Variant { index, variant } = arg {
                    (self.check)(
                        index.value.as_ref(),
                        variant.value.as_ref(),
                        expr.span,
                        self.src,
                    )?;
                }
            }
        }
        self.visit_expr(inner)
    }

    fn visit_map_entries(&mut self, _expr: &Expr, entries: &[MapEntry]) -> Result<(), Self::Error> {
        for entry in entries {
            if let Some(key) = entry.keys.first() {
                (self.check)(
                    key.index.value.as_ref(),
                    key.variant.value.as_ref(),
                    key.index.span,
                    self.src,
                )?;
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
                (self.check)(
                    qi.value.as_ref(),
                    arm.pattern.variant_name.value.as_ref(),
                    arm.pattern.span,
                    self.src,
                )?;
            }
            self.visit_expr(&arm.body)?;
        }
        Ok(())
    }
}

/// Check that an expression contains no variant literals of
/// `pub(bind)` index declarations (V004 / A10(c)).
///
/// Bindable indexes can be overridden by importers, so the defining
/// library must abstract over them — any body of a non-bindable kind
/// that mentions a `pub(bind)` index's variant literal would orphan
/// under rebinding. Plain `pub` (fixed) indexes are not subject to
/// A10, so this check ignores them.
pub(super) fn check_no_pub_index_variant_literals(
    expr: &Expr,
    pub_bind_index_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if pub_bind_index_names.is_empty() {
        return Ok(());
    }
    let mut checker = VariantLiteralChecker {
        check: |index: &str,
                variant: &str,
                span: Span,
                src: &NamedSource<Arc<String>>|
         -> Result<(), GraphcalError> {
            if pub_bind_index_names.contains(index) {
                return Err(GraphcalError::PubIndexVariantLiteral {
                    index: index.to_string(),
                    variant: variant.to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        },
        src,
    };
    checker.visit_expr(expr)
}
