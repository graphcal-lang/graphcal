//! Typed document revisions and dependency-aware analysis inputs.
//!
//! This module is the functional core for workspace invalidation. The async
//! server assigns revisions and performs I/O; these values only compare exact
//! immutable snapshots and walk typed document-identity edges.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tower_lsp::lsp_types::Url;

/// Canonical identity of one editor document at the filesystem/protocol shell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DocumentIdentity {
    File(PathBuf),
    Virtual(Url),
}

impl DocumentIdentity {
    #[must_use]
    pub const fn file(path: PathBuf) -> Self {
        Self::File(path)
    }

    #[must_use]
    pub const fn virtual_uri(uri: Url) -> Self {
        Self::Virtual(uri)
    }
}

/// Globally monotonic identity for one observed open-document text snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRevision(u64);

/// The process-wide revision counter exhausted its representable domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("document revision counter exhausted")]
pub struct RevisionExhausted;

/// Imperative-shell allocator for globally unique document revisions.
#[derive(Debug, Default)]
pub struct RevisionClock(AtomicU64);

impl RevisionClock {
    pub fn next(&self) -> Result<DocumentRevision, RevisionExhausted> {
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(|previous| DocumentRevision(previous + 1))
            .map_err(|_| RevisionExhausted)
    }
}

/// All open-buffer revisions visible when an analysis worker was launched.
#[derive(Debug, Clone)]
pub struct AnalysisInputSnapshot {
    root: DocumentIdentity,
    observed: HashMap<DocumentIdentity, DocumentRevision>,
}

impl AnalysisInputSnapshot {
    #[must_use]
    pub fn new(
        root: DocumentIdentity,
        root_revision: DocumentRevision,
        mut observed: HashMap<DocumentIdentity, DocumentRevision>,
    ) -> Self {
        observed.insert(root.clone(), root_revision);
        Self { root, observed }
    }

    /// Restrict the launch snapshot to identities the loader actually consumed.
    #[must_use]
    pub fn finish(&self, dependencies: HashSet<DocumentIdentity>) -> AnalysisInputs {
        let expected_revisions = std::iter::once(self.root.clone())
            .chain(dependencies.iter().cloned())
            .map(|identity| {
                let revision = self.observed.get(&identity).copied();
                (identity, revision)
            })
            .collect();
        AnalysisInputs {
            root: self.root.clone(),
            dependencies,
            expected_revisions,
        }
    }
}

/// Exact open-buffer revisions and source dependencies consumed by one result.
#[derive(Debug, Clone)]
pub struct AnalysisInputs {
    root: DocumentIdentity,
    dependencies: HashSet<DocumentIdentity>,
    expected_revisions: HashMap<DocumentIdentity, Option<DocumentRevision>>,
}

impl AnalysisInputs {
    #[cfg(test)]
    #[must_use]
    pub fn untracked_for_test(root: DocumentIdentity) -> Self {
        Self {
            root,
            dependencies: HashSet::new(),
            expected_revisions: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn root(&self) -> &DocumentIdentity {
        &self.root
    }

    pub fn dependencies(&self) -> impl Iterator<Item = &DocumentIdentity> {
        self.dependencies.iter()
    }

    #[must_use]
    pub fn root_is_current(&self, current: &HashMap<DocumentIdentity, DocumentRevision>) -> bool {
        self.expected_revisions
            .get(&self.root)
            .is_some_and(|expected| current.get(&self.root).copied() == *expected)
    }

    /// True only when every consumed identity has the same open/closed state
    /// and revision as it had when analysis started.
    #[must_use]
    pub fn is_current(&self, current: &HashMap<DocumentIdentity, DocumentRevision>) -> bool {
        self.expected_revisions
            .iter()
            .all(|(identity, expected)| current.get(identity).copied() == *expected)
    }
}

/// Reverse invalidation graph for the latest accepted open-document analyses.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    dependencies_by_root: HashMap<DocumentIdentity, HashSet<DocumentIdentity>>,
}

impl DependencyGraph {
    pub fn update(&mut self, root: DocumentIdentity, dependencies: HashSet<DocumentIdentity>) {
        self.dependencies_by_root.insert(root, dependencies);
    }

    pub fn remove(&mut self, root: &DocumentIdentity) {
        self.dependencies_by_root.remove(root);
    }

    /// Return every analyzed root that transitively consumes `changed`.
    #[must_use]
    pub fn transitive_importers(&self, changed: &DocumentIdentity) -> HashSet<DocumentIdentity> {
        let mut affected = HashSet::new();
        let mut frontier = vec![changed.clone()];
        while let Some(dependency) = frontier.pop() {
            for (root, dependencies) in &self.dependencies_by_root {
                if dependencies.contains(&dependency) && affected.insert(root.clone()) {
                    frontier.push(root.clone());
                }
            }
        }
        affected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> DocumentIdentity {
        DocumentIdentity::file(PathBuf::from(name))
    }

    #[test]
    fn analysis_inputs_compare_open_closed_state_and_revision() {
        let clock = RevisionClock::default();
        let root = file("root.gcl");
        let dependency = file("dep.gcl");
        let root_revision = clock.next().unwrap();
        let dependency_revision = clock.next().unwrap();
        let snapshot = AnalysisInputSnapshot::new(
            root.clone(),
            root_revision,
            HashMap::from([(dependency.clone(), dependency_revision)]),
        );
        let inputs = snapshot.finish(HashSet::from([dependency.clone()]));

        assert!(inputs.is_current(&HashMap::from([
            (root.clone(), root_revision),
            (dependency.clone(), dependency_revision),
        ])));
        assert!(!inputs.is_current(&HashMap::from([
            (root.clone(), root_revision),
            (dependency, clock.next().unwrap()),
        ])));
        assert!(!inputs.is_current(&HashMap::from([(root, root_revision)])));
    }

    #[test]
    fn dependency_opened_after_disk_snapshot_makes_result_stale() {
        let clock = RevisionClock::default();
        let root = file("root.gcl");
        let dependency = file("dep.gcl");
        let root_revision = clock.next().unwrap();
        let snapshot = AnalysisInputSnapshot::new(root.clone(), root_revision, HashMap::new());
        let inputs = snapshot.finish(HashSet::from([dependency.clone()]));

        assert!(inputs.is_current(&HashMap::from([(root.clone(), root_revision)])));
        assert!(!inputs.is_current(&HashMap::from([
            (root, root_revision),
            (dependency, clock.next().unwrap()),
        ])));
    }

    #[test]
    fn reverse_graph_finds_transitive_and_multiple_importers() {
        let leaf = file("leaf.gcl");
        let middle = file("middle.gcl");
        let first = file("first.gcl");
        let second = file("second.gcl");
        let unrelated = file("unrelated.gcl");
        let mut graph = DependencyGraph::default();
        graph.update(middle.clone(), HashSet::from([leaf.clone()]));
        graph.update(first.clone(), HashSet::from([middle.clone()]));
        graph.update(second.clone(), HashSet::from([leaf.clone()]));
        graph.update(unrelated, HashSet::new());

        assert_eq!(
            graph.transitive_importers(&leaf),
            HashSet::from([middle, first, second])
        );
    }
}
