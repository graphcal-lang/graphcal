//! The memo of static TODO reachability shared by every evaluation of one
//! execution plan.
//!
//! Reachability is value-independent, so an answer computed for one
//! expression holds for every later expression and evaluation of the same
//! plan. The analysis itself lives in `static_incompleteness`; this module
//! only owns its answers, so the plan can hold them without depending on the
//! analysis.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, PoisonError};

use graphcal_compiler::resolved_name::ResolvedDeclName;

/// The parameters a call binds explicitly, which cut off their defaults.
pub type BoundParameters = BTreeSet<ResolvedDeclName>;
/// A declaration and the parameters bound around it.
pub type Query = (ResolvedDeclName, BoundParameters);
/// The unfinished declarations a query can reach.
pub type Origins = BTreeSet<ResolvedDeclName>;

/// Completed reachability answers of one plan.
///
/// The lock is held only inside [`Self::get`] and [`Self::record`], never
/// across an analysis, so concurrent evaluations of one plan never wait for
/// each other's traversal and keep observing their own cancellation. Two
/// evaluations racing on one query may both compute it; the answers are
/// value-independent, so either is the answer.
#[derive(Debug, Default)]
pub struct UnfinishedReachMemo {
    answers: Mutex<HashMap<Query, Origins>>,
}

impl UnfinishedReachMemo {
    /// The completed answer to `query`, if any evaluation recorded one.
    pub(crate) fn get(&self, query: &Query) -> Option<Origins> {
        self.lock().get(query).cloned()
    }

    /// Record the completed answer to `query`, keeping an earlier one.
    pub(crate) fn record(&self, query: Query, origins: Origins) {
        self.lock().entry(query).or_insert(origins);
    }

    /// Only completed answers are ever stored, and no code panics while
    /// holding the lock, so poison never marks an inconsistent map.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Query, Origins>> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
