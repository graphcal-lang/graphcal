//! Shared frame mechanics. Expression interpretation and result reporting are adapters.

use crate::checked_program::SealedDag;
use crate::constant_pools::RuntimeValueMap;
use crate::domain_check::check_domain_constraint;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::eval::types::NodeUnavailable;
use crate::execution_plan::{CallablePlan, DeclarationBody, ExecPlan};
use crate::presentation_evidence::PresentationInstanceMap;
use crate::runtime_presentation::EvaluatedRuntimeValue;
use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::TExpr;
use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum FailurePolicy {
    Contain,
    Propagate,
}

/// The values of one callable's declarations while its plan runs.
///
/// The fields are private so the frame keeps its invariants by construction:
/// every bound value passed its domain check, a presentation is kept only for
/// a bound value, and a declaration holds a value or an unavailability, never
/// both. Callers supply arguments and runtime imports through the operations
/// below, read the frame while it runs, and take the outcome with
/// [`ExecutionFrame::finish`].
pub struct ExecutionFrame<'a> {
    plan: &'a ExecPlan<'a>,
    callable: &'a CallablePlan<'a>,
    policy: FailurePolicy,
    values: RuntimeValueMap,
    presentations: PresentationInstanceMap,
    errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

/// What a finished frame computed.
pub struct FrameOutcome {
    pub values: RuntimeValueMap,
    pub presentations: PresentationInstanceMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

/// One scheduled declaration handed to a frame's expression adapter.
///
/// Its scope is the step's own and is not exposed: the adapter evaluates the
/// body in the context
/// [`EvalContext::for_declaration`](crate::eval_expr::EvalContext::for_declaration)
/// selects from the step.
pub struct ScheduledDeclaration<'a> {
    key: &'a ResolvedDeclName,
    scope: SealedDag<'a>,
    body: &'a TExpr,
}

impl<'a> ScheduledDeclaration<'a> {
    /// The declaration's runtime identity.
    #[must_use]
    pub const fn key(&self) -> &'a ResolvedDeclName {
        self.key
    }

    /// The declaration's checked tree.
    #[must_use]
    pub const fn body(&self) -> &'a TExpr {
        self.body
    }

    /// The sealed DAG declaring it, for selecting its evaluation context.
    pub(crate) const fn scope(&self) -> SealedDag<'a> {
        self.scope
    }
}

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
    /// A frame of `callable`, seeded with its constants and constant imports.
    pub fn new(
        plan: &'a ExecPlan<'a>,
        callable: &'a CallablePlan<'a>,
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
        let presentations = plan
            .program()
            .facts()
            .const_presentations()
            .filter(|(key, _)| values.contains_key(*key))
            .map(|(key, evidence)| (key.clone(), evidence.clone()))
            .collect();
        Self {
            plan,
            callable,
            policy,
            values,
            presentations,
            errors: HashMap::new(),
        }
    }

    /// Values bound so far.
    #[must_use]
    pub const fn values(&self) -> &RuntimeValueMap {
        &self.values
    }

    /// Presentations of the values bound so far.
    #[must_use]
    pub const fn presentations(&self) -> &PresentationInstanceMap {
        &self.presentations
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
            presentations: self.presentations,
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
            (GraphcalError::InternalError { .. } | GraphcalError::Cancelled(_), _) => Err(error),
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
        source: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
        let (value, presentation) = value.into_parts();
        if let Some(constraint) = domain
            && let Err(violation) = check_domain_constraint(&value, constraint)
        {
            self.values.remove(key);
            return self.failure(
                key,
                GraphcalError::EvalError {
                    message: violation.message,
                    src: source.clone(),
                    span: span.into(),
                },
            );
        }
        self.values.insert(key.clone(), value);
        if !presentation.is_none() {
            self.presentations.insert(key.clone(), presentation);
        }
        Ok(())
    }

    /// Bind a value the caller supplies for `key` (a runtime parameter
    /// binding or a call argument) before the frame runs.
    ///
    /// The value is domain-checked like every value the frame computes.
    pub fn bind_argument(
        &mut self,
        key: &ResolvedDeclName,
        value: EvaluatedRuntimeValue,
        source: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
        let domain = self.plan.domain_constraint(key);
        self.bind(key, domain, value, source, span)
    }

    /// Seed each prepared runtime import of this callable that is not bound
    /// yet with the value `lookup` finds for it in the caller's frames.
    ///
    /// Supplied values and retained checked constants always win.
    pub fn seed_runtime_imports(
        &mut self,
        mut lookup: impl FnMut(&ResolvedDeclName) -> Option<EvaluatedRuntimeValue>,
    ) {
        for key in &self.callable.imports().runtime {
            if self.values.contains_key(key) {
                continue;
            }
            if let Some(imported) = lookup(key) {
                let (value, presentation) = imported.into_parts();
                self.values.insert(key.clone(), value);
                if !presentation.is_none() {
                    self.presentations.insert(key.clone(), presentation);
                }
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
        ) -> Result<EvaluatedRuntimeValue, GraphcalError>,
    ) -> Result<(), GraphcalError> {
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
                DeclarationBody::Todo => {
                    self.errors.insert(
                        key.clone(),
                        NodeUnavailable::Todo {
                            declaration: key.clone(),
                        },
                    );
                    continue;
                }
                DeclarationBody::Expression { root, tree } => (*root, tree),
                DeclarationBody::Supplied => {
                    return Err(GraphcalError::internal_error(
                        format!("TIR runtime declaration missing for `{key}`"),
                        scope.source(),
                        DiagnosticAnchor::WholeFile,
                    ));
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
            let body = tree.as_ref().map_err(|error| {
                GraphcalError::internal_error(error.to_string(), scope.source(), root.span.into())
            })?;
            let result = evaluate(ScheduledDeclaration { key, scope, body }, self);
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
                Err(error) => self.failure(key, error)?,
            }
        }
        Ok(())
    }
}
