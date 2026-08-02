//! Plan-ready runtime parameter bindings shared by normal and model evaluation.

use std::collections::HashMap;

use graphcal_compiler::hir;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

use crate::decl_key::RuntimeDeclKey;

/// One value injected for a compiled parameter.
#[derive(Debug, Clone)]
pub(super) struct RuntimeParameterBinding {
    /// Semantic runtime value in Graphcal's internal representation.
    pub(crate) value: RuntimeValue,
    /// Closed HIR value expression retained only for display-unit metadata.
    ///
    /// Arrow-originated model values have no source display expression and use
    /// the model boundary's canonical unit instead.
    pub(crate) display_expr: Option<hir::Expr>,
}

/// Plan-keyed parameter bindings for one evaluation row.
pub(super) type RuntimeParameterBindings = HashMap<RuntimeDeclKey, RuntimeParameterBinding>;
