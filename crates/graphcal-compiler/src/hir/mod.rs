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
//! This module currently defines and lowers the first boundary slice: type-level
//! references. Module-aware TIR type resolution consumes this HIR slice before
//! converting back to its legacy runtime-facing type representation. Remaining
//! expression/value consumers should move to HIR rather than adding more
//! compatibility lookups to the syntax AST.

pub mod expr;
pub mod lower;
pub mod types;

pub use expr::{
    BuiltinConst, BuiltinFnName, ConstRef, Expr, ExprKind, ExprLowerError, ExprLoweringContext,
    FunctionRef, LocalDef, LocalId, lower_expr,
};
pub use lower::{
    GenericParamBinding, GenericScope, HirLowerError, PreludeTypeScope, TypeLoweringContext,
    lower_generic_params, lower_nat_expr, lower_type_expr,
};
pub use types::{
    BuiltinType, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericParamDef, GenericParamId,
    GenericParamOwner, IndexRef, NatExpr, TypeExpr, TypeExprKind,
};
