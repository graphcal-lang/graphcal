//! textDocument/documentLink handler.

use tower_lsp::lsp_types::DocumentLink;

use crate::convert::span_to_range;
use crate::server::AnalysisResult;

/// Build document links from the loader-resolved import targets.
pub fn document_links(analysis: &AnalysisResult) -> Option<Vec<DocumentLink>> {
    if analysis.import_links.is_empty() {
        return None;
    }

    let links: Vec<DocumentLink> = analysis
        .import_links
        .iter()
        .map(|link| DocumentLink {
            range: span_to_range(&analysis.source, link.path_span),
            target: Some(link.target_uri.clone()),
            tooltip: Some("Open imported file".to_string()),
            data: None,
        })
        .collect();

    if links.is_empty() { None } else { Some(links) }
}
