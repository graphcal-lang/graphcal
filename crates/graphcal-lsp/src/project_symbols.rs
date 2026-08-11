//! Complete, immutable symbol occurrences for one loaded project snapshot.
//!
//! The analysis shell builds this value from every loader-resolved source file.
//! References and rename then query one canonical index instead of composing
//! partial answers from whichever editor buffers happen to be open.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use graphcal_compiler::syntax::lexer::Lexer;
use graphcal_compiler::syntax::span::Span;
use tower_lsp::lsp_types::Url;

use crate::symbol_identity::{ReferenceTarget, UnresolvedSymbol, VisibleBinding};
use crate::symbol_table::{DefinitionInfo, ReferenceInfo, SymbolKey, SymbolTable};

/// Symbol data for one physical source document in a project snapshot.
#[derive(Debug)]
pub struct ProjectDocumentSymbols {
    pub uri: Url,
    pub source: Arc<String>,
    pub table: SymbolTable,
    pub imported_bindings: Vec<VisibleBinding>,
}

impl ProjectDocumentSymbols {
    #[must_use]
    pub const fn new(
        uri: Url,
        source: Arc<String>,
        table: SymbolTable,
        imported_bindings: Vec<VisibleBinding>,
    ) -> Self {
        Self {
            uri,
            source,
            table,
            imported_bindings,
        }
    }

    fn resolve_reference(&self, reference: &ReferenceInfo) -> Option<SymbolKey> {
        self.table
            .resolve_local_target(&reference.target)
            .map(|target| self.canonical_visible_target(target))
            .or_else(|| match &reference.target {
                ReferenceTarget::Resolved(target) => {
                    Some(self.canonical_visible_target(target.clone()))
                }
                ReferenceTarget::Unresolved(unresolved) => {
                    resolve_visible_target(&self.imported_bindings, unresolved)
                }
            })
    }

    fn canonical_visible_target(&self, resolved: SymbolKey) -> SymbolKey {
        if self.table.definitions.contains_key(&resolved) {
            return resolved;
        }
        let mut candidates = self
            .imported_bindings
            .iter()
            .filter(|binding| {
                binding.target().same_namespace(&resolved)
                    && binding.spelling().leaf().as_str() == resolved.leaf_name()
            })
            .map(VisibleBinding::target);
        let Some(candidate) = candidates.next() else {
            return resolved;
        };
        if candidates.all(|other| other == candidate) {
            candidate.clone()
        } else {
            resolved
        }
    }

    fn may_unresolved_reference_target(
        &self,
        unresolved: &UnresolvedSymbol,
        target: &SymbolKey,
    ) -> bool {
        if !unresolved.accepts(target) {
            return false;
        }
        let local_spelling_matches = match unresolved {
            UnresolvedSymbol::Field(field) => field.as_str() == target.leaf_name(),
            _ => unresolved
                .path()
                .is_some_and(|path| path.is_local() && path.leaf().as_str() == target.leaf_name()),
        };
        local_spelling_matches
            || self
                .imported_bindings
                .iter()
                .any(|binding| binding.target() == target && binding.resolves(unresolved))
    }
}

/// One URI/span pair from the immutable project snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectOccurrence {
    pub uri: Url,
    pub span: Span,
}

/// A definition occurrence with the metadata needed by editor features.
#[derive(Debug)]
pub struct ProjectDefinition<'a> {
    pub occurrence: ProjectOccurrence,
    pub definition: &'a DefinitionInfo,
}

/// Whether the index proves that no reverse importer exists outside its source set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProjectCoverage {
    /// A virtual standalone buffer has no filesystem importers.
    #[cfg(test)]
    Standalone,
    /// A loader root includes its transitive dependencies but not sibling roots
    /// that may import the root itself.
    #[default]
    DependencyClosure,
}

/// Complete occurrence index for every source file loaded by one analysis.
#[derive(Debug, Default)]
pub struct ProjectSymbolIndex {
    root_uri: Option<Url>,
    documents: HashMap<Url, ProjectDocumentSymbols>,
    coverage: ProjectCoverage,
}

impl ProjectSymbolIndex {
    #[must_use]
    pub fn loaded_dependency_closure(
        root_uri: Url,
        documents: impl IntoIterator<Item = ProjectDocumentSymbols>,
    ) -> Self {
        Self::with_coverage(root_uri, documents, ProjectCoverage::DependencyClosure)
    }

    #[cfg(test)]
    #[must_use]
    pub fn standalone(
        root_uri: Url,
        documents: impl IntoIterator<Item = ProjectDocumentSymbols>,
    ) -> Self {
        Self::with_coverage(root_uri, documents, ProjectCoverage::Standalone)
    }

    fn with_coverage(
        root_uri: Url,
        documents: impl IntoIterator<Item = ProjectDocumentSymbols>,
        coverage: ProjectCoverage,
    ) -> Self {
        Self {
            root_uri: Some(root_uri),
            documents: documents
                .into_iter()
                .map(|document| (document.uri.clone(), document))
                .collect(),
            coverage,
        }
    }

    /// Whether externally visible definitions in the root have no possible
    /// reverse importers outside this snapshot.
    #[must_use]
    pub const fn covers_reverse_dependencies(&self) -> bool {
        match self.coverage {
            #[cfg(test)]
            ProjectCoverage::Standalone => true,
            ProjectCoverage::DependencyClosure => false,
        }
    }

    #[must_use]
    pub fn resolve_root_reference(&self, reference: &ReferenceInfo) -> Option<SymbolKey> {
        self.root_uri
            .as_ref()
            .and_then(|uri| self.documents.get(uri))?
            .resolve_reference(reference)
    }

    #[must_use]
    pub fn document(&self, uri: &Url) -> Option<&ProjectDocumentSymbols> {
        self.documents.get(uri)
    }

    #[must_use]
    pub fn definition(&self, target: &SymbolKey) -> Option<ProjectDefinition<'_>> {
        self.documents.values().find_map(|document| {
            let definition = document.table.definitions.get(target)?;
            definition.is_navigable().then(|| ProjectDefinition {
                occurrence: ProjectOccurrence {
                    uri: document.uri.clone(),
                    span: definition.name_span,
                },
                definition,
            })
        })
    }

    #[must_use]
    pub fn references(&self, target: &SymbolKey) -> Vec<ProjectOccurrence> {
        let mut occurrences: Vec<_> = self
            .documents
            .values()
            .flat_map(|document| {
                document
                    .table
                    .references()
                    .iter()
                    .filter(|reference| {
                        document.resolve_reference(reference).as_ref() == Some(target)
                    })
                    .map(|reference| ProjectOccurrence {
                        uri: document.uri.clone(),
                        span: reference.span,
                    })
            })
            .collect();
        sort_and_dedup(&mut occurrences);
        occurrences
    }

    /// Source ranges that must change when the canonical target's leaf changes.
    ///
    /// An imported alias still resolves to the target, but its authored leaf is
    /// intentionally different and therefore does not participate in a
    /// canonical-definition rename.
    #[must_use]
    pub fn rename_occurrences(&self, target: &SymbolKey) -> Vec<ProjectOccurrence> {
        let mut occurrences: Vec<_> = self
            .documents
            .values()
            .flat_map(|document| {
                let definitions = document
                    .table
                    .definitions
                    .get(target)
                    .filter(|definition| definition.is_navigable())
                    .map(|definition| ProjectOccurrence {
                        uri: document.uri.clone(),
                        span: definition.name_span,
                    });
                let references = document.table.references().iter().filter_map(|reference| {
                    (document.resolve_reference(reference).as_ref() == Some(target))
                        .then(|| rename_leaf_span(&document.source, reference.span, target))
                        .flatten()
                        .map(|span| ProjectOccurrence {
                            uri: document.uri.clone(),
                            span,
                        })
                });
                definitions.into_iter().chain(references)
            })
            .collect();
        sort_and_dedup(&mut occurrences);
        occurrences
    }

    /// Whether a tolerant occurrence could denote `target` but lacks one
    /// canonical resolution. A rename is unsafe while this is true.
    #[must_use]
    pub fn has_ambiguous_reference_to(&self, target: &SymbolKey) -> bool {
        self.documents.values().any(|document| {
            document.table.references().iter().any(|reference| {
                let ReferenceTarget::Unresolved(unresolved) = &reference.target else {
                    return false;
                };
                document.resolve_reference(reference).is_none()
                    && document.may_unresolved_reference_target(unresolved, target)
            })
        })
    }

    /// Whether changing a canonical leaf would introduce a declaration or
    /// direct-import collision in any affected source scope.
    #[must_use]
    pub fn collides_with(
        &self,
        target: &SymbolKey,
        renamed_target: &SymbolKey,
        new_name: &str,
    ) -> bool {
        if self.documents.values().any(|document| {
            document
                .table
                .definitions
                .get(renamed_target)
                .is_some_and(|definition| !definition.is_builtin())
        }) {
            return true;
        }

        self.documents.values().any(|document| {
            let direct_binding = document.imported_bindings.iter().any(|binding| {
                binding.target() == target
                    && binding.spelling().is_local()
                    && binding.spelling().leaf().as_str() == target.leaf_name()
            });
            let locally_defined = document.table.definitions.contains_key(target)
                && target.owner() == Some(document.table.owner());
            let definition_collision =
                document
                    .table
                    .definitions
                    .iter()
                    .any(|(candidate, definition)| {
                        !definition.is_builtin()
                            && candidate.owner() == Some(document.table.owner())
                            && candidate.leaf_name() == new_name
                            && candidate.same_namespace(target)
                    });
            let imported_collision = document.imported_bindings.iter().any(|binding| {
                binding.target() != target
                    && binding.spelling().is_local()
                    && binding.spelling().leaf().as_str() == new_name
                    && binding.target().same_namespace(target)
            });
            let top_level_namespace = matches!(
                target,
                SymbolKey::Declaration(_)
                    | SymbolKey::Dimension(_)
                    | SymbolKey::Unit(_)
                    | SymbolKey::StructType(_)
                    | SymbolKey::Constructor(_)
                    | SymbolKey::Index(_)
            );
            (direct_binding && (definition_collision || imported_collision))
                || (locally_defined && top_level_namespace && imported_collision)
        })
    }

    /// Whether the cursor occurrence itself spells the canonical leaf and can
    /// therefore initiate a project-wide canonical rename. Alias uses remain
    /// navigable but deliberately cannot trigger a surprising API rename.
    #[must_use]
    pub fn occurrence_can_initiate_rename(
        &self,
        uri: &Url,
        span: Span,
        target: &SymbolKey,
    ) -> bool {
        self.documents
            .get(uri)
            .is_some_and(|document| rename_leaf_span(&document.source, span, target).is_some())
    }
}

/// Provenance of the project occurrence set attached to an analysis result.
#[derive(Debug, Default)]
pub enum ProjectSymbols {
    Complete(ProjectSymbolIndex),
    /// Loading failed before every transitive source could be indexed.
    #[default]
    Incomplete,
}

impl ProjectSymbols {
    #[must_use]
    pub const fn complete(&self) -> Option<&ProjectSymbolIndex> {
        match self {
            Self::Complete(index) => Some(index),
            Self::Incomplete => None,
        }
    }
}

fn resolve_visible_target(
    bindings: &[VisibleBinding],
    unresolved: &UnresolvedSymbol,
) -> Option<SymbolKey> {
    let mut targets = bindings
        .iter()
        .filter(|binding| binding.resolves(unresolved))
        .map(VisibleBinding::target);
    let target = targets.next()?;
    targets
        .all(|candidate| candidate == target)
        .then(|| target.clone())
}

fn rename_leaf_span(source: &str, occurrence: Span, target: &SymbolKey) -> Option<Span> {
    let text = source.get(occurrence.offset()..occurrence.offset() + occurrence.len())?;
    let mut lexer = Lexer::new(text);
    let mut leaf = None;
    while let Some((token, span)) = lexer.next_token() {
        if token.is_identifier() {
            leaf = Some(span);
        }
    }
    let leaf = leaf?;
    let spelling = text.get(leaf.offset()..leaf.offset() + leaf.len())?;
    (spelling == target.leaf_name())
        .then(|| Span::new(occurrence.offset() + leaf.offset(), leaf.len()))
}

fn sort_and_dedup(occurrences: &mut Vec<ProjectOccurrence>) {
    occurrences.sort_by(|left, right| {
        left.uri
            .as_str()
            .cmp(right.uri.as_str())
            .then_with(|| left.span.offset().cmp(&right.span.offset()))
            .then_with(|| left.span.len().cmp(&right.span.len()))
    });
    let mut seen = HashSet::new();
    occurrences.retain(|occurrence| seen.insert((occurrence.uri.clone(), occurrence.span)));
}
