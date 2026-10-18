//! Shared frame mechanics. Expression interpretation and result reporting are adapters.

use crate::constant_pools::RuntimeValueMap;
use crate::domain_check::check_domain_constraint;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::eval::types::{NodeUnavailable, RuntimeUnavailable};
use crate::execution_plan::{
    CallablePlan, ComputedBody, ExecPlan, PlannedGroup, RecurrenceBody, RecurrenceMemberPlan,
    RunUnit, Step,
};
use crate::invariant::Failure;
use crate::recurrence_slots::{ElementSlot, RecurrenceSlots};
use crate::runtime_presentation::EvaluatedRuntimeValue;
use crate::runtime_presentation::PendingPresentedMap;
use crate::runtime_value::{IndexedValue, KeyValue};
use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::SemanticErrorKind;
use graphcal_compiler::semantic_error::evaluation::EvaluationError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::TExpr;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub enum FailurePolicy {
    Contain,
    Propagate,
}

/// The values of one callable's declarations while its plan runs.
///
/// The fields are private so the frame keeps its invariants by construction:
/// every bound value passed its domain check, a presented value is kept only
/// for a bound value with a presentation (and holds that same value), and a
/// declaration holds a value or an unavailability, never both. Callers supply arguments through the operations
/// below, read the frame while it runs, and take the outcome with
/// [`ExecutionFrame::finish`].
pub struct ExecutionFrame<'a> {
    plan: &'a ExecPlan<'a>,
    callable: &'a CallablePlan<'a>,
    policy: FailurePolicy,
    values: RuntimeValueMap,
    presented: PendingPresentedMap,
    errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
}

/// What a finished frame computed.
pub struct FrameOutcome {
    pub values: RuntimeValueMap,
    /// The presented value of every value with a presentation.
    pub presented: PendingPresentedMap,
    pub errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
}

/// One scheduled declaration handed to a frame's expression adapter.
///
/// Its checked tree comes with the scope of the DAG that owns the
/// declaration, so the adapter evaluates it in that scope.
pub struct ScheduledDeclaration<'a> {
    key: &'a ResolvedDeclName,
    source: SourceId,
    body: ScopedTree<'a, &'a TExpr>,
}

impl<'a> ScheduledDeclaration<'a> {
    /// The declaration's runtime identity.
    #[must_use]
    pub const fn key(&self) -> &'a ResolvedDeclName {
        self.key
    }

    /// The declaration's checked tree, in its owner's scope.
    #[must_use]
    pub const fn body(&self) -> &ScopedTree<'a, &'a TExpr> {
        &self.body
    }

    /// The source the declaration's diagnostics point into.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }
}

/// The previous step of an `unfold` member's element: the previous key, the
/// previous state, and the init the state was seeded by.
#[derive(Debug, Clone, Copy)]
pub struct PreviousStep<'s> {
    pub key: &'s KeyValue,
    pub state: &'s EvaluatedRuntimeValue,
    pub initial: &'s EvaluatedRuntimeValue,
}

/// One element of a recurrence group member handed to a frame's expression
/// adapter: the member's checked body, the key to evaluate it at, for an
/// `unfold` after its first step the previous step, and the group's element
/// slots, which the adapter reads members through.
pub struct ScheduledElement<'a, 's> {
    key: &'a ResolvedDeclName,
    source: SourceId,
    member: RecurrenceMemberPlan<'a>,
    at: &'s KeyValue,
    previous: Option<PreviousStep<'s>>,
    slots: &'s RecurrenceSlots,
}

impl<'a, 's> ScheduledElement<'a, 's> {
    /// The member's runtime identity.
    #[must_use]
    pub const fn key(&self) -> &'a ResolvedDeclName {
        self.key
    }

    /// The source the member's diagnostics point into.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// The member's checked body.
    pub(crate) const fn member(&self) -> RecurrenceMemberPlan<'a> {
        self.member
    }

    /// The key of the step to evaluate.
    pub(crate) const fn at(&self) -> &'s KeyValue {
        self.at
    }

    /// The previous step of an `unfold` member after its first step.
    pub(crate) const fn previous(&self) -> Option<PreviousStep<'s>> {
        self.previous
    }

    /// The group's elements computed so far.
    #[must_use]
    pub const fn slots(&self) -> &'s RecurrenceSlots {
        self.slots
    }
}

/// A recurrence group whose evaluation would read an element before it is
/// evaluated: the checker's plan and the evaluator disagree.
struct Stuck {
    member: ResolvedDeclName,
    step: usize,
}

/// Why a recurrence group stopped before its outcomes were recorded.
enum GroupStop {
    /// Cancellation, or a failure the frame's policy propagates.
    Outcome(Outcome<SemanticError>),
    /// The plan and the evaluator disagree.
    Stuck(Stuck),
}

impl From<Outcome<SemanticError>> for GroupStop {
    fn from(outcome: Outcome<SemanticError>) -> Self {
        Self::Outcome(outcome)
    }
}

impl From<graphcal_compiler::cancellation::Cancelled> for GroupStop {
    fn from(cancelled: graphcal_compiler::cancellation::Cancelled) -> Self {
        Self::Outcome(cancelled.into())
    }
}

impl From<SemanticError> for GroupStop {
    fn from(error: SemanticError) -> Self {
        Self::Outcome(error.into())
    }
}

/// What a frame hands its expression adapter to evaluate.
pub enum Scheduled<'a, 's> {
    /// A declaration, evaluated as a whole.
    Declaration(ScheduledDeclaration<'a>),
    /// One element of a recurrence group member.
    Element(ScheduledElement<'a, 's>),
}

/// What went wrong with one recurrence group member while its group ran.
enum MemberProblem {
    /// One of its elements failed; the failure at its earliest step.
    Failed(SemanticError),
    /// An input of the member is unavailable.
    Unavailable(RuntimeUnavailable),
}

#[must_use]
pub fn eval_failed_node_error(error: &SemanticError) -> RuntimeUnavailable {
    match error {
        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            kind:
                SemanticErrorKind::Evaluation(EvaluationError::Unavailable {
                    reason: NodeUnavailable::Todo { declaration },
                    ..
                }),
            ..
        }) => NodeUnavailable::Blocked {
            unfinished: graphcal_compiler::syntax::non_empty::NonEmpty::singleton(
                declaration.clone(),
            ),
            failed_deps: Vec::new(),
        },
        SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason, .. }),
            ..
        }) => reason.clone(),
        other => NodeUnavailable::EvalFailed {
            message: other.to_string(),
        },
    }
}

impl<'a> ExecutionFrame<'a> {
    /// A frame of `callable`, seeded with its constants and constant
    /// imports.
    #[must_use]
    pub fn new(
        plan: &'a ExecPlan<'a>,
        callable: &'a CallablePlan<'a>,
        policy: FailurePolicy,
    ) -> Self {
        let mut values = RuntimeValueMap::new();
        for import in callable.constant_imports() {
            values.insert(import.destination.clone(), import.value.value().clone());
        }
        values.extend(
            callable
                .execution_dags()
                .iter()
                .flat_map(|closure| closure.scope().const_values().iter())
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        let presented = plan
            .program()
            .facts()
            .const_presentations()
            .filter(|(key, _)| values.contains_key(*key))
            .map(|(key, presented)| (key.clone(), presented.clone()))
            .collect();
        Self {
            plan,
            callable,
            policy,
            values,
            presented,
            errors: HashMap::new(),
        }
    }

    /// Values bound so far.
    #[must_use]
    pub const fn values(&self) -> &RuntimeValueMap {
        &self.values
    }

    /// Presented values of the values bound so far that have a presentation.
    #[must_use]
    pub const fn presentations(&self) -> &PendingPresentedMap {
        &self.presented
    }

    /// Declarations found unavailable so far.
    #[must_use]
    pub const fn errors(&self) -> &HashMap<ResolvedDeclName, RuntimeUnavailable> {
        &self.errors
    }

    /// The frame's outcome, once the caller is done running it.
    #[must_use]
    pub fn finish(self) -> FrameOutcome {
        FrameOutcome {
            values: self.values,
            presented: self.presented,
            errors: self.errors,
        }
    }

    pub(crate) fn unfinished_origins(
        &self,
    ) -> impl Iterator<Item = &graphcal_compiler::resolved_name::ResolvedDeclName> {
        self.errors.values().flat_map(NodeUnavailable::unfinished)
    }

    fn failure(
        &mut self,
        key: &ResolvedDeclName,
        error: SemanticError,
    ) -> Result<(), SemanticError> {
        let only_incomplete = matches!(&error, SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic { kind: SemanticErrorKind::Evaluation(EvaluationError::Unavailable { reason, .. }), .. }) if !reason.has_failure());
        match (&error, self.policy) {
            (SemanticError::Internal(_), _) => Err(error),
            (_, FailurePolicy::Propagate) if !only_incomplete => Err(error),
            _ => {
                self.errors
                    .insert(key.clone(), eval_failed_node_error(&error));
                Ok(())
            }
        }
    }

    /// Bind `key` to `value` after its domain check, recording a violation
    /// under the frame's failure policy.
    fn bind(
        &mut self,
        key: &ResolvedDeclName,
        domain: Option<&ResolvedDomainConstraint>,
        value: EvaluatedRuntimeValue,
        source: SourceId,
        span: Span,
    ) -> Result<(), SemanticError> {
        if let Some(error) = Self::domain_violation(&value, domain, source, span) {
            self.values.remove(key);
            self.presented.remove(key);
            return self.failure(key, error);
        }
        self.store(key, value);
        Ok(())
    }

    /// Record `value` as the value of `key`.
    fn store(&mut self, key: &ResolvedDeclName, value: EvaluatedRuntimeValue) {
        if value.is_plain() {
            self.values.insert(key.clone(), value.into_value());
            self.presented.remove(key);
        } else {
            self.values.insert(key.clone(), value.value().into_owned());
            self.presented.insert(key.clone(), value);
        }
    }

    /// Bind a value the caller supplies for `key` (a runtime parameter
    /// binding or a call argument) before the frame runs.
    ///
    /// The value is domain-checked like every value the frame computes.
    pub fn bind_argument(
        &mut self,
        key: &ResolvedDeclName,
        value: EvaluatedRuntimeValue,
        source: SourceId,
        span: Span,
    ) -> Result<(), SemanticError> {
        let domain = self.plan.domain_constraint(key);
        self.bind(key, domain, value, source, span)
    }

    /// The domain violation of `value`, as the diagnostic `bind` reports.
    fn domain_violation(
        value: &EvaluatedRuntimeValue,
        domain: Option<&ResolvedDomainConstraint>,
        source: SourceId,
        span: Span,
    ) -> Option<SemanticError> {
        let constraint = domain?;
        let failure = check_domain_constraint(&value.value(), constraint).err()?;
        Some(match failure {
            Failure::Error(violation) => SemanticError::located(
                source,
                span,
                EvaluationError::Runtime(
                    graphcal_compiler::semantic_error::evaluation::EvaluatorFailure::new(violation),
                ),
            ),
            Failure::Invariant(invariant) => invariant.into_internal_error(source),
        })
    }

    /// Run every unit of the callable that is not already bound, in order.
    ///
    /// A step whose declaration is unfinished, or which depends on a failed
    /// step, is recorded without calling `evaluate`. A recurrence group is
    /// evaluated element by element (see [`Self::run_group`]).
    pub fn run(
        &mut self,
        cancellation: &CancellationToken,
        mut evaluate: impl for<'s> FnMut(
            Scheduled<'a, 's>,
            &Self,
        )
            -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>>,
    ) -> Result<(), Outcome<SemanticError>> {
        crate::pipeline_metrics::record(crate::pipeline_metrics::Event::FrameExecution);
        let callable = self.callable;
        for unit in callable.units() {
            cancellation.checkpoint()?;
            match *unit {
                RunUnit::Step(step) => self.run_step(callable.step(step), &mut evaluate)?,
                RunUnit::Group(group) => {
                    self.run_group(callable.group(group), cancellation, &mut evaluate)?;
                }
            }
        }
        Ok(())
    }

    /// Evaluate one declaration as a whole.
    fn run_step(
        &mut self,
        step: &'a Step<'a>,
        evaluate: &mut impl for<'s> FnMut(
            Scheduled<'a, 's>,
            &Self,
        )
            -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>>,
    ) -> Result<(), Outcome<SemanticError>> {
        let callable = self.callable;
        let declaration = step.declaration();
        let key = declaration.key();
        if self.values.contains_key(key) || self.errors.contains_key(key) {
            return Ok(());
        }
        let scope = declaration.scope();
        let (root, tree) = match step.body() {
            ComputedBody::Todo => {
                self.errors.insert(
                    key.clone(),
                    NodeUnavailable::Todo {
                        declaration: key.clone(),
                    },
                );
                return Ok(());
            }
            ComputedBody::BlockedInGroup(unfinished) => {
                let todo = unfinished
                    .iter()
                    .map(|member| {
                        (
                            member,
                            NodeUnavailable::Todo {
                                declaration: member.clone(),
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                let outside = step.deps().iter().filter_map(|dep| {
                    let dependency = callable.step(*dep).declaration().key();
                    self.errors
                        .get(dependency)
                        .map(|reason| (dependency, reason))
                });
                if let Some(reason) = NodeUnavailable::blocked_by(
                    todo.iter()
                        .map(|(member, reason)| (*member, reason))
                        .chain(outside),
                ) {
                    self.errors.insert(key.clone(), reason);
                }
                return Ok(());
            }
            ComputedBody::Expression { root, tree } => (root.get(), tree),
        };
        if let Some(reason) = NodeUnavailable::blocked_by(step.deps().iter().filter_map(|dep| {
            let dependency = callable.step(*dep).declaration().key();
            self.errors
                .get(dependency)
                .map(|reason| (dependency, reason))
        })) {
            self.errors.insert(key.clone(), reason);
            return Ok(());
        }
        let body = *tree.as_ref().map_err(|error| {
            SemanticError::internal_error(error.to_string(), scope.source(), root.span.into())
        })?;
        let result = evaluate(
            Scheduled::Declaration(ScheduledDeclaration {
                key,
                source: scope.source(),
                body,
            }),
            self,
        );
        match result {
            Ok(evaluated) => {
                self.bind(
                    key,
                    declaration.domain(),
                    evaluated,
                    scope.source(),
                    root.span,
                )?;
            }
            Err(Outcome::Cancelled) => return Err(Outcome::Cancelled),
            Err(Outcome::Failed(error)) => self.failure(key, error)?,
        }
        Ok(())
    }

    /// Evaluate a recurrence group element by element: for every step along
    /// the group axis, every member that is not supplied in plan order. An
    /// element is evaluated only when every element it reads is computed,
    /// and is domain-checked as it is committed.
    ///
    /// Outcomes are reported per declaration, as everywhere else: a member
    /// whose element failed reports that failure, and a member that depends,
    /// through the group's reads, on a failed or blocked member reports the
    /// dependency as unavailable.
    fn run_group(
        &mut self,
        group: &'a PlannedGroup<'a>,
        cancellation: &CancellationToken,
        evaluate: &mut impl for<'s> FnMut(
            Scheduled<'a, 's>,
            &Self,
        )
            -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>>,
    ) -> Result<(), Outcome<SemanticError>> {
        match self.evaluate_group(group, cancellation, evaluate) {
            Ok(()) => Ok(()),
            Err(GroupStop::Outcome(outcome)) => Err(outcome),
            Err(GroupStop::Stuck(Stuck { member, step })) => {
                let (source, anchor) = group
                    .members()
                    .iter()
                    .find(|planned| {
                        self.callable.step(planned.step()).declaration().key() == &member
                    })
                    .map_or_else(
                        || (self.callable.scope().source(), DiagnosticAnchor::WholeFile),
                        |planned| {
                            (
                                self.callable
                                    .step(planned.step())
                                    .declaration()
                                    .scope()
                                    .source(),
                                planned.root().span().into(),
                            )
                        },
                    );
                Err(SemanticError::internal_error(
                    format!(
                        "recurrence group element `{member}` at step {step} is read before it is evaluated"
                    ),
                    source,
                    anchor,
                )
                .into())
            }
        }
    }

    /// [`Self::run_group`] up to the first disagreement between the plan and
    /// the evaluator.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the group's elements, then its outcomes"
    )]
    fn evaluate_group(
        &mut self,
        group: &'a PlannedGroup<'a>,
        cancellation: &CancellationToken,
        evaluate: &mut impl for<'s> FnMut(
            Scheduled<'a, 's>,
            &Self,
        )
            -> Result<EvaluatedRuntimeValue, Outcome<SemanticError>>,
    ) -> Result<(), GroupStop> {
        let callable = self.callable;
        let plan = group.plan();
        let members = group.members();
        let steps = members
            .iter()
            .map(|member| callable.step(member.step()))
            .collect::<Vec<_>>();
        let keys = steps
            .iter()
            .map(|step| step.declaration().key())
            .collect::<Vec<_>>();
        let axis_keys = KeyValue::all(group.axis());
        let mut slots = RecurrenceSlots::new(keys.iter().copied(), axis_keys.len());
        // Supplied members are filled before the group runs; members with an
        // unavailable input outside the group are blocked.
        let mut supplied = vec![false; members.len()];
        let mut problems: Vec<Option<MemberProblem>> = Vec::with_capacity(members.len());
        for (position, step) in steps.iter().enumerate() {
            let key = keys[position];
            if let Some(value) = self.values.get(key) {
                supplied[position] = true;
                let evaluated = self
                    .presented
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| EvaluatedRuntimeValue::plain(value.clone()));
                if let Ok(entries) = evaluated.into_entries() {
                    for (index, element) in entries.values().iter().enumerate() {
                        slots.set(position, index, ElementSlot::Computed(element.clone()));
                    }
                }
                problems.push(None);
                continue;
            }
            if let Some(reason) = self.errors.get(key) {
                problems.push(Some(MemberProblem::Unavailable(reason.clone())));
                continue;
            }
            problems.push(
                NodeUnavailable::blocked_by(step.deps().iter().filter_map(|dep| {
                    let dependency = callable.step(*dep).declaration().key();
                    self.errors
                        .get(dependency)
                        .map(|reason| (dependency, reason))
                }))
                .map(MemberProblem::Unavailable),
            );
        }
        let mut previous_key: Option<&KeyValue> = None;
        for (step, at) in axis_keys.iter().enumerate() {
            cancellation.checkpoint()?;
            for (position, member) in members.iter().enumerate() {
                if supplied[position]
                    || matches!(problems[position], Some(MemberProblem::Unavailable(_)))
                {
                    if !supplied[position] {
                        slots.set(position, step, ElementSlot::Unavailable);
                    }
                    continue;
                }
                let stuck = || {
                    GroupStop::Stuck(Stuck {
                        member: keys[position].clone(),
                        step,
                    })
                };
                let dependencies = plan.element_dependencies(position, step);
                let mut ready = true;
                for (dependency, at_step) in &dependencies {
                    match slots.slot(*dependency, *at_step) {
                        None | Some(ElementSlot::Pending) => return Err(stuck()),
                        Some(ElementSlot::Computed(_)) => {}
                        Some(ElementSlot::Failed | ElementSlot::Unavailable) => ready = false,
                    }
                }
                if !ready {
                    slots.set(position, step, ElementSlot::Unavailable);
                    continue;
                }
                let previous = match (member.body(), previous_key) {
                    (RecurrenceBody::Unfold { .. }, Some(previous_key)) => {
                        let state = slots.computed(keys[position], step.saturating_sub(1));
                        let initial = slots.computed(keys[position], 0);
                        match (state, initial) {
                            (Ok(state), Ok(initial)) => Some(PreviousStep {
                                key: previous_key,
                                state,
                                initial,
                            }),
                            _ => return Err(stuck()),
                        }
                    }
                    _ => None,
                };
                let source = steps[position].declaration().scope().source();
                let span = member.root().span();
                let result = evaluate(
                    Scheduled::Element(ScheduledElement {
                        key: keys[position],
                        source,
                        member: *member,
                        at,
                        previous,
                        slots: &slots,
                    }),
                    self,
                );
                let failure = match result {
                    Ok(value) => {
                        match Self::domain_violation(
                            &value,
                            steps[position].declaration().domain(),
                            source,
                            span,
                        ) {
                            None => {
                                slots.set(position, step, ElementSlot::Computed(value));
                                continue;
                            }
                            Some(error) => error,
                        }
                    }
                    Err(Outcome::Cancelled) => return Err(Outcome::Cancelled.into()),
                    Err(Outcome::Failed(error)) => error,
                };
                let incomplete = match &failure {
                    SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                        kind:
                            SemanticErrorKind::Evaluation(EvaluationError::Unavailable {
                                reason, ..
                            }),
                        ..
                    }) => Some(reason.clone()),
                    _ => None,
                };
                match (&failure, self.policy, incomplete) {
                    (SemanticError::Internal(_), _, _) | (_, FailurePolicy::Propagate, None) => {
                        return Err(failure.into());
                    }
                    (_, _, Some(reason)) => {
                        slots.set(position, step, ElementSlot::Unavailable);
                        if problems[position].is_none() {
                            problems[position] = Some(MemberProblem::Unavailable(reason));
                        }
                    }
                    (_, FailurePolicy::Contain, None) => {
                        slots.set(position, step, ElementSlot::Failed);
                        if problems[position].is_none() {
                            problems[position] = Some(MemberProblem::Failed(failure));
                        }
                    }
                }
            }
            previous_key = Some(at);
        }
        let troubled = problems.iter().map(Option::is_some).collect::<Vec<_>>();
        for (position, problem) in problems.into_iter().enumerate() {
            let key = keys[position];
            match problem {
                Some(MemberProblem::Failed(error)) => self.failure(key, error)?,
                Some(MemberProblem::Unavailable(reason)) => {
                    self.errors.entry(key.clone()).or_insert(reason);
                }
                None => {}
            }
        }
        // A member that reads, through members that are not supplied, a
        // member with a problem depends on it: record the dependency, nearest
        // first, as the frame records every other unavailable dependency.
        let mut troubled = troubled;
        loop {
            let mut progressed = false;
            for position in 0..members.len() {
                if supplied[position] || troubled[position] {
                    continue;
                }
                let failed_reads = plan.members()[position]
                    .reads()
                    .iter()
                    .map(graphcal_compiler::tir::recurrence_plan::GroupRead::target)
                    .filter(|target| !supplied[*target] && troubled[*target])
                    .collect::<Vec<_>>();
                let Ok(failed_reads) = NonEmpty::try_from_vec(failed_reads) else {
                    continue;
                };
                let reason =
                    NodeUnavailable::blocked_by(failed_reads.iter().filter_map(|target| {
                        let dependency = keys[*target];
                        self.errors
                            .get(dependency)
                            .map(|reason| (dependency, reason))
                    }))
                    .unwrap_or_else(|| NodeUnavailable::DependencyFailed {
                        failed_deps: failed_reads.map(|target| keys[target].clone()),
                    });
                self.errors.insert(keys[position].clone(), reason);
                troubled[position] = true;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        for (position, member) in members.iter().enumerate() {
            let key = keys[position];
            if supplied[position] || troubled[position] {
                continue;
            }
            // Every element of a member without a problem is computed
            // (`ValidOrder.run_memberOk`).
            let mut elements = slots.take_computed(position).into_iter().flatten();
            let entries = IndexedValue::try_from_axis(group.axis().clone(), |_| {
                elements.next().ok_or(GroupStop::Stuck(Stuck {
                    member: key.clone(),
                    step: 0,
                }))
            })?;
            self.bind(
                key,
                steps[position].declaration().domain(),
                EvaluatedRuntimeValue::from_indexed(entries),
                steps[position].declaration().scope().source(),
                member.root().span(),
            )?;
        }
        Ok(())
    }
}
