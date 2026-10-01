//! Evaluated constant pools.
//!
//! A [`ConstPool`] is built only by evaluating a checked TIR's constants in
//! the checker's [`ConstSchedule`](graphcal_compiler::tir::schedule::ConstSchedule),
//! so it covers every constant of every DAG exactly once. Its per-DAG pools are
//! immutable and shared: callable plans view them through their sealed DAGs and
//! imports through a [`ConstantReference`], without copying constant payloads.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::runtime_value::RuntimeValue;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::evaluation_unit::BodyKind;
use thiserror::Error;

pub type RuntimeValueMap = HashMap<ResolvedDeclName, RuntimeValue>;

#[derive(Debug, Error)]
pub enum ConstantPoolError {
    #[error("DAG `{0}` is neither scheduled by this check nor inherited from a checked module")]
    Uncovered(DagId),
    #[error("DAG `{0}` is scheduled by this check but was inherited from a checked module")]
    Rescheduled(DagId),
    #[error("scheduled DAG `{0}` has no checked body")]
    MissingDag(DagId),
    #[error("constant schedule references missing declaration `{0}`")]
    MissingDeclaration(ResolvedDeclName),
}

/// Why a [`ConstPool`] could not be built.
#[derive(Debug)]
pub enum ConstPoolBuildError<E> {
    /// The evaluation of one scheduled constant failed.
    Evaluation(E),
    /// The schedule or the inherited pools do not match the checked TIR.
    Invalid(ConstantPoolError),
}

/// One scheduled constant, handed to the evaluation closure of
/// [`ConstPool::build`] after every constant it reads.
pub struct ConstStep<'a> {
    /// The checked TIR being evaluated.
    pub tir: &'a CheckedTir,
    pub key: &'a ResolvedDeclName,
    /// The constant's expression, in the scope of the DAG that owns it.
    pub expression: Scoped<'a, Expr>,
    /// Every constant evaluated so far, inherited ones included.
    pub visible: &'a RuntimeValueMap,
}

/// The evaluated constants of every DAG of one checked TIR, one immutable
/// pool per DAG.
#[derive(Debug, Clone, Default)]
pub struct ConstPool {
    by_dag: HashMap<DagId, Arc<RuntimeValueMap>>,
}

impl ConstPool {
    /// Evaluate the constants of `tir`'s scheduled DAGs, calling `evaluate`
    /// exactly once per constant in the checker's constant schedule; every
    /// other DAG of `tir` shares its pool from `inherited`.
    ///
    /// The result covers exactly the DAGs of `tir`.
    ///
    /// # Errors
    ///
    /// Returns the first evaluation error, or [`ConstantPoolError`] when a DAG
    /// of `tir` is neither scheduled nor inherited, or both.
    pub fn build<E>(
        tir: &CheckedTir,
        inherited: &Self,
        mut evaluate: impl FnMut(ConstStep<'_>) -> Result<RuntimeValue, E>,
    ) -> Result<Self, ConstPoolBuildError<E>> {
        let invalid = |error| Err(ConstPoolBuildError::Invalid(error));
        let schedule = tir.const_schedule();
        let scheduled = schedule.dags().iter().collect::<HashSet<_>>();
        if let Some(missing) = schedule
            .dags()
            .iter()
            .find(|dag_id| tir.dag_registry().get(dag_id).is_none())
        {
            return invalid(ConstantPoolError::MissingDag(missing.clone()));
        }
        let mut by_dag = HashMap::new();
        let mut fresh = HashMap::new();
        for dag_id in tir.dag_registry().keys() {
            match (scheduled.contains(dag_id), inherited.by_dag.get(dag_id)) {
                (true, None) => {
                    fresh.insert(dag_id.clone(), RuntimeValueMap::new());
                }
                (false, Some(pool)) => {
                    by_dag.insert(dag_id.clone(), Arc::clone(pool));
                }
                (true, Some(_)) => return invalid(ConstantPoolError::Rescheduled(dag_id.clone())),
                (false, None) => return invalid(ConstantPoolError::Uncovered(dag_id.clone())),
            }
        }
        let mut visible = by_dag
            .values()
            .flat_map(|pool| pool.iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<RuntimeValueMap>();
        for key in schedule.order() {
            let (Some(expression), Some(pool)) = (
                tir.declaration_body(key)
                    .and_then(|body| match body.kind() {
                        BodyKind::Const(expression) => Some(expression),
                        _ => None,
                    }),
                fresh.get_mut(key.owner()),
            ) else {
                return invalid(ConstantPoolError::MissingDeclaration(key.clone()));
            };
            let value = evaluate(ConstStep {
                tir,
                key,
                expression,
                visible: &visible,
            })
            .map_err(ConstPoolBuildError::Evaluation)?;
            visible.insert(key.clone(), value.clone());
            pool.insert(key.clone(), value);
        }
        by_dag.extend(
            fresh
                .into_iter()
                .map(|(dag_id, pool)| (dag_id, Arc::new(pool))),
        );
        Ok(Self { by_dag })
    }

    /// The evaluated constants of one DAG.
    #[must_use]
    pub fn for_dag(&self, dag_id: &DagId) -> Option<&Arc<RuntimeValueMap>> {
        self.by_dag.get(dag_id)
    }

    /// The DAGs this pool covers.
    pub fn dags(&self) -> impl Iterator<Item = &DagId> {
        self.by_dag.keys()
    }

    /// A reference to one evaluated constant in its defining DAG's pool.
    #[must_use]
    pub fn reference(&self, key: &ResolvedDeclName) -> Option<ConstantReference> {
        let pool = self.by_dag.get(key.owner())?;
        pool.contains_key(key).then(|| ConstantReference {
            pool: Arc::clone(pool),
            key: key.clone(),
        })
    }
}

/// One evaluated constant in its defining DAG's immutable pool, not a copied
/// value. Only [`ConstPool::reference`] creates one, so it always resolves.
#[derive(Debug, Clone)]
pub struct ConstantReference {
    pool: Arc<RuntimeValueMap>,
    key: ResolvedDeclName,
}

impl ConstantReference {
    /// The referenced constant's identity.
    #[must_use]
    pub const fn key(&self) -> &ResolvedDeclName {
        &self.key
    }

    /// The referenced constant's value.
    #[must_use]
    pub fn value(&self) -> &RuntimeValue {
        &self.pool[&self.key]
    }
}
