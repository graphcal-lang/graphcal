//! HIR expression/value reference types.
//!
//! This module is the expression-side counterpart to [`super::types`]. It
//! defines the HIR expression tree whose reference positions use canonical
//! module identities or lexical local IDs. Unit references retain their
//! structured source spelling only for diagnostics and display labels,
//! alongside a canonical resolved target; semantic lookup never uses that
//! spelling. `super::expr_lower` produces these trees from the desugared
//! syntax AST.
//!
//! - `model`: the node shapes and reference payloads.
//! - `visit`: structural traversal and the queries built on it.
//! - `checked`: finished bodies that carry occurrence identities.
//! - `local_env`: the evaluation-time environment keyed by [`LocalId`].

mod checked;
mod local_env;
mod model;
mod visit;

pub use checked::{CheckedAssertBody, CheckedExpr};
pub use local_env::LocalEnv;
pub use model::{
    AssertBody, ConstRef, DagCallIndexBinding, DagCallStaticBindings, Expr, ExprKind, ExternFnRef,
    FieldInit, ForBinding, ForBindingIndex, FunctionRef, IndexArg, IndexVariantRef, LocalDef,
    LocalId, MapEntry, MapEntryKey, MatchArm, MatchPattern, ParamBinding, PatternBinding,
    ResolvedUnitExpr, ResolvedUnitExprItem, ResolvedUnitRef, TypeSystemRef, UnappliedFunctionRef,
    UnfoldRecurrence,
};
pub(crate) use visit::find_extern_call;
pub use visit::{
    ExprDependencies, collect_expr_dependencies, find_dag_call, visit_expr, visit_expr_children,
    visit_expr_postorder,
};
