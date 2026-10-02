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
///
/// A registry may extend a shared parent registry: it then also resolves every
/// id the parent issued, while ids it issues itself resolve only here.
#[derive(Debug)]
pub struct SourceRegistry {
    identity: u64,
    parent: Option<Arc<Self>>,
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
            parent: None,
            sources: Vec::new(),
        }
    }

    /// Create an empty registry with a fresh identity that also resolves every
    /// id issued by `parent`, for sources that exist only next to an already
    /// shared registry (such as an external value bound to a prepared project).
    #[must_use]
    pub fn extending(parent: Arc<Self>) -> Self {
        Self {
            parent: Some(parent),
            ..Self::new()
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
        if id.registry() == self.identity {
            return self.sources.get(id.index()).ok_or(ForeignSourceId);
        }
        self.parent
            .as_deref()
            .map_or(Err(ForeignSourceId), |parent| parent.named_source(id))
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

    #[test]
    fn an_extending_registry_resolves_its_parent_ids_but_not_vice_versa() {
        let mut parent = SourceRegistry::new();
        let inherited = parent.register("main.gcl", Arc::new("main".to_string()));
        let parent = Arc::new(parent);
        let mut child = SourceRegistry::extending(Arc::clone(&parent));
        let own = child.register("<--param x>", Arc::new("1.0 m".to_string()));

        assert_eq!(
            child.named_source(inherited).expect("parent id").name(),
            "main.gcl"
        );
        assert_eq!(
            child.named_source(own).expect("own id").name(),
            "<--param x>"
        );
        assert!(matches!(parent.named_source(own), Err(ForeignSourceId)));
        let unrelated = SourceRegistry::new();
        let mut sibling = SourceRegistry::extending(parent);
        let sibling_id = sibling.register("<other>", Arc::new(String::new()));
        assert!(matches!(
            child.named_source(sibling_id),
            Err(ForeignSourceId)
        ));
        assert!(matches!(unrelated.named_source(own), Err(ForeignSourceId)));
    }
}
