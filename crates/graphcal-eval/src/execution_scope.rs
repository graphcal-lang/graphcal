//! Matching borrowed DAG and execution facts, without caller-assembled pairs.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::ir::imported_binding::{ImportedBinding, ImportedValueKind};
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::tir::typed::{DagTIR, TIR};
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;

use crate::execution_facts::{CheckedDagExecutionFacts, CheckedExecutionFacts};

/// Failure to select an execution scope from retained project artifacts.
#[derive(Debug, Error)]
pub enum ExecutionScopeError {
    #[error("DAG `{0}` has no compiled body")]
    MissingBody(DagId),
    #[error("DAG `{0}` has no checked execution facts")]
    MissingFacts(DagId),
    #[error("checked execution facts for `{actual}` were paired with DAG `{expected}`")]
    WrongOwner { expected: DagId, actual: DagId },
    #[error("imported declaration `{0}` has no containing body")]
    MissingDeclaration(ResolvedDeclName),
    #[error("imported declaration `{target}` is not a checked {kind:?} value")]
    WrongImportedKind {
        target: ResolvedDeclName,
        kind: ImportedValueKind,
    },
    #[error("imported constant `{0}` has no checked value in its defining body's pool")]
    MissingConstant(ResolvedDeclName),
}

/// A canonical body paired with its own facts from the same selected stores.
///
/// This checks scope identity, not source type correctness. The enclosing
/// `CheckedProject` remains responsible for completing mandatory static checks.
#[derive(Debug, Clone, Copy)]
pub struct CheckedExecutionScope<'a> {
    dag: &'a DagTIR,
    facts: &'a CheckedDagExecutionFacts,
}

/// Resolve imported constants from their defining body's facts. `None` means
/// a validated deferred runtime import, never an absent required constant.
/// Preparation and call setup share this fail-closed lookup until callable plans
/// retain the imported pools directly.
pub fn checked_imported_constant<'a>(
    tir: &'a TIR,
    facts: &'a CheckedExecutionFacts,
    binding: &ImportedBinding,
) -> Result<Option<&'a RuntimeValue>, ExecutionScopeError> {
    let target = binding.target();
    let dag = tir
        .dag_containing_declaration(target)
        .ok_or_else(|| ExecutionScopeError::MissingDeclaration(target.clone()))?;
    let scope = CheckedExecutionScope::new(tir, facts, dag.dag_id())?;
    let is_constant = scope.dag().const_expr(target).is_some();
    match (binding.kind(), is_constant) {
        (ImportedValueKind::Runtime, false) => Ok(None),
        (ImportedValueKind::Constant, true) => scope
            .facts()
            .const_values
            .get(&RuntimeDeclKey::resolved(target.clone()))
            .map(Some)
            .ok_or_else(|| ExecutionScopeError::MissingConstant(target.clone())),
        (kind, _) => Err(ExecutionScopeError::WrongImportedKind {
            target: target.clone(),
            kind,
        }),
    }
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
