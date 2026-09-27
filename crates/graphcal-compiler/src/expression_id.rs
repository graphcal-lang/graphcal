//! Expression identity within one immutable lowering revision, independent of source coordinates.

use thiserror::Error;

use crate::fresh_identity::FreshIdentity;

/// One occurrence in a body revision. Holding the revision prevents address reuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExprId {
    revision: FreshIdentity,
    // A position in an in-memory body, not a language Nat or serialized identity.
    ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("HIR expression has no assigned body identity")]
pub struct UnassignedExprId;

/// Construction-only allocator. Each allocator represents a fresh body revision.
pub(crate) struct ExprIds {
    revision: FreshIdentity,
    next: usize,
}

impl Default for ExprIds {
    fn default() -> Self {
        Self {
            revision: FreshIdentity::fresh(),
            next: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("expression identity space exhausted")]
pub struct ExprIdExhausted;

impl ExprIds {
    pub(crate) fn allocate(&mut self) -> Result<ExprId, ExprIdExhausted> {
        let next = self.next.checked_add(1).ok_or(ExprIdExhausted)?;
        let id = ExprId {
            revision: self.revision.clone(),
            ordinal: self.next,
        };
        self.next = next;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn equal_ordinals_cannot_cross_body_revisions_or_reuse_a_dropped_revision() {
        let original = ExprIds::default().allocate().unwrap();
        let mut other = ExprIds::default();
        let first = other.allocate().unwrap();
        let second = other.allocate().unwrap();
        assert_ne!(original, first);
        assert_ne!(first, second);
        assert_eq!(original, original.clone());
        assert_eq!(
            HashSet::from([original.clone(), original, first, second]).len(),
            3
        );
    }

    #[test]
    fn exhausted_allocator_does_not_publish_or_wrap() {
        let mut ids = ExprIds {
            revision: FreshIdentity::fresh(),
            next: usize::MAX,
        };
        assert!(ids.allocate().is_err());
        assert_eq!(ids.next, usize::MAX);
    }
}
