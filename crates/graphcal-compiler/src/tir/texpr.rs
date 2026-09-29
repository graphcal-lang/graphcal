//! Typed expression trees: what checking proved about a body, as one tree.
//!
//! A [`TExpr`] is the checked form of one HIR value expression. Every node
//! carries its [`CheckedType`] and owns its typed children, and the facts that
//! only some expression forms have live on those forms' variants: a
//! constructor carries its [`ConstructorApplication`], a match arm its
//! [`ConstructorMatch`] target, a statically checked position its
//! [`StaticPosition`] proof. Contextual literals (plot strings, datetime and
//! timezone literals) are never values; they are [`TContextual`] leaves that
//! appear only where a construct accepts them ([`TArg`], [`TBody`]).
//!
//! Inference emits these trees through its observation sink while it checks
//! each body (see `assembly`). Until the evaluator consumes them, the
//! retained expression facts are the executable record and publication
//! verifies that both agree (`fact_agreement`).

mod assembly;
pub(crate) mod fact_agreement;
mod model;
mod typed_bodies;

#[cfg(test)]
mod tests;

pub(crate) use assembly::{AssemblyError, NodeFacts, PendingNodes};
pub use model::{
    ContextualLiteral, StaticPosition, TArg, TBody, TConstRef, TContextual, TExpr, TExprKind,
    TFieldInit, TIndexArg, TMapEntry, TMatchArm, TMatchPattern, TNodeRef, TParamBinding,
    visit_tnodes,
};
pub use typed_bodies::{TypedBodies, TypedBodiesError};

#[cfg(doc)]
use crate::registry::checked_type::CheckedType;
#[cfg(doc)]
use crate::tir::expression_facts::{ConstructorApplication, ConstructorMatch};
