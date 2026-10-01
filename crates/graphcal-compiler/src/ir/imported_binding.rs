//! Canonical imported constant bindings at the HIR/TIR boundary.
//!
//! HIR records only a lexical binding's canonical target. Only constants can
//! be imported as values: every import site binds a resolver export whose
//! category is a constant declaration. Checked type facts are attached when
//! HIR becomes TIR; compile-time values remain in owner execution-fact stores.

use crate::resolved_name::ResolvedDeclName;
use crate::semantic::checked_type::CheckedType;

/// The checked declared type of an imported constant, as published by the
/// module that defines it, or `None` when no checked interface is available.
pub type ImportedConstantTypes<'a> = dyn Fn(&ResolvedDeclName) -> Option<CheckedType> + 'a;

/// Checked interface metadata attached to one source-visible imported
/// constant.
///
/// Compile-time values are facts of the defining body's checked constant pool,
/// not mutable fields on this binding. This keeps a published body independent
/// of the importer that happens to use it.
#[derive(Debug, Clone)]
pub struct ImportedBinding {
    target: ResolvedDeclName,
    declared_type: CheckedType,
}

impl ImportedBinding {
    /// Preserve the checked interface of the canonical constant.
    #[must_use]
    pub const fn new(target: ResolvedDeclName, declared_type: CheckedType) -> Self {
        Self {
            target,
            declared_type,
        }
    }

    /// Canonical constant declaration selected by this lexical binding.
    #[must_use]
    pub const fn target(&self) -> &ResolvedDeclName {
        &self.target
    }

    /// Declared type of the canonical target.
    #[must_use]
    pub const fn declared_type(&self) -> &CheckedType {
        &self.declared_type
    }
}
