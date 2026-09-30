//! Runtime evaluation: running the root's execution plan, then assembling
//! its public result in stages — values, assertions, plots and their
//! compositions, and the per-declaration tables.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::{DeclarationBody, Scoped};

use crate::domain_constraint::ResolvedDomainConstraint;
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_root_with_presentation};
use crate::execution_plan::ExecPlan;
use crate::presentation_evidence::{PendingPresentationMap, ResolvedPresentationMap};

use super::types::{EvalResult, NodeUnavailable};

mod assertions;
mod plots;
mod root_names;
mod root_outcome;
mod value_entries;

use assertions::evaluate_assertions;
pub(super) use root_outcome::{RootFailure, RootOutcome};

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub(super) struct EvalLoopResult {
    pub unfinished_calls: std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    pub values: RuntimeValueMap,
    pub presentations: PendingPresentationMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

/// One completed runtime evaluation before project-level public output assembly.
///
/// Keeping values, contained node errors, and the display-aware root result in
/// one artifact makes the evaluator's direct output available to debugging
/// consumers without running the evaluator a second time.
pub struct RuntimeEvaluation {
    pub(super) result: EvalResult,
    pub(super) presentations: ResolvedPresentationMap,
    pub(super) values: RuntimeValueMap,
    pub(super) errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

impl std::fmt::Debug for RuntimeEvaluation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeEvaluation")
            .field("result", &self.result)
            .field("presentations", &self.presentations)
            .field("values", &self.values)
            .field("errors", &self.errors)
            .finish()
    }
}

impl RuntimeEvaluation {
    /// Whether evaluation produced a node, assertion, or plot failure.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors.values().any(NodeUnavailable::has_failure) || self.result.has_errors()
    }

    /// Display-aware result for the directly evaluated root DAG.
    #[must_use]
    pub const fn result(&self) -> &EvalResult {
        &self.result
    }
}

/// Execute the root with ordinary failures contained by the shared machine.
pub(super) fn run_eval_loop_with_bindings(
    plan: &ExecPlan<'_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, GraphcalError> {
    use crate::execution_frame::{ExecutionFrame, FailurePolicy};
    cancellation.checkpoint()?;
    let unfinished_calls = std::cell::RefCell::new(BTreeSet::new());
    let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Contain);
    for (key, binding) in bindings {
        frame.bind_argument(
            key,
            crate::runtime_presentation::EvaluatedRuntimeValue::new(
                binding.value.clone(),
                binding.presentation.clone(),
            ),
            src,
            Span::new(0, 0),
        )?;
    }
    frame.run(cancellation, |entry, frame| {
        // Root declarations keep their existing work allowance; nested calls
        // share this context's budget through immutable scope reselection.
        let root = EvalSession::checked(plan, src, host_fns, cancellation.clone())
            .with_roots(frame.values(), Some(frame.presentations()))
            .with_unavailable(frame.errors())
            .with_unfinished_calls(&unfinished_calls);
        let session = root.for_declaration(&entry);
        eval_root_with_presentation(
            entry.body(),
            frame.values(),
            frame.presentations(),
            &session,
        )
    })?;
    let outcome = frame.finish();
    Ok(EvalLoopResult {
        unfinished_calls,
        values: outcome.values,
        presentations: outcome.presentations,
        errors: outcome.errors,
    })
}

/// What one run of the root evaluated: the values of its successful
/// declarations, its contained failures, and its presentations.
#[derive(Clone, Copy)]
struct EvaluatedRoot<'a> {
    values: &'a RuntimeValueMap,
    errors: &'a HashMap<ResolvedDeclName, NodeUnavailable>,
    /// The presentations of the declarations, resolved against the complete
    /// root frame.
    presentations: &'a ResolvedPresentationMap,
    /// The same presentations as the root frame holds them, still pending,
    /// for an expression evaluated over the root frame (a plot channel),
    /// whose own presentation is then resolved against `values`.
    frame_presentations: &'a PendingPresentationMap,
}

/// Evaluate a plan with one row of runtime parameter bindings, then assemble
/// the root's public result.
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
/// Internal invariant violations abort evaluation as `X001`.
pub(super) fn evaluate_plan_with_values_and_bindings_and_cancellation(
    plan: &ExecPlan<'_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeEvaluation, GraphcalError> {
    cancellation.checkpoint()?;
    let outcome = RootOutcome::evaluate(plan, bindings, src, host_fns, cancellation)?;
    cancellation.checkpoint()?;
    let ctx = outcome.session(plan, src, host_fns, cancellation);
    let presentations = outcome
        .presentations()
        .iter()
        .map(|(key, presentation)| {
            crate::eval_expr::presentation::resolve(presentation.clone(), outcome.values(), &ctx)
                .map(|presentation| (key.clone(), presentation))
        })
        .collect::<Result<ResolvedPresentationMap, _>>()?;
    let evaluated = EvaluatedRoot {
        values: outcome.values(),
        errors: outcome.errors(),
        presentations: &presentations,
        frame_presentations: outcome.presentations(),
    };

    let value_entries::ValueEntries {
        entries,
        output_surface,
        mut presentation_diagnostics,
    } = value_entries::assemble_value_entries(plan, evaluated, &ctx)?;
    cancellation.checkpoint()?;

    let plots::PlotOutputs {
        plots,
        plot_errors,
        figures,
        layers,
    } = plots::evaluate_root_plots(plan, evaluated, &ctx)?;
    cancellation.checkpoint()?;
    presentation_diagnostics.extend(
        plots
            .iter()
            .flat_map(|plot| plot.presentation_diagnostics.iter().cloned()),
    );
    let assumes_map = assertions::root_assumes_map(plan, src)?;

    let root_outcome::RootOutcomeParts {
        unfinished_calls,
        values,
        errors,
        assertions,
    } = outcome.into_parts();
    let result = EvalResult {
        unfinished_calls: unfinished_calls.into_iter().collect(),
        entries,
        output_surface,
        assertions,
        plots,
        plot_errors,
        presentation_diagnostics,
        figures,
        layers,
        assumes_map,
        render: super::types::RenderContext::new(
            plan.tir().registry().dimensions.base_unit_symbols(),
            plan.tir().registry().time_zones.clone(),
        ),
        domain_constraints: root_domain_constraints(plan),
    };
    Ok(RuntimeEvaluation {
        result,
        presentations,
        values,
        errors,
    })
}

/// The domain constraints of the root's own declarations, by their source
/// names.
fn root_domain_constraints(plan: &ExecPlan<'_>) -> HashMap<ScopedName, ResolvedDomainConstraint> {
    plan.root()
        .scope()
        .dag()
        .decls()
        .iter()
        .filter_map(|entry| {
            plan.domain_constraint(&entry.identity())
                .map(|constraint| (ScopedName::local(entry.name().clone()), constraint.clone()))
        })
        .collect()
}

/// The source of `declaration` in the scope of its owner.
fn declaration_body<'tir>(
    tir: &'tir graphcal_compiler::tir::typed::CheckedTir,
    declaration: &ResolvedDeclName,
    src: &NamedSource<Arc<String>>,
) -> Result<DeclarationBody<'tir>, GraphcalError> {
    tir.declaration_body(declaration).ok_or_else(|| {
        GraphcalError::internal_error(
            format!("declaration `{declaration}` is absent from its owner's checked body"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

/// If any declaration referenced by the given expressions failed to
/// evaluate, render a `dependency failed: ...` message naming each failed
/// dependency (direct failures carry their root cause inline).
///
/// Shared by assertions (#814) and plots (#842): a reference to a failed
/// declaration is not "undefined", it is unevaluable, and the report must
/// point at the root cause.
fn dependency_failure_message<'a>(
    exprs: impl IntoIterator<Item = Scoped<'a, graphcal_compiler::hir::Expr>>,
    errors: &HashMap<ResolvedDeclName, NodeUnavailable>,
) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let deps: std::collections::BTreeSet<_> =
        exprs.into_iter().flat_map(Scoped::graph_refs).collect();
    let failed: Vec<String> =
        deps.iter()
            .filter_map(|dep| {
                errors.get(dep).map(|err| {
                    let leaf = dep.atom();
                    match err {
                        NodeUnavailable::EvalFailed { message } => format!("{leaf} ({message})"),
                        NodeUnavailable::DependencyFailed { .. } => leaf.to_string(),
                        reason @ (NodeUnavailable::Todo { .. }
                        | NodeUnavailable::Blocked { .. }) => format!("{leaf} ({reason})"),
                    }
                })
            })
            .collect();
    (!failed.is_empty()).then(|| format!("dependency failed: {}", failed.join(", ")))
}
