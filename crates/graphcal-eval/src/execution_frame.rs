//! Shared frame mechanics. Expression interpretation and result reporting are adapters.

use crate::checked_program::SealedDag;
use crate::constant_pools::RuntimeValueMap;
use crate::domain_check::check_domain_constraint;
use crate::eval::types::NodeUnavailable;
use crate::execution_plan::{CallablePlan, ExecPlan};
use crate::presentation_evidence::PresentationInstanceMap;
use crate::runtime_presentation::EvaluatedRuntimeValue;
use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum FailurePolicy {
    Contain,
    Propagate,
}

#[derive(Debug, thiserror::Error)]
pub enum FramePreparationError {
    #[error(transparent)]
    Callable(#[from] crate::execution_plan::CallablePlanError),
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
    plan: &'a ExecPlan,
    callable: &'a CallablePlan,
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

pub struct ScheduledDeclaration<'a> {
    pub key: &'a ResolvedDeclName,
    pub scope: SealedDag<'a>,
    pub expression: &'a graphcal_compiler::hir::expr::Expr,
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
    pub fn new(
        plan: &'a ExecPlan,
        owner: &graphcal_compiler::dag_id::DagId,
        policy: FailurePolicy,
    ) -> Result<Self, FramePreparationError> {
        let callable = plan.callable(owner)?;
        let mut values = RuntimeValueMap::new();
        for import in &callable.imports.constants {
            values.insert(import.destination.clone(), import.value.value().clone());
        }
        values.extend(
            callable
                .const_values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        let presentations = plan
            .program()
            .facts()
            .const_presentations()
            .filter(|(key, _)| values.contains_key(*key))
            .map(|(key, evidence)| (key.clone(), evidence.clone()))
            .collect();
        Ok(Self {
            plan,
            callable,
            policy,
            values,
            presentations,
            errors: HashMap::new(),
        })
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
        value: EvaluatedRuntimeValue,
        source: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
        let (value, presentation) = value.into_parts();
        if let Some(constraint) = self.callable.domain_constraints.get(key)
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
        self.bind(key, value, source, span)
    }

    /// Seed each prepared runtime import of this callable that is not bound
    /// yet with the value `lookup` finds for it in the caller's frames.
    ///
    /// Supplied values and retained checked constants always win.
    pub fn seed_runtime_imports(
        &mut self,
        mut lookup: impl FnMut(&ResolvedDeclName) -> Option<EvaluatedRuntimeValue>,
    ) {
        for key in &self.callable.imports.runtime {
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

    pub fn run(
        &mut self,
        source: &NamedSource<Arc<String>>,
        cancellation: &CancellationToken,
        mut evaluate: impl FnMut(
            ScheduledDeclaration<'_>,
            &Self,
        ) -> Result<EvaluatedRuntimeValue, GraphcalError>,
    ) -> Result<(), GraphcalError> {
        crate::pipeline_metrics::record(crate::pipeline_metrics::Event::FrameExecution);
        for (key, dependencies) in self.callable.schedule.steps() {
            cancellation.checkpoint()?;
            if self.values.contains_key(key) || self.errors.contains_key(key) {
                continue;
            }
            let internal = |message: String| {
                GraphcalError::internal_error(message, source, DiagnosticAnchor::WholeFile)
            };
            let body = self
                .plan
                .declaration_locations
                .body_for(key)
                .map_err(|error| internal(error.to_string()))?;
            let scope = self
                .plan
                .program()
                .dag(body)
                .ok_or_else(|| internal(format!("DAG `{body}` has no compiled body")))?;
            if scope.dag().todo(key).is_some() {
                self.errors.insert(
                    key.clone(),
                    NodeUnavailable::Todo {
                        declaration: key.clone(),
                    },
                );
                continue;
            }
            if let Some(reason) =
                NodeUnavailable::blocked_by(dependencies.iter().filter_map(|dependency| {
                    self.errors
                        .get(dependency)
                        .map(|reason| (dependency, reason))
                }))
            {
                self.errors.insert(key.clone(), reason);
                continue;
            }
            let expression = scope
                .dag()
                .runtime_expr(key)
                .ok_or_else(|| internal(format!("TIR runtime declaration missing for `{key}`")))?;
            let result = evaluate(
                ScheduledDeclaration {
                    key,
                    scope,
                    expression,
                },
                self,
            );
            match result {
                Ok(evaluated) => self.bind(key, evaluated, scope.source(), expression.span)?,
                Err(error) => self.failure(key, error)?,
            }
        }
        Ok(())
    }
}
