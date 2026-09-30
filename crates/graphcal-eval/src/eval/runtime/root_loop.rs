//! The root's execution: its plan run by the shared frame machine with
//! ordinary failures contained.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::span::Span;

use crate::eval::types::NodeUnavailable;
use crate::eval_expr::{EvalSession, RuntimeValueMap, eval_root_with_presentation};
use crate::execution_plan::ExecPlan;
use crate::runtime_presentation::PendingPresentedMap;

/// Result of running the core eval loop: successfully evaluated values and per-node errors.
pub struct EvalLoopResult {
    pub unfinished_calls: std::cell::RefCell<BTreeSet<ResolvedDeclName>>,
    pub values: RuntimeValueMap,
    pub presentations: PendingPresentedMap,
    pub errors: HashMap<ResolvedDeclName, NodeUnavailable>,
}

/// Execute the root with ordinary failures contained by the shared machine.
pub fn run_eval_loop_with_bindings(
    plan: &ExecPlan<'_>,
    bindings: &crate::eval::bindings::RuntimeParameterBindings,
    src: &NamedSource<Arc<String>>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalLoopResult, GraphcalError> {
    use crate::execution_frame::{ExecutionFrame, FailurePolicy};
    cancellation.checkpoint()?;
    let unfinished_calls = std::cell::RefCell::new(BTreeSet::new());
    let mut frame = ExecutionFrame::new(plan, plan.root(), FailurePolicy::Contain);
    for (key, binding) in bindings {
        frame.bind_argument(key, binding.clone(), src, Span::new(0, 0))?;
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
        presentations: outcome.presented,
        errors: outcome.errors,
    })
}
