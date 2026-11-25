//! Strict evaluation of several inputs of one operation.
//!
//! Every input is evaluated, even after one of them has no value. When more
//! than one has none because of an unfinished formula or a failed
//! declaration, their reasons are merged, so the reported reason names every
//! origin and does not depend on the order the inputs are written in. Any
//! other failure, including a violated invariant, is reported as it is: it
//! explains itself, and it is never hidden by an unfinished input.

use graphcal_compiler::diagnostic::Diagnostic;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::semantic_error::{SemanticError, SemanticErrorKind};

type Evaluated<T> = Result<T, Outcome<SemanticError>>;

/// Both inputs, or why one of them has no value.
pub(super) fn both<A, B>(first: Evaluated<A>, second: Evaluated<B>) -> Evaluated<(A, B)> {
    match (first, second) {
        (Ok(first), Ok(second)) => Ok((first, second)),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(merge(first, second)),
    }
}

/// Every input, or why some of them have no value.
#[expect(
    clippy::manual_try_fold,
    reason = "every input is evaluated, also after one has no value"
)]
pub(super) fn all<T>(inputs: impl IntoIterator<Item = Evaluated<T>>) -> Evaluated<Vec<T>> {
    inputs.into_iter().fold(Ok(Vec::new()), |collected, input| {
        both(collected, input).map(|(mut collected, input)| {
            collected.push(input);
            collected
        })
    })
}

/// Why two inputs have no value, as one reason.
pub(super) fn merge(
    first: Outcome<SemanticError>,
    second: Outcome<SemanticError>,
) -> Outcome<SemanticError> {
    match (first, second) {
        (Outcome::Cancelled, _) | (_, Outcome::Cancelled) => Outcome::Cancelled,
        (Outcome::Failed(first), Outcome::Failed(second)) => {
            Outcome::Failed(merge_errors(first, second))
        }
    }
}

fn merge_errors(mut first: SemanticError, second: SemanticError) -> SemanticError {
    match (unavailable(&mut first), reason(second)) {
        (Some(reason), Ok(other)) => {
            let merged = reason.clone().merged(other);
            *reason = merged;
            first
        }
        // `second` failed by itself: it is reported, never hidden.
        (Some(_), Err(second)) => second,
        (None, _) => first,
    }
}

/// The reason of an error that only says a value is unavailable.
const fn unavailable(
    error: &mut SemanticError,
) -> Option<&mut graphcal_compiler::node_unavailable::RuntimeUnavailable> {
    match error {
        SemanticError::Located(Diagnostic {
            kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason }),
            ..
        }) => Some(reason),
        _ => None,
    }
}

/// The reason of `error` if it only says a value is unavailable, else the
/// error itself.
fn reason(
    error: SemanticError,
) -> Result<graphcal_compiler::node_unavailable::RuntimeUnavailable, SemanticError> {
    match error {
        SemanticError::Located(Diagnostic {
            kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason }),
            ..
        }) => Ok(reason),
        other => Err(other),
    }
}
