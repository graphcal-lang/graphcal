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
//! - [`category`]: declaration kinds, import categories, and include projections.
//! - [`namespace`]: [`Namespace`](namespace::Namespace), the unit of collision
//!   checking and lookup.
//! - [`symbols`]: per-module declaration tables of [`Symbol`]s.
//! - [`scope`]: per-module import scopes (aliases and selective imports).
//! - [`exports`]: the public module surface.
//! - [`error`]: resolver errors.
//! - `tables`: per-namespace access to the declaration and import tables.
//! - `imports`, `projection`, `lookup`: registration of import/include edges,
//!   selective-include Static projection, and path resolution over
//!   [`ModuleResolver`].

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
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::{
    ResolvedDimName, ResolvedIndexName, ResolvedName, ResolvedStructTypeName,
};
use crate::syntax::ast::BindableVisibility;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::span::Span;

use self::error::{ModuleResolveError, NameCategory};
use self::scope::{
    ModuleAliasRole, ModuleAliasTarget, ModuleScope, PluginAliasTarget, declare_aliases,
};
use self::symbols::{ModuleSymbols, Symbol};
use self::tables::SymbolTables;

/// Project-wide module resolver backed by canonical [`DagId`] identities.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleResolver {
    modules: HashMap<DagId, ModuleSymbols>,
    scopes: HashMap<DagId, ModuleScope>,
}

impl ModuleResolver {
    /// Build a resolver from `(DagId, File)` pairs without registering any
    /// import scopes.
    ///
    /// Call [`Self::register_import`] / [`Self::register_include`] for each
    /// loader-resolved edge after all modules have been added.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] on duplicate modules or duplicate symbols.
    pub fn from_modules<'a>(
        modules: impl IntoIterator<Item = (DagId, &'a ast::File)>,
    ) -> Result<Self, ModuleResolveError> {
        let mut resolver = Self::default();
        for (owner, file) in modules {
            resolver.add_module(owner, &file.declarations)?;
        }
        Ok(resolver)
    }

    /// Add one module's declaration symbols.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateModule`] when `owner` already has
    /// a symbol table, or [`ModuleResolveError::DuplicateSymbol`] for duplicate
    /// namespace-local definitions inside the module.
    pub fn add_module(
        &mut self,
        owner: DagId,
        declarations: &[ast::Declaration],
    ) -> Result<(), ModuleResolveError> {
        if self.modules.contains_key(&owner) {
            return Err(ModuleResolveError::DuplicateModule { owner });
        }
        let symbols = ModuleSymbols::from_declarations(owner.clone(), declarations)?;
        let scope = self.scopes.entry(owner.clone()).or_default();
        declare_aliases(scope, &symbols, declarations)?;
        self.modules.insert(owner, symbols);
        Ok(())
    }

    /// Copy a source module's completed import scope onto an instantiated
    /// synthetic module with the same declaration body.
    ///
    /// Synthetic include modules are added before import edges are registered.
    /// Once the source scope is complete, copying it preserves selective public
    /// re-exports (including plots) without rebuilding or flattening symbols.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::UnknownModule`] if either module is absent.
    pub fn inherit_module_scope(
        &mut self,
        source: &DagId,
        instance: &DagId,
    ) -> Result<(), ModuleResolveError> {
        self.module_symbols(source)?;
        self.module_symbols(instance)?;
        let scope =
            self.scopes
                .get(source)
                .cloned()
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: source.clone(),
                })?;
        let target =
            self.scopes
                .get_mut(instance)
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: instance.clone(),
                })?;
        *target = scope;
        Ok(())
    }

    /// Role of one source-visible module alias in the owner's Term scope.
    #[must_use]
    pub(crate) fn module_alias_role(
        &self,
        owner: &DagId,
        alias: &ModuleAliasName,
    ) -> Option<ModuleAliasRole> {
        self.scopes
            .get(owner)?
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
                .scopes
                .get(&id)
                .and_then(|scope| scope.plugin_aliases.get(alias))
            {
                return Some(target);
            }
            current = id.parent();
        }
        None
    }

    /// Borrow all module symbol tables.
    #[must_use]
    pub const fn modules(&self) -> &HashMap<DagId, ModuleSymbols> {
        &self.modules
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

    /// Borrow all module import scopes.
    #[must_use]
    pub const fn scopes(&self) -> &HashMap<DagId, ModuleScope> {
        &self.scopes
    }

    /// The declaration a canonical name denotes, in its owner's own table.
    fn declared_symbol<Ns: SymbolTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Option<&Symbol<Ns, Ns::Declared>> {
        Ns::declared(self.modules.get(name.owner())?).get(&name.to_unowned_def_name())
    }

    /// The declaration a canonical name denotes.
    fn declaration<Ns: SymbolTables>(
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
    fn declared<Ns: SymbolTables>(
        &self,
        name: &ResolvedName<Ns>,
    ) -> Result<&Ns::Declared, ModuleResolveError> {
        self.declaration(name).map(Symbol::data)
    }

    fn module_symbols(&self, owner: &DagId) -> Result<&ModuleSymbols, ModuleResolveError> {
        self.modules
            .get(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })
    }

    /// Import scope registered for a module, if any.
    ///
    /// IDE consumers use this to map canonical owners back to the module
    /// aliases a file spelled in its imports.
    #[must_use]
    pub fn scope(&self, owner: &DagId) -> Option<&ModuleScope> {
        self.scopes.get(owner)
    }
    fn module_scope(&self, owner: &DagId) -> Result<&ModuleScope, ModuleResolveError> {
        self.scopes
            .get(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })
    }
}
