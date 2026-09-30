//! Plan-ready runtime parameter bindings shared by normal and model evaluation.

use std::collections::HashMap;

use crate::runtime_presentation::EvaluatedRuntimeValue;

use graphcal_compiler::resolved_name::ResolvedDeclName;

/// One value injected for a compiled parameter, with its checked
/// presentation.
///
/// Arrow-originated model values use the model boundary's canonical unit and
/// therefore carry no authored presentation preference.
pub type RuntimeParameterBinding = EvaluatedRuntimeValue;

/// Plan-keyed parameter bindings for one evaluation row.
pub type RuntimeParameterBindings = HashMap<ResolvedDeclName, RuntimeParameterBinding>;
