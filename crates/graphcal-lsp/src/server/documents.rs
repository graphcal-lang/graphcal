//! Open-document state: latest text, cached analyses, and merged diagnostics.

use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::analysis::AnalysisResult;
use crate::workspace_revision::{
    AnalysisFreshness, CurrentRevisions, DocumentIdentity, DocumentRevision,
};

/// Latest text and LSP version for one open editor document.
#[derive(Debug, Clone)]
pub(super) struct OpenDocumentSnapshot {
    pub(super) identity: DocumentIdentity,
    pub(super) revision: DocumentRevision,
    pub(super) text: Arc<String>,
    pub(super) version: i32,
}

#[derive(Debug)]
pub(super) struct RecordedDocument {
    pub(super) identity: DocumentIdentity,
    pub(super) revision: DocumentRevision,
}

/// One open document's cached analysis state.
#[cfg_attr(test, derive(Debug))]
pub(super) struct StoredDocument {
    /// The semantic state editor features answer from. Shared so a request
    /// clones the `Arc` under the lock and works on it off the serving task.
    pub(super) analysis: Arc<AnalysisResult>,
    /// Diagnostics of the document's latest pass. Newer than
    /// `analysis.diagnostics` after a parse-failure pass kept the previous
    /// semantic state (#834); diagnostics are published from here.
    pub(super) diagnostics: Arc<HashMap<Url, Vec<Diagnostic>>>,
}

impl StoredDocument {
    pub(super) fn new(analysis: AnalysisResult) -> Self {
        Self {
            diagnostics: Arc::clone(&analysis.diagnostics),
            analysis: Arc::new(analysis),
        }
    }
}

/// An analysis whose every input is current, so exact coordinates apply.
pub(super) struct CurrentAnalysis(pub(super) Arc<AnalysisResult>);

impl CurrentAnalysis {
    pub(super) fn get(&self) -> &AnalysisResult {
        &self.0
    }
}

/// An analysis whose dependencies are current; the root text may be newer.
pub(super) struct SemanticAnalysis(pub(super) Arc<AnalysisResult>);

impl SemanticAnalysis {
    pub(super) fn get(&self) -> &AnalysisResult {
        &self.0
    }
}

pub(super) fn is_current_analysis(
    analysis: &AnalysisResult,
    revisions: &CurrentRevisions<'_>,
) -> bool {
    analysis.inputs.freshness(revisions) == AnalysisFreshness::Current
}

pub(super) fn is_semantic_analysis(
    analysis: &AnalysisResult,
    revisions: &CurrentRevisions<'_>,
) -> bool {
    analysis.inputs.freshness(revisions) != AnalysisFreshness::DependencyChanged
}

pub(super) fn open_revisions(
    snapshots: &HashMap<Url, OpenDocumentSnapshot>,
) -> impl Iterator<Item = (&DocumentIdentity, DocumentRevision)> + '_ {
    snapshots
        .values()
        .map(|snapshot| (&snapshot.identity, snapshot.revision))
}

/// Compute the diagnostics to publish for each URI in `targets` from the
/// whole open-document set.
///
/// Ownership rule (#832): a URI that is itself an open, analyzed document is
/// authoritative for its own diagnostics — its analysis ran on the exact
/// buffer the user sees. For URIs that are not open (imported files on
/// disk), the contributions of every open document are merged, deduplicating
/// identical diagnostics reported by multiple importers. A URI no open
/// document reports on yields an empty list, clearing stale squiggles.
pub(super) fn merged_diagnostics_for(
    docs: &HashMap<Url, StoredDocument>,
    targets: &[Url],
) -> Vec<(Url, Vec<Diagnostic>)> {
    targets
        .iter()
        .map(|target| {
            if let Some(own) = docs.get(target) {
                let diags = own.diagnostics.get(target).cloned().unwrap_or_default();
                return (target.clone(), diags);
            }
            let mut merged: Vec<Diagnostic> = Vec::new();
            for stored in docs.values() {
                for diag in stored.diagnostics.get(target).into_iter().flatten() {
                    if !merged.contains(diag) {
                        merged.push(diag.clone());
                    }
                }
            }
            (target.clone(), merged)
        })
        .collect()
}
