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
mod call_targets;
mod checked_bodies;
pub(crate) mod map;
mod model;
mod nominal;
pub mod operators;

#[cfg(test)]
mod tests;

pub(crate) use assembly::{AssemblyError, NodeFacts, PendingNodes};
pub use call_targets::{CallSlot, CallTargets};
pub use checked_bodies::{
    CheckedBodies, CheckedBody, DischargeError, ExecutableBodyError, TypedBodiesError,
};
pub(crate) use checked_bodies::{ClaimedRoots, claim_roots};
pub use model::{
    ContextualLiteral, CoordinateSearch, DatetimeLiteral, ExternArgKind, StaticPosition, TArg,
    TBody, TConstRef, TConstructorArm, TContextual, TExpr, TExprKind, TExternArg, TFieldInit,
    TIndexArg, TKeyForm, TLabelArm, TMapEntry, TMatchArms, TNodeRef, TParamBinding, visit_tnodes,
};
pub use nominal::{ConstructorApplication, ConstructorMatch, NominalObservation};

#[cfg(doc)]
use crate::semantic::checked_type::CheckedType;
