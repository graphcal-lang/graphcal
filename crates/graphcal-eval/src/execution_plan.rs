//! Immutable execution-plan data, independent of preparation algorithms.
//!
//! An [`ExecPlan`] borrows the [`CheckedProgram`] it was prepared from. Every
//! body a frame runs, every dependency between scheduled declarations and
//! every callable is selected once, when the plan is prepared: running a
//! [`CallablePlan`] walks its [`Step`]s by plan-internal index and never looks
//! a declaration or a DAG up again.

use std::collections::HashMap;
use std::marker::PhantomData;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::hir::expr::UnfoldRecurrence;
use graphcal_compiler::ir::instance::{
    ExposedValueBody, InstanceAssertionProjection, InstancePlotProjection, InstanceValueProjection,
};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic::index_axis::IndexAxis;
use graphcal_compiler::semantic::time_zone::IanaTimeZoneId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::{ScopeSegment, ScopedName};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::tir::recurrence_plan::RecurrenceGroupPlan;
use graphcal_compiler::tir::texpr::{ExecutableBodyError, TExpr, TForBinding};
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::checked_instance::{CheckedInstance, ResolvedProjection};
use graphcal_compiler::tir::typed::dag_position::DagPosition;
use graphcal_compiler::tir::typed::declaration_view::DeclarationView;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use graphcal_compiler::tir::typed::evaluation_unit::{BodyKind, DeclarationBody};
use graphcal_compiler::tir::typed::model::{TypedAssertEntry, TypedPlotEntry};
use graphcal_compiler::tir::typed::scoped_node::{
    NodeKind, ScopedCall, ScopedNode, ScopedUnitExpr,
};
use thiserror::Error;

use crate::checked_program::{CheckedProgram, SealedDag};
use crate::constant_pools::ConstantReference;
use crate::domain_constraint::ResolvedDomainConstraint;

/// An index into one [`IndexVec`], whose items are pushed in the order of
/// their indices.
trait PlanIndex: Copy {
    fn position(self) -> usize;
}

/// A plan-internal index, created only by the constructors in this module
/// from the position of an element they push.
trait MintedIndex: PlanIndex {
    fn new(position: usize) -> Self;
}

/// The position of a step in its [`CallablePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StepIdx(usize);

impl PlanIndex for StepIdx {
    fn position(self) -> usize {
        self.0
    }
}

impl MintedIndex for StepIdx {
    fn new(position: usize) -> Self {
        Self(position)
    }
}

/// The position of a recurrence group in its [`CallablePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GroupIdx(usize);

impl PlanIndex for GroupIdx {
    fn position(self) -> usize {
        self.0
    }
}

impl MintedIndex for GroupIdx {
    fn new(position: usize) -> Self {
        Self(position)
    }
}

/// A registry position indexes the callables of the program's DAGs, resolved
/// in position order.
impl PlanIndex for DagPosition {
    fn position(self) -> usize {
        self.index()
    }
}

/// A vector indexed only by its own index type.
struct IndexVec<I, T> {
    items: Vec<T>,
    index: PhantomData<fn(I) -> I>,
}

impl<I: PlanIndex, T> IndexVec<I, T> {
    fn from_items(items: Vec<T>) -> Self {
        Self {
            items,
            index: PhantomData,
        }
    }

    fn iter(&self) -> std::slice::Iter<'_, T> {
        self.items.iter()
    }
}

impl<I: PlanIndex, T> std::ops::Index<I> for IndexVec<I, T> {
    type Output = T;

    fn index(&self, index: I) -> &T {
        &self.items[index.position()]
    }
}

/// What running one value declaration does.
#[derive(Clone)]
pub enum PlannedBody<'p> {
    /// A param default, a node formula, or an unfinished node: a callable
    /// computes it.
    Computed(ComputedBody<'p>),
    /// A constant or a required port: nothing is evaluated at runtime, and
    /// no callable has a step for it.
    Supplied,
}

/// What computing one value declaration does.
#[derive(Clone)]
pub enum ComputedBody<'p> {
    /// An unfinished node: running it records the TODO.
    Todo,
    /// A finished member of a recurrence group that also has unfinished
    /// members: every member reaches them, so running it records BLOCKED by
    /// them without evaluating. Only a group's planned steps carry it.
    BlockedInGroup(NonEmpty<ResolvedDeclName>),
    /// A param default or a node formula, in the scope of the DAG that owns
    /// the declaration.
    Expression {
        /// The HIR root, for its source span and its inline calls.
        root: Scoped<'p, Expr>,
        /// Its checked tree, or why the tree cannot be executed.
        tree: Result<ScopedTree<'p, &'p TExpr>, ExecutableBodyError>,
    },
}

/// One value declaration of the program, with everything running it needs.
#[derive(Clone)]
pub struct PlannedDeclaration<'p> {
    key: &'p ResolvedDeclName,
    scope: SealedDag<'p>,
    body: PlannedBody<'p>,
    reads: &'p [ResolvedDeclName],
    domain: Option<&'p ResolvedDomainConstraint>,
}

impl<'p> PlannedDeclaration<'p> {
    /// Plan one declaration of `scope`.
    #[must_use]
    pub const fn new(
        key: &'p ResolvedDeclName,
        scope: SealedDag<'p>,
        body: PlannedBody<'p>,
        reads: &'p [ResolvedDeclName],
        domain: Option<&'p ResolvedDomainConstraint>,
    ) -> Self {
        Self {
            key,
            scope,
            body,
            reads,
            domain,
        }
    }

    /// The declaration's runtime identity.
    #[must_use]
    pub const fn key(&self) -> &'p ResolvedDeclName {
        self.key
    }

    /// The sealed DAG whose body declares it.
    #[must_use]
    pub const fn scope(&self) -> SealedDag<'p> {
        self.scope
    }

    /// What running it does.
    #[must_use]
    pub const fn body(&self) -> &PlannedBody<'p> {
        &self.body
    }

    /// Everything its expression reads, in runtime identity, including reads
    /// of unscheduled constants and imports. Empty for a declaration without
    /// a runtime expression.
    #[must_use]
    pub const fn reads(&self) -> &'p [ResolvedDeclName] {
        self.reads
    }

    /// Its resolved domain constraint, if it has one.
    #[must_use]
    pub const fn domain(&self) -> Option<&'p ResolvedDomainConstraint> {
        self.domain
    }
}

impl std::fmt::Debug for PlannedDeclaration<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlannedDeclaration")
            .field("key", self.key)
            .field("body", &self.scope.dag().dag_id())
            .field(
                "kind",
                &match &self.body {
                    PlannedBody::Computed(ComputedBody::Todo) => "todo",
                    PlannedBody::Computed(ComputedBody::BlockedInGroup(_)) => "blocked in group",
                    PlannedBody::Computed(ComputedBody::Expression { tree: Ok(_), .. }) => {
                        "executable"
                    }
                    PlannedBody::Computed(ComputedBody::Expression { tree: Err(_), .. }) => {
                        "deferred"
                    }
                    PlannedBody::Supplied => "supplied",
                },
            )
            .finish_non_exhaustive()
    }
}

/// One scheduled declaration of a callable that the callable computes: its
/// planned body and the steps of the same callable it depends on outside its
/// recurrence group.
pub struct Step<'p> {
    declaration: PlannedDeclaration<'p>,
    body: ComputedBody<'p>,
    deps: Vec<StepIdx>,
}

impl std::fmt::Debug for Step<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The declaration's debug output names the kind of its body.
        formatter
            .debug_struct("Step")
            .field("declaration", &self.declaration)
            .field("deps", &self.deps)
            .finish_non_exhaustive()
    }
}

impl<'p> Step<'p> {
    /// The declaration this step runs.
    #[must_use]
    pub const fn declaration(&self) -> &PlannedDeclaration<'p> {
        &self.declaration
    }

    /// What computing the declaration does.
    #[must_use]
    pub(crate) const fn body(&self) -> &ComputedBody<'p> {
        &self.body
    }

    /// The earlier steps of the same callable whose declarations it reads,
    /// except the members of its own recurrence group.
    #[must_use]
    pub(crate) fn deps(&self) -> &[StepIdx] {
        &self.deps
    }
}

/// How a recurrence member presents its value: the display of a top-level
/// conversion, applied to every element.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RecurrenceDisplay<'p> {
    /// No top-level conversion.
    Plain,
    /// `… -> unit`.
    Unit(ScopedUnitExpr<'p>),
    /// `… -> "Area/Location"`.
    Timezone(&'p IanaTimeZoneId),
}

/// The checked body of a recurrence member under its top-level conversion:
/// the only two shapes a member can have.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RecurrenceBody<'p> {
    /// `for t: A, … { body }`.
    For {
        first: &'p TForBinding,
        remaining: &'p [TForBinding],
        body: ScopedNode<'p>,
    },
    /// `unfold(A, init, |prev_state, prev_t, t| body)`.
    Unfold {
        recurrence: &'p UnfoldRecurrence,
        init: ScopedNode<'p>,
        body: ScopedNode<'p>,
        axis: &'p IndexAxis,
    },
}

impl<'p> RecurrenceBody<'p> {
    /// The body of a member's checked tree, under at most one top-level
    /// display conversion.
    fn of(root: ScopedNode<'p>) -> Option<(RecurrenceDisplay<'p>, Self)> {
        let (display, body) = match root.kind() {
            NodeKind::Convert { expr, target } => (RecurrenceDisplay::Unit(target), expr),
            NodeKind::DisplayTimezone { expr, timezone } => {
                (RecurrenceDisplay::Timezone(timezone), expr)
            }
            _ => (RecurrenceDisplay::Plain, root),
        };
        let body = match body.kind() {
            NodeKind::For { bindings, body } => {
                let (first, remaining) = bindings.split_first();
                Self::For {
                    first,
                    remaining,
                    body,
                }
            }
            NodeKind::Unfold {
                recurrence,
                init,
                body,
                axis,
            } => Self::Unfold {
                recurrence,
                init,
                body,
                axis,
            },
            _ => return None,
        };
        Some((display, body))
    }

    /// The axis the body steps along.
    pub(crate) const fn axis(&self) -> &'p IndexAxis {
        match self {
            Self::For { first, .. } => &first.axis,
            Self::Unfold { axis, .. } => axis,
        }
    }
}

/// One member of an accepted recurrence group, as a callable runs it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecurrenceMemberPlan<'p> {
    step: StepIdx,
    root: ScopedNode<'p>,
    display: RecurrenceDisplay<'p>,
    body: RecurrenceBody<'p>,
}

impl<'p> RecurrenceMemberPlan<'p> {
    /// The member's step.
    pub(crate) const fn step(&self) -> StepIdx {
        self.step
    }

    /// The root of the member's checked tree, for its dependencies, source
    /// span, and DAG.
    pub(crate) const fn root(&self) -> ScopedNode<'p> {
        self.root
    }

    /// The display of the member's top-level conversion.
    pub(crate) const fn display(&self) -> RecurrenceDisplay<'p> {
        self.display
    }

    /// The member's `for` or `unfold` body.
    pub(crate) const fn body(&self) -> RecurrenceBody<'p> {
        self.body
    }
}

/// One accepted recurrence group of a callable: the checker's plan, its
/// members in plan order with their checked bodies, and the axis they step
/// along.
pub(crate) struct PlannedGroup<'p> {
    plan: &'p RecurrenceGroupPlan,
    members: Vec<RecurrenceMemberPlan<'p>>,
    axis: IndexAxis,
}

impl<'p> PlannedGroup<'p> {
    /// The checker's plan.
    pub(crate) const fn plan(&self) -> &'p RecurrenceGroupPlan {
        self.plan
    }

    /// The members, in plan order.
    pub(crate) fn members(&self) -> &[RecurrenceMemberPlan<'p>] {
        &self.members
    }

    /// The axis every member steps along.
    pub(crate) const fn axis(&self) -> &IndexAxis {
        &self.axis
    }
}

/// One unit a callable runs, in evaluation order.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RunUnit {
    /// One declaration, evaluated as a whole.
    Step(StepIdx),
    /// A recurrence group, evaluated element by element.
    Group(GroupIdx),
}

/// One unit of a callable's schedule, with its planned declarations.
pub(crate) enum PlannedUnit<'p> {
    Declaration(PlannedDeclaration<'p>),
    Recurrence {
        plan: &'p RecurrenceGroupPlan,
        /// The members' declarations, in plan order.
        members: Vec<PlannedDeclaration<'p>>,
    },
}

#[derive(Debug)]
pub struct PreparedConstantImport {
    pub(crate) destination: ResolvedDeclName,
    pub value: ConstantReference,
}

/// Why a callable's steps could not be indexed.
#[derive(Debug, Error)]
pub(crate) enum StepIndexError {
    #[error("declaration `{0}` is scheduled twice")]
    Duplicate(ResolvedDeclName),
    #[error(
        "scheduled declaration `{dependent}` reads `{dependency}`, which is scheduled after it"
    )]
    Unordered {
        dependent: ResolvedDeclName,
        dependency: ResolvedDeclName,
    },
    #[error("recurrence group member `{0}` has no executable formula")]
    NotAFormulaMember(ResolvedDeclName),
    #[error("recurrence group member `{0}` is not a top-level `for` or `unfold`")]
    NotARecurrenceBody(ResolvedDeclName),
    #[error("recurrence group member `{0}` steps along another axis than its group")]
    AxisMismatch(ResolvedDeclName),
}

/// One DAG a callable executes — its own body, or a semantic instance below
/// it — with the instance scopes that name it below the callable's body.
///
/// Created only by `ClosureDag::below`, so every DAG of a callable's
/// closure has a name in the callable's body.
#[derive(Debug, Clone)]
pub struct ClosureDag<'p> {
    scope: SealedDag<'p>,
    scopes: Vec<ScopeSegment>,
}

impl<'p> ClosureDag<'p> {
    /// `scope` as a DAG of the closure of the callable whose body is
    /// `owner`, when it is that body or a DAG below it.
    #[must_use]
    pub(crate) fn below(owner: &DagId, scope: SealedDag<'p>) -> Option<Self> {
        scope
            .dag()
            .dag_id()
            .scopes_below(owner)
            .map(|scopes| Self { scope, scopes })
    }

    /// The sealed DAG.
    #[must_use]
    pub const fn scope(&self) -> SealedDag<'p> {
        self.scope
    }

    /// `name`, written in this DAG, as the callable's body names it:
    /// qualified by the scopes of this DAG below the body.
    #[must_use]
    pub(crate) fn qualify(&self, name: &ScopedName) -> ScopedName {
        qualified(&self.scopes, name)
    }

    /// The declaration `leaf` of this DAG, as the callable's body names it.
    #[must_use]
    pub(crate) fn member(&self, leaf: &DeclName) -> ScopedName {
        self.qualify(&ScopedName::local(leaf.clone()))
    }
}

/// `name` qualified by `scopes`, then by its own qualifier.
fn qualified(scopes: &[ScopeSegment], name: &ScopedName) -> ScopedName {
    let qualifier = scopes
        .iter()
        .chain(name.qualifier())
        .cloned()
        .collect::<Vec<_>>();
    ScopedName::from_parts(NonEmpty::try_from_vec(qualifier).ok(), name.leaf().clone())
}

/// One semantic instance a callable's body includes, with the sealed DAG
/// that runs it and the checked declarations its include site exposes.
#[derive(Debug, Clone)]
pub struct PlannedInstance<'p> {
    instance: CheckedInstance<'p>,
    scope: ClosureDag<'p>,
    outputs: Vec<PlannedOutput<'p>>,
    assertions: Vec<PlannedAssertion<'p>>,
    plots: Vec<PlannedPlot<'p>>,
}

/// One value an include site exposes from the instance's own body, with
/// the category of the declaration that produces it.
#[derive(Debug, Clone)]
pub(crate) struct PlannedOutput<'p> {
    /// The declaration the projection exposes, as the instance runs it.
    pub target: ResolvedDeclName,
    /// The include-site projection, for its exposed name.
    pub projection: &'p InstanceValueProjection,
    /// The category of the exposed declaration.
    pub category: ValueDeclCategory,
}

/// One assertion an include site exposes, with its checked body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlannedAssertion<'p> {
    /// The include-site projection, for its exposed name and options.
    pub projection: &'p InstanceAssertionProjection,
    /// The assertion's checked body, in the scope of the DAG that owns it.
    pub body: DeclarationBody<'p>,
    /// The assertion's declaration.
    pub entry: Scoped<'p, TypedAssertEntry>,
}

/// One plot an include site requests, with its checked body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlannedPlot<'p> {
    /// The include-site projection, for its alias and visibility.
    pub projection: &'p InstancePlotProjection,
    /// The plot's checked body, in the scope of the DAG that owns it, which
    /// may be an instance nested in this one when a template forwards its
    /// own plot.
    pub body: DeclarationBody<'p>,
    /// The plot's declaration.
    pub entry: Scoped<'p, TypedPlotEntry>,
}

/// Why a semantic instance cannot be planned.
#[derive(Debug, Error)]
pub enum PlannedInstanceError {
    /// The sealed DAG is not the instance's own DAG.
    #[error("semantic instance `{instance}` is run by DAG `{actual}`")]
    ForeignScope { instance: DagId, actual: DagId },
    /// An exposed value is not a runtime value of the instance.
    #[error("projected declaration `{0}` is not a runtime value")]
    NotAValue(ResolvedDeclName),
    /// An exposed assertion has no checked assertion body.
    #[error("assertion `{0}` has no checked body")]
    NotAnAssertion(ResolvedDeclName),
    /// A requested plot has no checked plot body.
    #[error("plot `{0}` has no checked body")]
    NotAPlot(ResolvedDeclName),
}

impl<'p> PlannedInstance<'p> {
    /// Pair `instance` with `scope`, the closure DAG that runs it, and find
    /// the checked declarations its include site exposes in `tir`.
    ///
    /// # Errors
    ///
    /// Returns [`PlannedInstanceError`] when `scope` is not the instance's
    /// own DAG or an exposed declaration has no checked body of its kind.
    pub fn try_new(
        tir: &'p CheckedTir,
        instance: CheckedInstance<'p>,
        scope: ClosureDag<'p>,
    ) -> Result<Self, PlannedInstanceError> {
        if !std::ptr::eq(instance.dag(), scope.scope().dag()) {
            return Err(PlannedInstanceError::ForeignScope {
                instance: instance.dag().dag_id().clone(),
                actual: scope.scope().dag().dag_id().clone(),
            });
        }
        // A value the including DAG declares itself (a projection alias) is
        // reported by the including DAG, so only the instance's own values
        // are planned here.
        let outputs = instance
            .output_projections()
            .filter(|resolved| resolved.projection.body() == ExposedValueBody::Instance)
            .map(|ResolvedProjection { target, projection }| {
                let category = instance
                    .dag()
                    .declaration(&target)
                    .and_then(DeclarationView::value)
                    .map(|value| value.category)
                    .ok_or_else(|| PlannedInstanceError::NotAValue(target.clone()))?;
                Ok(PlannedOutput {
                    target,
                    projection,
                    category,
                })
            })
            .collect::<Result<_, _>>()?;
        let assertions = instance
            .assertion_projections()
            .map(|ResolvedProjection { target, projection }| {
                let body = tir.declaration_body(&target);
                match body.map(|body| (body, body.kind())) {
                    Some((body, BodyKind::Assert(entry))) => Ok(PlannedAssertion {
                        projection,
                        body,
                        entry,
                    }),
                    _ => Err(PlannedInstanceError::NotAnAssertion(target)),
                }
            })
            .collect::<Result<_, _>>()?;
        let plots = instance
            .plot_projections()
            .map(|ResolvedProjection { target, projection }| {
                let body = tir.declaration_body(&target);
                match body.map(|body| (body, body.kind())) {
                    Some((body, BodyKind::Plot(entry))) => Ok(PlannedPlot {
                        projection,
                        body,
                        entry,
                    }),
                    _ => Err(PlannedInstanceError::NotAPlot(target)),
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            instance,
            scope,
            outputs,
            assertions,
            plots,
        })
    }

    /// The include edge and the checked DAG that runs its instance.
    #[must_use]
    pub const fn instance(&self) -> CheckedInstance<'p> {
        self.instance
    }

    /// The instance's sealed DAG, with the source its diagnostics point into.
    #[must_use]
    pub const fn scope(&self) -> SealedDag<'p> {
        self.scope.scope()
    }

    /// The instance's DAG in the closure of the callable that plans it.
    #[must_use]
    pub const fn closure(&self) -> &ClosureDag<'p> {
        &self.scope
    }

    /// The values the include site exposes from the instance's own body, in
    /// record order.
    #[must_use]
    pub(crate) fn outputs(&self) -> &[PlannedOutput<'p>] {
        &self.outputs
    }

    /// The assertions the include site exposes, in record order.
    #[must_use]
    pub(crate) fn assertions(&self) -> &[PlannedAssertion<'p>] {
        &self.assertions
    }

    /// The plots the include site requests, in record order.
    #[must_use]
    pub(crate) fn plots(&self) -> &[PlannedPlot<'p>] {
        &self.plots
    }
}

/// One body and its included-instance closure, prepared before evaluation.
pub struct CallablePlan<'p> {
    scope: SealedDag<'p>,
    execution_dags: Vec<ClosureDag<'p>>,
    instances: Vec<PlannedInstance<'p>>,
    closure_instances: Vec<(ClosureDag<'p>, Vec<PlannedInstance<'p>>)>,
    imports: Vec<PreparedConstantImport>,
    steps: IndexVec<StepIdx, Step<'p>>,
    groups: IndexVec<GroupIdx, PlannedGroup<'p>>,
    units: Vec<RunUnit>,
}

/// A computed declaration and its body, with the recurrence group it belongs
/// to: the group's schedule position (for its dependencies) and, when the
/// group runs as one unit, the group.
type ComputedEntry<'p> = (
    PlannedDeclaration<'p>,
    ComputedBody<'p>,
    Option<(usize, Option<GroupIdx>)>,
);

/// Add the members of the recurrence group `plan`, scheduled as group
/// `membership`, to `computed`. A group whose members all have executable
/// bodies runs as one unit and joins `groups`. A group with unfinished members
/// does not run: those members are TODO and the others are blocked by them.
/// Otherwise, as in a template that is only run through its instances, its
/// members are steps that report their bodies.
fn plan_recurrence<'p>(
    plan: &'p RecurrenceGroupPlan,
    members: Vec<PlannedDeclaration<'p>>,
    membership: usize,
    computed: &mut Vec<ComputedEntry<'p>>,
    groups: &mut Vec<PlannedGroup<'p>>,
) -> Result<(), StepIndexError> {
    let unfinished = members
        .iter()
        .filter(|declaration| matches!(declaration.body, PlannedBody::Computed(ComputedBody::Todo)))
        .map(|declaration| declaration.key.clone())
        .collect::<Vec<_>>();
    if let Ok(unfinished) = NonEmpty::try_from_vec(unfinished) {
        for declaration in members {
            let body = match &declaration.body {
                PlannedBody::Computed(ComputedBody::Todo) => ComputedBody::Todo,
                PlannedBody::Computed(ComputedBody::Expression { .. }) => {
                    ComputedBody::BlockedInGroup(unfinished.clone())
                }
                PlannedBody::Computed(ComputedBody::BlockedInGroup(_)) | PlannedBody::Supplied => {
                    return Err(StepIndexError::NotAFormulaMember(declaration.key.clone()));
                }
            };
            computed.push((declaration, body, Some((membership, None))));
        }
        return Ok(());
    }
    let first_step = computed.len();
    let mut planned = Vec::with_capacity(members.len());
    let mut axis = None::<IndexAxis>;
    for declaration in members {
        let body = match &declaration.body {
            PlannedBody::Computed(body @ ComputedBody::Expression { .. }) => body.clone(),
            PlannedBody::Computed(ComputedBody::Todo | ComputedBody::BlockedInGroup(_))
            | PlannedBody::Supplied => {
                return Err(StepIndexError::NotAFormulaMember(declaration.key.clone()));
            }
        };
        if let ComputedBody::Expression { tree: Ok(tree), .. } = &body {
            let root = tree.into_root();
            let (display, member_body) = RecurrenceBody::of(root)
                .ok_or_else(|| StepIndexError::NotARecurrenceBody(declaration.key.clone()))?;
            match &axis {
                None => axis = Some(member_body.axis().clone()),
                Some(first) if first.len() == member_body.axis().len() => {}
                Some(_) => return Err(StepIndexError::AxisMismatch(declaration.key.clone())),
            }
            planned.push(RecurrenceMemberPlan {
                step: StepIdx::new(computed.len()),
                root,
                display,
                body: member_body,
            });
        }
        computed.push((declaration, body, Some((membership, None))));
    }
    let added = &mut computed[first_step..];
    if let (true, Some(axis)) = (planned.len() == added.len(), axis) {
        let group = GroupIdx::new(groups.len());
        for (_, _, entry) in added {
            *entry = Some((membership, Some(group)));
        }
        groups.push(PlannedGroup {
            plan,
            members: planned,
            axis,
        });
    }
    Ok(())
}

impl<'p> CallablePlan<'p> {
    /// Index the computed declarations of the `scheduled` units, in their
    /// evaluation order, as the steps of the callable `scope`. Each step
    /// depends on the earlier steps among the declarations it reads outside
    /// its recurrence group; reads of declarations this callable does not
    /// compute (constants, imports, and required ports, whose values are
    /// supplied) have no step.
    ///
    /// # Errors
    ///
    /// Returns a [`StepIndexError`] when a declaration is scheduled twice or
    /// before a scheduled declaration it reads, or a recurrence group member
    /// has no formula.
    pub(crate) fn new(
        scope: SealedDag<'p>,
        execution_dags: Vec<ClosureDag<'p>>,
        instances: Vec<PlannedInstance<'p>>,
        closure_instances: Vec<(ClosureDag<'p>, Vec<PlannedInstance<'p>>)>,
        imports: Vec<PreparedConstantImport>,
        scheduled: Vec<PlannedUnit<'p>>,
    ) -> Result<Self, StepIndexError> {
        let mut computed = Vec::<ComputedEntry<'p>>::new();
        let mut groups = Vec::new();
        let mut scheduled_groups = 0_usize;
        for unit in scheduled {
            match unit {
                PlannedUnit::Declaration(declaration) => match &declaration.body {
                    PlannedBody::Computed(body) => {
                        let body = body.clone();
                        computed.push((declaration, body, None));
                    }
                    PlannedBody::Supplied => {}
                },
                PlannedUnit::Recurrence { plan, members } => {
                    plan_recurrence(plan, members, scheduled_groups, &mut computed, &mut groups)?;
                    scheduled_groups = scheduled_groups.saturating_add(1);
                }
            }
        }
        let mut positions = HashMap::with_capacity(computed.len());
        for (position, (declaration, _, membership)) in computed.iter().enumerate() {
            let group = membership.map(|(id, _)| id);
            if positions
                .insert(declaration.key, (StepIdx::new(position), group))
                .is_some()
            {
                return Err(StepIndexError::Duplicate(declaration.key.clone()));
            }
        }
        let mut units = Vec::new();
        let mut steps = Vec::with_capacity(computed.len());
        for (position, (declaration, body, membership)) in computed.into_iter().enumerate() {
            let group = membership.map(|(id, _)| id);
            let deps = declaration
                .reads
                .iter()
                .filter_map(|read| positions.get(read).map(|step| (read, *step)))
                .filter(|(_, (_, read_group))| group.is_none() || *read_group != group)
                .map(|(read, (step, _))| {
                    if step.position() < position {
                        Ok(step)
                    } else {
                        Err(StepIndexError::Unordered {
                            dependent: declaration.key.clone(),
                            dependency: read.clone(),
                        })
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            match membership.and_then(|(_, runnable)| runnable) {
                Some(group) => {
                    if units
                        .last()
                        .is_none_or(|unit| !matches!(unit, RunUnit::Group(last) if *last == group))
                    {
                        units.push(RunUnit::Group(group));
                    }
                }
                None => units.push(RunUnit::Step(StepIdx::new(position))),
            }
            steps.push(Step {
                declaration,
                body,
                deps,
            });
        }
        Ok(Self {
            scope,
            execution_dags,
            instances,
            closure_instances,
            imports,
            steps: IndexVec::from_items(steps),
            groups: IndexVec::from_items(groups),
            units,
        })
    }

    /// The callable's own body.
    #[must_use]
    pub const fn scope(&self) -> SealedDag<'p> {
        self.scope
    }

    /// The callable followed by its instance closure in [`DagId`] order.
    #[must_use]
    pub fn execution_dags(&self) -> &[ClosureDag<'p>] {
        &self.execution_dags
    }

    /// The semantic instances the callable's own body includes, in record
    /// order.
    #[must_use]
    pub fn semantic_instances(&self) -> &[PlannedInstance<'p>] {
        &self.instances
    }

    /// The callable's own body and every semantic instance of its execution
    /// closure, in [`DagId`] order, each with the semantic instances it
    /// includes, in record order.
    #[must_use]
    pub(crate) fn closure_instances(&self) -> &[(ClosureDag<'p>, Vec<PlannedInstance<'p>>)] {
        &self.closure_instances
    }

    /// Whether `dag` is one of this callable's execution DAGs.
    #[must_use]
    pub(crate) fn executes(&self, dag: &DagId) -> bool {
        self.execution_dags
            .iter()
            .any(|closure| closure.scope().dag().dag_id() == dag)
    }

    /// The constants the callable's execution DAGs import.
    #[must_use]
    pub fn constant_imports(&self) -> &[PreparedConstantImport] {
        &self.imports
    }

    /// Every step, in evaluation order.
    pub fn steps(&self) -> impl Iterator<Item = &Step<'p>> {
        self.steps.iter()
    }

    /// One step of this callable.
    #[must_use]
    pub(crate) fn step(&self, index: StepIdx) -> &Step<'p> {
        &self.steps[index]
    }

    /// Every unit, in evaluation order.
    pub(crate) fn units(&self) -> &[RunUnit] {
        &self.units
    }

    /// One recurrence group of this callable.
    #[must_use]
    pub(crate) fn group(&self, index: GroupIdx) -> &PlannedGroup<'p> {
        &self.groups[index]
    }
}

impl std::fmt::Debug for CallablePlan<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CallablePlan")
            .field("owner", self.scope.dag().dag_id())
            .field(
                "execution_dags",
                &self
                    .execution_dags
                    .iter()
                    .map(|closure| closure.scope().dag().dag_id())
                    .collect::<Vec<_>>(),
            )
            .field("imports", &self.imports)
            .field("steps", &self.steps.items)
            .finish()
    }
}

/// The callable one evaluation enters, with the plan it belongs to.
///
/// Created only by [`ExecPlan::root_entry`], for the root whose inputs a
/// parameter row binds, and by [`ExecPlan::defaults_entries`], for the DAGs
/// with no input only a binding supplies. So every entry's inputs are supplied.
#[derive(Clone, Copy)]
pub(crate) struct Entry<'a, 'p> {
    plan: &'a ExecPlan<'p>,
    callable: &'a CallablePlan<'p>,
}

impl<'a, 'p> Entry<'a, 'p> {
    /// The plan the entry belongs to.
    pub(crate) const fn plan(self) -> &'a ExecPlan<'p> {
        self.plan
    }

    /// The callable the evaluation runs.
    pub(crate) const fn callable(self) -> &'a CallablePlan<'p> {
        self.callable
    }

    /// The DAG the evaluation runs.
    pub(crate) const fn dag_id(self) -> &'a DagId {
        self.callable.scope().dag().dag_id()
    }
}

/// A compiled execution plan ready for runtime evaluation.
pub struct ExecPlan<'p> {
    program: &'p CheckedProgram,
    has_unfinished_definitions: bool,
    declarations: HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
    /// The callable of each DAG of the program, by registry position.
    callables: IndexVec<DagPosition, CallablePlan<'p>>,
    /// The callee of each call slot of each DAG of the program, by registry
    /// position, then by slot.
    calls: IndexVec<DagPosition, Box<[DagPosition]>>,
}

impl<'p> ExecPlan<'p> {
    /// Assemble the plan of `program` from `prepare`, which prepares the
    /// callable of the sealed DAG it is given: it is called once for each
    /// DAG of the program's registry, in position order, so every DAG, and
    /// with it the callee of every inline call, has exactly one callable, at
    /// the DAG's position.
    ///
    /// # Errors
    ///
    /// Returns the first error of `prepare`.
    pub(crate) fn new<E>(
        program: &'p CheckedProgram,
        declarations: HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
        prepare: impl FnMut(SealedDag<'p>) -> Result<CallablePlan<'p>, E>,
    ) -> Result<Self, E> {
        let callables = program
            .positioned()
            .map(|(_, scope)| scope)
            .map(prepare)
            .collect::<Result<Vec<_>, _>>()
            .map(IndexVec::from_items)?;
        let registry = program.tir().dag_registry();
        let calls = IndexVec::from_items(
            registry
                .positioned()
                .map(|(caller, _)| registry.callee_positions(caller).into())
                .collect(),
        );
        let has_unfinished_definitions = declarations.values().any(|declaration| {
            matches!(declaration.body, PlannedBody::Computed(ComputedBody::Todo))
        });
        Ok(Self {
            program,
            has_unfinished_definitions,
            declarations,
            callables,
            calls,
        })
    }

    /// The sealed program this plan executes.
    #[must_use]
    pub const fn program(&self) -> &'p CheckedProgram {
        self.program
    }

    /// The checked TIR this plan executes.
    #[must_use]
    pub const fn tir(&self) -> &'p graphcal_compiler::tir::typed::checked::CheckedTir {
        self.program.tir()
    }

    /// Whether any declaration of the program is unfinished.
    pub(crate) const fn has_unfinished_definitions(&self) -> bool {
        self.has_unfinished_definitions
    }

    /// The root DAG's callable.
    #[must_use]
    pub fn root(&self) -> &CallablePlan<'p> {
        &self.callables[DagPosition::ROOT]
    }

    /// The root as the entry of an evaluation, whose inputs a parameter row
    /// binds.
    #[must_use]
    pub(crate) fn root_entry(&self) -> Entry<'_, 'p> {
        Entry {
            plan: self,
            callable: self.root(),
        }
    }

    /// Every DAG of the program that runs on its own defaults, as the entry
    /// of an evaluation: the program's
    /// [`default_runnable_dags`](CheckedTir::default_runnable_dags).
    pub(crate) fn defaults_entries(&self) -> impl Iterator<Item = Entry<'_, 'p>> {
        let tir = self.tir();
        let runnable = tir
            .default_runnable_dags()
            .collect::<std::collections::HashSet<_>>();
        tir.dag_registry()
            .positioned()
            .filter(move |(_, dag)| runnable.contains(dag.dag_id()))
            .map(|(position, _)| Entry {
                plan: self,
                callable: &self.callables[position],
            })
    }

    /// Every callable, the root first.
    #[cfg(any(test, feature = "test-internals"))]
    pub fn callables(&self) -> impl Iterator<Item = &CallablePlan<'p>> {
        self.callables.iter()
    }

    /// The callables the body of `owner` calls, by call slot, when `owner`
    /// is a DAG of the program.
    #[cfg(any(test, feature = "test-internals"))]
    #[must_use]
    pub fn callees_of(&self, owner: &DagId) -> Option<Vec<&CallablePlan<'p>>> {
        let registry = self.tir().dag_registry();
        registry.get_positioned(owner).map(|(caller, _)| {
            self.calls[caller]
                .iter()
                .map(|&callee| &self.callables[callee])
                .collect()
        })
    }

    /// What an inline call runs: the callable of the DAG the program's
    /// registry resolved for the call's slot.
    ///
    /// The call comes from a tree of this plan's program, whose every DAG
    /// has a callable.
    #[must_use]
    pub(crate) fn call(&self, call: ScopedCall<'_>) -> &CallablePlan<'p> {
        &self.callables[self.calls[call.caller()][call.get().index()]]
    }

    /// Any value declaration of the program.
    #[must_use]
    pub fn declaration(&self, key: &ResolvedDeclName) -> Option<&PlannedDeclaration<'p>> {
        self.declarations.get(key)
    }

    /// The resolved domain constraint of a value declaration, if it has one.
    #[must_use]
    pub fn domain_constraint(
        &self,
        key: &ResolvedDeclName,
    ) -> Option<&'p ResolvedDomainConstraint> {
        self.declaration(key)?.domain
    }
}

impl std::fmt::Debug for ExecPlan<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut declarations = self.declarations.values().collect::<Vec<_>>();
        declarations.sort_by_key(|declaration| declaration.key);
        formatter
            .debug_struct("ExecPlan")
            .field("program", self.program)
            .field(
                "has_unfinished_definitions",
                &self.has_unfinished_definitions,
            )
            .field("declarations", &declarations)
            .field("callables", &self.callables.items)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::syntax::module_name::ModuleAliasName;

    use super::*;

    fn named(alias: &str) -> ScopeSegment {
        ScopeSegment::Named(ModuleAliasName::expect_valid(alias))
    }

    fn leaf(name: &str) -> DeclName {
        DeclName::expect_valid(name)
    }

    #[test]
    fn closure_names_are_qualified_by_every_scope_then_their_own_qualifier() {
        let local = ScopedName::local(leaf("x"));
        assert_eq!(qualified(&[], &local), local);
        assert_eq!(
            qualified(&[named("outer"), named("inner")], &local),
            ScopedName::qualified(
                NonEmpty::new(named("outer"), vec![named("inner")]),
                leaf("x")
            )
        );
        let exposed = ScopedName::in_scope(named("inst"), leaf("ok"));
        assert_eq!(
            qualified(&[named("outer")], &exposed),
            ScopedName::qualified(
                NonEmpty::new(named("outer"), vec![named("inst")]),
                leaf("ok")
            )
        );
        assert_eq!(qualified(&[], &exposed), exposed);
    }
}
