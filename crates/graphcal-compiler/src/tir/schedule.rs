//! Evaluation schedules computed once by the checker.
//!
//! [`ConstSchedule`] orders every constant of a checked file's local DAGs.
//! [`RuntimeSchedule`] orders the params and nodes of one callable DAG
//! together with its semantic-instance closure. Only the checker builds them,
//! from the same dependency maps it validates, so every order covers exactly
//! its declarations and evaluates each one after all of its dependencies.

use std::collections::{HashMap, HashSet};

use crate::dag_id::DagId;
use crate::dependency_graph::{Cycle, DependencyGraph, TopoOrder};
use crate::ir::entry::Decl;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::span::Span;
use crate::tir::typed::dag_position::DagPosition;
use crate::tir::typed::model::DagTIR;

/// One scheduled declaration, with the site a dependency cycle through it is
/// reported at.
///
/// A scheduled declaration is identified by its identity alone: equality and
/// hashing ignore the site, so the scheduling graph is keyed (and can be
/// queried) by identity.
#[derive(Debug, Clone)]
pub(crate) struct ScheduledDecl {
    identity: ResolvedDeclName,
    name: DeclName,
    span: Span,
}

impl ScheduledDecl {
    /// The declaration's identity.
    pub(crate) const fn identity(&self) -> &ResolvedDeclName {
        &self.identity
    }

    /// The declaration's local name.
    pub(crate) const fn name(&self) -> &DeclName {
        &self.name
    }

    /// The span of the declaration.
    pub(crate) const fn span(&self) -> Span {
        self.span
    }

    fn into_identity(self) -> ResolvedDeclName {
        self.identity
    }
}

impl PartialEq for ScheduledDecl {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for ScheduledDecl {}

impl std::hash::Hash for ScheduledDecl {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.identity.hash(state);
    }
}

impl std::borrow::Borrow<ResolvedDeclName> for ScheduledDecl {
    fn borrow(&self) -> &ResolvedDeclName {
        &self.identity
    }
}

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
    ) -> Result<Self, Cycle<ScheduledDecl>> {
        let mut dags = dags.into_iter().collect::<Vec<_>>();
        dags.sort_by(|(_, left), (_, right)| left.dag_id().cmp(right.dag_id()));
        let mut graph = DependencyGraph::new();
        for (_, dag) in &dags {
            for entry in dag.consts() {
                graph.add_node(ScheduledDecl {
                    identity: entry.identity(),
                    name: entry.name().clone(),
                    span: entry.span,
                });
            }
        }
        for (_, dag) in &dags {
            let const_deps = &dag.semantic().dependencies.const_deps;
            for entry in dag.consts() {
                let constant = entry.identity();
                for dependency in const_deps.get(&constant).into_iter().flatten() {
                    graph.add_labelled_dependency_between(&constant, dependency, ());
                }
            }
        }
        Ok(Self {
            dags: dags.iter().map(|(position, _)| *position).collect(),
            order: graph.into_topo_order()?.map(ScheduledDecl::into_identity),
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
    /// `dag_at` reads the DAG at a position and `instances_of` the
    /// positions of the instances a DAG's edges materialize.
    ///
    /// # Errors
    ///
    /// Returns the first [`Cycle`] among the params and nodes.
    pub(crate) fn build<'a>(
        callable: DagPosition,
        dag_at: impl Fn(DagPosition) -> &'a DagTIR,
        instances_of: impl Fn(DagPosition) -> &'a [DagPosition],
    ) -> Result<Self, Cycle<ScheduledDecl>> {
        let dags = instance_closure(callable, dag_at, instances_of);
        let mut graph = DependencyGraph::new();
        let mut dependencies = HashMap::new();
        for dag in &dags {
            let runtime_deps = &dag.semantic().dependencies.runtime_deps;
            for (entry, span) in dag.decls().iter().filter_map(|entry| match entry {
                Decl::Param(param) => Some((entry, param.span)),
                Decl::Node(node) => Some((entry, node.span)),
                Decl::Const(_)
                | Decl::Assert(_)
                | Decl::Plot(_)
                | Decl::Figure(_)
                | Decl::Layer(_) => None,
            }) {
                let declaration = entry.identity();
                let reads = runtime_deps
                    .get(&declaration)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>();
                graph.add_node(ScheduledDecl {
                    identity: declaration.clone(),
                    name: entry.name().clone(),
                    span,
                });
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
                    graph.add_labelled_dependency_between(&declaration, dependency, ());
                }
            }
        }
        Ok(Self {
            execution_dags: dags.iter().map(|dag| dag.dag_id().clone()).collect(),
            order: graph.into_topo_order()?.map(ScheduledDecl::into_identity),
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
    callable: DagPosition,
    dag_at: impl Fn(DagPosition) -> &'a DagTIR,
    instances_of: impl Fn(DagPosition) -> &'a [DagPosition],
) -> Vec<&'a DagTIR> {
    let mut pending = vec![callable];
    let mut visited = HashSet::from([callable]);
    let mut instances = Vec::new();
    while let Some(position) = pending.pop() {
        for &instance in instances_of(position) {
            if visited.insert(instance) {
                pending.push(instance);
                instances.push(dag_at(instance));
            }
        }
    }
    instances.sort_by(|left, right| left.dag_id().cmp(right.dag_id()));
    std::iter::once(dag_at(callable)).chain(instances).collect()
}
