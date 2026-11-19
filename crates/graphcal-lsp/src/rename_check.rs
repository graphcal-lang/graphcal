//! Compiler re-check of a planned rename before it reaches the client.
//!
//! The compiler alone decides which names a program may use (reserved
//! built-in spellings, the shared Static and Term universes, duplicates,
//! locals that shadow visible terms, …). Instead of copying those rules, a
//! rename is applied to the analysed sources and the project is checked
//! again: the rename is offered only when it adds no compile error and every
//! reference still resolves to the same definition.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::syntax::span::Span;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Url};

use crate::analysis::{AnalysisResult, ProjectCheck};
use crate::analysis_pipeline::{
    AnalysisDemand, EvaluationDemand, OpenBuffer, ViewModelDemand, run_analysis_with_cancellation,
};
use crate::project_symbols::{ProjectOccurrence, ResolutionSite};
use crate::workspace_revision::{AnalysisInputSnapshot, RevisionClock};

/// Every occurrence a rename respells, as byte spans of the analysed sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRename {
    new_name: String,
    /// Sorted, duplicate-free spans per document.
    occurrences: HashMap<Url, Vec<Span>>,
}

impl PlannedRename {
    #[must_use]
    pub fn new(new_name: &str, occurrences: impl IntoIterator<Item = (Url, Span)>) -> Self {
        let mut grouped: HashMap<Url, Vec<Span>> = HashMap::new();
        for (uri, span) in occurrences {
            grouped.entry(uri).or_default().push(span);
        }
        for spans in grouped.values_mut() {
            spans.sort_unstable_by_key(|span| (span.offset(), span.len()));
            spans.dedup();
        }
        Self {
            new_name: new_name.to_string(),
            occurrences: grouped,
        }
    }

    #[must_use]
    pub fn new_name(&self) -> &str {
        &self.new_name
    }

    #[must_use]
    pub const fn occurrences(&self) -> &HashMap<Url, Vec<Span>> {
        &self.occurrences
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.occurrences.values().all(Vec::is_empty)
    }

    /// `source` with this rename's occurrences in `uri` respelled.
    fn edited(&self, uri: &Url, source: &str) -> String {
        let mut edited = source.to_string();
        for span in self.occurrences.get(uri).into_iter().flatten().rev() {
            edited.replace_range(span.offset()..span.offset() + span.len(), &self.new_name);
        }
        edited
    }

    /// Where the byte `offset` of the analysed `uri` lies after the rename.
    ///
    /// Occurrences never overlap a reference or definition boundary, so an
    /// offset is either before or after each respelled span.
    fn moved_offset(&self, uri: &Url, offset: usize) -> usize {
        let new_len = self.new_name.len();
        self.occurrences
            .get(uri)
            .into_iter()
            .flatten()
            .take_while(|span| span.offset() + span.len() <= offset)
            .fold(offset, |moved, span| moved + new_len - span.len())
    }

    fn moved_occurrence(&self, occurrence: &ProjectOccurrence) -> ProjectOccurrence {
        let start = self.moved_offset(&occurrence.uri, occurrence.span.offset());
        let end = self.moved_offset(
            &occurrence.uri,
            occurrence.span.offset() + occurrence.span.len(),
        );
        ProjectOccurrence {
            uri: occurrence.uri.clone(),
            span: Span::new(start, end - start),
        }
    }

    fn moved_site(&self, site: ResolutionSite) -> ResolutionSite {
        match site {
            ResolutionSite::Definition(definition) => {
                ResolutionSite::Definition(self.moved_occurrence(&definition))
            }
            other => other,
        }
    }
}

/// A compile error the compiler reported, as published: its diagnostic code
/// and its message without the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileErrorReport {
    pub code: Option<NumberOrString>,
    pub message: String,
}

impl std::fmt::Display for CompileErrorReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(NumberOrString::String(code)) => write!(f, "{code}: {}", self.message),
            Some(NumberOrString::Number(code)) => write!(f, "{code}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Why a planned rename was rejected by the re-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameCheckFailure {
    /// The edited sources raise a compile error the original did not.
    NewCompileError(CompileErrorReport),
    /// The analysed project already fails to compile, so the re-check cannot
    /// validate the rename.
    OriginalDoesNotCompile,
    /// A reference would resolve to a different definition after the edit.
    ChangedResolution,
    /// An edited document is not part of the analysed snapshot.
    MissingSource,
    /// The request was cancelled while checking.
    Cancelled,
}

/// The shell inputs a rename re-check analyses the edited sources with.
pub struct RenameCheck<'a> {
    open_buffers: Vec<OpenBuffer>,
    plugin_host: &'a graphcal_plugin_host::PluginHost,
    cancellation: &'a CancellationToken,
}

impl<'a> RenameCheck<'a> {
    /// `open_buffers` are the other open documents, overlaid as the editor
    /// analysis overlays them.
    #[must_use]
    pub const fn new(
        open_buffers: Vec<OpenBuffer>,
        plugin_host: &'a graphcal_plugin_host::PluginHost,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            open_buffers,
            plugin_host,
            cancellation,
        }
    }

    /// Re-check `analysis`'s project with `plan` applied.
    pub fn check(
        &self,
        analysis: &AnalysisResult,
        root_uri: &Url,
        plan: &PlannedRename,
    ) -> Result<(), RenameCheckFailure> {
        // A failing project hides later errors behind the first one, so the
        // re-check could not tell whether the rename adds another.
        if analysis.check == ProjectCheck::Failed {
            return Err(RenameCheckFailure::OriginalDoesNotCompile);
        }
        let edited = edited_sources(analysis, root_uri, plan)?;
        let root_text = edited
            .get(root_uri)
            .cloned()
            .unwrap_or_else(|| analysis.source.as_str().to_string());
        let overlays: Vec<OpenBuffer> = self
            .open_buffers
            .iter()
            .filter(|buffer| buffer.uri() != root_uri && !edited.contains_key(buffer.uri()))
            .cloned()
            .chain(
                edited
                    .iter()
                    .filter(|(uri, _)| *uri != root_uri)
                    .filter_map(|(uri, text)| OpenBuffer::new(uri.clone(), Arc::new(text.clone()))),
            )
            .collect();
        // The re-check is a throwaway analysis: its revision only has to
        // exist, and a fresh clock is never exhausted.
        let Ok(revision) = RevisionClock::default().next() else {
            return Err(RenameCheckFailure::Cancelled);
        };
        let snapshot = AnalysisInputSnapshot::new(
            crate::file_identity::document_identity(root_uri),
            revision,
            HashMap::new(),
        );
        let checked = run_analysis_with_cancellation(
            root_uri,
            &root_text,
            &overlays,
            self.plugin_host,
            &snapshot,
            AnalysisDemand {
                view: ViewModelDemand::Skip,
                evaluation: EvaluationDemand::Skip,
            },
            self.cancellation,
        )
        .map_err(|_| RenameCheckFailure::Cancelled)?
        .analysis;
        if let Some(error) = new_compile_error(&analysis.diagnostics, &checked.diagnostics) {
            return Err(RenameCheckFailure::NewCompileError(error));
        }
        if resolution_changed(analysis, &checked, root_uri, plan) {
            return Err(RenameCheckFailure::ChangedResolution);
        }
        Ok(())
    }
}

/// The edited text of every document the plan touches.
fn edited_sources(
    analysis: &AnalysisResult,
    root_uri: &Url,
    plan: &PlannedRename,
) -> Result<HashMap<Url, String>, RenameCheckFailure> {
    plan.occurrences()
        .keys()
        .map(|uri| {
            let source = if uri == root_uri {
                Some(analysis.source.as_str())
            } else {
                analysis
                    .project_symbols
                    .complete()
                    .and_then(|project| project.document(uri))
                    .map(|document| document.source.as_str())
            };
            source
                .map(|source| (uri.clone(), plan.edited(uri, source)))
                .ok_or(RenameCheckFailure::MissingSource)
        })
        .collect()
}

/// Error diagnostics counted by code, the only rendering-independent part of
/// a published diagnostic (messages spell the renamed name).
fn error_codes(
    diagnostics: &HashMap<Url, Vec<Diagnostic>>,
) -> HashMap<Option<&NumberOrString>, usize> {
    let mut counts = HashMap::new();
    for diagnostic in diagnostics
        .values()
        .flatten()
        .filter(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR))
    {
        *counts.entry(diagnostic.code.as_ref()).or_insert(0) += 1;
    }
    counts
}

/// The first error of `after` beyond what `before` already reported.
fn new_compile_error(
    before: &HashMap<Url, Vec<Diagnostic>>,
    after: &HashMap<Url, Vec<Diagnostic>>,
) -> Option<CompileErrorReport> {
    let mut budget = error_codes(before);
    after
        .values()
        .flatten()
        .filter(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR))
        .find(|diagnostic| {
            let remaining = budget.entry(diagnostic.code.as_ref()).or_insert(0);
            if *remaining == 0 {
                true
            } else {
                *remaining -= 1;
                false
            }
        })
        .map(|diagnostic| CompileErrorReport {
            code: diagnostic.code.clone(),
            message: diagnostic
                .message
                .split_once("\n\n")
                .map_or(diagnostic.message.as_str(), |(message, _)| message)
                .to_string(),
        })
}

/// Every reference occurrence with its resolution: from the project index
/// when both snapshots have one, else from the root document's own table.
fn resolutions(
    analysis: &AnalysisResult,
    root_uri: &Url,
    use_project: bool,
) -> HashMap<ProjectOccurrence, HashSet<ResolutionSite>> {
    let mut resolved: HashMap<ProjectOccurrence, HashSet<ResolutionSite>> = HashMap::new();
    let entries: Box<dyn Iterator<Item = (ProjectOccurrence, ResolutionSite)>> =
        match analysis.project_symbols.complete() {
            Some(project) if use_project => Box::new(project.resolutions()),
            _ => {
                let table = &analysis.symbol_table;
                Box::new(table.references().iter().map(|reference| {
                    let site = table.resolve_local_target(&reference.target).map_or(
                        ResolutionSite::Unresolved,
                        |target| match table.definitions.get(&target) {
                            Some(definition) if definition.is_navigable() => {
                                ResolutionSite::Definition(ProjectOccurrence {
                                    uri: root_uri.clone(),
                                    span: definition.name_span,
                                })
                            }
                            _ => ResolutionSite::Identity(target),
                        },
                    );
                    (
                        ProjectOccurrence {
                            uri: root_uri.clone(),
                            span: reference.span,
                        },
                        site,
                    )
                }))
            }
        };
    for (occurrence, site) in entries {
        resolved.entry(occurrence).or_default().insert(site);
    }
    resolved
}

/// Whether some reference present in both snapshots resolves elsewhere after
/// the edit.
///
/// The compiler rejects every shadowing a rename could introduce today, so
/// this guards the planned edit itself (and any future scoping rule that
/// lets a renamed declaration capture another reference) rather than a known
/// capture. A reference that was unresolved before may resolve afterwards:
/// that repairs a broken program and is not a silent change.
fn resolution_changed(
    before: &AnalysisResult,
    after: &AnalysisResult,
    root_uri: &Url,
    plan: &PlannedRename,
) -> bool {
    let use_project =
        before.project_symbols.complete().is_some() && after.project_symbols.complete().is_some();
    retargets(
        resolutions(before, root_uri, use_project),
        &resolutions(after, root_uri, use_project),
        plan,
    )
}

fn retargets(
    before: HashMap<ProjectOccurrence, HashSet<ResolutionSite>>,
    after: &HashMap<ProjectOccurrence, HashSet<ResolutionSite>>,
    plan: &PlannedRename,
) -> bool {
    before.into_iter().any(|(occurrence, sites)| {
        let moved: HashSet<_> = sites
            .into_iter()
            .filter(comparable)
            .map(|site| plan.moved_site(site))
            .collect();
        !moved.is_empty()
            && after
                .get(&plan.moved_occurrence(&occurrence))
                .is_some_and(|after_sites| {
                    moved
                        != after_sites
                            .iter()
                            .filter(|site| comparable(site))
                            .cloned()
                            .collect()
                })
    })
}

/// Whether a resolution can be compared across the edit. A definition site
/// moves predictably; an identity without a definition is stable only when it
/// has no owner (a built-in): an owned one, such as a member of an include
/// instance, may embed source positions the edit shifts.
const fn comparable(site: &ResolutionSite) -> bool {
    match site {
        ResolutionSite::Definition(_) => true,
        ResolutionSite::Identity(identity) => identity.owner().is_none(),
        ResolutionSite::Unresolved => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.gcl").unwrap()
    }

    #[test]
    fn edits_and_moves_offsets_around_respelled_spans() {
        let source = "node a: Int = 1; node b: Int = @a + @a;";
        let first = source.find("a:").unwrap();
        let second = source.find("@a").unwrap() + 1;
        let third = source.rfind("@a").unwrap() + 1;
        let plan = PlannedRename::new(
            "alpha",
            [third, first, second, first].map(|offset| (uri(), Span::new(offset, 1))),
        );
        assert_eq!(plan.occurrences()[&uri()].len(), 3);
        let edited = plan.edited(&uri(), source);
        assert_eq!(
            edited,
            "node alpha: Int = 1; node b: Int = @alpha + @alpha;"
        );
        let b = source.find("b:").unwrap();
        assert_eq!(&edited[plan.moved_offset(&uri(), b)..][..1], "b");
        let moved = plan.moved_occurrence(&ProjectOccurrence {
            uri: uri(),
            span: Span::new(third - 1, 2),
        });
        assert_eq!(
            &edited[moved.span.offset()..moved.span.offset() + moved.span.len()],
            "@alpha"
        );
    }

    fn error(code: &str, message: &str) -> Diagnostic {
        Diagnostic {
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(code.to_string())),
            message: message.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn only_errors_beyond_the_original_count_are_new() {
        let before = HashMap::from([(uri(), vec![error("graphcal::T001", "old")])]);
        let same = HashMap::from([(uri(), vec![error("graphcal::T001", "renamed")])]);
        assert_eq!(new_compile_error(&before, &same), None);
        let added = HashMap::from([(
            uri(),
            vec![
                error("graphcal::T001", "renamed"),
                error(
                    "graphcal::N009",
                    "node `PI` shadows a built-in name\n\nhint: rename it",
                ),
            ],
        )]);
        let report = new_compile_error(&before, &added).expect("a new error");
        assert_eq!(
            report,
            CompileErrorReport {
                code: Some(NumberOrString::String("graphcal::N009".to_string())),
                message: "node `PI` shadows a built-in name".to_string(),
            }
        );
        assert_eq!(
            report.to_string(),
            "graphcal::N009: node `PI` shadows a built-in name"
        );
    }

    fn occurrence(offset: usize, len: usize) -> ProjectOccurrence {
        ProjectOccurrence {
            uri: uri(),
            span: Span::new(offset, len),
        }
    }

    fn profile(
        entries: impl IntoIterator<Item = (ProjectOccurrence, ResolutionSite)>,
    ) -> HashMap<ProjectOccurrence, HashSet<ResolutionSite>> {
        let mut profile: HashMap<_, HashSet<_>> = HashMap::new();
        for (occurrence, site) in entries {
            profile.entry(occurrence).or_default().insert(site);
        }
        profile
    }

    /// `node b …; node k …; … @k` with `b` renamed to `bee`: the reference to
    /// `k` must still reach `k`'s (moved) definition, not `bee`'s.
    #[test]
    fn a_reference_that_moves_to_another_definition_is_a_retarget() {
        let b = occurrence(5, 1);
        let k = occurrence(20, 1);
        let reference = occurrence(40, 2);
        let plan = PlannedRename::new("bee", [(uri(), b.span)]);
        let before = profile([(reference.clone(), ResolutionSite::Definition(k))]);
        let moved_k = occurrence(22, 1);
        let moved_reference = occurrence(42, 2);
        let kept = profile([(moved_reference.clone(), ResolutionSite::Definition(moved_k))]);
        assert!(!retargets(before.clone(), &kept, &plan));
        let captured = profile([(
            moved_reference,
            ResolutionSite::Definition(occurrence(5, 3)),
        )]);
        assert!(retargets(before, &captured, &plan));
        // Repairing a reference that resolved nowhere is not a retarget.
        let unresolved = profile([(reference, ResolutionSite::Unresolved)]);
        assert!(!retargets(unresolved, &captured, &plan));
    }
}
