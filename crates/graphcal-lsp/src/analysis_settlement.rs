//! Which change generation of each document has finished its analysis pass,
//! so a request can wait for the pass that answers the document's latest text.
//!
//! An exact-coordinate request that arrives between a keystroke and the end of
//! that text's (debounced) analysis has no current analysis to answer from.
//! Clients that support `workspace/inlayHint/refresh` ask again when told to;
//! others (Helix) keep the empty answer until the view changes. Waiting for the
//! pass lets such a request answer the latest text instead.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, RwLock};
use tower_lsp::lsp_types::Url;

/// The latest finished analysis pass of each document, by change generation.
#[derive(Debug, Default)]
pub struct AnalysisSettlement {
    settled: Mutex<HashMap<Url, u64>>,
    notify: Notify,
}

impl AnalysisSettlement {
    /// Record that the pass of `generation` for `uri` finished, whatever its
    /// outcome: stored, superseded, failed, or timed out.
    pub(crate) fn settle(&self, uri: &Url, generation: u64) {
        self.settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(uri.clone())
            .and_modify(|latest| *latest = (*latest).max(generation))
            .or_insert(generation);
        self.notify.notify_waiters();
    }

    fn is_settled(&self, uri: &Url, generation: u64) -> bool {
        self.settled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(uri)
            .copied()
            .unwrap_or_default()
            >= generation
    }

    /// Wait until the pass of the current generation of `uri` in
    /// `generations` has finished, or `timeout` elapses. Returns whether it
    /// finished.
    ///
    /// The current generation is read again after every finished pass: a
    /// newer edit supersedes the pass being waited for.
    pub(crate) async fn wait_for_current(
        &self,
        uri: &Url,
        generations: &RwLock<HashMap<Url, u64>>,
        timeout: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Register before checking, so a pass finishing in between still
            // wakes this waiter.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let current = generations
                .read()
                .await
                .get(uri)
                .copied()
                .unwrap_or_default();
            if self.is_settled(uri, current) {
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
    async fn an_unchanged_document_needs_no_wait() {
        let settlement = AnalysisSettlement::default();
        let generations = RwLock::new(HashMap::new());
        assert!(
            settlement
                .wait_for_current(&uri(), &generations, Duration::ZERO)
                .await
        );
    }

    #[tokio::test]
    async fn waits_for_the_pass_of_the_latest_generation() {
        let settlement = Arc::new(AnalysisSettlement::default());
        let generations = Arc::new(RwLock::new(HashMap::from([(uri(), 2)])));
        let waiter = {
            let settlement = Arc::clone(&settlement);
            let generations = Arc::clone(&generations);
            tokio::spawn(async move {
                settlement
                    .wait_for_current(&uri(), &generations, Duration::from_secs(10))
                    .await
            })
        };
        tokio::task::yield_now().await;
        // A newer edit supersedes generation 2; its own pass ends the wait.
        generations.write().await.insert(uri(), 3);
        settlement.settle(&uri(), 2);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        settlement.settle(&uri(), 3);
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn gives_up_after_the_timeout() {
        let settlement = AnalysisSettlement::default();
        settlement.settle(&uri(), 1);
        let generations = RwLock::new(HashMap::from([(uri(), 2)]));
        assert!(
            !settlement
                .wait_for_current(&uri(), &generations, Duration::from_millis(10))
                .await
        );
    }
}
