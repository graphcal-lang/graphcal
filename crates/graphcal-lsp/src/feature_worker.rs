//! Runs CPU-bound editor-feature work off tower-lsp's serving task.
//!
//! tower-lsp 0.20 polls message input, its request handlers and output inside
//! one task, so synchronous work inside a handler blocks every other request
//! and the processing of `$/cancelRequest` itself. Feature work therefore runs
//! on Tokio's blocking pool while the handler only awaits it. Cancelling the
//! request drops that await, which cancels the work's token.

use graphcal_compiler::cancellation::CancellationToken;

use crate::cancel_on_drop::CancelOnDrop;

/// Why feature work produced no answer.
#[derive(Debug, thiserror::Error)]
pub enum FeatureWorkError {
    #[error("editor feature worker panicked: {0}")]
    Panicked(String),
}

/// Run `work` on the blocking pool with a token that is cancelled as soon as
/// the returned future is dropped (for example by `$/cancelRequest`).
///
/// Work that is still queued when its request is cancelled never starts.
pub async fn run_feature<R, F>(work: F) -> Result<Option<R>, FeatureWorkError>
where
    F: FnOnce(&CancellationToken) -> Option<R> + Send + 'static,
    R: Send + 'static,
{
    let cancel_on_drop = CancelOnDrop::new();
    let token = cancel_on_drop.token();
    let task = tokio::task::spawn_blocking(move || {
        if token.is_cancelled() {
            return None;
        }
        work(&token)
    });
    let result = task
        .await
        .map_err(|error| FeatureWorkError::Panicked(error.to_string()));
    drop(cancel_on_drop);
    result
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn returns_the_work_result() {
        let result = run_feature(|_| Some(42)).await.unwrap();
        assert_eq!(result, Some(42));
    }

    #[tokio::test]
    async fn a_panicking_worker_becomes_an_error() {
        let result = run_feature::<(), _>(|_| panic!("boom")).await;
        assert!(matches!(result, Err(FeatureWorkError::Panicked(_))));
    }

    /// Dropping the waiter (what tower-lsp does on `$/cancelRequest`) cancels
    /// the running work, and the serving thread stays free meanwhile.
    #[tokio::test]
    async fn dropping_the_waiter_cancels_the_running_work() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let waiter = tokio::spawn(run_feature(move |token| {
            let _ = started_tx.send(());
            while !token.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = observed_tx.send(());
            None::<()>
        }));
        started_rx.await.unwrap();
        // The current-thread runtime still makes progress while the work runs.
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        tokio::task::spawn_blocking(move || {
            observed_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("the worker observes the cancellation");
        })
        .await
        .unwrap();
    }
}
