//! Phase-specific construction of immutable expression environments.
//!
//! An [`EvalSession`] carries everything evaluation needs except a scope: it
//! cannot resolve a body handle. An [`EvalContext`] is a session entered into
//! the scope of one tree, and is created only by entering a
//! [`ScopedTree`] — a tree the compiler handed out together with the scope of
//! the DAG that owns it. Code evaluating a body therefore never chooses the
//! frame its handles resolve in.

use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::sync::Arc;

use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::FormattingRegistry;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::TExpr;
use graphcal_compiler::tir::typed::{
    BodyScope, CheckedTir, Scoped, ScopedTree, StructFieldConstraintKey,
};
use miette::NamedSource;

use crate::constant_pools::RuntimeValueMap;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_frame::ScheduledDeclaration;
use crate::execution_plan::ExecPlan;
use crate::host_fns::HostFunctionRegistry;
use crate::invariant::Failure;
use crate::presentation_evidence::PendingPresentationMap;
use crate::static_incompleteness::ExpressionDependencies;

use super::work_budget::WorkBudget;

#[derive(Clone, Copy)]
enum Capabilities<'a> {
    /// Constants are evaluated before field constraints have been resolved.
    ProvisionalConstants,
    /// Field validation is mandatory; callable access is explicitly supplied.
    Checked {
        plan: &'a ExecPlan<'a>,
        host: &'a HostFunctionRegistry,
    },
}

/// Read-only data exposed by an evaluation session.
///
/// There is deliberately no mutable dereference from `EvalSession`: callers
/// cannot replace a registry or fact store independently after selection.
#[derive(Clone)]
pub struct EvalEnvironment<'a> {
    pub cancellation: CancellationToken,
    pub(in crate::eval_expr) work_budget: WorkBudget,
    pub registry: &'a FormattingRegistry,
    pub src: &'a NamedSource<Arc<String>>,
    pub tir: &'a CheckedTir,
    pub current_decl: Option<ResolvedDeclName>,
    pub root_values: Option<&'a RuntimeValueMap>,
    pub unavailable: Option<
        &'a HashMap<
            graphcal_compiler::resolved_name::ResolvedDeclName,
            graphcal_compiler::node_unavailable::NodeUnavailable,
        >,
    >,
    pub unfinished_calls: Option<&'a std::cell::RefCell<BTreeSet<ResolvedDeclName>>>,
    pub root_presentation_instances: Option<&'a PendingPresentationMap>,
}

/// An immutable environment whose capabilities can only be selected by phase.
///
/// A session has no scope: it evaluates a tree only by entering the scope the
/// tree was handed out with.
#[derive(Clone)]
pub struct EvalSession<'a> {
    environment: EvalEnvironment<'a>,
    capabilities: Capabilities<'a>,
}

impl<'a> Deref for EvalSession<'a> {
    type Target = EvalEnvironment<'a>;

    fn deref(&self) -> &Self::Target {
        &self.environment
    }
}

/// A session entered into the scope of the tree it evaluates.
///
/// Created only by [`EvalSession::enter`], from the scope a [`ScopedTree`]
/// was handed out with, so every body handle it resolves is resolved in the
/// frame of the DAG that owns the tree.
#[derive(Clone)]
pub struct EvalContext<'a> {
    session: EvalSession<'a>,
    scope: BodyScope<'a>,
}

impl<'a> Deref for EvalContext<'a> {
    type Target = EvalSession<'a>;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl<'a> EvalSession<'a> {
    fn environment(
        tir: &'a CheckedTir,
        src: &'a NamedSource<Arc<String>>,
        cancellation: CancellationToken,
    ) -> EvalEnvironment<'a> {
        EvalEnvironment {
            cancellation,
            work_budget: WorkBudget::default(),
            registry: tir.registry(),
            src,
            tir,
            current_decl: None,
            root_values: None,
            unavailable: None,
            unfinished_calls: None,
            root_presentation_instances: None,
        }
    }

    /// Select provisional constant evaluation. DAG/host calls are unavailable
    /// and field constraints are deferred to mandatory constant-field
    /// checking.
    #[must_use]
    pub fn provisional_constants(
        tir: &'a CheckedTir,
        src: &'a NamedSource<Arc<String>>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            environment: Self::environment(tir, src, cancellation),
            capabilities: Capabilities::ProvisionalConstants,
        }
    }

    /// Select checked runtime evaluation of `plan`. Field constraints cannot
    /// be omitted or supplied independently of the selected checked project
    /// facts.
    #[must_use]
    pub fn checked(
        plan: &'a ExecPlan<'a>,
        src: &'a NamedSource<Arc<String>>,
        host: &'a HostFunctionRegistry,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            environment: Self::environment(plan.tir(), src, cancellation),
            capabilities: Capabilities::Checked { plan, host },
        }
    }

    /// Enter the scope `root` was handed out with, to evaluate it, preserving
    /// capabilities and enclosing work.
    pub(in crate::eval_expr) fn enter<'b, T>(&'b self, root: &ScopedTree<'b, T>) -> EvalContext<'b>
    where
        'a: 'b,
        T: std::borrow::Borrow<TExpr>,
    {
        EvalContext {
            session: self.clone(),
            scope: root.scope(),
        }
    }

    /// The executable tree of an expression root of an evaluation unit.
    pub fn executable<'t>(
        &self,
        root: Scoped<'t, Expr>,
    ) -> Result<ScopedTree<'t, &'t TExpr>, GraphcalError> {
        root.executable()
            .map_err(|error| self.internal_error(error.to_string(), root.get().span))
    }

    /// The text of an expression root of an evaluation unit checked as a
    /// contextual string.
    pub fn checked_string<'t>(&self, root: Scoped<'t, Expr>) -> Result<&'t str, GraphcalError> {
        root.checked_string()
            .map_err(|error| self.internal_error(error.to_string(), root.get().span))
    }

    pub fn execution_plan(&self) -> Result<&'a ExecPlan<'a>, GraphcalError> {
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
        match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { plan, .. } => {
                Some(plan.program().facts().struct_field_constraints())
            }
        }
    }

    pub const fn host_fns(&self) -> Option<&'a HostFunctionRegistry> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { host, .. } => Some(host),
        }
    }

    #[must_use]
    pub const fn with_unavailable(
        mut self,
        unavailable: &'a HashMap<
            graphcal_compiler::resolved_name::ResolvedDeclName,
            graphcal_compiler::node_unavailable::NodeUnavailable,
        >,
    ) -> Self {
        self.environment.unavailable = Some(unavailable);
        self
    }

    #[must_use]
    pub const fn with_unfinished_calls(
        mut self,
        calls: &'a std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    ) -> Self {
        self.environment.unfinished_calls = Some(calls);
        self
    }

    /// Static dependency availability of expression roots of evaluation
    /// units, including references in unselected branches. Each root's
    /// references are resolved in its own scope.
    pub fn unavailable_dependencies<'e>(
        &self,
        roots: impl IntoIterator<Item = Scoped<'e, Expr>>,
    ) -> Result<Option<graphcal_compiler::node_unavailable::NodeUnavailable>, GraphcalError> {
        let roots = roots.into_iter().collect::<Vec<_>>();
        self.unavailable_among(
            || roots.iter().flat_map(|root| root.graph_refs()).collect(),
            roots.iter().map(|root| root.get()),
        )
    }

    /// Availability of `graph_refs` and of the call outputs of `expressions`.
    fn unavailable_among<'e, E: ExpressionDependencies + 'e>(
        &self,
        graph_refs: impl FnOnce() -> Vec<ResolvedDeclName>,
        expressions: impl IntoIterator<Item = &'e E>,
    ) -> Result<Option<graphcal_compiler::node_unavailable::NodeUnavailable>, GraphcalError> {
        let plan = match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { plan, .. } => Some(plan),
        };
        if self.unavailable.is_none_or(HashMap::is_empty)
            && plan.is_none_or(|plan| !plan.has_unfinished_definitions())
        {
            return Ok(None);
        }
        let mut dependencies = graph_refs()
            .into_iter()
            .filter_map(|key| {
                self.unavailable
                    .and_then(|unavailable| unavailable.get(&key))
                    .map(|reason| (key, reason.clone()))
            })
            .collect::<Vec<_>>();
        if let Some(plan) = plan {
            for expression in expressions {
                dependencies.extend(crate::static_incompleteness::collect(
                    expression,
                    plan,
                    self.src,
                    &self.cancellation,
                )?);
            }
        }
        Ok(
            graphcal_compiler::node_unavailable::NodeUnavailable::blocked_by(
                dependencies.iter().map(|(name, reason)| (name, reason)),
            ),
        )
    }

    #[must_use]
    pub const fn with_roots(
        mut self,
        values: &'a RuntimeValueMap,
        instances: Option<&'a PendingPresentationMap>,
    ) -> Self {
        self.environment.root_values = Some(values);
        self.environment.root_presentation_instances = instances;
        self
    }

    #[must_use]
    pub fn with_src<'b>(&'b self, src: &'b NamedSource<Arc<String>>) -> EvalSession<'b>
    where
        'a: 'b,
    {
        let mut session = self.clone();
        session.environment.src = src;
        session
    }

    /// Select the source and declaration of one scheduled step, preserving
    /// capabilities and enclosing work. The step's tree carries its own
    /// scope.
    #[must_use]
    pub fn for_declaration<'b>(&'b self, step: &ScheduledDeclaration<'b>) -> EvalSession<'b>
    where
        'a: 'b,
    {
        let mut session = self.with_src(step.source());
        session.environment.current_decl = Some(step.key().clone());
        session
    }

    /// Select the declaration whose unit is evaluated next, for diagnostics.
    #[must_use]
    pub fn for_decl(&self, declaration: &ResolvedDeclName) -> Self {
        let mut session = self.clone();
        session.environment.current_decl = Some(declaration.clone());
        session
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

    /// The diagnostic for a failed runtime operation: a user-facing failure
    /// is an evaluation error, a violated invariant an internal error.
    pub fn failure_error(
        &self,
        failure: Failure<impl std::fmt::Display>,
        span: Span,
    ) -> GraphcalError {
        match failure {
            Failure::Error(error) => self.eval_error(error.to_string(), span),
            Failure::Invariant(invariant) => self.internal_error(invariant.to_string(), span),
        }
    }

    /// Like [`Self::failure_error`], keeping cancellation as control flow.
    pub fn outcome_error(
        &self,
        outcome: Outcome<Failure<impl std::fmt::Display>>,
        span: Span,
    ) -> GraphcalError {
        match outcome {
            Outcome::Cancelled => GraphcalError::from(Cancelled),
            Outcome::Failed(failure) => self.failure_error(failure, span),
        }
    }
}

impl<'a> EvalContext<'a> {
    /// Identity of the DAG whose tree this context evaluates.
    #[must_use]
    pub fn dag_id(&self) -> &'a graphcal_compiler::dag_id::DagId {
        self.scope.dag_id()
    }

    /// The declaration `reference` denotes in the scope of the tree this
    /// context evaluates.
    #[must_use]
    pub fn resolve(&self, reference: &graphcal_compiler::hir::expr::LocalDecl) -> ResolvedDeclName {
        self.scope.resolve(reference)
    }

    /// The unit whose scale `unit` has in the scope of the tree this context
    /// evaluates.
    #[must_use]
    pub fn resolve_unit(
        &self,
        unit: &graphcal_compiler::hir::expr::LocalUnit,
    ) -> graphcal_compiler::resolved_name::ResolvedUnitName {
        self.scope.resolve_unit(unit)
    }

    /// The nominal type `source` stands for in the scope of the tree this
    /// context evaluates, after its Static type substitution.
    #[must_use]
    pub fn runtime_struct_type(
        &self,
        source: &graphcal_compiler::resolved_name::ResolvedStructTypeName,
    ) -> graphcal_compiler::resolved_name::ResolvedStructTypeName {
        self.scope.runtime_struct_type(source)
    }

    /// Determine, once for a whole root tree, whether every dependency it may
    /// read (including those of unselected branches) is available.
    pub fn check_dependencies(&self, expression: &TExpr) -> Result<(), GraphcalError> {
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::DependencyAvailabilityCheck,
        );
        self.unavailable_among(
            || {
                crate::static_incompleteness::graph_refs(expression)
                    .iter()
                    .map(|reference| self.resolve(reference))
                    .collect()
            },
            std::iter::once(expression),
        )?
        .map_or(Ok(()), |reason| {
            Err(GraphcalError::EvaluationUnavailable {
                reason,
                src: self.src.clone(),
                span: expression.span().into(),
            })
        })
    }
}
