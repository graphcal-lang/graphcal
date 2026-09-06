//! Immutable execution-plan data, independent of preparation algorithms.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::assertion_expectation::ExpectedFail;

use crate::decl_key::RuntimeDeclKey;
use crate::declaration_locations::DeclarationLocations;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_facts::{CheckedExecutionFacts, RuntimeValueMap};

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Physical bodies selected from completed declaration indexes at preparation.
    pub(crate) declaration_locations: DeclarationLocations,
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
    /// Per-DAG checked facts required by nested callable evaluation.
    pub(crate) checked_execution_facts: CheckedExecutionFacts,
}
