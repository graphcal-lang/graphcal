//! The typed trees of every checked root of one DAG.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::expression_id::ExprId;
use crate::hir::expr::Expr;
use crate::registry::checked_type::{Concreteness, Symbolic};

use super::assembly::PendingNodes;
use super::model::{TArg, TBody};

/// Why the typed trees of one checking pass cannot be published.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TypedBodiesError {
    #[error("checked root {0:?} has no typed tree")]
    MissingRoot(ExprId),
    #[error("typed node {0:?} belongs to no checked root")]
    Unclaimed(ExprId),
}

/// The typed tree of each checked root, keyed by the root's occurrence.
///
/// Clones share one immutable publication.
#[derive(Debug, Clone)]
pub struct TypedBodies<V: Concreteness = Symbolic> {
    roots: Arc<HashMap<ExprId, TBody<V>>>,
}

impl TypedBodies<Symbolic> {
    /// Claim the typed tree of every root; every recorded node must belong to one.
    pub(crate) fn publish(
        roots: &[&Expr],
        mut pending: PendingNodes,
    ) -> Result<Self, TypedBodiesError> {
        let mut published = HashMap::new();
        for root in roots {
            let id = root.id();
            if published.contains_key(id) {
                continue;
            }
            let body = match pending.take_root(id) {
                Some(TArg::Value(value)) => TBody::Value(value),
                Some(TArg::Contextual(literal)) => TBody::Contextual(literal),
                None => return Err(TypedBodiesError::MissingRoot(id.clone())),
            };
            published.insert(id.clone(), body);
        }
        if let Some(unclaimed) = pending.unclaimed() {
            return Err(TypedBodiesError::Unclaimed(unclaimed.clone()));
        }
        Ok(Self {
            roots: Arc::new(published),
        })
    }
}

impl<V: Concreteness> TypedBodies<V> {
    /// The typed tree of one checked root.
    #[must_use]
    pub fn get(&self, root: &ExprId) -> Option<&TBody<V>> {
        self.roots.get(root)
    }

    /// Every checked root's typed tree.
    pub fn roots(&self) -> impl Iterator<Item = (&ExprId, &TBody<V>)> {
        self.roots.iter()
    }
}
