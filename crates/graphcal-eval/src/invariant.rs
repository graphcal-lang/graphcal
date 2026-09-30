//! Violated evaluator invariants, kept apart from user-facing failures.
//!
//! Some runtime checks guard states that the checker or a validated
//! constructor already rules out. Such a check failing is a bug in Graphcal,
//! not a problem with the program being evaluated, so it must never be
//! reported as an ordinary evaluation error. An operation with both kinds of
//! failure returns [`Failure<E>`]: `E` lists only the user-facing failures,
//! and [`Invariant`] is the single carrier for violated invariants, which the
//! evaluator always reports as internal errors.

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

impl<E> From<Invariant> for Failure<E> {
    fn from(invariant: Invariant) -> Self {
        Self::Invariant(invariant)
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, Invariant};

    #[test]
    fn invariants_render_their_description_and_convert_into_failures() {
        let invariant = Invariant::violated(format_args!("rank {} != {}", 1, 2));
        assert_eq!(invariant.to_string(), "rank 1 != 2");
        let failure: Failure<&str> = invariant.clone().into();
        assert!(matches!(failure, Failure::Invariant(found) if found == invariant));
    }
}
