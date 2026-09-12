//! Shared frame mechanics. Expression interpretation and result reporting are adapters.

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::check_domain_constraint;
use crate::eval::types::NodeUnavailable;
use crate::execution_facts::RuntimeValueMap;
use crate::execution_plan::{CallablePlan, ExecPlan};
use crate::execution_scope::CheckedExecutionScope;
use crate::presentation_evidence::PresentationInstanceMap;
use crate::runtime_presentation::EvaluatedRuntimeValue;
use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::{error::GraphcalError, runtime_value::RuntimeValue};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::model::TIR;
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
    #[error(transparent)]
    Constant(#[from] crate::constant_pools::ConstantPoolError),
}

pub struct ExecutionFrame<'a> {
    plan: &'a ExecPlan,
    callable: &'a CallablePlan,
    policy: FailurePolicy,
    pub values: RuntimeValueMap,
    pub presentations: PresentationInstanceMap,
    pub errors: HashMap<RuntimeDeclKey, NodeUnavailable>,
}

pub struct ScheduledDeclaration<'a> {
    pub key: &'a RuntimeDeclKey,
    pub scope: CheckedExecutionScope<'a>,
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
            values.insert(import.destination.clone(), import.value.value()?.clone());
        }
        values.extend(
            callable
                .const_values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        let presentations = plan
            .checked_execution_facts
            .by_dag
            .values()
            .flat_map(|facts| facts.const_presentations.iter())
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
    ) -> impl Iterator<Item = &graphcal_compiler::syntax::decl_name::ResolvedDeclName> {
        self.errors.values().flat_map(NodeUnavailable::unfinished)
    }

    fn failure(&mut self, key: &RuntimeDeclKey, error: GraphcalError) -> Result<(), GraphcalError> {
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
        key: &RuntimeDeclKey,
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
        tir: &TIR,
        source: &NamedSource<Arc<String>>,
        cancellation: &CancellationToken,
        mut evaluate: impl FnMut(
            ScheduledDeclaration<'_>,
            &Self,
        ) -> Result<EvaluatedRuntimeValue, GraphcalError>,
    ) -> Result<(), GraphcalError> {
        crate::pipeline_metrics::record(crate::pipeline_metrics::Event::FrameExecution);
        for key in &self.callable.topo_order {
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
            let scope = CheckedExecutionScope::new(tir, &self.plan.checked_execution_facts, body)
                .map_err(|error| internal(error.to_string()))?;
            let dependencies = self.callable.dependencies.get(key).ok_or_else(|| {
                internal(format!(
                    "scheduled declaration `{key}` has no prepared dependencies"
                ))
            })?;
            if scope.dag().todo(key.as_resolved()).is_some() {
                self.errors.insert(
                    key.clone(),
                    NodeUnavailable::Todo {
                        declaration: key.as_resolved().clone(),
                    },
                );
                continue;
            }
            if let Some(reason) =
                NodeUnavailable::blocked_by(dependencies.iter().filter_map(|dependency| {
                    self.errors
                        .get(dependency)
                        .map(|reason| (dependency.as_resolved(), reason))
                }))
            {
                self.errors.insert(key.clone(), reason);
                continue;
            }
            let expression = scope
                .dag()
                .runtime_expr(key.as_resolved())
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
                    self.bind(key, value, scope.facts().source(), expression.span)?;
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
