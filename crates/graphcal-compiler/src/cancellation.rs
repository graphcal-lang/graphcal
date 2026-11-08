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
            #[cfg(feature = "test-cancellation")]
            deterministic: None,
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
    #[cfg(feature = "test-cancellation")]
    deterministic: Option<Arc<DeterministicCancellation>>,
}

#[cfg(feature = "test-cancellation")]
#[derive(Debug)]
enum DeterministicCancellation {
    /// Cancel after this many more successful checkpoints.
    Checkpoints {
        remaining_successful_checkpoints: std::sync::atomic::AtomicUsize,
    },
    /// Cancel once the creating thread has made `at` steps of `phase`.
    Progress { phase: ProgressPhase, at: usize },
}

/// A unit of potentially exponential work that tests can watch, to cancel
/// only once it is under way.
#[cfg(feature = "test-cancellation")]
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressPhase {
    /// One include instance expanded by the module resolver.
    IncludeExpansion,
    /// One semantic include edge instantiated.
    Instantiation,
}

#[cfg(feature = "test-cancellation")]
thread_local! {
    static PROGRESS: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0; 2]) };
}

#[cfg(feature = "test-cancellation")]
impl ProgressPhase {
    const fn slot(self) -> usize {
        match self {
            Self::IncludeExpansion => 0,
            Self::Instantiation => 1,
        }
    }

    /// Steps of this phase the current thread has made so far.
    #[doc(hidden)]
    #[must_use]
    pub fn steps_on_this_thread(self) -> usize {
        PROGRESS.with(|progress| progress.get()[self.slot()])
    }

    /// Record one step of this phase on the current thread.
    pub(crate) fn record(self) {
        PROGRESS.with(|progress| {
            let mut steps = progress.get();
            steps[self.slot()] = steps[self.slot()].saturating_add(1);
            progress.set(steps);
        });
    }
}

impl CancellationToken {
    /// A token that can never be cancelled.
    ///
    /// Batch compiler and CLI entry points use this mode so they share the
    /// controlled implementation without allocating cancellation state.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            state: None,
            #[cfg(feature = "test-cancellation")]
            deterministic: None,
        }
    }

    /// Test-only token that cancels after an exact number of successful calls.
    #[cfg(feature = "test-cancellation")]
    #[doc(hidden)]
    #[must_use]
    pub fn cancel_after_successful_checkpoints(checkpoints: usize) -> Self {
        Self {
            state: None,
            deterministic: Some(Arc::new(DeterministicCancellation::Checkpoints {
                remaining_successful_checkpoints: std::sync::atomic::AtomicUsize::new(checkpoints),
            })),
        }
    }

    /// Test-only token that cancels once the current thread has made
    /// `steps` more steps of `phase`: every checkpoint after that point
    /// fails, so the work stops at the phase's next checkpoint.
    #[cfg(feature = "test-cancellation")]
    #[doc(hidden)]
    #[must_use]
    pub fn cancel_after_progress(phase: ProgressPhase, steps: usize) -> Self {
        Self {
            state: None,
            deterministic: Some(Arc::new(DeterministicCancellation::Progress {
                phase,
                at: phase.steps_on_this_thread().saturating_add(steps),
            })),
        }
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.load(Ordering::Relaxed))
            || self.deterministic_is_cancelled()
    }

    #[cfg(feature = "test-cancellation")]
    fn deterministic_is_cancelled(&self) -> bool {
        self.deterministic
            .as_deref()
            .is_some_and(|state| match state {
                DeterministicCancellation::Checkpoints {
                    remaining_successful_checkpoints,
                } => remaining_successful_checkpoints
                    .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_err(),
                DeterministicCancellation::Progress { phase, at } => {
                    phase.steps_on_this_thread() >= *at
                }
            })
    }

    #[cfg(not(feature = "test-cancellation"))]
    #[expect(
        clippy::unused_self,
        reason = "the feature-gated implementation reads per-token deterministic state"
    )]
    const fn deterministic_is_cancelled(&self) -> bool {
        false
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
