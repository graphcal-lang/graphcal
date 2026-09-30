//! Plan-ready runtime parameter bindings shared by normal and model evaluation.

use std::collections::HashMap;

use crate::presentation_evidence::PendingPresentation;
use crate::runtime_value::RuntimeValue;

use graphcal_compiler::resolved_name::ResolvedDeclName;

/// One value injected for a compiled parameter.
#[derive(Debug, Clone)]
pub(super) struct RuntimeParameterBinding {
    /// Semantic runtime value in Graphcal's internal representation.
    pub(crate) value: RuntimeValue,
    /// Checked presentation provenance for the injected value.
    ///
    /// Arrow-originated model values use the model boundary's canonical unit
    /// and therefore carry no authored presentation preference here.
    pub(crate) presentation: PendingPresentation,
}

/// Plan-keyed parameter bindings for one evaluation row.
pub(super) type RuntimeParameterBindings = HashMap<ResolvedDeclName, RuntimeParameterBinding>;
