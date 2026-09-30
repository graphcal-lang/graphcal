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
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::tir::texpr::{ExecutableBodyError, TExpr};
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked_instance::CheckedInstance;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use thiserror::Error;

use crate::checked_program::{CheckedProgram, SealedDag};
use crate::constant_pools::ConstantReference;
use crate::domain_constraint::ResolvedDomainConstraint;

/// A plan-internal index into one [`IndexVec`].
///
/// Indices are created only by the constructors in this module, from the
/// position of an element they push.
trait PlanIndex: Copy {
    fn new(position: usize) -> Self;
    fn position(self) -> usize;
}

/// The position of a callable in its [`ExecPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CallableIdx(usize);

/// The position of a step in its [`CallablePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepIdx(usize);

impl PlanIndex for CallableIdx {
    fn new(position: usize) -> Self {
        Self(position)
    }

    fn position(self) -> usize {
        self.0
    }
}

impl PlanIndex for StepIdx {
    fn new(position: usize) -> Self {
        Self(position)
    }

    fn position(self) -> usize {
        self.0
    }
}

/// A vector indexed only by its own plan-internal index type.
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

    fn indices(&self) -> impl Iterator<Item = I> + use<I, T> {
        (0..self.items.len()).map(I::new)
    }

    const fn len(&self) -> usize {
        self.items.len()
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
    /// A constant or a required port: nothing is evaluated at runtime.
    Supplied,
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
                    PlannedBody::Todo => "todo",
                    PlannedBody::Expression { tree: Ok(_), .. } => "executable",
                    PlannedBody::Expression { tree: Err(_), .. } => "deferred",
                    PlannedBody::Supplied => "supplied",
                },
            )
            .finish_non_exhaustive()
    }
}

/// One scheduled declaration of a callable: its planned body and the steps
/// of the same callable it depends on.
#[derive(Debug)]
pub struct Step<'p> {
    declaration: PlannedDeclaration<'p>,
    deps: Vec<StepIdx>,
}

impl<'p> Step<'p> {
    /// The declaration this step runs.
    #[must_use]
    pub const fn declaration(&self) -> &PlannedDeclaration<'p> {
        &self.declaration
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

#[derive(Debug, Default)]
pub struct PreparedImports {
    pub constants: Vec<PreparedConstantImport>,
    pub runtime: Vec<ResolvedDeclName>,
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
/// that runs it.
#[derive(Debug, Clone, Copy)]
pub struct PlannedInstance<'p> {
    instance: CheckedInstance<'p>,
    scope: SealedDag<'p>,
}

/// Why a sealed DAG cannot run a semantic instance.
#[derive(Debug, Error)]
#[error("semantic instance `{instance}` is run by DAG `{actual}`")]
pub struct PlannedInstanceError {
    instance: DagId,
    actual: DagId,
}

impl<'p> PlannedInstance<'p> {
    /// Pair `instance` with `scope`, the sealed DAG that runs it.
    ///
    /// # Errors
    ///
    /// Returns [`PlannedInstanceError`] when `scope` is not the instance's
    /// own DAG.
    pub fn try_new(
        instance: CheckedInstance<'p>,
        scope: SealedDag<'p>,
    ) -> Result<Self, PlannedInstanceError> {
        if std::ptr::eq(instance.dag(), scope.dag()) {
            Ok(Self { instance, scope })
        } else {
            Err(PlannedInstanceError {
                instance: instance.dag().dag_id().clone(),
                actual: scope.dag().dag_id().clone(),
            })
        }
    }

    /// The include edge and the checked DAG that runs its instance.
    #[must_use]
    pub const fn instance(self) -> CheckedInstance<'p> {
        self.instance
    }

    /// The instance's sealed DAG, with the source its diagnostics point into.
    #[must_use]
    pub const fn scope(self) -> SealedDag<'p> {
        self.scope
    }
}

/// One body and its included-instance closure, prepared before evaluation.
pub struct CallablePlan<'p> {
    scope: SealedDag<'p>,
    execution_dags: Vec<SealedDag<'p>>,
    instances: Vec<PlannedInstance<'p>>,
    closure_instances: Vec<(SealedDag<'p>, Vec<PlannedInstance<'p>>)>,
    imports: PreparedImports,
    steps: IndexVec<StepIdx, Step<'p>>,
}

impl<'p> CallablePlan<'p> {
    /// Index `scheduled` declarations, in their evaluation order, as the
    /// steps of the callable `scope`. Each step depends on the earlier steps
    /// among the declarations it reads; reads of declarations this callable
    /// does not schedule (constants and imports) have no step.
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
        imports: PreparedImports,
        scheduled: Vec<PlannedDeclaration<'p>>,
    ) -> Result<Self, StepIndexError> {
        let mut positions = HashMap::with_capacity(scheduled.len());
        for (position, declaration) in scheduled.iter().enumerate() {
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
            .map(|(position, declaration)| {
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
                Ok(Step { declaration, deps })
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
    pub fn closure_instances(&self) -> &[(SealedDag<'p>, Vec<PlannedInstance<'p>>)] {
        &self.closure_instances
    }

    /// Whether `dag` is one of this callable's execution DAGs.
    #[must_use]
    pub fn executes(&self, dag: &DagId) -> bool {
        self.execution_dags
            .iter()
            .any(|scope| scope.dag().dag_id() == dag)
    }

    /// Retained constant references and explicit runtime imports.
    #[must_use]
    pub const fn imports(&self) -> &PreparedImports {
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
    root: CallableIdx,
    callables: IndexVec<CallableIdx, CallablePlan<'p>>,
    by_dag: HashMap<&'p DagId, CallableIdx>,
}

/// Why prepared callables do not form a plan.
#[derive(Debug, Error)]
pub enum ExecPlanError {
    #[error("DAG `{0}` has more than one prepared callable plan")]
    DuplicateCallable(DagId),
}

impl<'p> ExecPlan<'p> {
    /// Assemble a plan from the root callable and every other callable of
    /// `program`.
    ///
    /// # Errors
    ///
    /// Returns [`ExecPlanError`] when two callables share a body.
    pub(crate) fn new(
        program: &'p CheckedProgram,
        declarations: HashMap<&'p ResolvedDeclName, PlannedDeclaration<'p>>,
        root: CallablePlan<'p>,
        others: Vec<CallablePlan<'p>>,
    ) -> Result<Self, ExecPlanError> {
        let callables = IndexVec::from_items(std::iter::once(root).chain(others).collect());
        let mut by_dag = HashMap::with_capacity(callables.len());
        for index in callables.indices() {
            let owner = callables[index].scope.dag().dag_id();
            if by_dag.insert(owner, index).is_some() {
                return Err(ExecPlanError::DuplicateCallable(owner.clone()));
            }
        }
        let has_unfinished_definitions = declarations
            .values()
            .any(|declaration| matches!(declaration.body, PlannedBody::Todo));
        Ok(Self {
            program,
            has_unfinished_definitions,
            declarations,
            root: CallableIdx::new(0),
            callables,
            by_dag,
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
        &self.callables[self.root]
    }

    /// Every callable, the root first.
    #[cfg(any(test, feature = "test-internals"))]
    pub fn callables(&self) -> impl Iterator<Item = &CallablePlan<'p>> {
        self.callables.iter()
    }

    /// The callable of an inline-call target.
    #[must_use]
    pub fn callable(&self, owner: &DagId) -> Option<&CallablePlan<'p>> {
        self.by_dag.get(owner).map(|index| &self.callables[*index])
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
