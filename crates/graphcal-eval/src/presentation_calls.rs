//! Evaluation-owned presentation call storage, separate from checked facts.
//!
//! Invocation handles retain a private allocation identity, not a global counter
//! or a convention that callers pair a row-local integer with the right store.
//! The token outlives its store when a handle does, preventing address reuse.
//! Whole-call environments remain temporary debt until presentation evidence
//! replaces replay; no checked artifact owns this mutable storage.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use graphcal_compiler::tir::presentation::PresentationCallKey;
use thiserror::Error;

use crate::execution_facts::RuntimeValueMap;

/// Allocation identity belonging to exactly one evaluation's call store.
#[derive(Debug, Default)]
struct InvocationScope;

/// Identity of one invocation, bound to its allocating evaluation.
#[derive(Debug, Clone)]
pub struct PresentationInvocationId {
    scope: Arc<InvocationScope>,
    ordinal: u64,
}

impl PartialEq for PresentationInvocationId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.scope, &other.scope) && self.ordinal == other.ordinal
    }
}

impl Eq for PresentationInvocationId {}

impl Hash for PresentationInvocationId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.scope), state);
        self.ordinal.hash(state);
    }
}

impl fmt::Display for PresentationInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.ordinal.fmt(formatter)
    }
}

#[derive(Debug)]
struct EvaluatedPresentationCall {
    key: PresentationCallKey,
    values: Arc<RuntimeValueMap>,
}

#[derive(Debug, Default)]
struct EvaluatedPresentationCallState {
    next_invocation: u64,
    by_invocation: HashMap<PresentationInvocationId, EvaluatedPresentationCall>,
}

/// Runtime environments retained by one evaluation, never by its shared plan.
#[derive(Debug, Default)]
pub struct EvaluatedPresentationCalls {
    scope: Arc<InvocationScope>,
    state: Mutex<EvaluatedPresentationCallState>,
}

/// Failure to retain or retrieve one evaluated call environment.
#[derive(Debug, Error)]
pub enum PresentationCallValuesError {
    #[error("evaluated presentation-call storage is poisoned")]
    Poisoned,
    #[error("presentation-call invocation identity space is exhausted")]
    IdentityExhausted,
    #[error("evaluated presentation-call invocation {invocation} belongs to another evaluation")]
    WrongEvaluation {
        invocation: PresentationInvocationId,
    },
    #[error("evaluated presentation-call invocation {invocation} is missing")]
    Missing {
        invocation: PresentationInvocationId,
    },
    #[error("evaluated presentation-call invocation {invocation} belongs to another call site")]
    WrongCallSite {
        invocation: PresentationInvocationId,
    },
}

impl EvaluatedPresentationCalls {
    /// Retain one invocation and return a handle scoped to this evaluation.
    pub fn record(
        &self,
        key: PresentationCallKey,
        values: RuntimeValueMap,
    ) -> Result<PresentationInvocationId, PresentationCallValuesError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PresentationCallValuesError::Poisoned)?;
        let next_invocation = state
            .next_invocation
            .checked_add(1)
            .ok_or(PresentationCallValuesError::IdentityExhausted)?;
        let invocation = PresentationInvocationId {
            scope: Arc::clone(&self.scope),
            ordinal: state.next_invocation,
        };
        state.next_invocation = next_invocation;
        let previous = state.by_invocation.insert(
            invocation.clone(),
            EvaluatedPresentationCall {
                key,
                values: Arc::new(values),
            },
        );
        debug_assert!(
            previous.is_none(),
            "fresh presentation invocation was reused"
        );
        drop(state);
        Ok(invocation)
    }

    /// Validate both the allocating evaluation and the static call site.
    pub fn invocation(
        &self,
        key: &PresentationCallKey,
        invocation: &PresentationInvocationId,
    ) -> Result<Arc<RuntimeValueMap>, PresentationCallValuesError> {
        if !Arc::ptr_eq(&self.scope, &invocation.scope) {
            return Err(PresentationCallValuesError::WrongEvaluation {
                invocation: invocation.clone(),
            });
        }
        let state = self
            .state
            .lock()
            .map_err(|_| PresentationCallValuesError::Poisoned)?;
        let call = state.by_invocation.get(invocation).ok_or_else(|| {
            PresentationCallValuesError::Missing {
                invocation: invocation.clone(),
            }
        })?;
        if &call.key != key {
            return Err(PresentationCallValuesError::WrongCallSite {
                invocation: invocation.clone(),
            });
        }
        let values = Arc::clone(&call.values);
        drop(state);
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
    use graphcal_compiler::syntax::non_empty::NonEmpty;
    use graphcal_compiler::syntax::span::Span;

    fn key(span: Span) -> PresentationCallKey {
        let expression = graphcal_compiler::hir::closed_expr::ClosedExpr::try_new(
            graphcal_compiler::hir::expr::Expr::new(
                graphcal_compiler::hir::expr::ExprKind::Bool(true),
                span,
            ),
        )
        .unwrap();
        PresentationCallKey::new(
            ResolvedDeclName::from_def(
                DagId::new("presentation-tests", NonEmpty::singleton(Arc::from("main"))),
                DeclName::expect_valid("output"),
            ),
            expression.id().unwrap().clone(),
        )
    }

    #[test]
    fn equal_call_sites_and_ordinals_cannot_cross_evaluations() {
        let site = key(Span::new(0, 1));
        let first = EvaluatedPresentationCalls::default();
        let second = EvaluatedPresentationCalls::default();
        let left = first.record(site.clone(), RuntimeValueMap::new()).unwrap();
        let right = second.record(site.clone(), RuntimeValueMap::new()).unwrap();
        assert_eq!(left.ordinal, right.ordinal);
        assert_ne!(left, right);
        assert!(matches!(
            second.invocation(&site, &left),
            Err(PresentationCallValuesError::WrongEvaluation { .. })
        ));
        drop(first);
        let third = EvaluatedPresentationCalls::default();
        assert!(matches!(
            third.invocation(&site, &left),
            Err(PresentationCallValuesError::WrongEvaluation { .. })
        ));
    }

    #[test]
    fn repeated_calls_have_distinct_handles_and_share_only_their_own_values() {
        let site = key(Span::new(0, 1));
        let calls = EvaluatedPresentationCalls::default();
        let first = calls.record(site.clone(), RuntimeValueMap::new()).unwrap();
        let second = calls.record(site.clone(), RuntimeValueMap::new()).unwrap();
        assert_ne!(first, second);
        assert!(Arc::ptr_eq(
            &calls.invocation(&site, &first).unwrap(),
            &calls.invocation(&site, &first.clone()).unwrap(),
        ));
        assert!(!Arc::ptr_eq(
            &calls.invocation(&site, &first).unwrap(),
            &calls.invocation(&site, &second).unwrap(),
        ));
        assert!(matches!(
            calls.invocation(&key(Span::new(0, 1)), &first),
            Err(PresentationCallValuesError::WrongCallSite { .. })
        ));
    }

    #[test]
    fn exhausted_identity_space_does_not_publish_an_invocation() {
        let calls = EvaluatedPresentationCalls::default();
        calls.state.lock().unwrap().next_invocation = u64::MAX;
        assert!(matches!(
            calls.record(key(Span::new(0, 1)), RuntimeValueMap::new()),
            Err(PresentationCallValuesError::IdentityExhausted)
        ));
        assert!(calls.state.lock().unwrap().by_invocation.is_empty());
    }
}
