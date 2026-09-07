//! Identity of a checked semantic-body revision, separate from its reusable source expressions.

use std::sync::Arc;

#[derive(Debug)]
struct Revision;

/// Equal DAG names or shared source expressions do not imply equal checked revisions.
#[derive(Debug, Clone)]
pub struct BodyRevision(Arc<Revision>);

impl BodyRevision {
    pub(crate) fn fresh() -> Self {
        Self(Arc::new(Revision))
    }
}

impl PartialEq for BodyRevision {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for BodyRevision {}
