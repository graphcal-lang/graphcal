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
use graphcal_compiler::registry::{error::GraphcalError, runtime_value::RuntimeValue};
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

pub struct ExecutionFrame<'a> {
    plan: &'a ExecPlan,
    callable: &'a CallablePlan,
    policy: FailurePolicy,
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

    pub fn bind(
        &mut self,
        key: &ResolvedDeclName,
        value: RuntimeValue,
        source: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<(), GraphcalError> {
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
        Ok(())
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
                Ok(evaluated) => {
                    let (value, presentation) = evaluated.into_parts();
                    self.bind(key, value, scope.source(), expression.span)?;
                    if self.values.contains_key(key) && !presentation.is_none() {
                        self.presentations.insert(key.clone(), presentation);
                    }
                }
                Err(error) => self.failure(key, error)?,
            }
        }
        Ok(())
    }
}
