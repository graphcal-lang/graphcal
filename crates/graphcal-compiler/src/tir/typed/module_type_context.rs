//! Module-aware type-resolution context for one DAG body.

use crate::resolve::symbols::ModuleSymbols;
use crate::resolve::{ModuleHandle, ModuleResolver};

use super::model::ProjectTypeStore;

/// Module-aware type-resolution context for one DAG body.
///
/// It carries the owner's own symbol table, read once through the owner's
/// module handle when the context is created, so the owner's declarations are
/// read without a lookup.
#[derive(Debug, Clone, Copy)]
pub struct ModuleTypeContext<'a> {
    pub(in crate::tir::typed) owner: &'a crate::dag_id::DagId,
    pub(in crate::tir::typed) resolver: &'a ModuleResolver,
    symbols: &'a ModuleSymbols,
    pub(in crate::tir::typed) types: &'a ProjectTypeStore,
}

impl<'a> ModuleTypeContext<'a> {
    /// The context of the DAG `resolver` issued `module` for.
    #[must_use]
    pub(crate) fn new(
        module: ModuleHandle,
        resolver: &'a ModuleResolver,
        types: &'a ProjectTypeStore,
    ) -> Self {
        let module = resolver.module(module);
        Self {
            owner: module.owner(),
            resolver,
            symbols: module.symbols(),
            types,
        }
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
