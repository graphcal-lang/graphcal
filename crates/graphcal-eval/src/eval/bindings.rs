//! Plan-ready runtime parameter bindings shared by normal and model evaluation.

use std::collections::HashMap;

use crate::presentation_evidence::PresentationInstance;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use crate::decl_key::RuntimeDeclKey;

/// One value injected for a compiled parameter.
#[derive(Debug, Clone)]
pub(super) struct RuntimeParameterBinding {
    /// Semantic runtime value in Graphcal's internal representation.
    pub(crate) value: RuntimeValue,
    /// Checked presentation provenance for the injected value.
    ///
    /// Arrow-originated model values use the model boundary's canonical unit
    /// and therefore carry no authored presentation preference here.
    pub(crate) presentation: PresentationInstance,
}

/// Plan-keyed parameter bindings for one evaluation row.
pub(super) type RuntimeParameterBindings = HashMap<RuntimeDeclKey, RuntimeParameterBinding>;
