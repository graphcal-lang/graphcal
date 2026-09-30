//! The outcome of one evaluation of the root DAG: what its execution plan
//! evaluated and every assertion it reports.
//!
//! Both consumers of an evaluation start from it: the full result assembly
//! of `graphcal eval` and the model-row path, which reports only the first
//! failure of the row.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;

use crate::eval::types::{AssertResult, NodeUnavailable};
use crate::eval_expr::{EvalSession, RuntimeValueMap};
use crate::execution_plan::ExecPlan;
use crate::host_fns::HostFunctionRegistry;
use crate::runtime_presentation::PendingPresentedMap;

use super::assertions::evaluate_assertions;
use super::root_loop::{EvalLoopResult, run_eval_loop_with_bindings};
use super::root_names::{instance_member_name, root_source_names};

/// One evaluation of the root DAG with one row of bindings.
pub struct RootOutcome {
    unfinished_calls: RefCell<BTreeSet<ResolvedDeclName>>,
    values: RuntimeValueMap,
    presentations: PendingPresentedMap,
    errors: HashMap<ResolvedDeclName, NodeUnavailable>,
    assertions: Vec<(ScopedName, AssertResult, Span)>,
}

/// The parts of a [`RootOutcome`], for the result assembly that consumes it.
pub struct RootOutcomeParts {
    pub unfinished_calls: BTreeSet<ResolvedDeclName>,
    pub values: RuntimeValueMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
    pub assertions: Vec<(ScopedName, AssertResult, Span)>,
}

/// The first failure of a root evaluation, in the order a model row reports
/// it.
#[derive(Debug)]
pub enum RootFailure<'o> {
    /// A declaration failed, or is unavailable, under its root name.
    Declaration {
        name: ScopedName,
        reason: &'o NodeUnavailable,
    },
    /// An assertion did not pass.
    Assertion {
        name: &'o ScopedName,
        message: String,
    },
    /// A call reached unfinished formulas of the DAG it invoked.
    UnfinishedCalls(Vec<ResolvedDeclName>),
}

impl RootOutcome {
    /// Run the root's plan with `bindings`, then evaluate every assertion the
    /// root reports.
    pub fn evaluate(
        plan: &ExecPlan<'_>,
        bindings: &crate::eval::bindings::RuntimeParameterBindings,
        src: &NamedSource<Arc<String>>,
        host_fns: &HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> Result<Self, GraphcalError> {
        let EvalLoopResult {
            unfinished_calls,
            values,
            presentations,
            errors,
        } = run_eval_loop_with_bindings(plan, bindings, src, host_fns, cancellation)?;
        cancellation.checkpoint()?;
        let mut outcome = Self {
            unfinished_calls,
            values,
            presentations,
            errors,
            assertions: Vec::new(),
        };
        let ctx = outcome.session(plan, src, host_fns, cancellation);
        let assertions = evaluate_assertions(plan, src, &ctx, &outcome.values, &outcome.errors)?;
        outcome.assertions = assertions;
        Ok(outcome)
    }

    /// A session over this outcome, for further root-level evaluation; calls
    /// that reach unfinished formulas are recorded in this outcome.
    pub fn session<'a>(
        &'a self,
        plan: &'a ExecPlan<'a>,
        src: &'a NamedSource<Arc<String>>,
        host_fns: &'a HostFunctionRegistry,
        cancellation: &graphcal_compiler::cancellation::CancellationToken,
    ) -> EvalSession<'a> {
        EvalSession::checked(plan, src, host_fns, cancellation.clone())
            .with_roots(&self.values, Some(&self.presentations))
            .with_unavailable(&self.errors)
            .with_unfinished_calls(&self.unfinished_calls)
    }

    /// The values of the declarations that evaluated successfully.
    pub const fn values(&self) -> &RuntimeValueMap {
        &self.values
    }

    /// The declarations that failed or are unavailable.
    pub const fn errors(&self) -> &HashMap<ResolvedDeclName, NodeUnavailable> {
        &self.errors
    }

    /// The presentations of the evaluated declarations, still pending: the
    /// result assembly resolves them against the complete root frame.
    pub const fn presentations(&self) -> &PendingPresentedMap {
        &self.presentations
    }

    pub fn into_parts(self) -> RootOutcomeParts {
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
    ///
    /// # Errors
    ///
    /// Returns an internal error when a failed declaration has neither name.
    pub fn first_failure(
        &self,
        plan: &ExecPlan<'_>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Option<RootFailure<'_>>, GraphcalError> {
        let exposed = root_source_names(plan).into_iter().find_map(|(key, name)| {
            self.errors
                .get(&key)
                .map(|reason| RootFailure::Declaration { name, reason })
        });
        let declaration = match exposed {
            Some(failure) => Some(failure),
            None => self
                .errors
                .iter()
                .min_by(|(left, _), (right, _)| left.cmp(right))
                .map(|(key, reason)| {
                    instance_member_name(plan.tir().root_dag_id(), key, src)
                        .map(|name| RootFailure::Declaration { name, reason })
                })
                .transpose()?,
        };
        let assertion = || {
            self.assertions
                .iter()
                .find_map(|(name, result, _)| match result {
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
            (!calls.is_empty())
                .then(|| RootFailure::UnfinishedCalls(calls.iter().cloned().collect()))
        };
        Ok(declaration.or_else(assertion).or_else(unfinished))
    }
}
