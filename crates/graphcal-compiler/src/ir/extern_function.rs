//! A resolved extern plugin function signature, as the IR records it.

use crate::extern_struct_result::ExternStructResult;
use crate::syntax::span::Span;

/// A resolved extern function declared by an `import plugin` block.
///
/// The signature is stored in structural (base-dimension exponent) form —
/// this is what Phase B of the plugin plan (#25) compares against embedded
/// plugin manifests.
#[derive(Debug, Clone)]
pub struct ExternFunctionEntry {
    /// Canonical plugin identity (the `import plugin "…"` path string).
    pub plugin: crate::plugin_identity::PluginIdentity,
    /// The alias the declaring file bound the plugin to (diagnostics only).
    pub alias: crate::syntax::module_name::ModuleAliasName,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
    /// The resolved dimensional signature. A struct result carries both the
    /// manifest-facing field shape and the nominal record type the
    /// declaration binds it to.
    pub signature: crate::function_signature::FunctionSignature<ExternStructResult>,
    /// Span of the function name inside the declaring block.
    pub name_span: Span,
    /// Span of the whole `fn ...;` declaration.
    pub decl_span: Span,
    /// Span of the `import plugin "…"` path string, for diagnostics about
    /// the plugin as a whole (unloadable file, hash mismatch, …).
    pub path_span: Span,
}

impl ExternFunctionEntry {
    /// The plugin-scoped identity this entry is registered under.
    #[must_use]
    pub fn key(&self) -> crate::plugin_identity::ExternFnKey {
        crate::plugin_identity::ExternFnKey {
            plugin: self.plugin.clone(),
            name: self.name.clone(),
        }
    }

    /// Whether two entries describe the same callable definition.
    ///
    /// Source aliases and spans are diagnostic provenance, not signature
    /// identity, so independently compiled copies may differ in those fields.
    pub(crate) fn has_same_callable_definition(&self, other: &Self) -> bool {
        self.plugin == other.plugin && self.name == other.name && self.signature == other.signature
    }
}
