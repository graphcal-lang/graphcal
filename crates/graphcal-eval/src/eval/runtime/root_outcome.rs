//! The outcome of one evaluation of the root DAG: what its execution plan
//! evaluated and every assertion it reports.
//!
//! Both consumers of an evaluation start from it: the full result assembly
//! of `graphcal eval` and the model-row path, which reports only the first
//! failure of the row.

use graphcal_compiler::source_registry::SourceRegistry;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;

use graphcal_compiler::display::source_display_names::SourceDisplayNames;

use crate::eval::output_decl_name::{OutputDeclName, OutputUnavailable};
use crate::eval::types::{AssertResult, EvalAssertion, NodeUnavailable, RuntimeUnavailable};
use crate::eval_expr::{EvalSession, RuntimeValueMap};
use crate::execution_plan::{Entry, ExecPlan};
use crate::host_fns::HostFunctionRegistry;
use crate::runtime_presentation::PendingPresentedMap;

use super::assertions::evaluate_assertions;
use super::root_loop::{EvalLoopResult, run_entry_loop};
use super::root_names::{RootNames, root_source_names};

/// One evaluation of the root DAG with one row of bindings.
pub struct RootOutcome {
    unfinished_calls: RefCell<BTreeSet<ResolvedDeclName>>,
    values: RuntimeValueMap,
    presentations: PendingPresentedMap,
    errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
    assertions: Vec<EvalAssertion>,
}

/// The parts of a [`RootOutcome`], for the result assembly that consumes it.
pub struct RootOutcomeParts {
    pub unfinished_calls: BTreeSet<ResolvedDeclName>,
    pub values: RuntimeValueMap,
    pub errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
    pub assertions: Vec<EvalAssertion>,
}

/// The first failure of a root evaluation, in the order a model row reports
/// it.
#[derive(Debug)]
pub enum RootFailure<'o> {
    /// A declaration failed, or is unavailable, under its root name.
    Declaration {
        name: ScopedName,
        reason: OutputUnavailable,
        /// The declarations that failed by themselves and that `reason`
        /// fails because of, followed through every failed dependency; empty
        /// when `reason` is itself such a failure.
        origins: Vec<FailureOrigin>,
    },
    /// An assertion did not pass.
    Assertion {
        name: &'o ScopedName,
        message: String,
    },
    /// A call reached unfinished formulas of the DAG it invoked.
    UnfinishedCalls(Vec<OutputDeclName>),
}

/// A declaration that failed by itself, as opposed to failing because a
/// dependency did: where a reported dependency failure originates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureOrigin {
    pub name: OutputDeclName,
    pub message: String,
}

impl RootOutcome {
    /// Run the root's plan with `bindings`, then evaluate every assertion the
    /// root reports, naming private include scopes and invoked modules by `display_names`.
    pub fn evaluate(
        plan: &ExecPlan<'_>,
        bindings: &crate::eval::bindings::RuntimeParameterBindings,
        src: SourceId,
        sources: &SourceRegistry,
        host_fns: &HostFunctionRegistry,
        display_names: &SourceDisplayNames,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, Outcome<SemanticError>> {
        Self::evaluate_entry(
            plan.root_entry(),
            bindings,
            src,
            sources,
            host_fns,
            display_names,
            cancellation,
        )
    }

    /// Run the plan of `entry` with `bindings`, then evaluate every assertion
    /// `entry` reports.
    pub(super) fn evaluate_entry(
        entry: Entry<'_, '_>,
        bindings: &crate::eval::bindings::RuntimeParameterBindings,
        src: SourceId,
        sources: &SourceRegistry,
        host_fns: &HostFunctionRegistry,
        display_names: &SourceDisplayNames,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, Outcome<SemanticError>> {
        let plan = entry.plan();
        let EvalLoopResult {
            unfinished_calls,
            values,
            presentations,
            errors,
        } = run_entry_loop(entry, bindings, src, sources, host_fns, cancellation)?;
        cancellation.checkpoint()?;
        let mut outcome = Self {
            unfinished_calls,
            values,
            presentations,
            errors,
            assertions: Vec::new(),
        };
        let ctx = outcome.session(plan, src, sources, host_fns, cancellation);
        let assertions = evaluate_assertions(
            entry,
            src,
            &ctx,
            &outcome.values,
            &outcome.errors,
            &RootNames::new(entry, display_names),
        )?;
        outcome.assertions = assertions;
        Ok(outcome)
    }

    /// A session over this outcome, for further root-level evaluation; calls
    /// that reach unfinished formulas are recorded in this outcome.
    pub(crate) fn session<'a>(
        &'a self,
        plan: &'a ExecPlan<'a>,
        src: SourceId,
        sources: &'a SourceRegistry,
        host_fns: &'a HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> EvalSession<'a> {
        EvalSession::checked(plan, src, sources, host_fns, cancellation.clone())
            .with_unavailable(&self.errors)
            .with_unfinished_calls(&self.unfinished_calls)
    }

    /// The values of the declarations that evaluated successfully.
    pub const fn values(&self) -> &RuntimeValueMap {
        &self.values
    }

    /// The declarations that failed or are unavailable.
    pub(crate) const fn errors(&self) -> &HashMap<ResolvedDeclName, RuntimeUnavailable> {
        &self.errors
    }

    /// The presentations of the evaluated declarations, still pending: the
    /// result assembly resolves them against the complete root frame.
    pub(crate) const fn presentations(&self) -> &PendingPresentedMap {
        &self.presentations
    }

    pub(crate) fn into_parts(self) -> RootOutcomeParts {
        RootOutcomeParts {
            unfinished_calls: self.unfinished_calls.into_inner(),
            values: self.values,
            errors: self.errors,
            assertions: self.assertions,
        }
    }

    /// The first failure of the evaluation: a failed or unavailable
    /// declaration, else an assertion that did not pass, else calls that
    /// reached unfinished formulas.
    ///
    /// A declaration the root exposes is reported first, in root-exposure
    /// order, under its source name; otherwise the smallest failed
    /// declaration of a semantic instance, under its instance member name.
    #[must_use]
    pub fn first_failure(
        &self,
        plan: &ExecPlan<'_>,
        display_names: &SourceDisplayNames,
    ) -> Option<RootFailure<'_>> {
        let root = plan.root_entry();
        let names = RootNames::new(root, display_names);
        let exposed = root_source_names(root).into_iter().find_map(|(key, name)| {
            self.errors
                .get(&key)
                .map(|reason| RootFailure::Declaration {
                    name,
                    reason: names.present(reason),
                    origins: self.failure_origins(reason, &names),
                })
        });
        // Otherwise the smallest failed declaration of a semantic instance:
        // every declaration the root evaluates is one of its closure DAGs'.
        let declaration = exposed.or_else(|| {
            plan.root()
                .execution_dags()
                .iter()
                .flat_map(|closure| {
                    closure
                        .scope()
                        .dag()
                        .declarations()
                        .filter_map(move |entry| {
                            self.errors.get(entry.identity()).map(|reason| {
                                (entry.identity(), closure.member(entry.name()), reason)
                            })
                        })
                })
                .min_by(|(left, ..), (right, ..)| left.cmp(right))
                .map(|(_, name, reason)| RootFailure::Declaration {
                    name,
                    reason: names.present(reason),
                    origins: self.failure_origins(reason, &names),
                })
        });
        let assertion = || {
            self.assertions
                .iter()
                .find_map(|EvalAssertion { name, outcome, .. }| match outcome {
                    AssertResult::Pass => None,
                    AssertResult::Blocked { reason } => Some((name, reason.to_string())),
                    AssertResult::Fail { message } | AssertResult::Error { message } => {
                        Some((name, message.clone()))
                    }
                })
                .map(|(name, message)| RootFailure::Assertion { name, message })
        };
        let unfinished = || {
            let calls = self.unfinished_calls.borrow();
            (!calls.is_empty()).then(|| {
                RootFailure::UnfinishedCalls(
                    calls
                        .iter()
                        .map(|declaration| names.name(declaration))
                        .collect(),
                )
            })
        };
        declaration.or_else(assertion).or_else(unfinished)
    }

    /// The declarations that failed by themselves and that `reason` fails
    /// because of, in identity order. Failed dependencies are followed until
    /// a declaration whose own evaluation failed; dependencies that are only
    /// unfinished are not failures and are not followed.
    fn failure_origins(
        &self,
        reason: &RuntimeUnavailable,
        names: &RootNames<'_>,
    ) -> Vec<FailureOrigin> {
        let mut visited = BTreeSet::new();
        let mut origins = BTreeSet::new();
        let mut pending = failed_dependencies(reason).to_vec();
        while let Some(declaration) = pending.pop() {
            if !visited.insert(declaration.clone()) {
                continue;
            }
            match self.errors.get(&declaration) {
                Some(NodeUnavailable::EvalFailed { .. }) => {
                    origins.insert(declaration);
                }
                Some(dependent) => pending.extend(failed_dependencies(dependent).iter().cloned()),
                // A failure recorded outside this evaluation's declarations,
                // e.g. inside an invoked call, has its cause in its message.
                None => {}
            }
        }
        origins
            .into_iter()
            .filter_map(|declaration| match self.errors.get(&declaration) {
                Some(NodeUnavailable::EvalFailed { message }) => Some(FailureOrigin {
                    name: names.name(&declaration),
                    message: message.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

/// The dependencies whose failure `reason` reports.
fn failed_dependencies(reason: &RuntimeUnavailable) -> &[ResolvedDeclName] {
    match reason {
        NodeUnavailable::DependencyFailed { failed_deps } => failed_deps.as_slice(),
        NodeUnavailable::Blocked { failed_deps, .. } => failed_deps,
        NodeUnavailable::EvalFailed { .. } | NodeUnavailable::Todo { .. } => &[],
    }
}
