//! High-level intermediate representation (HIR) boundary.
//!
//! HIR is the first compiler layer whose reference positions are intended to be
//! **truly resolved**. The current `syntax::phase::Resolved` AST is only a
//! locally normalized syntax tree: it has no `UnresolvedRef` nodes, but many
//! module-sensitive references still carry source paths or leaf-only display names.
//!
//! The HIR boundary is deliberately separate from the syntax AST so the syntax
//! phase can stay path-first and honest, while HIR can require stronger
//! invariants:
//!
//! - definition sites are owned by canonical [`DagId`](crate::dag_id::DagId)
//!   identities;
//! - module-level reference sites use [`ResolvedName`](crate::syntax::names::ResolvedName)
//!   or [`ResolvedIndexVariant`](crate::syntax::index_name::ResolvedIndexVariant);
//! - lexical references, such as locals and generic parameters, use dedicated
//!   lexical IDs instead of module names;
//! - built-ins use explicit variants or dedicated typed wrappers, not ad-hoc
//!   string dispatch;
//! - no HIR reference field stores a dotted source alias string.
//!
//! This module defines and lowers the semantic boundary for type expressions,
//! value expressions, and assertion bodies. Module-aware TIR and runtime
//! evaluation consume this HIR slice for declaration/assertion semantics rather
//! than re-resolving source-shaped syntax AST references.

pub(crate) mod diagnostics;
pub mod expr;
pub mod lower;
pub mod types;

pub use expr::{
    AssertBody, ConstRef, Expr, ExprDependencies, ExprKind, ExprLowerError, ExprLoweringContext,
    ExternFnRef, FunctionRef, LocalDef, LocalEnv, LocalId, collect_expr_dependencies,
    find_extern_call, has_ref_outside_unfold, lower_assert_body, lower_assert_body_tolerant,
    lower_expr, lower_expr_tolerant,
};
pub use lower::{
    GenericParamBinding, GenericScope, HirLowerError, PreludeTypeScope, TypeLoweringContext,
    lower_generic_params, lower_nat_expr, lower_type_expr,
};
pub use types::{
    BuiltinType, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericParamDef, GenericParamId,
    GenericParamOwner, IndexRef, NatExpr, TypeExpr, TypeExprKind,
};
