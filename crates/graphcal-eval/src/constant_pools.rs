//! Evaluated constant pools.
//!
//! A [`ConstPool`] is built only by evaluating a checked TIR's constants in
//! the checker's [`ConstSchedule`](graphcal_compiler::tir::schedule::ConstSchedule),
//! so it covers every constant of every DAG exactly once. Its per-DAG pools are
//! immutable and shared: callable plans view them through their sealed DAGs and
//! imports through a [`ConstantReference`], without copying constant payloads.
//!
//! An unfinished constant has no value, never a placeholder: its formula, or
//! the formula of a constant it reads, contains a `todo` hole. The pool
//! records why, as an [`UnfinishedConstant`], instead of a value.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::runtime_value::RuntimeValue;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::node_unavailable::{NodeUnavailable, RuntimeUnavailable};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::dag_position::DagPosition;
use graphcal_compiler::tir::typed::evaluation_unit::{BodyKind, FormulaBody};
use thiserror::Error;

pub type RuntimeValueMap = HashMap<ResolvedDeclName, RuntimeValue>;

/// Why a constant has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnfinishedConstant {
    /// Its own formula contains a `todo` hole.
    Todo,
    /// It reads unfinished constants: the constants whose own formulas
    /// contain the holes.
    Blocked(NonEmpty<ResolvedDeclName>),
}

impl UnfinishedConstant {
    /// The unfinished origins of the constant `key`: `key` itself, or the
    /// constants it is blocked by.
    #[must_use]
    pub fn origins<'a>(&'a self, key: &'a ResolvedDeclName) -> &'a [ResolvedDeclName] {
        match self {
            Self::Todo => std::slice::from_ref(key),
            Self::Blocked(unfinished) => unfinished.as_slice(),
        }
    }

    /// The constant `key`'s reason, as evaluation records one for a value
    /// declaration.
    #[must_use]
    pub fn reason(&self, key: &ResolvedDeclName) -> RuntimeUnavailable {
        match self {
            Self::Todo => NodeUnavailable::Todo {
                declaration: key.clone(),
            },
            Self::Blocked(unfinished) => NodeUnavailable::Blocked {
                unfinished: unfinished.clone(),
                failed_deps: Vec::new(),
            },
        }
    }
}

/// The unfinished constants of a pool, keyed by canonical identity.
pub type UnfinishedConstantMap = HashMap<ResolvedDeclName, UnfinishedConstant>;

#[derive(Debug, Error)]
pub enum ConstantPoolError {
    #[error("DAG `{0}` is neither scheduled by this check nor inherited from a checked module")]
    Uncovered(DagId),
    #[error("DAG `{0}` is scheduled by this check but was inherited from a checked module")]
    Rescheduled(DagId),
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
/// pool per DAG, and the unfinished constants of every DAG.
///
/// Every scheduled constant is in exactly one of the two: [`ConstPool::build`]
/// either evaluates a constant or records why it has no value.
#[derive(Debug, Clone, Default)]
pub struct ConstPool {
    by_dag: HashMap<DagId, Arc<RuntimeValueMap>>,
    unfinished: Arc<UnfinishedConstantMap>,
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
    ) -> Result<(Self, PositionedPools), ConstPoolBuildError<E>> {
        let invalid = |error| Err(ConstPoolBuildError::Invalid(error));
        let schedule = tir.const_schedule();
        let scheduled = schedule.dags().iter().copied().collect::<HashSet<_>>();
        let mut by_dag = HashMap::new();
        let mut fresh = HashMap::new();
        for (position, dag) in tir.dag_registry().positioned() {
            let dag_id = dag.dag_id();
            match (scheduled.contains(&position), inherited.by_dag.get(dag_id)) {
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
        let mut unfinished = inherited.unfinished.as_ref().clone();
        for key in schedule.order() {
            let (Some(formula), Some(pool)) = (
                tir.declaration_body(key)
                    .and_then(|body| match body.kind() {
                        BodyKind::Const(formula) => Some(formula),
                        _ => None,
                    }),
                fresh.get_mut(key.owner()),
            ) else {
                return invalid(ConstantPoolError::MissingDeclaration(key.clone()));
            };
            let expression = match formula {
                FormulaBody::Unfinished(_) => {
                    unfinished.insert(key.clone(), UnfinishedConstant::Todo);
                    continue;
                }
                FormulaBody::Formula(expression) => expression,
            };
            if let Some(blocked) = blocked_by(tir, key, &unfinished) {
                unfinished.insert(key.clone(), blocked);
                continue;
            }
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
        // The loop above gave every DAG of the registry a pool.
        let by_position = tir
            .dag_registry()
            .positioned()
            .map(|(_, dag)| Arc::clone(&by_dag[dag.dag_id()]))
            .collect();
        Ok((
            Self {
                by_dag,
                unfinished: Arc::new(unfinished),
            },
            PositionedPools { by_position },
        ))
    }

    /// Why the constant `key` has no value, when it is unfinished.
    #[must_use]
    pub fn unfinished(&self, key: &ResolvedDeclName) -> Option<&UnfinishedConstant> {
        self.unfinished.get(key)
    }

    /// Every unfinished constant of every DAG this pool covers.
    pub fn unfinished_constants(
        &self,
    ) -> impl Iterator<Item = (&ResolvedDeclName, &UnfinishedConstant)> {
        self.unfinished.iter()
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

    /// A reference to one constant of this pool: its value in its defining
    /// DAG's pool, or why it has none.
    #[must_use]
    pub fn reference(&self, key: &ResolvedDeclName) -> Option<ConstantReference> {
        let pool = self.by_dag.get(key.owner())?;
        if pool.contains_key(key) {
            return Some(ConstantReference {
                key: key.clone(),
                target: ReferenceTarget::Value(Arc::clone(pool)),
            });
        }
        self.unfinished
            .get(key)
            .map(|unfinished| ConstantReference {
                key: key.clone(),
                target: ReferenceTarget::Unfinished(unfinished.clone()),
            })
    }
}

/// Why the constant `key` is blocked, when it reads an unfinished constant.
fn blocked_by(
    tir: &CheckedTir,
    key: &ResolvedDeclName,
    unfinished: &UnfinishedConstantMap,
) -> Option<UnfinishedConstant> {
    let origins = tir
        .dag_containing_declaration(key)
        .and_then(|dag| dag.dependencies().const_deps.get(key))
        .into_iter()
        .flatten()
        .filter_map(|dependency| {
            unfinished
                .get(dependency)
                .map(|reason| reason.origins(dependency))
        })
        .flatten()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    NonEmpty::try_from_vec(origins.into_iter().collect())
        .ok()
        .map(UnfinishedConstant::Blocked)
}

/// The pool of every DAG of the checked TIR one [`ConstPool`] was built
/// for, at the DAG's position in that TIR's registry.
///
/// Only [`ConstPool::build`] creates one, aligned with the registry it
/// covers, so every position of that registry has a pool.
#[derive(Debug, Clone)]
pub struct PositionedPools {
    by_position: Vec<Arc<RuntimeValueMap>>,
}

impl PositionedPools {
    /// The evaluated constants of the DAG at `position`.
    ///
    /// # Panics
    ///
    /// Panics when `position` is a position of another registry with more
    /// DAGs.
    #[must_use]
    pub fn at(&self, position: DagPosition) -> &Arc<RuntimeValueMap> {
        &self.by_position[position.index()]
    }
}

/// One constant of a [`ConstPool`]: its value in its defining DAG's
/// immutable pool, not a copied value, or why it has none. Only
/// [`ConstPool::reference`] creates one, so it always resolves.
#[derive(Debug, Clone)]
pub struct ConstantReference {
    key: ResolvedDeclName,
    target: ReferenceTarget,
}

#[derive(Debug, Clone)]
enum ReferenceTarget {
    Value(Arc<RuntimeValueMap>),
    Unfinished(UnfinishedConstant),
}

impl ConstantReference {
    /// The referenced constant's identity.
    #[must_use]
    pub const fn key(&self) -> &ResolvedDeclName {
        &self.key
    }

    /// The referenced constant's value.
    ///
    /// # Errors
    ///
    /// Returns why the constant has no value when it is unfinished.
    pub fn value(&self) -> Result<&RuntimeValue, &UnfinishedConstant> {
        match &self.target {
            ReferenceTarget::Value(pool) => Ok(&pool[&self.key]),
            ReferenceTarget::Unfinished(unfinished) => Err(unfinished),
        }
    }
}
