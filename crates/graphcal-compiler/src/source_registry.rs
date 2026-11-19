//! Shell-side registry mapping [`SourceId`]s to renderable named sources.
//!
//! Loaders register each source text once and hand the returned [`SourceId`]
//! to the functional core. Diagnostics produced by the core carry only that id;
//! the shell resolves it here when rendering, to a [`RegisteredSource`] that
//! keeps the text's typed [`SourceOrigin`] next to its display name, so a
//! consumer that routes a diagnostic to a file (an editor) never has to parse
//! the display name back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use miette::{MietteError, NamedSource, SourceCode, SourceSpan, SpanContents};

use crate::source_id::SourceId;

/// Registry identities are process-unique so ids from one registry never
/// resolve in another.
static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(0);

/// Where a registered source text comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceOrigin {
    /// A file the project loader read, at its canonical path (a virtual path
    /// for an in-memory filesystem).
    File(PathBuf),
    /// A text that is no file of the project: an in-memory editor buffer, an
    /// externally supplied value, or a synthetic source.
    Detached,
}

impl SourceOrigin {
    /// The canonical path of a file source; `None` for a detached text.
    #[must_use]
    pub fn file_path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Detached => None,
        }
    }
}

/// One source text with its typed origin and its display name, as a rendered
/// diagnostic carries it.
///
/// The display name is for people only (it may be `pkg:/abs/path.gcl` or
/// `<--param x>`); [`Self::origin`] says which file, if any, the text is.
///
/// Shared, so every diagnostic about one source refers to one allocation and
/// an error carrying it stays small.
#[derive(Clone, Debug)]
pub struct RegisteredSource(Arc<RegisteredText>);

#[derive(Debug)]
struct RegisteredText {
    origin: SourceOrigin,
    named: NamedSource<Arc<String>>,
}

impl RegisteredSource {
    fn new(origin: SourceOrigin, name: impl AsRef<str>, text: Arc<String>) -> Self {
        Self(Arc::new(RegisteredText {
            origin,
            named: NamedSource::new(name, text),
        }))
    }

    /// A text that is no project file, under the display name `name`.
    #[must_use]
    pub fn detached(name: impl AsRef<str>, text: Arc<String>) -> Self {
        Self::new(SourceOrigin::Detached, name, text)
    }

    /// Where the text comes from.
    #[must_use]
    pub fn origin(&self) -> &SourceOrigin {
        &self.0.origin
    }

    /// The display name diagnostics render the text under.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.named.name()
    }

    /// The source text the diagnostic's spans index into.
    #[must_use]
    pub fn text(&self) -> &Arc<String> {
        self.0.named.inner()
    }
}

impl SourceCode for RegisteredSource {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        self.0
            .named
            .read_span(span, context_lines_before, context_lines_after)
    }
}

/// Owner of the named sources that diagnostics refer to by [`SourceId`].
///
/// A registry may extend a shared parent registry: it then also resolves every
/// id the parent issued, while ids it issues itself resolve only here.
#[derive(Debug)]
pub struct SourceRegistry {
    identity: u64,
    parent: Option<Arc<Self>>,
    sources: Vec<RegisteredSource>,
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

    /// Register a text that is no project file under its display name.
    pub fn register(&mut self, name: impl AsRef<str>, text: Arc<String>) -> SourceId {
        self.register_with_origin(SourceOrigin::Detached, name, text)
    }

    /// Register a text from `origin` under its display name.
    pub fn register_with_origin(
        &mut self,
        origin: SourceOrigin,
        name: impl AsRef<str>,
        text: Arc<String>,
    ) -> SourceId {
        let id = SourceId::new(self.identity, self.sources.len(), text.len());
        self.sources.push(RegisteredSource::new(origin, name, text));
        id
    }

    /// The source registered as `id`.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignSourceId`] if `id` was issued by another registry.
    pub fn source(&self, id: SourceId) -> Result<&RegisteredSource, ForeignSourceId> {
        if id.registry() == self.identity {
            return self.sources.get(id.index()).ok_or(ForeignSourceId);
        }
        self.parent
            .as_deref()
            .map_or(Err(ForeignSourceId), |parent| parent.source(id))
    }
}

impl SourceRegistry {
    /// The source registered as `id`, for rendering; an id this registry
    /// did not issue renders against an empty, explicitly unknown source
    /// instead of an unrelated one.
    #[must_use]
    pub fn renderable(&self, id: SourceId) -> RegisteredSource {
        self.source(id).map_or_else(
            |ForeignSourceId| {
                RegisteredSource::detached("<unknown source>", Arc::new(String::new()))
            },
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

        let resolved = registry.source(second).expect("own id");
        assert_eq!(resolved.name(), "b.gcl");
        let contents = resolved
            .read_span(&(0, 6).into(), 0, 0)
            .expect("span within source");
        assert_eq!(contents.data(), b"second");
        assert_eq!(registry.source(first).expect("own id").name(), "a.gcl");
    }

    #[test]
    fn ids_from_another_registry_are_rejected() {
        let mut issuing = SourceRegistry::new();
        let mut other = SourceRegistry::new();
        let foreign = issuing.register("a.gcl", Arc::new(String::new()));
        other.register("b.gcl", Arc::new(String::new()));

        assert!(matches!(other.source(foreign), Err(ForeignSourceId)));
    }

    #[test]
    fn an_extending_registry_resolves_its_parent_ids_but_not_vice_versa() {
        let mut parent = SourceRegistry::new();
        let inherited = parent.register("main.gcl", Arc::new("main".to_string()));
        let parent = Arc::new(parent);
        let mut child = SourceRegistry::extending(Arc::clone(&parent));
        let own = child.register("<--param x>", Arc::new("1.0 m".to_string()));

        assert_eq!(
            child.source(inherited).expect("parent id").name(),
            "main.gcl"
        );
        assert_eq!(child.source(own).expect("own id").name(), "<--param x>");
        assert!(matches!(parent.source(own), Err(ForeignSourceId)));
        let unrelated = SourceRegistry::new();
        let mut sibling = SourceRegistry::extending(parent);
        let sibling_id = sibling.register("<other>", Arc::new(String::new()));
        assert!(matches!(child.source(sibling_id), Err(ForeignSourceId)));
        assert!(matches!(unrelated.source(own), Err(ForeignSourceId)));
    }
}
