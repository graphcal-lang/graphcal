//! Immutable execution-plan data, independent of preparation algorithms.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::assertion_expectation::ExpectedFail;
use graphcal_compiler::dag_id::DagId;
use thiserror::Error;

use crate::checked_program::CheckedProgram;
use crate::constant_pools::{ConstantPools, ConstantReference};
use crate::declaration_locations::DeclarationLocations;
use crate::domain_constraint::ResolvedDomainConstraint;
use graphcal_compiler::resolved_name::ResolvedDeclName;

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    pub(crate) has_unfinished_definitions: bool,
    /// Physical bodies selected from completed declaration indexes at preparation.
    pub(crate) declaration_locations: DeclarationLocations,
    pub(crate) root: CallablePlan,
    /// Non-root bodies; the root has the same callable contract without a copy.
    pub(crate) callables: HashMap<DagId, CallablePlan>,
    /// The sealed program every callable plan was prepared from.
    pub(crate) program: CheckedProgram,
}

#[derive(Debug, Error)]
pub enum CallablePlanError {
    #[error("DAG `{0}` has no prepared callable plan")]
    Missing(DagId),
    #[error("prepared callable plan for `{expected}` belongs to `{actual}`")]
    WrongOwner { expected: DagId, actual: DagId },
}

impl ExecPlan {
    /// The sealed program this plan executes.
    pub(crate) const fn program(&self) -> &CheckedProgram {
        &self.program
    }

    /// The checked TIR this plan executes.
    pub(crate) const fn tir(&self) -> &graphcal_compiler::tir::typed::checked::CheckedTir {
        self.program.tir()
    }

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

#[derive(Debug)]
pub struct PreparedConstantImport {
    pub(crate) destination: ResolvedDeclName,
    pub(crate) value: ConstantReference,
}

#[derive(Debug, Default)]
pub struct PreparedImports {
    pub(crate) constants: Vec<PreparedConstantImport>,
    pub(crate) runtime: Vec<ResolvedDeclName>,
}

/// One body and its included-instance closure, prepared before evaluation.
#[derive(Debug)]
pub struct CallablePlan {
    pub(crate) owner: DagId,
    pub(crate) execution_dags: Vec<DagId>,
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: ConstantPools,
    /// Retained constant references and explicit runtime imports, selected once
    /// from lexical bindings during preparation.
    pub(crate) imports: PreparedImports,
    /// The checker's runtime schedule of this body and its instance closure.
    pub(crate) schedule: graphcal_compiler::tir::schedule::RuntimeSchedule,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<ResolvedDeclName, ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: Arc<HashMap<ResolvedDeclName, ResolvedDomainConstraint>>,
}
