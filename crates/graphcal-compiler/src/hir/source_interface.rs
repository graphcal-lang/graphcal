//! Direct source-declaration provenance retained at the HIR boundary.

use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;

/// One runtime-interface-relevant declaration authored directly in a DAG.
///
/// Include elaboration can merge additional declarations into a [`HirDag`](crate::ir::lower::HirDag).
/// This record deliberately excludes those merged declarations, preserving the
/// distinction between an entry DAG's own ports and its internal instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDeclaration {
    Parameter { name: DeclName, span: Span },
    Node { name: DeclName, span: Span },
    Index { name: IndexName, span: Span },
}
