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
//! [`CancellationToken`](crate::cancellation::CancellationToken). `?` on
//! [`CancellationToken::checkpoint`](crate::cancellation::CancellationToken::checkpoint)
//! converts through [`From<Cancelled>`]; phase errors are wrapped explicitly
//! with [`Outcome::Failed`] (or [`Outcome::map_failed`] when re-typing an
//! upstream outcome), so a phase error can never be mistaken for cancellation.

use crate::cancellation::Cancelled;

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
}

impl<E> From<Cancelled> for Outcome<E> {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
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
    }
}
