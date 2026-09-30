//! The immutable store of checked bodies one module publishes.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::dag_id::DagId;
use crate::resolved_name::ResolvedUnitName;
use crate::semantic::unit_scale::UnitInfo;

use super::checked_dag::CheckedDag;

/// Immutable canonical checked bodies published by one module in a
/// compilation session.
///
/// The store is created by consuming a
/// [`CheckedTir`](super::checked::CheckedTir). It has no mutation or
/// completion API. Cloning it shares body and unit handles; project artifacts
/// share the complete store through `Arc`.
///
/// A store records every DAG outside it that its bodies call, so a program
/// can install it only together with the bodies it calls into.
#[derive(Debug, Clone)]
pub struct DagStore {
    pub(super) dags: HashMap<DagId, Arc<CheckedDag>>,
    pub(super) runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    /// Every DAG outside this store a body of it calls, with the first
    /// calling DAG in identity order.
    pub(super) external_callees: BTreeMap<DagId, DagId>,
}

/// Every DAG outside `dags` that a body of `dags` calls, with the first
/// calling DAG in identity order.
pub(super) fn external_callees(dags: &HashMap<DagId, Arc<CheckedDag>>) -> BTreeMap<DagId, DagId> {
    let mut calls = dags
        .iter()
        .flat_map(|(caller, dag)| {
            dag.call_targets()
                .iter()
                .map(move |(_, target)| (target, caller))
        })
        .filter(|(target, _)| !dags.contains_key(*target))
        .collect::<Vec<_>>();
    calls.sort();
    let mut external = BTreeMap::new();
    for (target, caller) in calls {
        external
            .entry(target.clone())
            .or_insert_with(|| caller.clone());
    }
    external
}

impl DagStore {
    /// Every DAG outside this store that its bodies call, each with one
    /// calling DAG of the store.
    pub fn external_callees(&self) -> impl Iterator<Item = (&DagId, &DagId)> {
        self.external_callees.iter()
    }

    /// Look up a canonical immutable body.
    #[must_use]
    pub fn get(&self, dag_id: &DagId) -> Option<&CheckedDag> {
        self.dags.get(dag_id).map(AsRef::as_ref)
    }

    /// Borrow the canonical body handle for pointer-identity checks and sharing.
    #[must_use]
    pub fn handle(&self, dag_id: &DagId) -> Option<&Arc<CheckedDag>> {
        self.dags.get(dag_id)
    }

    /// Iterate over canonical immutable bodies.
    pub fn iter(&self) -> impl Iterator<Item = (&DagId, &CheckedDag)> {
        self.dags.iter().map(|(id, dag)| (id, dag.as_ref()))
    }

    /// Look up a runtime unit overlay owned by this publishing module.
    #[must_use]
    pub fn unit_info(&self, name: &ResolvedUnitName) -> Option<&UnitInfo> {
        self.runtime_units.get(name).map(AsRef::as_ref)
    }

    /// Number of canonical bodies in this store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dags.len()
    }

    /// Whether this store has no bodies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dags.is_empty()
    }
}
