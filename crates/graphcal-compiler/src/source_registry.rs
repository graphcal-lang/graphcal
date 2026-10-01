//! Shell-side registry mapping [`SourceId`]s to renderable named sources.
//!
//! Loaders register each source text once and hand the returned [`SourceId`]
//! to the functional core. Diagnostics produced by the core carry only that id;
//! the shell resolves it here when rendering.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use miette::NamedSource;

use crate::source_id::SourceId;

/// Registry identities are process-unique so ids from one registry never
/// resolve in another.
static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(0);

/// Owner of the named sources that diagnostics refer to by [`SourceId`].
#[derive(Debug)]
pub struct SourceRegistry {
    identity: u64,
    sources: Vec<NamedSource<Arc<String>>>,
}

/// A [`SourceId`] was resolved against a registry that did not issue it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("source id was issued by a different source registry")]
pub struct ForeignSourceId;

impl SourceRegistry {
    /// Create an empty registry with a fresh identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            identity: NEXT_REGISTRY.fetch_add(1, Ordering::Relaxed),
            sources: Vec::new(),
        }
    }

    /// Register a source text under its display name.
    pub fn register(&mut self, name: impl AsRef<str>, text: Arc<String>) -> SourceId {
        let id = SourceId::new(self.identity, self.sources.len(), text.len());
        self.sources.push(NamedSource::new(name, text));
        id
    }

    /// The named source registered as `id`.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignSourceId`] if `id` was issued by another registry.
    pub fn named_source(&self, id: SourceId) -> Result<&NamedSource<Arc<String>>, ForeignSourceId> {
        if id.registry() != self.identity {
            return Err(ForeignSourceId);
        }
        self.sources.get(id.index()).ok_or(ForeignSourceId)
    }
}

impl SourceRegistry {
    /// The named source registered as `id`, for rendering; an id this
    /// registry did not issue renders against an empty, explicitly unknown
    /// source instead of an unrelated one.
    #[must_use]
    pub fn renderable(&self, id: SourceId) -> NamedSource<Arc<String>> {
        self.named_source(id).map_or_else(
            |ForeignSourceId| NamedSource::new("<unknown source>", Arc::new(String::new())),
            Clone::clone,
        )
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use miette::SourceCode;

    use super::*;

    #[test]
    fn registered_sources_resolve_to_their_name_and_text() {
        let mut registry = SourceRegistry::new();
        let first = registry.register("a.gcl", Arc::new("first".to_string()));
        let second = registry.register("b.gcl", Arc::new("second".to_string()));

        let resolved = registry.named_source(second).expect("own id");
        assert_eq!(resolved.name(), "b.gcl");
        let contents = resolved
            .read_span(&(0, 6).into(), 0, 0)
            .expect("span within source");
        assert_eq!(contents.data(), b"second");
        assert_eq!(
            registry.named_source(first).expect("own id").name(),
            "a.gcl"
        );
    }

    #[test]
    fn ids_from_another_registry_are_rejected() {
        let mut issuing = SourceRegistry::new();
        let mut other = SourceRegistry::new();
        let foreign = issuing.register("a.gcl", Arc::new(String::new()));
        other.register("b.gcl", Arc::new(String::new()));

        assert!(matches!(other.named_source(foreign), Err(ForeignSourceId)));
    }
}
