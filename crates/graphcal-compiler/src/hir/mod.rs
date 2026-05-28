//! High-level intermediate representation (HIR) boundary.
//!
//! HIR is the first compiler layer whose reference positions are intended to be
//! **truly resolved**. The current `syntax::phase::Resolved` AST is only a
//! locally normalized syntax tree: it has no `UnresolvedRef` nodes, but many
//! module-sensitive references still carry source paths or legacy leaf names.
//!
//! The HIR boundary is deliberately separate from the syntax AST so the syntax
//! phase can stay path-first and honest, while HIR can require stronger
//! invariants:
//!
//! - definition sites are owned by canonical [`DagId`](crate::dag_id::DagId)
//!   identities;
//! - module-level reference sites use [`ResolvedName`](crate::syntax::names::ResolvedName)
//!   or [`ResolvedIndexVariant`](crate::syntax::names::ResolvedIndexVariant);
//! - lexical references, such as locals and generic parameters, use dedicated
//!   lexical IDs instead of module names;
//! - built-ins use explicit variants or dedicated typed wrappers, not ad-hoc
//!   string dispatch;
//! - no HIR reference field stores a dotted source alias string.
//!
//! This module currently defines the boundary types, starting with type-level
//! references because those are the positions already exercised by the
//! module-aware TIR bridge. Lowering from the syntax AST into these types is the
//! next migration step; downstream IR/TIR/eval consumers should then move to HIR
//! rather than adding more compatibility lookups to the syntax AST.

pub mod lower;
pub mod types;

pub use lower::{
    GenericParamBinding, GenericScope, HirLowerError, TypeLoweringContext, lower_generic_params,
    lower_type_expr,
};
pub use types::{
    BuiltinType, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericParamDef, GenericParamId,
    GenericParamOwner, IndexRef, NatExpr, TypeExpr, TypeExprKind,
};
