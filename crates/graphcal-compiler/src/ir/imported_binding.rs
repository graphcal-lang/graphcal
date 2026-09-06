//! Canonical imported value bindings at the HIR/TIR boundary.
//!
//! HIR records only a lexical binding's canonical target. Checked type facts
//! are attached when HIR becomes TIR; compile-time values remain in owner
//! execution-fact stores.

use crate::registry::declared_type::DeclaredType;
use crate::syntax::decl_name::ResolvedDeclName;

/// Canonical target of one source-visible import at the HIR boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirImportedBinding {
    target: ResolvedDeclName,
}

impl HirImportedBinding {
    /// Record the canonical declaration selected by a lexical import.
    #[must_use]
    pub const fn new(target: ResolvedDeclName) -> Self {
        Self { target }
    }

    /// Canonical declaration selected by this lexical binding.
    #[must_use]
    pub const fn target(&self) -> &ResolvedDeclName {
        &self.target
    }
}

/// Whether an imported value comes from checked constants or a runtime frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedValueKind {
    Constant,
    Runtime,
}

/// Checked interface metadata attached to one source-visible imported binding.
///
/// Compile-time values are facts of the defining body's checked constant pool,
/// not mutable fields on this binding. This keeps a published body independent
/// of the importer that happens to use it.
#[derive(Debug, Clone)]
pub struct ImportedBinding {
    target: ResolvedDeclName,
    declared_type: DeclaredType,
    kind: ImportedValueKind,
}

impl ImportedBinding {
    /// Preserve the checked interface and the authoritative declaration category.
    #[must_use]
    pub const fn new(
        target: ResolvedDeclName,
        declared_type: DeclaredType,
        kind: ImportedValueKind,
    ) -> Self {
        Self {
            target,
            declared_type,
            kind,
        }
    }

    /// Select a checked constant pool or a deferred runtime environment.
    #[must_use]
    pub const fn kind(&self) -> ImportedValueKind {
        self.kind
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
}
