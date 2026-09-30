//! Violated evaluator invariants, kept apart from user-facing failures.
//!
//! Some runtime checks guard states that the checker or a validated
//! constructor already rules out. Such a check failing is a bug in Graphcal,
//! not a problem with the program being evaluated, so it must never be
//! reported as an ordinary evaluation error. An operation with both kinds of
//! failure returns [`Failure<E>`]: `E` lists only the user-facing failures,
//! and [`Invariant`] is the single carrier for violated invariants, which the
//! evaluator always reports as internal errors. A cancellable operation
//! returns `Outcome<Failure<E>>`, keeping cancellation out of both.

use graphcal_compiler::outcome::Outcome;

/// A violated evaluator invariant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invariant(String);

impl Invariant {
    /// Record a violated invariant, described for the internal error.
    pub fn violated(description: impl std::fmt::Display) -> Self {
        Self(description.to_string())
    }
}

/// Why a runtime operation failed.
#[derive(Debug)]
pub enum Failure<E> {
    /// A user-facing failure described by `E`.
    Error(E),
    /// A violated invariant (a bug in Graphcal).
    Invariant(Invariant),
}

impl<E> Failure<E> {
    /// Re-type the user-facing failure, keeping a violated invariant.
    pub fn map_error<F>(self, map: impl FnOnce(E) -> F) -> Failure<F> {
        match self {
            Self::Error(error) => Failure::Error(map(error)),
            Self::Invariant(invariant) => Failure::Invariant(invariant),
        }
    }
}

impl<E> From<Invariant> for Failure<E> {
    fn from(invariant: Invariant) -> Self {
        Self::Invariant(invariant)
    }
}

impl<E> From<Invariant> for Outcome<Failure<E>> {
    fn from(invariant: Invariant) -> Self {
        Self::Failed(Failure::Invariant(invariant))
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::cancellation::Cancelled;

    use super::{Failure, Invariant, Outcome};

    #[test]
    fn invariants_render_their_description_and_convert_into_failures() {
        let invariant = Invariant::violated(format_args!("rank {} != {}", 1, 2));
        assert_eq!(invariant.to_string(), "rank 1 != 2");
        let failure: Failure<&str> = invariant.clone().into();
        assert!(matches!(failure, Failure::Invariant(ref found) if *found == invariant));
        let outcome: Outcome<Failure<&str>> = invariant.clone().into();
        assert!(
            matches!(outcome, Outcome::Failed(Failure::Invariant(found)) if found == invariant)
        );
        let cancelled: Outcome<Failure<&str>> = Cancelled.into();
        assert!(matches!(cancelled, Outcome::Cancelled));
    }

    #[test]
    fn mapping_the_error_keeps_invariants() {
        assert!(matches!(
            Failure::Error(2_u8).map_error(u16::from),
            Failure::Error(2_u16)
        ));
        assert!(matches!(
            Failure::<u8>::Invariant(Invariant::violated("broken")).map_error(u16::from),
            Failure::Invariant(invariant) if invariant.to_string() == "broken"
        ));
    }
}
