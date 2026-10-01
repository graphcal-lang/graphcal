//! Shared frame mechanics. Expression interpretation and result reporting are adapters.

use crate::constant_pools::RuntimeValueMap;
use crate::domain_check::check_domain_constraint;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::eval::types::NodeUnavailable;
use crate::execution_plan::{
    CallImport, CallablePlan, ExecPlan, ImportSource, PlannedBody, PlannedCall,
};
use crate::runtime_presentation::EvaluatedRuntimeValue;
use crate::runtime_presentation::PendingPresentedMap;
use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::source_id::SourceId;
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
/// declaration holds a value or an unavailability, never both. Callers supply arguments and runtime imports through the operations
/// below, read the frame while it runs, and take the outcome with
/// [`ExecutionFrame::finish`].
pub struct ExecutionFrame<'a> {
    plan: &'a ExecPlan<'a>,
    callable: &'a CallablePlan<'a>,
    /// The runtime imports the frame's caller supplies, with their frames.
    imports: &'a [CallImport],
    policy: FailurePolicy,
    values: RuntimeValueMap,
    presented: PendingPresentedMap,
    errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

/// The values of a running or finished frame, with the presented values of
/// those that have a presentation.
#[derive(Clone, Copy)]
pub struct FrameValues<'v> {
    pub values: &'v RuntimeValueMap,
    pub presentations: Option<&'v PendingPresentedMap>,
}

impl FrameValues<'_> {
    /// The value of `key`, with its presentation when it has one.
    fn presented(self, key: &ResolvedDeclName) -> Option<EvaluatedRuntimeValue> {
        let value = self.values.get(key)?;
        Some(
            self.presentations
                .and_then(|presented| presented.get(key))
                .map_or_else(
                    || crate::runtime_presentation::PresentedRef::plain(value),
                    EvaluatedRuntimeValue::as_ref,
                )
                .to_owned_with(crate::runtime_value::RuntimeValue::clone),
        )
    }
}

/// What a finished frame computed.
pub struct FrameOutcome {
    pub values: RuntimeValueMap,
    /// The presented value of every value with a presentation.
    pub presented: PendingPresentedMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
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

#[must_use]
pub fn eval_failed_node_error(error: &GraphcalError) -> NodeUnavailable {
    match error {
        GraphcalError::EvaluationUnavailable {
            reason: NodeUnavailable::Todo { declaration },
            ..
        } => NodeUnavailable::Blocked {
            unfinished: graphcal_compiler::syntax::non_empty::NonEmpty::singleton(
                declaration.clone(),
            ),
            failed_deps: Vec::new(),
        },
        GraphcalError::EvaluationUnavailable { reason, .. } => reason.clone(),
        GraphcalError::EvalError { message, .. } => NodeUnavailable::EvalFailed {
            message: message.clone(),
        },
        other => NodeUnavailable::EvalFailed {
            message: other.to_string(),
        },
    }
}

impl<'a> ExecutionFrame<'a> {
    /// A frame of `callable` run without a caller, seeded with its constants
    /// and constant imports; no caller supplies it runtime imports.
    #[must_use]
    pub fn new(
        plan: &'a ExecPlan<'a>,
        callable: &'a CallablePlan<'a>,
        policy: FailurePolicy,
    ) -> Self {
        Self::with_imports(plan, callable, &[], policy)
    }

    /// A frame of the callable `call` runs, seeded with its constants and
    /// constant imports, to be seeded with the runtime imports the call
    /// supplies by [`Self::seed_runtime_imports`].
    #[must_use]
    pub fn called(
        plan: &'a ExecPlan<'a>,
        call: PlannedCall<'a, 'a>,
        policy: FailurePolicy,
    ) -> Self {
        Self::with_imports(plan, call.callable(), call.imports(), policy)
    }

    fn with_imports(
        plan: &'a ExecPlan<'a>,
        callable: &'a CallablePlan<'a>,
        imports: &'a [CallImport],
        policy: FailurePolicy,
    ) -> Self {
        let mut values = RuntimeValueMap::new();
        for import in &callable.imports().constants {
            values.insert(import.destination.clone(), import.value.value().clone());
        }
        values.extend(
            callable
                .execution_dags()
                .iter()
                .flat_map(|scope| scope.const_values().iter())
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
            imports,
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
    pub const fn errors(&self) -> &HashMap<ResolvedDeclName, NodeUnavailable> {
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

    pub fn unfinished_origins(
        &self,
    ) -> impl Iterator<Item = &graphcal_compiler::resolved_name::ResolvedDeclName> {
        self.errors.values().flat_map(NodeUnavailable::unfinished)
    }

    fn failure(
        &mut self,
        key: &ResolvedDeclName,
        error: GraphcalError,
    ) -> Result<(), GraphcalError> {
        let only_incomplete = matches!(&error, GraphcalError::EvaluationUnavailable { reason, .. } if !reason.has_failure());
        match (&error, self.policy) {
            (GraphcalError::InternalError { .. }, _) => Err(error),
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
    ) -> Result<(), GraphcalError> {
        if let Some(constraint) = domain
            && let Err(violation) = check_domain_constraint(&value.value(), constraint)
        {
            self.values.remove(key);
            self.presented.remove(key);
            return self.failure(
                key,
                GraphcalError::EvalError {
                    message: violation.message,
                    src: source,
                    span: span.into(),
                },
            );
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
    ) -> Result<(), GraphcalError> {
        let domain = self.plan.domain_constraint(key);
        self.bind(key, domain, value, source, span)
    }

    /// Seed each runtime import the caller supplies that is not bound yet
    /// with its value in the frame the plan classified it to: the `caller`'s
    /// frame or, when there is one, the `root` frame.
    ///
    /// Supplied values and retained checked constants always win.
    pub fn seed_runtime_imports(&mut self, caller: FrameValues<'_>, root: Option<FrameValues<'_>>) {
        for import in self.imports {
            let key = import.key();
            if self.values.contains_key(key) {
                continue;
            }
            let frame = match import.source() {
                ImportSource::Caller => Some(caller),
                ImportSource::Root => root,
            };
            if let Some(imported) = frame.and_then(|frame| frame.presented(key)) {
                self.store(key, imported);
            }
        }
    }

    /// Run every step of the callable that is not already bound, in order.
    ///
    /// A step whose declaration is unfinished, or which depends on a failed
    /// step, is recorded without calling `evaluate`.
    pub fn run(
        &mut self,
        cancellation: &CancellationToken,
        mut evaluate: impl FnMut(
            ScheduledDeclaration<'a>,
            &Self,
        ) -> Result<EvaluatedRuntimeValue, Outcome<GraphcalError>>,
    ) -> Result<(), Outcome<GraphcalError>> {
        crate::pipeline_metrics::record(crate::pipeline_metrics::Event::FrameExecution);
        let callable = self.callable;
        for step in callable.steps() {
            cancellation.checkpoint()?;
            let declaration = step.declaration();
            let key = declaration.key();
            if self.values.contains_key(key) || self.errors.contains_key(key) {
                continue;
            }
            let scope = declaration.scope();
            let (root, tree) = match declaration.body() {
                PlannedBody::Todo => {
                    self.errors.insert(
                        key.clone(),
                        NodeUnavailable::Todo {
                            declaration: key.clone(),
                        },
                    );
                    continue;
                }
                PlannedBody::Expression { root, tree } => (root.get(), tree),
                PlannedBody::Supplied => {
                    return Err(GraphcalError::internal_error(
                        format!("TIR runtime declaration missing for `{key}`"),
                        scope.source(),
                        DiagnosticAnchor::WholeFile,
                    )
                    .into());
                }
            };
            if let Some(reason) =
                NodeUnavailable::blocked_by(step.deps().iter().filter_map(|dep| {
                    let dependency = callable.step(*dep).declaration().key();
                    self.errors
                        .get(dependency)
                        .map(|reason| (dependency, reason))
                }))
            {
                self.errors.insert(key.clone(), reason);
                continue;
            }
            let body = *tree.as_ref().map_err(|error| {
                GraphcalError::internal_error(error.to_string(), scope.source(), root.span.into())
            })?;
            let result = evaluate(
                ScheduledDeclaration {
                    key,
                    source: scope.source(),
                    body,
                },
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
        }
        Ok(())
    }
}
