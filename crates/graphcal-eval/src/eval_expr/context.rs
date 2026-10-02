//! Phase-specific construction of immutable expression environments.
//!
//! An [`EvalSession`] carries everything evaluation needs except a scope: it
//! cannot resolve a body handle. The kernel reads trees only as
//! [`ScopedNode`]s, whose children and resolved references come out in the
//! scope the compiler handed the tree out with, so code evaluating a body
//! never chooses the frame its handles resolve in.

use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::display::formatting_registry::FormattingRegistry;
use graphcal_compiler::hir::expr::Expr;
use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::evaluation::{EvaluationError, EvaluatorFailure};
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::source_registry::SourceRegistry;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::texpr::TExpr;
use graphcal_compiler::tir::typed::body_scope::Scoped;
use graphcal_compiler::tir::typed::checked::CheckedTir;
use graphcal_compiler::tir::typed::evaluation_unit::ScopedTree;
use graphcal_compiler::tir::typed::model::StructFieldConstraintKey;
use graphcal_compiler::tir::typed::scoped_node::ScopedNode;

use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_frame::ScheduledDeclaration;
use crate::execution_plan::ExecPlan;
use crate::host_fns::HostFunctionRegistry;
use crate::invariant::Failure;
use crate::static_incompleteness::ExpressionDependencies;

use super::runtime_failure::RuntimeFailure;
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
    pub(super) work_budget: WorkBudget,
    pub registry: &'a FormattingRegistry,
    pub src: SourceId,
    /// Names of the sources `src` and every diagnostic id resolve in, for
    /// runtime presentation messages.
    pub sources: &'a SourceRegistry,
    pub tir: &'a CheckedTir,
    pub current_decl: Option<ResolvedDeclName>,
    pub unavailable: Option<
        &'a HashMap<
            graphcal_compiler::resolved_name::ResolvedDeclName,
            graphcal_compiler::node_unavailable::NodeUnavailable,
        >,
    >,
    pub unfinished_calls: Option<&'a std::cell::RefCell<BTreeSet<ResolvedDeclName>>>,
}

impl EvalEnvironment<'_> {
    /// The display name of a registered source, for runtime presentation
    /// messages.
    #[must_use]
    pub(crate) fn source_name(&self, source: SourceId) -> &str {
        self.sources
            .named_source(source)
            .map_or("<unknown source>", miette::NamedSource::name)
    }
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

impl<'a> EvalSession<'a> {
    fn environment(
        tir: &'a CheckedTir,
        src: SourceId,
        sources: &'a SourceRegistry,
        cancellation: CancellationToken,
    ) -> EvalEnvironment<'a> {
        EvalEnvironment {
            cancellation,
            work_budget: WorkBudget::default(),
            registry: tir.registry(),
            src,
            sources,
            tir,
            current_decl: None,
            unavailable: None,
            unfinished_calls: None,
        }
    }

    /// Select provisional constant evaluation. DAG/host calls are unavailable
    /// and field constraints are deferred to mandatory constant-field
    /// checking.
    #[must_use]
    pub fn provisional_constants(
        tir: &'a CheckedTir,
        src: SourceId,
        sources: &'a SourceRegistry,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            environment: Self::environment(tir, src, sources, cancellation),
            capabilities: Capabilities::ProvisionalConstants,
        }
    }

    /// Select checked runtime evaluation of `plan`. Field constraints cannot
    /// be omitted or supplied independently of the selected checked project
    /// facts.
    #[must_use]
    pub fn checked(
        plan: &'a ExecPlan<'a>,
        src: SourceId,
        sources: &'a SourceRegistry,
        host: &'a HostFunctionRegistry,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            environment: Self::environment(plan.tir(), src, sources, cancellation),
            capabilities: Capabilities::Checked { plan, host },
        }
    }

    /// The executable tree of an expression root of an evaluation unit.
    pub fn executable<'t>(
        &self,
        root: Scoped<'t, Expr>,
    ) -> Result<ScopedTree<'t, &'t TExpr>, SemanticError> {
        root.executable()
            .map_err(|error| self.internal_error(error.to_string(), root.get().span))
    }

    /// The text of an expression root of an evaluation unit checked as a
    /// contextual string.
    pub(crate) fn checked_string<'t>(
        &self,
        root: Scoped<'t, Expr>,
    ) -> Result<&'t str, SemanticError> {
        root.checked_string()
            .map_err(|error| self.internal_error(error.to_string(), root.get().span))
    }

    pub fn execution_plan(&self) -> Result<&'a ExecPlan<'a>, SemanticError> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => Err(self.internal_error(
                "provisional constant evaluation has no callable execution plans",
                DiagnosticAnchor::WholeFile,
            )),
            Capabilities::Checked { plan, .. } => Ok(plan),
        }
    }

    #[must_use]
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

    #[must_use]
    pub const fn host_fns(&self) -> Option<&'a HostFunctionRegistry> {
        match self.capabilities {
            Capabilities::ProvisionalConstants => None,
            Capabilities::Checked { host, .. } => Some(host),
        }
    }

    #[must_use]
    pub(crate) const fn with_unavailable(
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
    pub(crate) const fn with_unfinished_calls(
        mut self,
        calls: &'a std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    ) -> Self {
        self.environment.unfinished_calls = Some(calls);
        self
    }

    /// Static dependency availability of expression roots of evaluation
    /// units, including references in unselected branches. Each root's
    /// references are resolved in its own scope.
    pub(crate) fn unavailable_dependencies<'e>(
        &self,
        roots: impl IntoIterator<Item = Scoped<'e, Expr>>,
    ) -> Result<Option<graphcal_compiler::node_unavailable::NodeUnavailable>, Outcome<SemanticError>>
    {
        let roots = roots.into_iter().collect::<Vec<_>>();
        self.unavailable_among(
            || roots.iter().flat_map(|root| root.graph_refs()).collect(),
            roots.iter().map(|root| root.get()),
        )
    }

    /// Availability of `graph_refs` and of the call outputs of `expressions`.
    fn unavailable_among<E: ExpressionDependencies>(
        &self,
        graph_refs: impl FnOnce() -> Vec<ResolvedDeclName>,
        expressions: impl IntoIterator<Item = E>,
    ) -> Result<Option<graphcal_compiler::node_unavailable::NodeUnavailable>, Outcome<SemanticError>>
    {
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
                    &expression,
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
    pub(crate) fn with_src<'b>(&'b self, src: SourceId) -> EvalSession<'b>
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
    pub(crate) fn for_declaration<'b>(&'b self, step: &ScheduledDeclaration<'b>) -> EvalSession<'b>
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

    /// The diagnostic for a runtime failure of the expression at `span`.
    pub(super) fn runtime_error(
        &self,
        failure: impl Into<RuntimeFailure>,
        span: Span,
    ) -> SemanticError {
        SemanticError::located(
            self.src,
            span,
            EvaluationError::Runtime(EvaluatorFailure::new(failure.into())),
        )
    }

    /// The diagnostic for a failed runtime operation: a user-facing failure
    /// is an evaluation error, a violated invariant an internal error.
    pub(super) fn failure_error(
        &self,
        failure: Failure<impl Into<RuntimeFailure>>,
        span: Span,
    ) -> SemanticError {
        match failure {
            Failure::Error(error) => self.runtime_error(error, span),
            Failure::Invariant(invariant) => self.internal_error(invariant.to_string(), span),
        }
    }

    /// Like [`Self::failure_error`], keeping cancellation as control flow.
    pub(super) fn outcome_error(
        &self,
        outcome: Outcome<Failure<impl Into<RuntimeFailure>>>,
        span: Span,
    ) -> Outcome<SemanticError> {
        outcome.map_failed(|failure| self.failure_error(failure, span))
    }

    #[cold]
    pub fn internal_error(
        &self,
        message: impl Into<String>,
        anchor: impl Into<DiagnosticAnchor>,
    ) -> SemanticError {
        SemanticError::internal_error(message, self.src, anchor.into())
    }
}

impl EvalSession<'_> {
    /// Determine, once for a whole root tree, whether every dependency it may
    /// read (including those of unselected branches) is available.
    pub(crate) fn check_dependencies(
        &self,
        expression: ScopedNode<'_>,
    ) -> Result<(), Outcome<SemanticError>> {
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::DependencyAvailabilityCheck,
        );
        self.unavailable_among(|| expression.graph_refs(), std::iter::once(expression))?
            .map_or(Ok(()), |reason| {
                Err(SemanticError::located(
                    self.src,
                    expression.span(),
                    EvaluationError::Unavailable { reason },
                )
                .into())
            })
    }
}
