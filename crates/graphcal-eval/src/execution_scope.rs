//! Matching borrowed DAG and execution facts, without caller-assembled pairs.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::tir::typed::{DagTIR, TIR};
use thiserror::Error;

use crate::execution_facts::{CheckedDagExecutionFacts, CheckedExecutionFacts};

/// Failure to select an execution scope from retained project artifacts.
#[derive(Debug, Error)]
pub(crate) enum ExecutionScopeError {
    #[error("DAG `{0}` has no compiled body")]
    MissingBody(DagId),
    #[error("DAG `{0}` has no checked execution facts")]
    MissingFacts(DagId),
    #[error("checked execution facts for `{actual}` were paired with DAG `{expected}`")]
    WrongOwner { expected: DagId, actual: DagId },
}

/// A canonical body paired with its own facts from the same selected stores.
///
/// This checks scope identity, not source type correctness. The enclosing
/// CheckedProject remains responsible for completing mandatory static checks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CheckedExecutionScope<'a> {
    dag: &'a DagTIR,
    facts: &'a CheckedDagExecutionFacts,
}

impl<'a> CheckedExecutionScope<'a> {
    pub(crate) fn new(
        tir: &'a TIR,
        facts: &'a CheckedExecutionFacts,
        owner: &DagId,
    ) -> Result<Self, ExecutionScopeError> {
        let dag = tir
            .dag_registry()
            .get(owner)
            .ok_or_else(|| ExecutionScopeError::MissingBody(owner.clone()))?;
        let facts = facts
            .for_dag(owner)
            .ok_or_else(|| ExecutionScopeError::MissingFacts(owner.clone()))?;
        if facts.dag_id != *dag.dag_id() {
            return Err(ExecutionScopeError::WrongOwner {
                expected: dag.dag_id().clone(),
                actual: facts.dag_id.clone(),
            });
        }
        Ok(Self { dag, facts })
    }

    pub(crate) const fn dag(self) -> &'a DagTIR {
        self.dag
    }

    pub(crate) const fn facts(self) -> &'a CheckedDagExecutionFacts {
        self.facts
    }
}
