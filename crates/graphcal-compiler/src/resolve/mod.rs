//! Module-aware symbol tables backing HIR name resolution.
//!
//! The resolver builds typed symbol tables for loaded DAG/module identities and
//! resolves syntactic [`NamePath`](crate::syntax::names::NamePath) /
//! [`IdentPath`](crate::syntax::ast::IdentPath) references to canonical
//! [`ResolvedName`] values. It consumes the
//! desugared AST, so it sits downstream of both [`crate::syntax`] and
//! [`crate::desugar`].
//!
//! The important invariant is that source spelling is used only to look up a
//! scoped symbol or DAG-module binding. Imported aliases may name a file root or
//! inline DAG directly, and local DAGs may qualify their children. Every
//! successful lookup carries the canonical [`DagId`] owner, not textual path
//! conventions.
//!
//! A resolver is built in three typed stages
//! ([`builder::SymbolTables`] → [`builder::ScopeBuilder`] →
//! [`ModuleResolver`]); only the last one answers queries, and it is
//! immutable.
//!
//! - [`category`]: declaration kinds, import categories, and include projections.
//! - [`namespace`]: [`Namespace`](namespace::Namespace), the unit of collision
//!   checking and lookup.
//! - [`symbols`]: per-module declaration tables of [`Symbol`](symbols::Symbol)s.
//! - [`scope`]: per-module import scopes (aliases and selective imports).
//! - [`builder`]: the typestate that builds a [`ModuleResolver`].
//! - [`exports`]: the public module surface.
//! - [`error`]: resolver errors.
//! - `tables`: per-namespace access to the declaration and import tables.
//! - `imports`, `projection`, `lookup`: registration of import/include edges,
//!   selective-include Static projection, and path resolution over
//!   [`ModuleResolver`].

pub mod builder;
pub mod category;
pub mod error;
pub mod exports;
mod imports;
mod lookup;
pub(crate) mod mint;
pub mod module_table;
pub mod namespace;
pub mod prelude;
mod projection;
pub mod reserved_name;
pub mod scope;
mod source_path;
pub mod symbols;
pub mod tables;
#[cfg(test)]
mod tests;

use crate::dag_id::DagId;
use crate::resolved_name::ResolvedName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::NameDef;

use self::error::ModuleResolveError;
pub use self::module_table::ModuleHandle;
use self::module_table::ModuleTable;
use self::scope::{ModuleAliasRole, ModuleAliasTarget, ModuleScope, PluginAliasTarget};
pub(crate) use self::source_path::SourcePathBoundary;
use self::symbols::{ModuleSymbols, SymbolRef};
use self::tables::NamespaceTables;

/// One module the resolver knows: its own declarations and its import scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntry {
    symbols: ModuleSymbols,
    scope: ModuleScope,
}

impl ModuleEntry {
    /// The module's own declarations.
    #[must_use]
    pub const fn symbols(&self) -> &ModuleSymbols {
        &self.symbols
    }

    /// The module's import scope (aliases and selective imports).
    #[must_use]
    pub const fn scope(&self) -> &ModuleScope {
        &self.scope
    }
}

/// One module of a [`ModuleResolver`], reached through its [`ModuleHandle`].
///
/// Every query is total: the module exists by construction of the handle.
#[derive(Debug, Clone, Copy)]
pub struct ModuleRef<'r> {
    owner: &'r DagId,
    entry: &'r ModuleEntry,
}

impl<'r> ModuleRef<'r> {
    /// The module's canonical identity.
    #[must_use]
    pub const fn owner(self) -> &'r DagId {
        self.owner
    }

    /// The module's own declarations.
    #[must_use]
    pub const fn symbols(self) -> &'r ModuleSymbols {
        &self.entry.symbols
    }

    /// Definition or import span occupying `(namespace, name)` in this
    /// module, if any binding occupies it.
    #[must_use]
    pub(crate) fn visible_span(
        self,
        namespace: crate::resolve::namespace::Namespace,
        name: &crate::syntax::names::NameAtom,
    ) -> Option<crate::syntax::span::Span> {
        self.entry
            .symbols
            .occupant(namespace, name)
            .or_else(|| self.entry.scope.occupant(namespace, name))
            .map(|occupant| occupant.span)
    }
}

/// Project-wide module resolver backed by canonical [`DagId`] identities.
///
/// Built by [`builder::SymbolTables`] and [`builder::ScopeBuilder::freeze`];
/// immutable once built.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleResolver {
    modules: ModuleTable<ModuleEntry>,
}

impl ModuleResolver {
    /// The handle of the module `owner`, if the resolver knows it.
    ///
    /// This is the one lookup by identity; every query through the handle is
    /// total.
    #[must_use]
    pub fn module_handle(&self, owner: &DagId) -> Option<ModuleHandle> {
        self.modules.handle(owner)
    }

    /// The module this resolver issued `handle` for.
    #[must_use]
    pub fn module(&self, handle: ModuleHandle) -> ModuleRef<'_> {
        ModuleRef {
            owner: self.modules.owner(handle),
            entry: self.modules.entry(handle),
        }
    }

    /// Role of one source-visible module alias in the owner's Term scope.
    #[must_use]
    pub(crate) fn module_alias_role(
        &self,
        owner: &DagId,
        alias: &ModuleAliasName,
    ) -> Option<ModuleAliasRole> {
        self.modules
            .get(owner)?
            .scope
            .module_aliases
            .get(alias)
            .map(ModuleAliasTarget::role)
    }

    /// Look up an extern-plugin alias visible from `owner`.
    ///
    /// Plugin imports are file-level declarations; inline `dag` children see
    /// the enclosing file's aliases, so the lookup walks up the owner chain.
    #[must_use]
    pub(crate) fn plugin_alias(
        &self,
        owner: &DagId,
        alias: &ModuleAliasName,
    ) -> Option<&PluginAliasTarget> {
        let mut current = Some(owner.clone());
        while let Some(id) = current {
            if let Some(target) = self
                .modules
                .get(&id)
                .and_then(|entry| entry.scope.plugin_aliases.get(alias))
            {
                return Some(target);
            }
            current = id.parent();
        }
        None
    }

    /// The declarations of one module, if the resolver knows it.
    #[must_use]
    pub fn symbols(&self, owner: &DagId) -> Option<&ModuleSymbols> {
        self.modules.get(owner).map(ModuleEntry::symbols)
    }

    /// The declaration of `name` in `owner`'s own table, regardless of its
    /// visibility.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::UnknownModule`] when the resolver does
    /// not know `owner`, or [`ModuleResolveError::UnknownName`] when `owner`
    /// declares no `name` in that namespace.
    pub fn declaration<Ns: NamespaceTables>(
        &self,
        owner: &DagId,
        name: &NameDef<Ns>,
    ) -> Result<SymbolRef<'_, Ns, Ns::Declared>, ModuleResolveError> {
        Ns::declared(self.module_symbols(owner)?)
            .get(name)
            .map(SymbolRef::new)
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: owner.clone(),
                category: error::NameCategory::Table(Ns::TABLE),
                name: name.atom().clone(),
            })
    }

    /// The declaration a canonical identity denotes, in its owner's own
    /// table, with the facts the resolver knows about it.
    ///
    /// Resolution already returns these facts with the name; this lookup is
    /// for identities carried past resolution (for example in HIR).
    #[must_use]
    pub fn symbol<Ns: NamespaceTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Option<SymbolRef<'_, Ns, Ns::Declared>> {
        Ns::declared(self.symbols(name.owner())?)
            .get(&name.to_unowned_def_name())
            .map(SymbolRef::new)
    }

    fn entry(&self, owner: &DagId) -> Result<&ModuleEntry, ModuleResolveError> {
        self.modules
            .get(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })
    }

    fn entry_mut(&mut self, owner: &DagId) -> Result<&mut ModuleEntry, ModuleResolveError> {
        self.modules
            .get_mut(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })
    }

    fn module_symbols(&self, owner: &DagId) -> Result<&ModuleSymbols, ModuleResolveError> {
        self.entry(owner).map(ModuleEntry::symbols)
    }

    fn module_scope(&self, owner: &DagId) -> Result<&ModuleScope, ModuleResolveError> {
        self.entry(owner).map(ModuleEntry::scope)
    }
}
