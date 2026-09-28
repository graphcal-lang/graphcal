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
//! - [`symbols`]: per-module declaration tables of [`Symbol`]s.
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
pub mod namespace;
mod projection;
pub mod scope;
pub mod symbols;
mod tables;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::resolved_name::{
    ResolvedDimName, ResolvedIndexName, ResolvedName, ResolvedStructTypeName,
};
use crate::syntax::ast::BindableVisibility;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::span::Span;

use self::error::{ModuleResolveError, NameCategory};
use self::scope::{ModuleAliasRole, ModuleAliasTarget, ModuleScope, PluginAliasTarget};
use self::symbols::{ModuleSymbols, Symbol};
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

/// Project-wide module resolver backed by canonical [`DagId`] identities.
///
/// Built by [`builder::SymbolTables`] and [`builder::ScopeBuilder::freeze`];
/// immutable once built.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleResolver {
    modules: HashMap<DagId, ModuleEntry>,
}

impl ModuleResolver {
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

    /// Visibility of a canonical dimension declaration.
    #[must_use]
    pub(crate) fn dimension_visibility(
        &self,
        name: &ResolvedDimName,
    ) -> Option<BindableVisibility> {
        self.declared_symbol(name).map(Symbol::visibility)
    }

    /// Visibility of a canonical index declaration.
    #[must_use]
    pub(crate) fn index_visibility(&self, name: &ResolvedIndexName) -> Option<BindableVisibility> {
        self.declared_symbol(name).map(Symbol::visibility)
    }

    /// Visibility of a canonical nominal type declaration.
    #[must_use]
    pub(crate) fn struct_type_visibility(
        &self,
        name: &ResolvedStructTypeName,
    ) -> Option<BindableVisibility> {
        self.declared_symbol(name).map(Symbol::visibility)
    }

    /// Source span of a canonical nominal type declaration.
    #[must_use]
    pub(crate) fn struct_type_span(&self, name: &ResolvedStructTypeName) -> Option<Span> {
        self.declared_symbol(name).map(Symbol::span)
    }

    /// The declaration a canonical name denotes, in its owner's own table.
    fn declared_symbol<Ns: NamespaceTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Option<&Symbol<Ns, Ns::Declared>> {
        Ns::declared(self.symbols(name.owner())?).get(&name.to_unowned_def_name())
    }

    /// The declaration a canonical name denotes.
    fn declaration<Ns: NamespaceTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Result<&Symbol<Ns, Ns::Declared>, ModuleResolveError> {
        Ns::declared(self.module_symbols(name.owner())?)
            .get(&name.to_unowned_def_name())
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: name.owner().clone(),
                category: NameCategory::Table(Ns::TABLE),
                name: name.atom().clone(),
            })
    }

    /// The payload of the declaration a canonical name denotes.
    fn declared<Ns: NamespaceTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Result<&Ns::Declared, ModuleResolveError> {
        self.declaration(name).map(Symbol::data)
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
