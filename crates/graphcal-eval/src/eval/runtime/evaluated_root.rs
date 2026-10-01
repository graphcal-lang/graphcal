//! What one run of the root evaluated, borrowed by the result-assembly
//! stages.

use std::collections::HashMap;

use graphcal_compiler::resolved_name::ResolvedDeclName;

use crate::eval::types::NodeUnavailable;
use crate::eval_expr::RuntimeValueMap;
use crate::runtime_presentation::{PendingPresentedMap, ResolvedPresentedMap};

use super::root_names::RootNames;

/// What one run of the root evaluated: the values of its successful
/// declarations, its contained failures, and its presentations.
#[derive(Clone, Copy)]
pub(super) struct EvaluatedRoot<'a> {
    pub(super) values: &'a RuntimeValueMap,
    pub(super) errors: &'a HashMap<ResolvedDeclName, NodeUnavailable>,
    /// The names the root gives the declarations the failures name.
    pub(super) names: &'a RootNames<'a>,
    /// The presented values of the declarations with a presentation,
    /// resolved against the complete root frame.
    pub(super) presentations: &'a ResolvedPresentedMap,
    /// The same presented values as the root frame holds them, still
    /// pending, for an expression evaluated over the root frame (a plot
    /// channel), whose own presentation is then resolved against `values`.
    pub(super) frame_presentations: &'a PendingPresentedMap,
}
