//! A declaration's resolved type together with its domain bounds.

use crate::hir::expr::CheckedExpr;
use crate::hir::types::DeclType;
use crate::syntax::ast::DomainBoundKind;
use crate::syntax::span::Span;

/// A canonically resolved declaration type and its HIR domain bounds.
#[derive(Debug, Clone)]
pub struct TypeAnnotation {
    pub decl_type: DeclType,
    pub domain_bounds: Vec<DomainBound>,
    pub span: Span,
}

/// One declaration domain bound lowered to HIR at the same boundary as its type.
#[derive(Debug, Clone)]
pub struct DomainBound {
    pub kind: DomainBoundKind,
    pub value: CheckedExpr,
    pub span: Span,
}
