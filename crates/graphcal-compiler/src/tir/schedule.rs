//! Evaluation schedules computed once by the checker.
//!
//! [`ConstSchedule`] orders every constant of a checked file's local DAGs.
//! [`RuntimeSchedule`] orders the params and nodes of one callable DAG
//! together with its semantic-instance closure. Only the checker builds them,
//! from the same dependency maps it validates, so every order covers exactly
//! its declarations and evaluates each one after all of its dependencies.

use std::collections::{HashMap, HashSet};

use crate::dag_id::DagId;
use crate::declaration_category::{DeclCategory, ValueDeclCategory};
use crate::dependency_graph::{Cycle, DependencyGraph, TopoOrder};
use crate::resolved_name::ResolvedDeclName;
use crate::tir::typed::dag_position::DagPosition;
use crate::tir::typed::model::DagTIR;

/// Evaluation order of every constant of a checked file's local DAGs.
///
/// Constants of one DAG may read constants of another (an importer reads an
/// included instance's constants), so the order spans all local DAGs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstSchedule {
    /// The positions of the scheduled DAGs, in [`DagId`] order.
    dags: Vec<DagPosition>,
    order: TopoOrder<ResolvedDeclName>,
}

impl ConstSchedule {
    /// Order the constants of `dags`: DAGs in [`DagId`] order and constants
    /// in source order, except that every constant follows the constants of
    /// these DAGs that it reads.
    ///
    /// # Errors
    ///
    /// Returns the first [`Cycle`] among the constants.
    pub(crate) fn build<'a>(
        dags: impl IntoIterator<Item = (DagPosition, &'a DagTIR)>,
    ) -> Result<Self, Cycle<ResolvedDeclName>> {
        let mut dags = dags.into_iter().collect::<Vec<_>>();
        dags.sort_by(|(_, left), (_, right)| left.dag_id().cmp(right.dag_id()));
        let mut graph = DependencyGraph::new();
        for (_, dag) in &dags {
            for entry in dag.consts() {
                graph.add_node(entry.identity());
            }
        }
        for (_, dag) in &dags {
            let const_deps = &dag.semantic().dependencies.const_deps;
            for entry in dag.consts() {
                let constant = entry.identity();
                for dependency in const_deps.get(&constant).into_iter().flatten() {
                    if graph.contains(dependency) {
                        graph.add_dependency(constant.clone(), dependency.clone());
                    }
                }
            }
        }
        Ok(Self {
            dags: dags.iter().map(|(position, _)| *position).collect(),
            order: graph.into_topo_order()?,
        })
    }

    /// The positions of the scheduled DAGs, in [`DagId`] order.
    #[must_use]
    pub fn dags(&self) -> &[DagPosition] {
        &self.dags
    }

    /// Every scheduled constant, each after the constants it reads.
    #[must_use]
    pub const fn order(&self) -> &TopoOrder<ResolvedDeclName> {
        &self.order
    }
}

/// Why a callable's [`RuntimeSchedule`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeScheduleError {
    /// A semantic instance edge names a DAG absent from the TIR.
    MissingInstance(DagId),
    /// The params and nodes of the closure depend on each other cyclically.
    Cycle(Cycle<ResolvedDeclName>),
}

/// Runtime evaluation schedule of one callable DAG and every semantic
/// instance it transitively includes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSchedule {
    execution_dags: Vec<DagId>,
    order: TopoOrder<ResolvedDeclName>,
    dependencies: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
}

impl RuntimeSchedule {
    /// Schedule the params and nodes of `callable` and its instance closure:
    /// the callable, then its instances in [`DagId`] order, declarations in
    /// source order, except that every declaration follows the scheduled
    /// declarations it reads.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeScheduleError`] for a dangling instance edge or a
    /// dependency cycle.
    pub(crate) fn build<'a>(
        callable: &'a DagTIR,
        dag: impl Fn(&DagId) -> Option<&'a DagTIR>,
    ) -> Result<Self, RuntimeScheduleError> {
        let dags = instance_closure(callable, dag)?;
        let mut graph = DependencyGraph::new();
        let mut dependencies = HashMap::new();
        for dag in &dags {
            let runtime_deps = &dag.semantic().dependencies.runtime_deps;
            for entry in dag.decls().iter().filter(|entry| {
                matches!(
                    entry.category(),
                    DeclCategory::Value(ValueDeclCategory::Param | ValueDeclCategory::Node)
                )
            }) {
                let declaration = entry.identity();
                let reads = runtime_deps
                    .get(&declaration)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>();
                graph.add_node(declaration.clone());
                dependencies.insert(declaration, reads);
            }
        }
        for dag in &dags {
            for entry in dag.decls().iter() {
                let declaration = entry.identity();
                let Some(reads) = dependencies.get(&declaration) else {
                    continue;
                };
                for dependency in reads {
                    if graph.contains(dependency) {
                        graph.add_dependency(declaration.clone(), dependency.clone());
                    }
                }
            }
        }
        Ok(Self {
            execution_dags: dags.iter().map(|dag| dag.dag_id().clone()).collect(),
            order: graph
                .into_topo_order()
                .map_err(RuntimeScheduleError::Cycle)?,
            dependencies,
        })
    }

    /// The callable followed by its instance closure in [`DagId`] order.
    #[must_use]
    pub fn execution_dags(&self) -> &[DagId] {
        &self.execution_dags
    }

    /// Every scheduled param and node, each after the ones it reads.
    #[must_use]
    pub const fn order(&self) -> &TopoOrder<ResolvedDeclName> {
        &self.order
    }

    /// Every scheduled param and node, each after the ones it reads, with
    /// everything it reads (see [`Self::dependencies_of`]).
    pub fn steps(&self) -> impl Iterator<Item = (&ResolvedDeclName, &[ResolvedDeclName])> {
        self.order.iter().map(|declaration| {
            // `build` records the reads of every declaration it orders.
            let reads = self
                .dependencies
                .get(declaration)
                .map_or(&[][..], Vec::as_slice);
            (declaration, reads)
        })
    }

    /// Every declaration a scheduled param or node reads, in runtime
    /// identity, including reads of unscheduled constants and imports.
    #[must_use]
    pub fn dependencies_of(&self, declaration: &ResolvedDeclName) -> Option<&[ResolvedDeclName]> {
        self.dependencies.get(declaration).map(Vec::as_slice)
    }
}

/// `callable` followed by every DAG its semantic instance edges reach, in
/// [`DagId`] order.
fn instance_closure<'a>(
    callable: &'a DagTIR,
    dag_of: impl Fn(&DagId) -> Option<&'a DagTIR>,
) -> Result<Vec<&'a DagTIR>, RuntimeScheduleError> {
    let mut pending = vec![callable];
    let mut visited = HashSet::from([callable.dag_id()]);
    let mut instances = Vec::new();
    while let Some(dag) = pending.pop() {
        for edge in dag.semantic_instances() {
            let owner = edge.instance.id().owner();
            let instance = dag_of(owner)
                .ok_or_else(|| RuntimeScheduleError::MissingInstance(owner.clone()))?;
            if visited.insert(instance.dag_id()) {
                pending.push(instance);
                instances.push(instance);
            }
        }
    }
    instances.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
    Ok(std::iter::once(callable).chain(instances).collect())
}
