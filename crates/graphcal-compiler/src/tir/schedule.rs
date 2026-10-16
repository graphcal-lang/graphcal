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
use crate::tir::recurrence_plan::{RecurrenceGroupPlan, RecurrenceMember};
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

/// One unit of a callable's runtime schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleUnit {
    /// A param or node evaluated as a whole.
    Declaration(ResolvedDeclName),
    /// An accepted recurrence group, evaluated element by element.
    Recurrence(RecurrenceGroupPlan),
}

impl ScheduleUnit {
    /// The declarations the unit evaluates: the declaration itself, or the
    /// group's members in plan order.
    pub fn declarations(&self) -> impl Iterator<Item = &ResolvedDeclName> {
        let (declaration, members) = match self {
            Self::Declaration(declaration) => (Some(declaration), None),
            Self::Recurrence(plan) => (None, Some(plan.members())),
        };
        declaration.into_iter().chain(
            members
                .into_iter()
                .flatten()
                .map(RecurrenceMember::declaration),
        )
    }
}

/// Why one cyclic component of a runtime schedule is not an accepted
/// recurrence group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComponentRejection {
    /// The component can never be accepted (it has no `unfold` member), or
    /// its first violation lies outside the checked callable's source: report
    /// the plain dependency cycle.
    PlainCycle,
    /// The first violated rule, at the member or read that violates it.
    Recurrence {
        name: DeclName,
        span: Span,
        reason: crate::semantic_error::graph::RecurrenceRejection,
    },
}

/// Why a callable's params and nodes cannot be scheduled.
#[derive(Debug, Clone)]
pub(crate) enum ScheduleRejection {
    /// A dependency cycle that cannot be a recurrence group: reported as a
    /// plain cycle.
    Cycle(Cycle<ScheduledDecl>),
    /// A dependency cycle with an `unfold` member that violates a
    /// recurrence-group rule, at the member or read that violates it.
    Recurrence {
        name: DeclName,
        span: Span,
        reason: crate::semantic_error::graph::RecurrenceRejection,
    },
}

/// Runtime evaluation schedule of one callable DAG and every semantic
/// instance it transitively includes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSchedule {
    execution_dags: Vec<DagId>,
    units: TopoOrder<ScheduleUnit>,
    dependencies: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
}

/// A schedule unit while ordering: a declaration, or an accepted group by
/// position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum UnitKey {
    Declaration(ResolvedDeclName),
    Group(usize),
}

impl RuntimeSchedule {
    /// Schedule the params and nodes of `callable` and its instance closure:
    /// the callable, then its instances in [`DagId`] order, declarations in
    /// source order, except that every declaration follows the scheduled
    /// declarations it reads.
    ///
    /// A dependency cycle is accepted only as a recurrence group, which
    /// `check_component` decides for each cyclic component (each declaration
    /// with the DAG that declares it): the group becomes one unit, ordered
    /// after everything its members read outside it.
    ///
    /// `dag_at` reads the DAG at a position and `instances_of` the positions
    /// of the instances a DAG's edges materialize.
    ///
    /// # Errors
    ///
    /// Returns the first rejected dependency cycle among the params and
    /// nodes.
    pub(crate) fn build<'a>(
        callable: DagPosition,
        dag_at: impl Fn(DagPosition) -> &'a DagTIR,
        instances_of: impl Fn(DagPosition) -> &'a [DagPosition],
        check_component: impl FnMut(
            &[(&ScheduledDecl, &'a DagTIR)],
        ) -> Result<RecurrenceGroupPlan, ComponentRejection>,
    ) -> Result<Self, ScheduleRejection> {
        let dags = instance_closure(callable, dag_at, instances_of);
        let mut graph = DependencyGraph::new();
        let mut dependencies = HashMap::new();
        let mut nodes = Vec::new();
        let mut owners = HashMap::new();
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
                let scheduled = ScheduledDecl {
                    identity: declaration.clone(),
                    name: entry.name().clone(),
                    span,
                };
                graph.add_node(scheduled.clone());
                nodes.push(scheduled);
                owners.insert(declaration.clone(), *dag);
                dependencies.insert(declaration, reads);
            }
        }
        for node in &nodes {
            for dependency in &dependencies[node.identity()] {
                graph.add_labelled_dependency_between(node.identity(), dependency, ());
            }
        }
        Ok(Self {
            execution_dags: dags.iter().map(|dag| dag.dag_id().clone()).collect(),
            units: order_units(graph, &nodes, &owners, &dependencies, check_component)?,
            dependencies,
        })
    }

    /// The callable followed by its instance closure in [`DagId`] order.
    #[must_use]
    pub fn execution_dags(&self) -> &[DagId] {
        &self.execution_dags
    }

    /// Every unit, each after the units it reads.
    #[must_use]
    pub const fn units(&self) -> &TopoOrder<ScheduleUnit> {
        &self.units
    }

    /// Every scheduled param and node in evaluation order: the declarations
    /// of each unit, a recurrence group's members in plan order.
    pub fn declarations(&self) -> impl Iterator<Item = &ResolvedDeclName> {
        self.units.iter().flat_map(ScheduleUnit::declarations)
    }

    /// Every scheduled param and node in evaluation order, with everything
    /// it reads (see [`Self::dependencies_of`]).
    pub fn steps(&self) -> impl Iterator<Item = (&ResolvedDeclName, &[ResolvedDeclName])> {
        self.declarations().map(|declaration| {
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

/// Order the scheduled declarations `nodes` (in insertion order, with their
/// `dependencies` already edges of `graph`) as schedule units: every cyclic
/// component must be an accepted recurrence group, which becomes one unit
/// ordered after everything its members read outside it.
fn order_units<'a>(
    graph: DependencyGraph<ScheduledDecl>,
    nodes: &[ScheduledDecl],
    owners: &HashMap<ResolvedDeclName, &'a DagTIR>,
    dependencies: &HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>,
    mut check_component: impl FnMut(
        &[(&ScheduledDecl, &'a DagTIR)],
    ) -> Result<RecurrenceGroupPlan, ComponentRejection>,
) -> Result<TopoOrder<ScheduleUnit>, ScheduleRejection> {
    let components = graph.cyclic_components();
    if components.is_empty() {
        return Ok(graph
            .into_topo_order()
            .map_err(ScheduleRejection::Cycle)?
            .map(|declaration| ScheduleUnit::Declaration(declaration.into_identity())));
    }
    let mut plans = Vec::new();
    let mut rejections = Vec::new();
    let mut group_of = HashMap::new();
    for component in components {
        let members = component
            .iter()
            .map(|declaration| (declaration, owners[declaration.identity()]))
            .collect::<Vec<_>>();
        match check_component(&members) {
            Ok(plan) => {
                for declaration in &component {
                    group_of.insert(declaration.identity().clone(), plans.len());
                }
                plans.push(plan);
            }
            Err(rejection) => rejections.push((component, rejection)),
        }
    }
    // The edges left once accepted groups are contracted: a cycle among them
    // lies in a rejected component.
    let mut contracted = DependencyGraph::new();
    for node in nodes {
        contracted.add_node(node.clone());
    }
    for node in nodes {
        for dependency in &dependencies[node.identity()] {
            let inside_group = group_of
                .get(node.identity())
                .is_some_and(|group| group_of.get(dependency) == Some(group));
            if !inside_group {
                contracted.add_labelled_dependency_between(node.identity(), dependency, ());
            }
        }
    }
    if let Err(cycle) = contracted.into_topo_order() {
        let rejection = rejections
            .into_iter()
            .find(|(component, _)| component.contains(cycle.entry()))
            .map(|(_, rejection)| rejection);
        return Err(match rejection {
            Some(ComponentRejection::Recurrence { name, span, reason }) => {
                ScheduleRejection::Recurrence { name, span, reason }
            }
            Some(ComponentRejection::PlainCycle) | None => ScheduleRejection::Cycle(cycle),
        });
    }
    let unit_of = |declaration: &ResolvedDeclName| {
        group_of.get(declaration).map_or_else(
            || UnitKey::Declaration(declaration.clone()),
            |group| UnitKey::Group(*group),
        )
    };
    let mut units = DependencyGraph::new();
    for node in nodes {
        units.add_node(unit_of(node.identity()));
    }
    for node in nodes {
        let dependent = unit_of(node.identity());
        for dependency in &dependencies[node.identity()] {
            let dependency = unit_of(dependency);
            if dependency != dependent {
                units.add_labelled_dependency_between(&dependent, &dependency, ());
            }
        }
    }
    // Contracting the accepted groups leaves no cycle; a cycle here would be
    // reported at a declaration of each unit on it.
    let declaration_of = |unit: UnitKey| match unit {
        UnitKey::Declaration(declaration) => nodes
            .iter()
            .find(|node| node.identity == declaration)
            .cloned(),
        UnitKey::Group(group) => plans
            .get(group)
            .and_then(|plan| plan.members().first())
            .and_then(|member| {
                nodes
                    .iter()
                    .find(|node| &node.identity == member.declaration())
            })
            .cloned(),
    };
    let order = units.into_topo_order().map_err(|cycle| {
        let fallback = nodes[0].clone();
        ScheduleRejection::Cycle(
            cycle.map(|unit| declaration_of(unit).unwrap_or_else(|| fallback.clone())),
        )
    })?;
    Ok(order.map(|unit| match unit {
        UnitKey::Declaration(declaration) => ScheduleUnit::Declaration(declaration),
        UnitKey::Group(group) => ScheduleUnit::Recurrence(plans[group].clone()),
    }))
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
