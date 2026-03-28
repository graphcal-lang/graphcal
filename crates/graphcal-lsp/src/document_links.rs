//! textDocument/documentLink handler.

use graphcal_compiler::syntax::ast::ImportPath;
use tower_lsp::lsp_types::{DocumentLink, Url};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;

/// Build document links for `use` declarations in the file.
pub fn document_links(analysis: &AnalysisResult, uri: &Url) -> Option<Vec<DocumentLink>> {
    if analysis.import_decls.is_empty() {
        return None;
    }

    let root_path = uri.to_file_path().ok()?;
    let root_dir = root_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    let mut links = Vec::new();

    for import_decl in &analysis.import_decls {
        let import_path = match &import_decl.path {
            ImportPath::FilePath { path, .. } => root_dir.join(path),
            // Bare module paths require manifest resolution which is not
            // available in the LSP document-link context yet. Skip them.
            ImportPath::ModulePath { .. } => continue,
        };
        let Ok(canonical) = import_path.canonicalize() else {
            continue;
        };
        let Ok(target_uri) = Url::from_file_path(&canonical) else {
            continue;
        };

        links.push(DocumentLink {
            range: span_to_range(&analysis.source, import_decl.path.span()),
            target: Some(target_uri),
            tooltip: Some("Open imported file".to_string()),
            data: None,
        });
    }

    if links.is_empty() { None } else { Some(links) }
}
