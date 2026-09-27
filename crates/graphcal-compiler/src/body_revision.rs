//! Identity of a checked semantic-body revision, separate from its reusable source expressions.

use crate::fresh_identity::FreshIdentity;

/// Equal DAG names or shared source expressions do not imply equal checked revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyRevision(FreshIdentity);

impl BodyRevision {
    pub(crate) fn fresh() -> Self {
        Self(FreshIdentity::fresh())
    }
}
