//! Module-aware type-resolution context for one DAG body.

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::resolve::ModuleResolver;
use crate::resolve::symbols::ModuleSymbols;
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;

use super::model::ProjectTypeStore;

/// Module-aware type-resolution context for one DAG body.
///
/// It carries the owner's own symbol table, looked up once when the context
/// is created, so the owner's declarations are read without a lookup.
#[derive(Debug, Clone, Copy)]
pub struct ModuleTypeContext<'a> {
    pub(in crate::tir::typed) owner: &'a crate::dag_id::DagId,
    pub(in crate::tir::typed) resolver: &'a ModuleResolver,
    symbols: &'a ModuleSymbols,
    pub(in crate::tir::typed) types: &'a ProjectTypeStore,
}

impl<'a> ModuleTypeContext<'a> {
    /// The context of the DAG `owner`, which `resolver` must know.
    ///
    /// # Errors
    ///
    /// Returns an internal error when `resolver` has no symbol table for
    /// `owner`.
    pub(crate) fn try_new(
        owner: &'a crate::dag_id::DagId,
        resolver: &'a ModuleResolver,
        types: &'a ProjectTypeStore,
        src: SourceId,
    ) -> Result<Self, SemanticError> {
        let symbols = resolver.symbols(owner).ok_or_else(|| {
            SemanticError::internal_error(
                format!("module symbol table missing for DAG `{owner}`"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        Ok(Self {
            owner,
            resolver,
            symbols,
            types,
        })
    }

    #[must_use]
    pub const fn owner(self) -> &'a crate::dag_id::DagId {
        self.owner
    }

    /// The owner's own symbol table.
    #[must_use]
    pub(in crate::tir::typed) const fn symbols(self) -> &'a ModuleSymbols {
        self.symbols
    }
}
