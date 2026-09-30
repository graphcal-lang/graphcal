//! Freezing a checked project TIR's local bodies into the immutable
//! [`DagStore`] its module publishes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::resolved_name::ResolvedUnitName;
use crate::semantic::unit_scale::UnitInfo;

use super::checked::{CheckedDagRegistry, CheckedTir};
use super::dag_store::DagStore;

/// Failure to freeze the locally owned portion of a checked registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DagStoreFreezeError {
    /// Every runtime unit must have a local or imported defining body.
    #[error("runtime unit `{identity}` has no defining DAG in the assembly registry")]
    MissingUnitOwner { identity: ResolvedUnitName },
}

impl CheckedDagRegistry {
    /// Consume the local checked bodies into immutable handles.
    ///
    /// Imported handles are deliberately not copied into the new store: they
    /// already belong to another canonical store.
    pub(super) fn freeze_local(
        self,
        runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>,
    ) -> Result<DagStore, DagStoreFreezeError> {
        runtime_units.keys().try_for_each(|identity| {
            self.get(identity.owner()).map(|_| ()).ok_or_else(|| {
                DagStoreFreezeError::MissingUnitOwner {
                    identity: identity.clone(),
                }
            })
        })?;
        let mut dags = self
            .other_dags
            .into_iter()
            .map(|(id, dag)| (id, Arc::new(dag)))
            .collect::<HashMap<_, _>>();
        let root_id = self.root.dag_id().clone();
        dags.insert(root_id, Arc::new(self.root));
        // Imported unit definitions stay in their publishing module, just like
        // imported bodies. Only the final importing TIR needs their lookup index.
        let runtime_units = runtime_units
            .into_iter()
            .filter(|(identity, _)| dags.contains_key(identity.owner()))
            .collect();
        Ok(DagStore {
            dags,
            runtime_units,
        })
    }
}

impl CheckedTir {
    /// Consume this TIR's local bodies into an immutable store. Imported
    /// bodies and runtime units remain owned by their publishing module.
    ///
    /// # Errors
    ///
    /// Returns [`DagStoreFreezeError`] if a runtime unit has no defining body.
    pub fn freeze_local_dag_store(self) -> Result<DagStore, DagStoreFreezeError> {
        self.dags.freeze_local(self.core.into_runtime_units())
    }
}
