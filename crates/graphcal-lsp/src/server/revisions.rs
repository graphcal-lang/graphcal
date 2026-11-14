//! Observation of disk revisions by in-flight passes, and their pruning.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::RwLock;

use crate::workspace_revision::{DependencyGraph, DocumentIdentity, DocumentRevision};

/// Analysis passes between observing the current revisions and checking
/// them again. Disk revisions are pruned only while there is none, so no pass
/// can miss a change that was recorded and pruned during its flight.
#[derive(Debug, Default)]
pub(super) struct RevisionObservers(AtomicUsize);

impl RevisionObservers {
    /// Register one observing pass. Requiring the disk-revision read guard
    /// the observation reads ensures a pruner holding the write lock sees it.
    pub(super) fn observe(
        &self,
        _observed: &tokio::sync::RwLockReadGuard<'_, HashMap<DocumentIdentity, DocumentRevision>>,
    ) -> RevisionObservation<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        RevisionObservation(self)
    }

    fn any(&self) -> bool {
        self.0.load(Ordering::SeqCst) != 0
    }
}

/// One registered observing pass; ends when dropped.
pub(super) struct RevisionObservation<'a>(&'a RevisionObservers);

impl Drop for RevisionObservation<'_> {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Forget the disk revisions of documents no dependency edge references, so
/// the map does not grow with every file a watcher ever reported.
pub(super) async fn prune_filesystem_revisions(
    filesystem_revisions: &RwLock<HashMap<DocumentIdentity, DocumentRevision>>,
    dependency_graph: &RwLock<DependencyGraph>,
    observers: &RevisionObservers,
) {
    let mut disk = filesystem_revisions.write().await;
    if observers.any() {
        return;
    }
    dependency_graph.read().await.retain_referenced(&mut disk);
}
