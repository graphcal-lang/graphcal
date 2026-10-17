//! The execution of an evaluation's entry (the root, or a DAG on its
//! defaults): its plan run by the shared frame machine with ordinary
//! failures contained.

use graphcal_compiler::source_registry::SourceRegistry;
use std::collections::{BTreeSet, HashMap};

use graphcal_compiler::outcome::Outcome;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::span::Span;

use crate::eval::types::RuntimeUnavailable;
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_scheduled};
use crate::execution_plan::{Entry, ExecPlan};
use crate::runtime_presentation::PendingPresentedMap;

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub struct EvalLoopResult {
    pub(crate) unfinished_calls: std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    pub values: RuntimeValueMap,
    pub(crate) presentations: PendingPresentedMap,
    pub errors: HashMap<ResolvedDeclName, RuntimeUnavailable>,
}

/// Execute the root with ordinary failures contained by the shared machine.
pub fn run_eval_loop_with_bindings(
    plan: &ExecPlan<'_>,
    bindings: &crate::eval::bindings::RuntimeParameterBindings,
    src: SourceId,
    sources: &SourceRegistry,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, Outcome<SemanticError>> {
    run_entry_loop(
        plan.root_entry(),
        bindings,
        src,
        sources,
        host_fns,
        cancellation,
    )
}

/// Execute `entry` with ordinary failures contained by the shared machine.
pub(super) fn run_entry_loop(
    entry: Entry<'_, '_>,
    bindings: &crate::eval::bindings::RuntimeParameterBindings,
    src: SourceId,
    sources: &SourceRegistry,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, Outcome<SemanticError>> {
    use crate::execution_frame::{ExecutionFrame, FailurePolicy};
    cancellation.checkpoint()?;
    let plan = entry.plan();
    let unfinished_calls = std::cell::RefCell::new(BTreeSet::new());
    let mut frame = ExecutionFrame::new(plan, entry.callable(), FailurePolicy::Contain);
    for (key, binding) in bindings {
        frame.bind_argument(key, binding.clone(), src, Span::new(0, 0))?;
    }
    frame.run(cancellation, |entry, frame| {
        // Root declarations keep their existing work allowance; nested calls
        // share this context's budget through immutable scope reselection.
        let root = EvalSession::checked(plan, src, sources, host_fns, cancellation.clone())
            .with_unavailable(frame.errors())
            .with_unfinished_calls(&unfinished_calls);
        eval_scheduled(&entry, frame.values(), frame.presentations(), &root)
    })?;
    let outcome = frame.finish();
    Ok(EvalLoopResult {
        unfinished_calls,
        values: outcome.values,
        presentations: outcome.presented,
        errors: outcome.errors,
    })
}
