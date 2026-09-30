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
//! each body (see `assembly`). Publication classifies each root's tree as
//! executable or deferred ([`CheckedBodies`]); a semantic instance's trees
//! are its template's, specialized (see `map`). Evaluation runs executable
//! trees directly.

mod assembly;
mod checked_bodies;
pub(crate) mod map;
mod model;
mod nominal;
pub mod operators;

#[cfg(test)]
mod tests;

pub(crate) use assembly::{AssemblyError, NodeFacts, PendingNodes};
pub(crate) use checked_bodies::claim_roots;
pub use checked_bodies::{
    CheckedBodies, CheckedBody, DischargeError, ExecutableBodyError, TypedBodiesError,
};
pub use model::{
    ContextualLiteral, StaticPosition, TArg, TBody, TConstRef, TContextual, TExpr, TExprKind,
    TFieldInit, TIndexArg, TMapEntry, TMatchArm, TMatchPattern, TNodeRef, TParamBinding,
    visit_tnodes,
};
pub use nominal::{ConstructorApplication, ConstructorMatch, NominalObservation};

#[cfg(doc)]
use crate::registry::checked_type::CheckedType;
