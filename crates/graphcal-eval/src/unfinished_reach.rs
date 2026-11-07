//! The memo of static TODO reachability shared by every evaluation of one
//! execution plan.
//!
//! Reachability is value-independent, so an answer computed for one
//! expression holds for every later expression and evaluation of the same
//! plan. The analysis itself lives in `static_incompleteness`; this module
//! only owns its answers, so the plan can hold them without depending on the
//! analysis.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use graphcal_compiler::resolved_name::ResolvedDeclName;

/// The parameters a call binds explicitly, which cut off their defaults.
pub type BoundParameters = BTreeSet<ResolvedDeclName>;
/// A declaration and the parameters bound around it.
pub type Query = (ResolvedDeclName, BoundParameters);
/// The unfinished declarations a query can reach.
pub type Origins = BTreeSet<ResolvedDeclName>;

/// Completed reachability answers of one plan.
#[derive(Debug, Default)]
pub struct UnfinishedReachMemo {
    answers: Mutex<HashMap<Query, Origins>>,
}

impl UnfinishedReachMemo {
    /// Exclusive access to the answers for one analysis.
    ///
    /// Only completed answers are ever stored, so a panic during another
    /// analysis leaves the map consistent and its poison is ignored.
    pub(crate) fn answers(&self) -> MutexGuard<'_, HashMap<Query, Origins>> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
