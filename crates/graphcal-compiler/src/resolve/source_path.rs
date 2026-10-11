//! Facts preserved by a source declaration-path lookup.

use crate::syntax::decl_name::DeclNameNamespace;

use super::category::DeclSymbolKind;
use super::scope::ModuleAliasRole;
use super::symbols::SymbolRef;

/// The source boundary used to reach a symbol, independent of its canonical owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePathBoundary {
    Local,
    ThroughAlias(ModuleAliasRole),
}

/// A declaration lookup together with the boundary traversed by its source path.
pub struct SourcePathResolution<'r> {
    pub symbol: SymbolRef<'r, DeclNameNamespace, DeclSymbolKind>,
    pub boundary: SourcePathBoundary,
}
