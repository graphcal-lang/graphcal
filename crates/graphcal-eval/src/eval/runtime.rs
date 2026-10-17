//! Runtime evaluation: running the root's execution plan, then assembling
//! its public result in stages — values, assertions, plots and their
//! compositions, and the per-declaration tables.

use graphcal_compiler::source_registry::SourceRegistry;
use std::collections::HashMap;

use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;

use crate::domain_constraint::ResolvedDomainConstraint;
use crate::eval_expr::RuntimeValueMap;
use crate::execution_plan::{Entry, ExecPlan};
use crate::runtime_presentation::ResolvedPresentedMap;

use super::types::{EvalResult, RuntimeUnavailable};

mod assertions;
mod dependency_failures;
mod evaluated_root;
mod plots;
mod root_loop;
mod root_names;
mod root_outcome;
mod value_entries;

use evaluated_root::EvaluatedRoot;
pub use root_loop::{EvalLoopResult, run_eval_loop_with_bindings};
pub use root_outcome::{RootFailure, RootOutcome};

/// One completed runtime evaluation before project-level public output assembly.
///
/// Keeping values, contained node errors, and the display-aware root result in
/// one artifact makes the evaluator's direct output available to debugging
/// consumers without running the evaluator a second time.
pub struct RuntimeEvaluation {
    result: EvalResult,
    presentations: ResolvedPresentedMap,
    values: RuntimeValueMap,
    errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
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
    /// The root result and the resolved presentations of its values, for
    /// project-level output assembly.
    #[must_use]
    pub fn into_result_and_presentations(self) -> (EvalResult, ResolvedPresentedMap) {
        (self.result, self.presentations)
    }

    /// Whether evaluation produced a node, assertion, or plot failure.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors.values().any(RuntimeUnavailable::has_failure) || self.result.has_errors()
    }
}

/// Evaluate a plan with one row of runtime parameter bindings, then assemble
/// the root's public result, naming private include scopes and invoked
/// modules by `display_names`.
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
/// Internal invariant violations abort evaluation as `X001`.
pub fn evaluate_plan_with_values_and_bindings_and_cancellation(
    plan: &ExecPlan<'_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: SourceId,
    sources: &SourceRegistry,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    display_names: &graphcal_compiler::display::source_display_names::SourceDisplayNames,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeEvaluation, Outcome<SemanticError>> {
    evaluate_entry(
        plan.root_entry(),
        bindings,
        src,
        sources,
        host_fns,
        display_names,
        cancellation,
    )
}

/// Evaluate every inline DAG of the root file that runs on its own defaults.
///
/// Each runs as if it were the entry of its own file, and its public result
/// is assembled the way
/// [`evaluate_plan_with_values_and_bindings_and_cancellation`] assembles the
/// root's: its own declarations under their local names. The DAGs are the
/// program's
/// [`default_runnable_dags`](graphcal_compiler::tir::typed::checked::CheckedTir::default_runnable_dags);
/// their values belong to the template DAGs themselves, never to an include
/// instance of one, so they are keyed by the templates' identities.
///
/// # Errors
///
/// Returns the outcomes of a root evaluation.
pub fn evaluate_dag_defaults_with_cancellation(
    plan: &ExecPlan<'_>,
    src: SourceId,
    sources: &SourceRegistry,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    display_names: &graphcal_compiler::display::source_display_names::SourceDisplayNames,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<Vec<RuntimeEvaluation>, Outcome<SemanticError>> {
    let defaults = super::bindings::RuntimeParameterBindings::new();
    plan.defaults_entries()
        .map(|entry| {
            evaluate_entry(
                entry,
                &defaults,
                src,
                sources,
                host_fns,
                display_names,
                cancellation,
            )
        })
        .collect()
}

/// Evaluate `entry` with `bindings`, then assemble its public result.
fn evaluate_entry(
    entry: Entry<'_, '_>,
    bindings: &super::bindings::RuntimeParameterBindings,
    src: SourceId,
    sources: &SourceRegistry,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    display_names: &graphcal_compiler::display::source_display_names::SourceDisplayNames,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<RuntimeEvaluation, Outcome<SemanticError>> {
    cancellation.checkpoint()?;
    let plan = entry.plan();
    let outcome = RootOutcome::evaluate_entry(
        entry,
        bindings,
        src,
        sources,
        host_fns,
        display_names,
        cancellation,
    )?;
    cancellation.checkpoint()?;
    let ctx = outcome.session(plan, src, sources, host_fns, cancellation);
    let presentations = outcome
        .presentations()
        .iter()
        .map(|(key, presentation)| {
            crate::eval_expr::resolve_presentation(presentation.clone(), outcome.values(), &ctx)
                .map(|presentation| (key.clone(), presentation))
        })
        .collect::<Result<ResolvedPresentedMap, _>>()?;
    let names = root_names::RootNames::new(entry, display_names);
    let evaluated = EvaluatedRoot {
        values: outcome.values(),
        errors: outcome.errors(),
        names: &names,
        presentations: &presentations,
        frame_presentations: outcome.presentations(),
    };

    let value_entries::ValueEntries {
        entries,
        output_surface,
        mut presentation_diagnostics,
        nested_instance_entries,
    } = value_entries::assemble_value_entries(entry, evaluated, &ctx)?;
    cancellation.checkpoint()?;

    let plots::PlotOutputs {
        plots,
        plot_errors,
        figures,
        layers,
    } = plots::evaluate_root_plots(entry, evaluated, &ctx)?;
    cancellation.checkpoint()?;
    presentation_diagnostics.extend(
        plots
            .iter()
            .flat_map(|plot| plot.presentation_diagnostics.iter().cloned()),
    );
    let assumes_map = assertions::root_assumes_map(entry);

    let root_outcome::RootOutcomeParts {
        unfinished_calls,
        values,
        errors,
        assertions,
    } = outcome.into_parts();
    let result = EvalResult {
        unfinished_calls: unfinished_calls
            .iter()
            .map(|declaration| names.name(declaration))
            .collect(),
        entries,
        nested_instance_entries,
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
        domain_constraints: root_domain_constraints(entry),
    };
    Ok(RuntimeEvaluation {
        result,
        presentations,
        values,
        errors,
    })
}

/// The domain constraints of the entry's own declarations, by their source
/// names.
fn root_domain_constraints(entry: Entry<'_, '_>) -> HashMap<ScopedName, ResolvedDomainConstraint> {
    let plan = entry.plan();
    entry
        .callable()
        .scope()
        .dag()
        .declarations()
        .filter_map(|entry| {
            plan.domain_constraint(entry.identity())
                .map(|constraint| (ScopedName::local(entry.name().clone()), constraint.clone()))
        })
        .collect()
}
