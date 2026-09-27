//! Opaque identity of one source text known to a diagnostic source registry.
//!
//! Core diagnostics refer to their source through a [`SourceId`] instead of
//! embedding the file name and full text. Only the shell-side
//! [`SourceRegistry`](crate::source_registry::SourceRegistry) issues ids and
//! maps them back to renderable sources, so the functional core never carries
//! presentation data.

/// Handle to one source text registered in a
/// [`SourceRegistry`](crate::source_registry::SourceRegistry).
///
/// The id remembers which registry issued it, so resolving it against a
/// different registry is detected instead of silently selecting an unrelated
/// source at the same position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId {
    registry: u64,
    index: usize,
}

impl SourceId {
    /// Only source registries mint ids.
    pub(crate) const fn new(registry: u64, index: usize) -> Self {
        Self { registry, index }
    }

    /// Identity of the registry that issued this id.
    pub(crate) const fn registry(self) -> u64 {
        self.registry
    }

    /// Position of the source within its issuing registry.
    pub(crate) const fn index(self) -> usize {
        self.index
    }
}
