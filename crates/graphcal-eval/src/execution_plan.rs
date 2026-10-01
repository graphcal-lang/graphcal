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
use graphcal_compiler::ir::instance::{
    ExposedValueBody, InstanceAssertionProjection, InstancePlotProjection, InstanceValueProjection,
};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::texpr::{ExecutableBodyError, TExpr};
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::checked_instance::{CheckedInstance, ResolvedProjection};
use graphcal_compiler::tir::typed::dag_position::DagPosition;
use graphcal_compiler::tir::typed::declaration_view::DeclarationView;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use graphcal_compiler::tir::typed::evaluation_unit::{BodyKind, DeclarationBody};
use graphcal_compiler::tir::typed::model::{TypedAssertEntry, TypedPlotEntry};
use graphcal_compiler::tir::typed::scoped_node::ScopedCall;
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
pub struct StepIdx(usize);

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
/// planned body and the steps of the same callable it depends on.
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
    pub const fn body(&self) -> &ComputedBody<'p> {
        &self.body
    }

    /// The earlier steps of the same callable whose declarations it reads.
    #[must_use]
    pub fn deps(&self) -> &[StepIdx] {
        &self.deps
    }
}

#[derive(Debug)]
pub struct PreparedConstantImport {
    pub destination: ResolvedDeclName,
    pub value: ConstantReference,
}

/// Why a callable's steps could not be indexed.
#[derive(Debug, Error)]
pub enum StepIndexError {
    #[error("declaration `{0}` is scheduled twice")]
    Duplicate(ResolvedDeclName),
    #[error(
        "scheduled declaration `{dependent}` reads `{dependency}`, which is scheduled after it"
    )]
    Unordered {
        dependent: ResolvedDeclName,
        dependency: ResolvedDeclName,
    },
}

/// One semantic instance a callable's body includes, with the sealed DAG
/// that runs it and the checked declarations its include site exposes.
#[derive(Debug, Clone)]
pub struct PlannedInstance<'p> {
    instance: CheckedInstance<'p>,
    scope: SealedDag<'p>,
    outputs: Vec<PlannedOutput<'p>>,
    assertions: Vec<PlannedAssertion<'p>>,
    plots: Vec<PlannedPlot<'p>>,
}

/// One value an include site exposes from the instance's own body, with
/// the category of the declaration that produces it.
#[derive(Debug, Clone)]
pub struct PlannedOutput<'p> {
    /// The declaration the projection exposes, as the instance runs it.
    pub target: ResolvedDeclName,
    /// The include-site projection, for its exposed name.
    pub projection: &'p InstanceValueProjection,
    /// The category of the exposed declaration.
    pub category: ValueDeclCategory,
}

/// One assertion an include site exposes, with its checked body.
#[derive(Debug, Clone, Copy)]
pub struct PlannedAssertion<'p> {
    /// The include-site projection, for its exposed name and options.
    pub projection: &'p InstanceAssertionProjection,
    /// The assertion's checked body, in the scope of the DAG that owns it.
    pub body: DeclarationBody<'p>,
    /// The assertion's declaration.
    pub entry: Scoped<'p, TypedAssertEntry>,
}

/// One plot an include site requests, with its checked body.
#[derive(Debug, Clone, Copy)]
pub struct PlannedPlot<'p> {
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
    /// Pair `instance` with `scope`, the sealed DAG that runs it, and find
    /// the checked declarations its include site exposes in `tir`.
    ///
    /// # Errors
    ///
    /// Returns [`PlannedInstanceError`] when `scope` is not the instance's
    /// own DAG or an exposed declaration has no checked body of its kind.
    pub fn try_new(
        tir: &'p CheckedTir,
        instance: CheckedInstance<'p>,
        scope: SealedDag<'p>,
    ) -> Result<Self, PlannedInstanceError> {
        if !std::ptr::eq(instance.dag(), scope.dag()) {
            return Err(PlannedInstanceError::ForeignScope {
                instance: instance.dag().dag_id().clone(),
                actual: scope.dag().dag_id().clone(),
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
        self.scope
    }

    /// The values the include site exposes from the instance's own body, in
    /// record order.
    #[must_use]
    pub fn outputs(&self) -> &[PlannedOutput<'p>] {
        &self.outputs
    }

    /// The assertions the include site exposes, in record order.
    #[must_use]
    pub fn assertions(&self) -> &[PlannedAssertion<'p>] {
        &self.assertions
    }

    /// The plots the include site requests, in record order.
    #[must_use]
    pub fn plots(&self) -> &[PlannedPlot<'p>] {
        &self.plots
    }
}

/// One body and its included-instance closure, prepared before evaluation.
pub struct CallablePlan<'p> {
    scope: SealedDag<'p>,
    execution_dags: Vec<SealedDag<'p>>,
    instances: Vec<PlannedInstance<'p>>,
    closure_instances: Vec<(SealedDag<'p>, Vec<PlannedInstance<'p>>)>,
    imports: Vec<PreparedConstantImport>,
    steps: IndexVec<StepIdx, Step<'p>>,
}

impl<'p> CallablePlan<'p> {
    /// Index the computed `scheduled` declarations, in their evaluation
    /// order, as the steps of the callable `scope`. Each step depends on the
    /// earlier steps among the declarations it reads; reads of declarations
    /// this callable does not compute (constants, imports, and required
    /// ports, whose values are supplied) have no step.
    ///
    /// # Errors
    ///
    /// Returns a [`StepIndexError`] when a declaration is scheduled twice or
    /// before a scheduled declaration it reads.
    pub fn new(
        scope: SealedDag<'p>,
        execution_dags: Vec<SealedDag<'p>>,
        instances: Vec<PlannedInstance<'p>>,
        closure_instances: Vec<(SealedDag<'p>, Vec<PlannedInstance<'p>>)>,
        imports: Vec<PreparedConstantImport>,
        scheduled: Vec<PlannedDeclaration<'p>>,
    ) -> Result<Self, StepIndexError> {
        let scheduled = scheduled
            .into_iter()
            .filter_map(|declaration| match &declaration.body {
                PlannedBody::Computed(body) => {
                    let body = body.clone();
                    Some((declaration, body))
                }
                PlannedBody::Supplied => None,
            })
            .collect::<Vec<_>>();
        let mut positions = HashMap::with_capacity(scheduled.len());
        for (position, (declaration, _)) in scheduled.iter().enumerate() {
            if positions
                .insert(declaration.key, StepIdx::new(position))
                .is_some()
            {
                return Err(StepIndexError::Duplicate(declaration.key.clone()));
            }
        }
        let steps = scheduled
            .into_iter()
            .enumerate()
            .map(|(position, (declaration, body))| {
                let deps = declaration
                    .reads
                    .iter()
                    .filter_map(|read| positions.get(read).map(|step| (read, *step)))
                    .map(|(read, step)| {
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
                Ok(Step {
                    declaration,
                    body,
                    deps,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            scope,
            execution_dags,
            instances,
            closure_instances,
            imports,
            steps: IndexVec::from_items(steps),
        })
    }

    /// The callable's own body.
    #[must_use]
    pub const fn scope(&self) -> SealedDag<'p> {
        self.scope
    }

    /// The callable followed by its instance closure in [`DagId`] order.
    #[must_use]
    pub fn execution_dags(&self) -> &[SealedDag<'p>] {
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
    pub(crate) fn closure_instances(&self) -> &[(SealedDag<'p>, Vec<PlannedInstance<'p>>)] {
        &self.closure_instances
    }

    /// Whether `dag` is one of this callable's execution DAGs.
    #[must_use]
    pub(crate) fn executes(&self, dag: &DagId) -> bool {
        self.execution_dags
            .iter()
            .any(|scope| scope.dag().dag_id() == dag)
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
    pub fn step(&self, index: StepIdx) -> &Step<'p> {
        &self.steps[index]
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
                    .map(|scope| scope.dag().dag_id())
                    .collect::<Vec<_>>(),
            )
            .field("imports", &self.imports)
            .field("steps", &self.steps.items)
            .finish()
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
    pub fn call(&self, call: ScopedCall<'_>) -> &CallablePlan<'p> {
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
