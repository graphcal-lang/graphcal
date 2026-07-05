//! Plugin identities for extern-function imports.

use std::sync::Arc;

/// Opaque identity of an extern-function plugin: the verbatim path string
/// from `import plugin "…"`.
///
/// The string is classified once by [`PluginPath::source_kind`] into the two
/// realizations a plugin can have — a WASM module file or an
/// embedder-provided host registry entry — and everything downstream
/// pattern-matches the typed kind. Keeping the identity a dedicated type
/// (rather than a bare `String`) fences that meaning behind one identity
/// used consistently by the AST, TIR, loader, and evaluator.
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

    /// Classify how this plugin identity is realized.
    ///
    /// Paths ending in `.wasm` name a WebAssembly module file; every other
    /// spelling is an opaque identity looked up in the embedder-injected
    /// host function registry (the Phase A semantics, kept for
    /// embedder-native plugins such as the built-in `graphcal:demo`).
    #[must_use]
    pub fn source_kind(&self) -> PluginSourceKind {
        if self.0.ends_with(".wasm") {
            PluginSourceKind::WasmModule
        } else {
            PluginSourceKind::HostRegistry
        }
    }
}

/// How a [`PluginPath`] is realized at load time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginSourceKind {
    /// The path names a vendored WebAssembly module file, resolved relative
    /// to the owning package root and executed by the WASM plugin host
    /// (Phase B of #25).
    WasmModule,
    /// The path is an opaque identity provided natively by the embedder
    /// through its host function registry.
    HostRegistry,
}

impl std::fmt::Display for PluginPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Canonical identity of one extern function: the plugin it belongs to plus
/// its leaf name. Host function registries and resolved signature tables key
/// on this.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExternFnKey {
    /// The owning plugin.
    pub plugin: PluginPath,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
}
