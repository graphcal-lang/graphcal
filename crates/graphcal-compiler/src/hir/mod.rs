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
//! - module-level reference sites use [`ResolvedName`](crate::resolved_name::ResolvedName)
//!   or [`ResolvedIndexVariant`](crate::resolved_name::ResolvedIndexVariant);
//! - lexical references, such as locals and generic parameters, use dedicated
//!   lexical IDs instead of module names;
//! - built-ins use explicit variants or dedicated typed wrappers, not ad-hoc
//!   string dispatch;
//! - no HIR reference field stores a dotted source alias string.
//!
//! This module defines and lowers the semantic boundary for nominal type
//! definitions, type expressions, value expressions, and assertion bodies.
//! Module-aware TIR and runtime evaluation consume these canonical definitions
//! rather than re-resolving source-shaped syntax AST references.

pub mod closed_expr;
pub mod const_expr;
pub(crate) mod const_lower;
pub(crate) mod diagnostics;
pub mod expr;
pub(crate) mod expr_lower;
pub mod lower;
pub mod node_definition;
pub mod nominal;
pub(crate) mod nominal_lower;
pub mod source_interface;
pub mod type_annotation;
pub mod types;

pub use diagnostics::expr_lower_error_to_graphcal;
pub use expr::{
    AssertBody, CheckedAssertBody, CheckedExpr, Completeness, ConstRef, Draft, Expr,
    ExprDependencies, ExprKind, ExternFnRef, FunctionRef, LocalDecl, LocalDef, LocalEnv, LocalId,
    LocalUnit, ResolvedUnitExpr, ResolvedUnitExprItem, ResolvedUnitRef, Strict,
    UnappliedFunctionRef, collect_expr_dependencies, find_dag_call,
};
pub(crate) use expr::{find_extern_call, visit_expr};
pub use expr_lower::context::ExprLoweringContext;
pub use expr_lower::context::{BindingOverlay, FrozenBindings};
pub use expr_lower::error::ExprLowerError;
pub use expr_lower::lower::{lower_expr_draft, lower_expr_tolerant};
pub use expr_lower::tolerant::{LoweringFailure, Tolerant};
pub use lower::{
    GenericApplicationTarget, GenericArgArity, GenericParamBinding, GenericScope, HirLowerError,
    ModuleScope, TypePathSlot,
};
pub use nominal::{
    NominalConstructor, NominalField, NominalGenericParam, NominalTypeDef, NominalTypeError,
    NominalTypeKind, NominalTypeRegistry,
};
pub use source_interface::{SourceDeclaration, StaticPort, StaticPortIdentity};
pub use type_annotation::{DomainBound, TypeAnnotation};
pub use types::{
    BuiltinType, DeclType, DimArg, DimExpr, DimExprItem, DimTermRef, DimTermTarget, GenericArg,
    GenericParamId, GenericParamOwner, IndexRef, NatExpr, ValueType, ValueTypeKind,
};
