//! Expression identity within one immutable lowering revision, independent of source coordinates.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug)]
struct Revision;

/// One occurrence in a body revision. Holding the revision prevents address reuse.
#[derive(Debug, Clone)]
pub struct ExprId {
    revision: Arc<Revision>,
    ordinal: u64,
}

impl PartialEq for ExprId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.revision, &other.revision) && self.ordinal == other.ordinal
    }
}
impl Eq for ExprId {}
impl Hash for ExprId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.revision), state);
        self.ordinal.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("HIR expression has no assigned body identity")]
pub struct UnassignedExprId;

/// Construction-only allocator. Each allocator represents a fresh body revision.
pub(crate) struct ExprIds {
    revision: Arc<Revision>,
    next: u64,
}

impl Default for ExprIds {
    fn default() -> Self {
        Self {
            revision: Arc::new(Revision),
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
            revision: Arc::clone(&self.revision),
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
            revision: Arc::new(Revision),
            next: u64::MAX,
        };
        assert!(ids.allocate().is_err());
        assert_eq!(ids.next, u64::MAX);
    }
}
