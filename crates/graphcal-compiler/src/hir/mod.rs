//! High-level intermediate representation (HIR) boundary.
//!
//! HIR is the first compiler layer whose reference positions are canonical.
//! The desugared syntax tree retains source paths and ambiguous reference
//! shapes. Project elaboration classifies module aliases, and HIR lowering turns
//! every surviving reference into its canonical semantic form.
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

pub use diagnostics::expr_lower_error_to_graphcal;
pub use expr::{
    AssertBody, ConstRef, Expr, ExprDependencies, ExprKind, ExprLowerError, ExprLoweringContext,
    ExternFnRef, FunctionRef, LocalDef, LocalEnv, LocalId, ResolvedUnitExpr, ResolvedUnitExprItem,
    ResolvedUnitRef, collect_expr_dependencies, find_dag_call, lower_expr_tolerant,
};
pub(crate) use expr::{find_extern_call, lower_assert_body, lower_expr, visit_expr};
pub use lower::{GenericParamBinding, GenericScope, HirLowerError, PreludeTypeScope};
pub(crate) use lower::{TypeLoweringContext, lower_type_expr};
pub use types::{
    BuiltinType, DimArg, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericArg,
    GenericParamId, GenericParamOwner, IndexRef, NatExpr, TypeExpr, TypeExprKind,
};
