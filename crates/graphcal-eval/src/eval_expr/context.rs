//! Phase-specific construction of immutable expression environments.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::builtins::BuiltinFunctions;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::FormattingRegistry;
use graphcal_compiler::syntax::decl_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{DagTIR, StructFieldConstraintKey, TIR};
use miette::NamedSource;

use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_facts::{CheckedExecutionFacts, RuntimeValueMap};
use crate::execution_plan::ExecPlan;
use crate::execution_scope::CheckedExecutionScope;
use crate::host_fns::HostFunctionRegistry;
use crate::presentation_calls::EvaluatedPresentationCalls;
use crate::runtime_presentation::PresentationInstanceMap;

use super::work_budget::WorkBudget;

#[derive(Clone, Copy)]
enum Capabilities<'a> {
    /// Constants are evaluated before field constraints have been resolved.
    ProvisionalConstants,
    /// Field validation is mandatory; callable access is explicitly supplied.
    Checked {
        plan: &'a ExecPlan,
        host: &'a HostFunctionRegistry,
    },
}

/// Read-only data exposed by an evaluation context.
///
/// There is deliberately no mutable dereference from `EvalContext`: callers
/// cannot replace a body, registry, or fact store independently after selection.
#[derive(Clone)]
pub struct EvalEnvironment<'a> {
    pub cancellation: CancellationToken,
    pub(in crate::eval_expr) work_budget: WorkBudget,
    pub builtin_fns: &'a BuiltinFunctions,
    pub registry: &'a FormattingRegistry,
    pub src: &'a NamedSource<Arc<String>>,
    pub tir: &'a TIR,
    pub current_dag: &'a DagTIR,
    pub current_decl: Option<ResolvedDeclName>,
    pub root_values: Option<&'a RuntimeValueMap>,
    pub root_presentation_instances: Option<&'a PresentationInstanceMap>,
    pub presentation_calls: Option<&'a EvaluatedPresentationCalls>,
}

/// An immutable environment whose capabilities can only be selected by phase.
#[derive(Clone)]
pub struct EvalContext<'a> {
    environment: EvalEnvironment<'a>,
    capabilities: Capabilities<'a>,
    independent_expressions:
        Option<&'a graphcal_compiler::tir::expression_facts::CheckedExpressionFacts>,
}

impl<'a> Deref for EvalContext<'a> {
    type Target = EvalEnvironment<'a>;

    fn deref(&self) -> &Self::Target {
        &self.environment
    }
}

impl<'a> EvalContext<'a> {
    fn environment(
        tir: &'a TIR,
        dag: &'a DagTIR,
        src: &'a NamedSource<Arc<String>>,
        builtin_fns: &'a BuiltinFunctions,
        cancellation: CancellationToken,
    ) -> EvalEnvironment<'a> {
        EvalEnvironment {
            cancellation,
            work_budget: WorkBudget::default(),
            builtin_fns,
            registry: tir.registry(),
            src,
            tir,
            current_dag: dag,
            current_decl: None,
            root_values: None,
            root_presentation_instances: None,
            presentation_calls: None,
        }
    }

    /// Select a provisional constant scope. DAG/host calls are unavailable and
    /// field constraints are deferred to mandatory constant-field checking.
    pub fn provisional_constants(
        tir: &'a TIR,
        owner: &DagId,
        src: &'a NamedSource<Arc<String>>,
        builtin_fns: &'a BuiltinFunctions,
        cancellation: CancellationToken,
    ) -> Result<Self, GraphcalError> {
        let dag = tir.dag_registry().get(owner).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("constant scope `{owner}` has no compiled body"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        Ok(Self {
            environment: Self::environment(tir, dag, src, builtin_fns, cancellation),
            capabilities: Capabilities::ProvisionalConstants,
            independent_expressions: None,
        })
    }

    /// Select a checked runtime scope. Field constraints cannot be omitted or
    /// supplied independently of the selected checked project facts.
    pub fn checked(
        tir: &'a TIR,
        plan: &'a ExecPlan,
        owner: &DagId,
        src: &'a NamedSource<Arc<String>>,
        builtin_fns: &'a BuiltinFunctions,
        host: &'a HostFunctionRegistry,
        cancellation: CancellationToken,
    ) -> Result<Self, GraphcalError> {
        plan.callable(owner).map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
        let scope = CheckedExecutionScope::new(tir, &plan.checked_execution_facts, owner).map_err(
            |error| {
                GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
            },
        )?;
        Ok(Self {
            environment: Self::environment(tir, scope.dag(), src, builtin_fns, cancellation),
            capabilities: Capabilities::Checked { plan, host },
            independent_expressions: None,
        })
    }

    pub fn with_expression_facts(
        mut self,
        facts: &'a graphcal_compiler::tir::expression_facts::CheckedExpressionFacts,
    ) -> Result<Self, GraphcalError> {
        facts
            .validate_environment(self.current_dag.dag_id(), self.current_dag.body_revision())
            .map_err(|error| self.internal_error(error.to_string(), DiagnosticAnchor::WholeFile))?;
        self.independent_expressions = Some(facts);
        Ok(self)
    }

    pub fn expression_fact(
        &self,
        expr: &graphcal_compiler::hir::expr::Expr,
    ) -> Result<&graphcal_compiler::tir::expression_facts::CheckedExpressionRecord, GraphcalError>
    {
        let facts = match self.independent_expressions {
            Some(facts) => facts,
            None => self
                .current_dag
                .expression_facts()
                .map_err(|error| self.internal_error(error.to_string(), expr.span))?,
        };
        facts
            .executable_value(
                expr.id()
                    .map_err(|error| self.internal_error(error.to_string(), expr.span))?,
            )
            .map_err(|error| self.internal_error(error.to_string(), expr.span))
    }

    pub const fn checked_execution_facts(&self) -> Option<&'a CheckedExecutionFacts> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { plan, .. } => Some(&plan.checked_execution_facts),
        }
    }

    pub fn execution_plan(&self) -> Result<&'a ExecPlan, GraphcalError> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => Err(self.internal_error(
                "provisional constant evaluation has no callable execution plans",
                DiagnosticAnchor::WholeFile,
            )),
            Capabilities::Checked { plan, .. } => Ok(plan),
        }
    }

    pub fn struct_field_constraints(
        &self,
    ) -> Option<&'a HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>> {
        self.checked_execution_facts()
            .map(|facts| facts.struct_field_constraints.as_ref())
    }

    pub const fn host_fns(&self) -> Option<&'a HostFunctionRegistry> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { host, .. } => Some(host),
        }
    }

    #[must_use]
    pub const fn with_roots(
        mut self,
        values: &'a RuntimeValueMap,
        instances: Option<&'a PresentationInstanceMap>,
    ) -> Self {
        self.environment.root_values = Some(values);
        self.environment.root_presentation_instances = instances;
        self
    }

    #[must_use]
    pub const fn with_presentation_calls(mut self, calls: &'a EvaluatedPresentationCalls) -> Self {
        self.environment.presentation_calls = Some(calls);
        self
    }

    #[must_use]
    pub fn with_src<'b>(&'b self, src: &'b NamedSource<Arc<String>>) -> EvalContext<'b>
    where
        'a: 'b,
    {
        let mut context = self.clone();
        context.environment.src = src;
        context
    }

    /// Re-select a canonical body, preserving capabilities and enclosing work.
    pub fn for_dag<'b>(
        &'b self,
        dag: &DagTIR,
        src: &'b NamedSource<Arc<String>>,
    ) -> Result<EvalContext<'b>, GraphcalError>
    where
        'a: 'b,
    {
        let mut context = self.with_src(src);
        context.environment.current_dag = match self.capabilities {
            Capabilities::ProvisionalConstants => {
                self.tir.dag_registry().get(dag.dag_id()).ok_or_else(|| {
                    context.internal_error(
                        format!("constant scope `{}` has no compiled body", dag.dag_id()),
                        DiagnosticAnchor::WholeFile,
                    )
                })?
            }
            Capabilities::Checked { plan, .. } => {
                plan.callable(dag.dag_id()).map_err(|error| {
                    context.internal_error(error.to_string(), DiagnosticAnchor::WholeFile)
                })?;
                CheckedExecutionScope::new(self.tir, &plan.checked_execution_facts, dag.dag_id())
                    .map_err(|error| {
                        context.internal_error(error.to_string(), DiagnosticAnchor::WholeFile)
                    })?
                    .dag()
            }
        };
        context.environment.current_decl = None;
        // Unit/call bodies select their own canonical table, never a fallback
        // from a closed input body's unrelated expression revision.
        context.independent_expressions = None;
        Ok(context)
    }

    pub fn for_checked_decl<'b>(
        &'b self,
        dag: &DagTIR,
        src: &'b NamedSource<Arc<String>>,
        declaration: &ResolvedDeclName,
    ) -> Result<EvalContext<'b>, GraphcalError>
    where
        'a: 'b,
    {
        Ok(self.for_dag(dag, src)?.for_decl(declaration))
    }

    #[must_use]
    pub fn for_decl(&self, declaration: &ResolvedDeclName) -> Self {
        let mut context = self.clone();
        context.environment.current_decl = Some(declaration.clone());
        context
    }

    pub fn eval_error(&self, message: impl Into<String>, span: Span) -> GraphcalError {
        GraphcalError::EvalError {
            message: message.into(),
            src: self.src.clone(),
            span: span.into(),
        }
    }

    #[cold]
    pub fn internal_error(
        &self,
        message: impl Into<String>,
        anchor: impl Into<DiagnosticAnchor>,
    ) -> GraphcalError {
        GraphcalError::internal_error(message, self.src, anchor.into())
    }
}
