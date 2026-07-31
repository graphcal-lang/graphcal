//! Cooperative cancellation for synchronous compiler and evaluation work.
//!
//! Cancellation is intentionally independent of any async runtime. The LSP
//! shell owns a [`CancellationSource`] and passes its read-only
//! [`CancellationToken`] through the synchronous functional core. Long-running
//! passes call [`CancellationToken::checkpoint`] at deterministic boundaries.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Authority to cancel one operation and every token derived from it.
#[derive(Clone, Debug, Default)]
pub struct CancellationSource {
    state: Arc<AtomicBool>,
}

impl CancellationSource {
    /// Create a new, initially active cancellation source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a read-only token observing this source.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            state: Some(Arc::clone(&self.state)),
        }
    }

    /// Request cancellation.
    ///
    /// This operation is idempotent. `Relaxed` ordering is sufficient because
    /// the flag communicates no data; it only changes control flow.
    pub fn cancel(&self) {
        self.state.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has already been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Relaxed)
    }
}

/// Read-only cooperative cancellation handle for one operation.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    state: Option<Arc<AtomicBool>>,
}

impl CancellationToken {
    /// A token that can never be cancelled.
    ///
    /// Batch compiler and CLI entry points use this mode so they share the
    /// controlled implementation without allocating cancellation state.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self { state: None }
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.load(Ordering::Relaxed))
    }

    /// Return [`Cancelled`] once cancellation has been requested.
    ///
    /// Call this at recursive choke points and between bounded units of work,
    /// rather than in tight arithmetic kernels.
    ///
    /// # Errors
    ///
    /// Returns [`Cancelled`] after the associated source is cancelled.
    pub fn checkpoint(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Distinct control-flow outcome for cooperatively cancelled work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, miette::Diagnostic)]
#[error("operation cancelled")]
#[diagnostic(code(graphcal::cancelled))]
pub struct Cancelled;

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn cancellation_stops_cooperative_work() {
        let source = CancellationSource::new();
        let token = source.token();
        let started = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            worker_started.wait();
            let result = std::iter::repeat_with(|| token.checkpoint())
                .find(Result::is_err)
                .expect("the source should eventually be cancelled");
            sender
                .send(result)
                .expect("test receiver should remain open");
        });

        started.wait();
        source.cancel();

        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("cooperative work should stop promptly"),
            Err(Cancelled)
        );
    }

    #[test]
    fn unbounded_token_never_cancels() {
        let token = CancellationToken::unbounded();
        assert!(!token.is_cancelled());
        assert_eq!(token.checkpoint(), Ok(()));
    }
}
