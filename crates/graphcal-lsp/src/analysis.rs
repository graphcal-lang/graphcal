//! The cached per-document analysis result consumed by every editor feature.

use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::project_symbols::ProjectSymbols;
use crate::symbol_identity::{UnresolvedSymbol, VisibleBinding, resolve_visible_target};
use crate::symbol_table::{SymbolKey, SymbolTable};
use crate::workspace_revision::AnalysisInputs;
use graphcal_compiler::syntax::module_name::ScopedName;

use crate::fn_signatures::{ExternCallee, FnSignatureInfo};
use crate::imported_definitions::ImportedDefinition;

/// A loader-resolved import link for Document Links.
///
/// Pairs the source-text span of the import path with the loader-resolved
/// target URI, so `document_links` doesn't need to re-resolve paths.
pub struct ResolvedImportLink {
    /// Span of the import path in the source text.
    pub path_span: graphcal_compiler::syntax::span::Span,
    /// Loader-resolved target URI.
    pub target_uri: Url,
}

/// Cached analysis result for a document.
pub struct AnalysisResult {
    /// Exact document revisions and dependencies consumed by this result.
    pub inputs: AnalysisInputs,
    /// The raw source text. Shared via `Arc` so hover, inlay-hint, and
    /// formatting handlers can borrow without cloning the full buffer.
    pub source: Arc<String>,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub symbol_table: SymbolTable,
    /// Definitions from imported files, keyed by symbol key.
    pub imported_definitions: HashMap<SymbolKey, ImportedDefinition>,
    /// Complete immutable occurrence index for the loaded project snapshot.
    pub project_symbols: ProjectSymbols,
    /// Source-visible aliases/qualifiers for imported canonical definitions.
    /// Several bindings may target one identity; no alias is chosen as the
    /// semantic key.
    pub imported_bindings: Vec<VisibleBinding>,
    /// Public symbols of each loader-resolved import path, categorized by the
    /// exact marker required in a selective import item.
    pub import_surfaces: HashMap<
        graphcal_compiler::syntax::non_empty::NonEmpty<String>,
        Vec<graphcal_compiler::resolve::exports::ExportedImportItem>,
    >,
    /// Diagnostics to publish, grouped by the URI they belong to. The active
    /// document's URI is always present (with an empty Vec when clean) so a
    /// previously-published diagnostic can be cleared. Shared via `Arc` so
    /// `store_and_publish` can hand a snapshot to the publish loop without
    /// deep-cloning the map on every analysis cycle.
    pub diagnostics: Arc<HashMap<Url, Vec<Diagnostic>>>,
    /// Computed values from evaluation, keyed by declaration name.
    /// Each value is a formatted display string (e.g., `"9.81 [m/s^2]"`).
    pub eval_values: HashMap<ScopedName, String>,
    /// Structured function signatures, keyed by function name.
    /// Points to a lazily-initialized static map (builtins never change).
    pub fn_signatures: &'static HashMap<String, FnSignatureInfo>,
    /// Extern (plugin) function signatures from this file's `import plugin`
    /// blocks, keyed by the typed `alias::name` call spelling. Per-file,
    /// unlike the static builtin map.
    pub extern_fn_signatures: HashMap<ExternCallee, FnSignatureInfo>,
    /// Loader-resolved import links (for Document Links).
    pub import_links: Vec<ResolvedImportLink>,
    /// `false` when this result is a parse-failure fallback: the buffer did
    /// not parse, so the symbol-dependent fields are empty placeholders and
    /// only `diagnostics` is meaningful. [`store_analysis`] keeps the
    /// previous good symbol state in that case (#834) — mid-edit buffers are
    /// unparsable more often than not, and completion/hover/goto should keep
    /// answering from the last successfully analyzed state.
    pub buffer_parsed: bool,
}

impl AnalysisResult {
    pub fn resolve_imported_target(&self, unresolved: &UnresolvedSymbol) -> Option<SymbolKey> {
        resolve_visible_target(&self.imported_bindings, unresolved)
    }
}

#[cfg(test)]
impl AnalysisResult {
    /// True when no diagnostics are present across any URI.
    pub(crate) fn has_no_diagnostics(&self) -> bool {
        self.diagnostics.values().all(Vec::is_empty)
    }
}

// `AnalysisResult`'s custom `Debug` shape (counts, not contents) is useful only
// inside test assertion messages; gating it behind `cfg(test)` keeps the
// release binary from carrying an impl no production code path can call.
#[cfg(test)]
impl std::fmt::Debug for AnalysisResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisResult")
            .field("inputs", &self.inputs)
            .field("source_len", &self.source.len())
            .field("symbol_table_defs", &self.symbol_table.definitions.len())
            .field(
                "project_symbols_complete",
                &self.project_symbols.complete().is_some(),
            )
            .field("imported_defs", &self.imported_definitions.len())
            .field("imported_bindings", &self.imported_bindings.len())
            .field("import_surfaces", &self.import_surfaces.len())
            .field(
                "diagnostics_count",
                &self.diagnostics.values().map(Vec::len).sum::<usize>(),
            )
            .field("eval_values_count", &self.eval_values.len())
            .field("fn_signatures_count", &self.fn_signatures.len())
            .field(
                "extern_fn_signatures_count",
                &self.extern_fn_signatures.len(),
            )
            .field("import_links_count", &self.import_links.len())
            .field("buffer_parsed", &self.buffer_parsed)
            .finish()
    }
}
