//! Failure classification that keeps cooperative cancellation out of error
//! types.
//!
//! A cancellable operation fails in one of two fundamentally different ways:
//! the embedding shell withdrew interest ([`Outcome::Cancelled`]), or the work
//! itself failed ([`Outcome::Failed`]). Folding the first into a phase's error
//! enum forces every consumer of that enum to remember that one "error" is not
//! a diagnostic. `Outcome` makes the distinction structural instead: the phase
//! error type `E` never needs a cancellation variant, and shells pattern-match
//! the two cases exhaustively.
//!
//! Use `Result<T, Outcome<E>>` as the return type of operations that observe a
//! [`CancellationToken`]. `?` on
//! [`CancellationToken::checkpoint`](crate::cancellation::CancellationToken::checkpoint)
//! converts through [`From<Cancelled>`]; phase errors are wrapped with
//! [`Outcome::Failed`] (or [`Outcome::map_failed`] when re-typing an upstream
//! outcome), so a phase error can never be mistaken for cancellation. A phase
//! error type `E` may implement `From<E> for Outcome<E>` (always
//! [`Outcome::Failed`]) so `?` wraps it; only [`Cancelled`] ever produces
//! [`Outcome::Cancelled`].

use crate::cancellation::{CancellationToken, Cancelled};

/// Why a cancellable operation did not produce a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Outcome<E> {
    /// The embedding shell cancelled the operation. This is control flow, not
    /// a diagnostic, and must never be rendered as one.
    Cancelled,
    /// The operation ran to a failure described by `E`.
    Failed(E),
}

impl<E> Outcome<E> {
    /// Re-type the failure while preserving cancellation.
    pub fn map_failed<F>(self, map: impl FnOnce(E) -> F) -> Outcome<F> {
        match self {
            Self::Cancelled => Outcome::Cancelled,
            Self::Failed(error) => Outcome::Failed(map(error)),
        }
    }

    /// Re-type the failure into a composed error type, keeping cancellation.
    pub fn map_into<F: From<E>>(self) -> Outcome<F> {
        self.map_failed(F::from)
    }
}

impl<E> From<Cancelled> for Outcome<E> {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Run a cancellable operation that nothing can cancel.
///
/// `operation` observes a token that never cancels, so its only possible
/// failure is `E`. This is the one place a caller without a cancellation
/// source discharges [`Outcome::Cancelled`].
///
/// # Errors
///
/// Returns the failure `operation` reports.
pub fn without_cancellation<T, E>(
    operation: impl FnOnce(&CancellationToken) -> Result<T, Outcome<E>>,
) -> Result<T, E> {
    operation(&CancellationToken::unbounded()).map_err(|outcome| match outcome {
        Outcome::Failed(error) => error,
        #[expect(
            clippy::unreachable,
            reason = "an unbounded cancellation token never cancels"
        )]
        Outcome::Cancelled => unreachable!("an unbounded cancellation token never cancels"),
    })
}

/// Whether a session of cancellable operations can be cancelled, and so how
/// its operations fail.
///
/// A session a shell cannot cancel ([`Uncancellable`]) fails with the phase
/// error alone; a session observing a token ([`Cancellable`]) fails with an
/// [`Outcome`]. Callers of an uncancellable session never handle a
/// cancellation that cannot happen.
pub trait CancellationMode: Clone {
    /// How an operation of this session fails with the phase error `E`.
    type Failure<E>;

    /// Run a cancellable operation under this session's cancellation.
    ///
    /// # Errors
    ///
    /// Returns the failure `operation` reports, as this session reports it.
    fn run<T, E>(
        &self,
        operation: impl FnOnce(&CancellationToken) -> Result<T, Outcome<E>>,
    ) -> Result<T, Self::Failure<E>>;
}

/// A session no shell can cancel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Uncancellable;

impl CancellationMode for Uncancellable {
    type Failure<E> = E;

    fn run<T, E>(
        &self,
        operation: impl FnOnce(&CancellationToken) -> Result<T, Outcome<E>>,
    ) -> Result<T, E> {
        without_cancellation(operation)
    }
}

/// A session that observes the embedding shell's cancellation token.
#[derive(Clone, Debug)]
pub struct Cancellable(pub CancellationToken);

impl CancellationMode for Cancellable {
    type Failure<E> = Outcome<E>;

    fn run<T, E>(
        &self,
        operation: impl FnOnce(&CancellationToken) -> Result<T, Outcome<E>>,
    ) -> Result<T, Outcome<E>> {
        operation(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::cancellation::{CancellationSource, CancellationToken};

    use super::*;

    fn checked_work(token: &CancellationToken, fail: bool) -> Result<u8, Outcome<&'static str>> {
        token.checkpoint()?;
        if fail {
            return Err(Outcome::Failed("failed"));
        }
        Ok(1)
    }

    #[test]
    fn checkpoint_question_mark_yields_cancelled() {
        let source = CancellationSource::new();
        source.cancel();
        assert_eq!(checked_work(&source.token(), true), Err(Outcome::Cancelled));
    }

    #[test]
    fn failures_stay_distinct_from_cancellation() {
        let token = CancellationToken::unbounded();
        assert_eq!(checked_work(&token, true), Err(Outcome::Failed("failed")));
        assert_eq!(checked_work(&token, false), Ok(1));
    }

    #[test]
    fn map_failed_preserves_cancellation() {
        assert_eq!(
            Outcome::<u8>::Cancelled.map_failed(u16::from),
            Outcome::<u16>::Cancelled
        );
        assert_eq!(
            Outcome::Failed(2_u8).map_failed(u16::from),
            Outcome::Failed(2_u16)
        );
        assert_eq!(
            Outcome::<u8>::Cancelled.map_into::<u16>(),
            Outcome::Cancelled
        );
        assert_eq!(
            Outcome::Failed(2_u8).map_into::<u16>(),
            Outcome::Failed(2_u16)
        );
    }

    #[test]
    fn uncancellable_sessions_fail_with_the_phase_error_alone() {
        assert_eq!(
            without_cancellation(|token| checked_work(token, true)),
            Err("failed")
        );
        assert_eq!(Uncancellable.run(|token| checked_work(token, false)), Ok(1));
        assert_eq!(
            Uncancellable.run(|token| checked_work(token, true)),
            Err("failed")
        );
    }

    #[test]
    fn cancellable_sessions_observe_their_token() {
        let source = CancellationSource::new();
        let session = Cancellable(source.token());
        assert_eq!(session.run(|token| checked_work(token, false)), Ok(1));
        assert_eq!(
            session.run(|token| checked_work(token, true)),
            Err(Outcome::Failed("failed"))
        );
        source.cancel();
        assert_eq!(
            session.run(|token| checked_work(token, false)),
            Err(Outcome::Cancelled)
        );
    }
}
