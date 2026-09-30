//! The position of a DAG in the registry of the program that runs it.

/// The position of one DAG in the
/// [`CheckedDagRegistry`](super::checked::CheckedDagRegistry) of its program.
///
/// Created only by the registry, so a position always names a DAG of the
/// registry that handed it out, and a program's per-DAG data can be indexed
/// by it. A body shared by several programs may have a different position in
/// each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DagPosition(usize);

impl DagPosition {
    /// The position of every registry's root DAG.
    pub(super) const ROOT: Self = Self(0);

    /// The position at `index`, for the registry that assigns it.
    pub(super) const fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position's index; the registry visits its DAGs in this order.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}
