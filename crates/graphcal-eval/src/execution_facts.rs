//! Immutable checked facts required to execute canonical DAG bodies.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::tir::presentation::PresentationCallKey;
use graphcal_compiler::tir::typed::StructFieldConstraintKey;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::ResolvedDomainConstraint;

pub type RuntimeValueMap = HashMap<RuntimeDeclKey, RuntimeValue>;

/// Opaque identity of one presentation-relevant inline DAG invocation.
///
/// IDs are allocated by one [`EvaluatedPresentationCalls`] store and are
/// meaningful only for that evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationInvocationId(u64);

impl fmt::Display for PresentationInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// One retained inline-call environment and its checked static call site.
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

/// Runtime environments retained for presentation-relevant inline DAG calls.
#[derive(Debug, Default)]
pub struct EvaluatedPresentationCalls {
    state: Mutex<EvaluatedPresentationCallState>,
}

/// Failure to retain or retrieve one evaluated call environment.
#[derive(Debug, Error)]
pub enum PresentationCallValuesError {
    #[error("evaluated presentation-call storage is poisoned")]
    Poisoned,
    #[error("presentation-call invocation identity space is exhausted")]
    IdentityExhausted,
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
    /// Retain one invocation and return its evaluation-scoped identity.
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
        let invocation = PresentationInvocationId(state.next_invocation);
        state.next_invocation = next_invocation;
        let previous = state.by_invocation.insert(
            invocation,
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

    /// Retrieve one invocation by identity and verify its static call site.
    pub fn invocation(
        &self,
        key: &PresentationCallKey,
        invocation: PresentationInvocationId,
    ) -> Result<Arc<RuntimeValueMap>, PresentationCallValuesError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PresentationCallValuesError::Poisoned)?;
        let call = state
            .by_invocation
            .get(&invocation)
            .ok_or(PresentationCallValuesError::Missing { invocation })?;
        if &call.key != key {
            return Err(PresentationCallValuesError::WrongCallSite { invocation });
        }
        let values = Arc::clone(&call.values);
        drop(state);
        Ok(values)
    }
}

/// Checked execution facts for one canonical DAG.
///
/// A callable DAG cannot be evaluated without this value: its constants,
/// declaration constraints, schedule, and diagnostic source are produced as
/// one atomic checked artifact. The canonical `dag_id` prevents facts from one
/// body being paired with another body by convention.
#[derive(Debug, Clone)]
pub struct CheckedDagExecutionFacts {
    pub dag_id: DagId,
    pub source: NamedSource<Arc<String>>,
    pub const_values: Arc<RuntimeValueMap>,
    pub topo_order: Arc<Vec<RuntimeDeclKey>>,
    pub domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>,
    pub struct_field_constraints: Arc<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
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
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            by_dag: Arc::new(HashMap::new()),
            struct_field_constraints: Arc::new(HashMap::new()),
        }
    }

    pub fn for_dag(&self, dag_id: &DagId) -> Option<&CheckedDagExecutionFacts> {
        self.by_dag.get(dag_id).map(AsRef::as_ref)
    }

    pub fn merge<'a>(facts: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut by_dag = HashMap::new();
        let mut struct_field_constraints = HashMap::new();
        for checked in facts {
            by_dag.extend(
                checked
                    .by_dag
                    .iter()
                    .map(|(dag_id, dag_facts)| (dag_id.clone(), Arc::clone(dag_facts))),
            );
            struct_field_constraints.extend(
                checked
                    .struct_field_constraints
                    .iter()
                    .map(|(key, constraint)| (key.clone(), constraint.clone())),
            );
        }
        Self {
            by_dag: Arc::new(by_dag),
            struct_field_constraints: Arc::new(struct_field_constraints),
        }
    }
}
