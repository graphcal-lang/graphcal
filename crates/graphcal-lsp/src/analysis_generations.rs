//! The change generation of each open document and which generation's
//! analysis pass has finished, so a request can wait for the pass that answers
//! the document's latest text.
//!
//! Every edit, open or save starts a new generation; a pass whose generation
//! is no longer current must not publish. An exact-coordinate request that
//! arrives between a keystroke and the end of that text's (debounced) analysis
//! has no current analysis to answer from. Clients that support
//! `workspace/inlayHint/refresh` ask again when told to; others (Helix) keep
//! the empty answer until the view changes. Waiting for the pass lets such a
//! request answer the latest text instead.
//!
//! Generations are drawn from one process-wide counter, so a document closed
//! and reopened never reuses a generation a stale pass may still hold. That
//! lets closing a document forget its state entirely.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tower_lsp::lsp_types::Url;

/// One tracked document's generations.
#[derive(Debug, Clone, Copy)]
struct DocumentGenerations {
    current: u64,
    settled: u64,
}

#[derive(Debug, Default)]
struct State {
    /// The last generation handed out, for any document.
    latest: u64,
    documents: HashMap<Url, DocumentGenerations>,
}

impl State {
    const fn next(&mut self) -> u64 {
        self.latest = self.latest.saturating_add(1);
        self.latest
    }
}

/// Change generations and finished passes of the open documents.
#[derive(Debug, Default)]
pub struct AnalysisGenerations {
    state: Mutex<State>,
    notify: Notify,
}

impl AnalysisGenerations {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start a new generation of `uri`, tracking it from now on.
    pub(crate) fn bump(&self, uri: &Url) -> u64 {
        let mut state = self.state();
        let generation = state.next();
        state
            .documents
            .entry(uri.clone())
            .and_modify(|document| document.current = generation)
            .or_insert(DocumentGenerations {
                current: generation,
                settled: 0,
            });
        generation
    }

    /// Start a new generation of `uri` only while it is tracked (open):
    /// dependency-driven re-analysis must not resurrect a closed document.
    pub(crate) fn advance(&self, uri: &Url) -> Option<u64> {
        let mut state = self.state();
        let generation = state.next();
        let tracked = state
            .documents
            .get_mut(uri)
            .map(|document| document.current = generation)
            .is_some();
        drop(state);
        tracked.then_some(generation)
    }

    /// Whether `generation` is the current generation of `uri`. Nothing is
    /// current for a document that is not tracked.
    pub(crate) fn is_current(&self, uri: &Url, generation: u64) -> bool {
        self.state()
            .documents
            .get(uri)
            .is_some_and(|document| document.current == generation)
    }

    /// The current generation of `uri`, or 0 when it is not tracked.
    #[cfg(test)]
    pub(crate) fn current(&self, uri: &Url) -> u64 {
        self.state()
            .documents
            .get(uri)
            .map_or(0, |document| document.current)
    }

    /// Record that the pass of `generation` for `uri` finished, whatever its
    /// outcome: stored, superseded, failed, or timed out. A pass finishing
    /// after its document closed records nothing.
    pub(crate) fn settle(&self, uri: &Url, generation: u64) {
        if let Some(document) = self.state().documents.get_mut(uri) {
            document.settled = document.settled.max(generation);
        }
        self.notify.notify_waiters();
    }

    /// Stop tracking a closed document: none of its generations is current
    /// any more and nothing waits for its passes.
    pub(crate) fn forget(&self, uri: &Url) {
        self.state().documents.remove(uri);
        self.notify.notify_waiters();
    }

    fn is_settled(&self, uri: &Url) -> bool {
        self.state()
            .documents
            .get(uri)
            .is_none_or(|document| document.settled >= document.current)
    }

    /// The number of tracked documents.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.state().documents.len()
    }

    /// Wait until the pass of the current generation of `uri` has finished,
    /// or `timeout` elapses. Returns whether it finished.
    ///
    /// The current generation is read again after every finished pass: a
    /// newer edit supersedes the pass being waited for.
    pub(crate) async fn wait_for_current(&self, uri: &Url, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Register before checking, so a pass finishing in between still
            // wakes this waiter.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_settled(uri) {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn uri() -> Url {
        Url::parse("untitled:settle.gcl").unwrap()
    }

    #[tokio::test]
    async fn an_untracked_document_needs_no_wait() {
        let generations = AnalysisGenerations::default();
        assert!(generations.wait_for_current(&uri(), Duration::ZERO).await);
    }

    #[tokio::test]
    async fn waits_for_the_pass_of_the_latest_generation() {
        let generations = Arc::new(AnalysisGenerations::default());
        let superseded = generations.bump(&uri());
        let waiter = {
            let generations = Arc::clone(&generations);
            tokio::spawn(async move {
                generations
                    .wait_for_current(&uri(), Duration::from_secs(10))
                    .await
            })
        };
        tokio::task::yield_now().await;
        // A newer edit supersedes the first generation; its own pass ends
        // the wait.
        let latest = generations.bump(&uri());
        generations.settle(&uri(), superseded);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        generations.settle(&uri(), latest);
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn gives_up_after_the_timeout() {
        let generations = AnalysisGenerations::default();
        let first = generations.bump(&uri());
        generations.settle(&uri(), first);
        generations.bump(&uri());
        assert!(
            !generations
                .wait_for_current(&uri(), Duration::from_millis(10))
                .await
        );
    }

    /// LSP-16 (#2253): closing a document drops its state, a late pass of the
    /// closed document records nothing, and a reopened document never reuses
    /// a stale generation.
    #[test]
    fn closing_forgets_the_document_without_generation_reuse() {
        let generations = AnalysisGenerations::default();
        let stale = generations.bump(&uri());
        generations.forget(&uri());
        assert_eq!(generations.tracked(), 0);
        generations.settle(&uri(), stale);
        assert_eq!(generations.tracked(), 0, "a late pass resurrects nothing");
        assert_eq!(generations.advance(&uri()), None);
        assert_eq!(generations.tracked(), 0, "re-analysis resurrects nothing");

        let reopened = generations.bump(&uri());
        assert!(reopened > stale);
        assert!(!generations.is_current(&uri(), stale));
        assert!(generations.is_current(&uri(), reopened));
    }
}
