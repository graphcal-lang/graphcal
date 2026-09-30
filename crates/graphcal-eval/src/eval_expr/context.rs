//! Phase-specific construction of immutable expression environments.

use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::sync::Arc;

use graphcal_compiler::cancellation::CancellationToken;
use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::types::FormattingRegistry;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{CheckedDag, CheckedTir, StructFieldConstraintKey};
use miette::NamedSource;

use crate::checked_program::SealedDag;
use crate::constant_pools::RuntimeValueMap;
use crate::domain_constraint::ResolvedDomainConstraint;
use crate::execution_frame::ScheduledDeclaration;
use crate::execution_plan::{CallablePlan, ExecPlan};
use crate::host_fns::HostFunctionRegistry;
use crate::presentation_evidence::PresentationInstanceMap;

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

/// Read-only data exposed by an evaluation context.
///
/// There is deliberately no mutable dereference from `EvalContext`: callers
/// cannot replace a body, registry, or fact store independently after selection.
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
    pub root_presentation_instances: Option<&'a PresentationInstanceMap>,
}

/// An immutable environment whose capabilities can only be selected by phase.
#[derive(Clone)]
pub struct EvalContext<'a> {
    environment: EvalEnvironment<'a>,
    /// The DAG whose bodies this context runs, selected only by the phase
    /// constructors and scope transitions below. Its frame resolves every
    /// body handle this context meets.
    dag: &'a CheckedDag,
    capabilities: Capabilities<'a>,
}

impl<'a> Deref for EvalContext<'a> {
    type Target = EvalEnvironment<'a>;

    fn deref(&self) -> &Self::Target {
        &self.environment
    }
}

impl<'a> EvalContext<'a> {
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

    /// Select a provisional constant scope. DAG/host calls are unavailable and
    /// field constraints are deferred to mandatory constant-field checking.
    pub fn provisional_constants(
        tir: &'a CheckedTir,
        owner: &DagId,
        src: &'a NamedSource<Arc<String>>,
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
            environment: Self::environment(tir, src, cancellation),
            dag,
            capabilities: Capabilities::ProvisionalConstants,
        })
    }

    /// Select the checked runtime scope of `callable`'s own body. Field
    /// constraints cannot be omitted or supplied independently of the
    /// selected checked project facts.
    #[must_use]
    pub fn checked(
        plan: &'a ExecPlan<'a>,
        callable: &'a CallablePlan<'a>,
        src: &'a NamedSource<Arc<String>>,
        host: &'a HostFunctionRegistry,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            environment: Self::environment(plan.tir(), src, cancellation),
            dag: callable.scope().dag(),
            capabilities: Capabilities::Checked { plan, host },
        }
    }

    /// The executable tree of one of the current body's expression roots.
    pub fn executable(
        &self,
        root: &graphcal_compiler::hir::expr::Expr,
    ) -> Result<&'a graphcal_compiler::tir::texpr::TExpr, GraphcalError> {
        self.dag
            .bodies()
            .executable_value(root.id())
            .map_err(|error| self.internal_error(error.to_string(), root.span))
    }

    /// The text of a root of the current body checked as a contextual string.
    pub fn checked_string(
        &self,
        root: &graphcal_compiler::hir::expr::Expr,
    ) -> Result<&'a str, GraphcalError> {
        use graphcal_compiler::tir::texpr::{CheckedBody, ContextualLiteral, TBody};
        let message = match self.dag.bodies().get(root.id()) {
            Some(CheckedBody::Executable(TBody::Contextual(literal))) => match literal.literal() {
                ContextualLiteral::String(text) => return Ok(text),
                ContextualLiteral::OffsetDateTime(_)
                | ContextualLiteral::CivilDateTime(_)
                | ContextualLiteral::ZonedDateTime(_)
                | ContextualLiteral::TimeZone(_) => {
                    "expected checked contextual operand String".to_owned()
                }
            },
            Some(_) => "expected checked contextual operand String".to_owned(),
            None => format!("missing checked expression: {:?}", root.id()),
        };
        Err(self.internal_error(message, root.span))
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

    /// Static dependency availability, including references in unselected branches.
    pub fn unavailable_dependencies<
        'e,
        E: crate::static_incompleteness::ExpressionDependencies + 'e,
    >(
        &self,
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
        let expressions = expressions.into_iter().collect::<Vec<_>>();
        let mut dependencies = expressions
            .iter()
            .flat_map(|expression| expression.graph_refs())
            .map(|reference| self.resolve(&reference))
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

    /// The DAG whose bodies this context runs.
    #[must_use]
    pub const fn dag(&self) -> &'a CheckedDag {
        self.dag
    }

    /// The declaration `reference` denotes in the DAG this context runs.
    ///
    /// The frame is the selected DAG's own; evaluation code cannot pick one.
    #[must_use]
    pub fn resolve(&self, reference: &graphcal_compiler::hir::expr::LocalDecl) -> ResolvedDeclName {
        self.dag.resolve(reference)
    }

    /// The unit whose scale `unit` has in the DAG this context runs.
    #[must_use]
    pub fn resolve_unit(
        &self,
        unit: &graphcal_compiler::hir::expr::LocalUnit,
    ) -> graphcal_compiler::resolved_name::ResolvedUnitName {
        self.dag.resolve_unit(unit)
    }

    /// Determine, once for a whole root tree, whether every dependency it may
    /// read (including those of unselected branches) is available.
    pub fn check_dependencies(
        &self,
        expression: &graphcal_compiler::tir::texpr::TExpr,
    ) -> Result<(), GraphcalError> {
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::DependencyAvailabilityCheck,
        );
        self.unavailable_dependencies(std::iter::once(expression))?
            .map_or(Ok(()), |reason| {
                Err(GraphcalError::EvaluationUnavailable {
                    reason,
                    src: self.src.clone(),
                    span: expression.span().into(),
                })
            })
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
        dag: &CheckedDag,
        src: &'b NamedSource<Arc<String>>,
    ) -> Result<EvalContext<'b>, GraphcalError>
    where
        'a: 'b,
    {
        let mut context = self.with_src(src);
        context.dag = match self.capabilities {
            Capabilities::ProvisionalConstants => {
                self.tir.dag_registry().get(dag.dag_id()).ok_or_else(|| {
                    context.internal_error(
                        format!("constant scope `{}` has no compiled body", dag.dag_id()),
                        DiagnosticAnchor::WholeFile,
                    )
                })?
            }
            Capabilities::Checked { plan, .. } => plan
                .program()
                .dag(dag.dag_id())
                .ok_or_else(|| {
                    context.internal_error(
                        format!("DAG `{}` has no compiled body", dag.dag_id()),
                        DiagnosticAnchor::WholeFile,
                    )
                })?
                .dag(),
        };
        context.environment.current_decl = None;
        Ok(context)
    }

    /// Re-select one of `callable`'s execution DAGs, preserving capabilities
    /// and enclosing work.
    #[must_use]
    pub fn for_execution_dag<'b>(&'b self, scope: SealedDag<'b>) -> EvalContext<'b>
    where
        'a: 'b,
    {
        let mut context = self.with_src(scope.source());
        context.dag = scope.dag();
        context.environment.current_decl = None;
        context
    }

    /// Select the scope and declaration of one scheduled step, preserving
    /// capabilities and enclosing work. The step chooses its own scope.
    #[must_use]
    pub fn for_declaration<'b>(&'b self, step: &ScheduledDeclaration<'b>) -> EvalContext<'b>
    where
        'a: 'b,
    {
        let scope = step.scope();
        let mut context = self.with_src(scope.source());
        context.dag = scope.dag();
        context.environment.current_decl = Some(step.key().clone());
        context
    }

    pub fn for_checked_decl<'b>(
        &'b self,
        dag: &CheckedDag,
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
