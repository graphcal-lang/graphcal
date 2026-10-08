//! A resolved extern plugin function signature, as the IR records it, and
//! the rule that merges repeated declarations of one plugin function.

use crate::semantic_error::plugin::ExternSignatureError;
use std::collections::HashMap;

use crate::extern_struct_result::ExternStructResult;
use crate::semantic_error::SemanticError;
use crate::semantic_error::plugin::PluginError;
use crate::source_id::SourceId;
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

/// Merge one declared extern signature into a map of already-declared ones.
///
/// The same plugin function may be declared under several aliases (or in
/// several DAG bodies of one file), but its signature is a single fact about
/// the plugin — conflicting declarations are rejected at the later
/// declaration. Comparison is structural: renaming dimension variables or
/// parameters does not make a different signature, and the first
/// declaration is kept.
///
/// # Errors
///
/// Returns [`PluginError::InvalidExternSignature`](PluginError::InvalidExternSignature) when `entry` disagrees
/// with an existing declaration of the same key on its signature or on its
/// nominal struct result type.
pub(crate) fn merge_extern_function(
    map: &mut HashMap<crate::plugin_identity::ExternFnKey, ExternFunctionEntry>,
    entry: ExternFunctionEntry,
    src: SourceId,
) -> Result<(), SemanticError> {
    use std::collections::hash_map::Entry;

    use crate::function_signature::ResultKind;

    match map.entry(entry.key()) {
        Entry::Occupied(existing) => {
            let existing = existing.get();
            if !existing.signature.structurally_equivalent(&entry.signature) {
                return Err(SemanticError::located(
                    src,
                    entry.decl_span,
                    PluginError::InvalidExternSignature {
                        error: ExternSignatureError::ConflictingSignature {
                            function: entry.name.clone(),
                            plugin: entry.plugin,
                        },
                    },
                ));
            }
            // A struct return is nominal at the declaration site: two
            // declarations must also agree on WHICH record type the shared
            // shape produces.
            if let (ResultKind::Struct(existing), ResultKind::Struct(declared)) =
                (existing.signature.result(), entry.signature.result())
                && !existing.same_record(declared)
            {
                return Err(SemanticError::located(
                    src,
                    entry.decl_span,
                    PluginError::InvalidExternSignature {
                        error: ExternSignatureError::ConflictingResultType {
                            function: entry.name.clone(),
                            plugin: entry.plugin.clone(),
                        },
                    },
                ));
            }
        }
        Entry::Vacant(slot) => {
            slot.insert(entry);
        }
    }
    Ok(())
}
