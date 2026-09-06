//! Immutable execution-plan data, independent of preparation algorithms.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::assertion_expectation::ExpectedFail;
use graphcal_compiler::dag_id::DagId;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;
use crate::declaration_locations::DeclarationLocations;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_facts::{CheckedExecutionFacts, RuntimeValueMap};

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Physical bodies selected from completed declaration indexes at preparation.
    pub(crate) declaration_locations: DeclarationLocations,
    pub(crate) root: CallablePlan,
    /// Non-root bodies; the root has the same callable contract without a copy.
    pub(crate) callables: HashMap<DagId, CallablePlan>,
    pub(crate) checked_execution_facts: CheckedExecutionFacts,
}

#[derive(Debug, Error)]
pub enum CallablePlanError {
    #[error("DAG `{0}` has no prepared callable plan")]
    Missing(DagId),
    #[error("prepared callable plan for `{expected}` belongs to `{actual}`")]
    WrongOwner { expected: DagId, actual: DagId },
}

impl ExecPlan {
    pub(crate) fn callable(&self, owner: &DagId) -> Result<&CallablePlan, CallablePlanError> {
        let plan = if owner == &self.root.owner {
            &self.root
        } else {
            self.callables
                .get(owner)
                .ok_or_else(|| CallablePlanError::Missing(owner.clone()))?
        };
        if &plan.owner != owner {
            return Err(CallablePlanError::WrongOwner {
                expected: owner.clone(),
                actual: plan.owner.clone(),
            });
        }
        Ok(plan)
    }
}

/// One body and its included-instance closure, prepared before evaluation.
#[derive(Debug)]
pub struct CallablePlan {
    pub(crate) owner: DagId,
    pub(crate) execution_dags: Vec<DagId>,
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: Arc<RuntimeValueMap>,
    /// Compile-time constants imported from dependency module artifacts.
    /// These are injected directly into the evaluation environment.
    /// Iterated once during env setup; feeds into `HashMap` (key-lookup only).
    pub(crate) imported_values: RuntimeValueMap,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub(crate) topo_order: Vec<RuntimeDeclKey>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<RuntimeDeclKey, Vec<RuntimeDeclKey>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<RuntimeDeclKey, ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>,
}
