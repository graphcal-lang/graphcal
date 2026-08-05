//! Canonical imported value bindings at the IR/TIR boundary.
//!
//! A source-visible import spelling is only a lexical key. Its semantic target,
//! declared type, and optional pre-evaluated value must travel together so an
//! include merge cannot independently collide or mix those facts.

use crate::registry::declared_type::DeclaredType;
use crate::registry::runtime_value::RuntimeValue;
use crate::syntax::decl_name::ResolvedDeclName;

/// Semantic facts attached to one source-visible imported value binding.
///
/// The containing map is keyed by the lexical [`ScopedName`](crate::syntax::module_name::ScopedName)
/// used in the current DAG. `target` is the canonical declaration identity and
/// remains unchanged when an include instance prefixes that lexical key.
#[derive(Debug, Clone)]
pub struct ImportedBinding {
    target: ResolvedDeclName,
    declared_type: DeclaredType,
    value: Option<RuntimeValue>,
}

impl ImportedBinding {
    /// Create a binding whose concrete value is already available.
    #[must_use]
    pub const fn with_value(
        target: ResolvedDeclName,
        declared_type: DeclaredType,
        value: RuntimeValue,
    ) -> Self {
        Self {
            target,
            declared_type,
            value: Some(value),
        }
    }

    /// Create a binding whose value is supplied by its canonical owner at runtime.
    #[must_use]
    pub const fn deferred(target: ResolvedDeclName, declared_type: DeclaredType) -> Self {
        Self {
            target,
            declared_type,
            value: None,
        }
    }

    /// Canonical declaration selected by this lexical binding.
    #[must_use]
    pub const fn target(&self) -> &ResolvedDeclName {
        &self.target
    }

    /// Declared type of the canonical target.
    #[must_use]
    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }

    /// Pre-evaluated value, when this compilation boundary has one.
    #[must_use]
    pub const fn value(&self) -> Option<&RuntimeValue> {
        self.value.as_ref()
    }

    /// Supply the canonical target's concrete value after its owning artifact evaluates.
    pub fn supply_value(&mut self, value: RuntimeValue) {
        self.value = Some(value);
    }

    /// Replace a provisional declared type with the canonical owner's resolved type.
    pub fn replace_declared_type(&mut self, declared_type: DeclaredType) {
        self.declared_type = declared_type;
    }
}
