//! Plugin identities for extern-function imports.

use std::sync::Arc;

/// Opaque identity of an extern-function plugin: the verbatim path string
/// from `import plugin "…"`.
///
/// In Phase A of the plugin plan (issue #25) this string has no filesystem
/// semantics — it is resolved only against the embedder-injected host
/// function registry. Phase B gives it WASM-module semantics; keeping it a
/// dedicated type (rather than a bare `String`) fences that future meaning
/// behind one identity used consistently by the AST, TIR, and evaluator.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginPath(Arc<str>);

impl PluginPath {
    /// Wrap the verbatim path text from an `import plugin "…"` declaration.
    #[must_use]
    pub fn new(path: impl Into<Arc<str>>) -> Self {
        Self(path.into())
    }

    /// The verbatim path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PluginPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
