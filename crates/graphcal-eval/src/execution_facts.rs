//! Immutable checked facts required to execute canonical DAG bodies.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::tir::typed::StructFieldConstraintKey;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_constraint::ResolvedDomainConstraint;

pub type RuntimeValueMap = HashMap<RuntimeDeclKey, RuntimeValue>;

/// Checked execution facts for one canonical DAG.
///
/// A callable DAG cannot be evaluated without this value: its constants,
/// declaration constraints, schedule, and diagnostic source are produced as
/// one atomic checked artifact. Canonical owner and body revision are both
/// required: equal names or shared source trees cannot authorize stale facts.
#[derive(Debug, Clone)]
pub struct CheckedDagExecutionFacts {
    pub dag_id: DagId,
    pub body_revision: graphcal_compiler::body_revision::BodyRevision,
    pub source: NamedSource<Arc<String>>,
    pub const_values: Arc<RuntimeValueMap>,
    /// Compile-time selections only; dynamic display requests have no invocation state.
    pub const_presentations: Arc<crate::presentation_evidence::PresentationInstanceMap>,
    pub topo_order: Arc<Vec<RuntimeDeclKey>>,
    pub domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>,
}

impl CheckedDagExecutionFacts {
    #[must_use]
    pub const fn source(&self) -> &NamedSource<Arc<String>> {
        &self.source
    }
}

/// Compile-time execution facts retained by a fully checked project, keyed by
/// canonical DAG identity.
#[derive(Debug, Clone)]
pub struct CheckedExecutionFacts {
    pub by_dag: Arc<HashMap<DagId, Arc<CheckedDagExecutionFacts>>>,
    pub struct_field_constraints: Arc<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
}

impl CheckedExecutionFacts {
    pub(crate) fn empty() -> Self {
        Self {
            by_dag: Arc::new(HashMap::new()),
            struct_field_constraints: Arc::new(HashMap::new()),
        }
    }

    pub fn for_dag(&self, dag_id: &DagId) -> Option<&CheckedDagExecutionFacts> {
        self.by_dag.get(dag_id).map(AsRef::as_ref)
    }
}
