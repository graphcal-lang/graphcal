//! Per-module import scopes: module and plugin aliases plus selective imports.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::ast::BindableVisibility;
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::module_name::{ModuleAliasName, ModuleAliasNameNamespace};
use crate::syntax::names::{NameDef, NameNamespace};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::error::ModuleResolveError;
use super::symbols::{ModuleDeclSymbol, ModuleSymbolLookup, ModuleSymbols};

/// Visibility rule applied when a module path or symbol is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Reached from the owning module or its parent (for example, an implicit
    /// local `dag` callable): private symbols and modules are accessible.
    Local,
    /// Cross-module import/include boundary: only public target symbols are accessible.
    CrossModule,
}

impl Access {
    pub(super) const fn requires_public(self) -> bool {
        matches!(self, Self::CrossModule)
    }
}

/// Semantic role of a module alias introduced by an import or include.
///
/// An imported module names a reusable DAG blueprint, so its alias may be
/// invoked directly (`@alias(args)::out`) or used to reach a child DAG
/// (`@alias::child(args)::out`). An included instance is already instantiated;
/// its alias is only a namespace for the selected instance's members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleAliasRole {
    /// Alias of a reusable file or inline-DAG module introduced by `import`.
    ImportedDag,
    /// Namespace of an already-instantiated DAG introduced by `include`.
    IncludedInstance,
}

impl ModuleAliasRole {
    pub(super) const fn is_callable(self) -> bool {
        matches!(self, Self::ImportedDag)
    }
}

/// A resolved module alias in one module's import scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAliasTarget {
    pub(super) target: DagId,
    pub(super) span: Span,
    pub(super) access: Access,
    pub(super) role: ModuleAliasRole,
    pub(super) visibility: BindableVisibility,
}

impl ModuleAliasTarget {
    /// Canonical DAG/module targeted by the alias.
    #[must_use]
    pub const fn target(&self) -> &DagId {
        &self.target
    }

    /// Source span of the local alias name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.span
    }

    /// Visibility rule for names reached through this alias.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// Whether this alias names an imported DAG or an included instance.
    #[must_use]
    pub const fn role(&self) -> ModuleAliasRole {
        self.role
    }

    /// Whether this whole-DAG alias is reachable through an importing module.
    #[must_use]
    pub const fn visibility(&self) -> BindableVisibility {
        self.visibility
    }
}

/// An extern-plugin alias registered in one module's import scope by
/// `import plugin "path" as alias { ... }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAliasTarget {
    pub(super) path: crate::plugin_identity::PluginIdentity,
    pub(super) span: Span,
    pub(super) functions: HashMap<crate::syntax::function_name::FnName, Span>,
}

impl PluginAliasTarget {
    /// The plugin identity the alias refers to.
    #[must_use]
    pub(crate) const fn path(&self) -> &crate::plugin_identity::PluginIdentity {
        &self.path
    }

    /// Source span of the local alias name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.span
    }

    /// The extern functions declared under this alias, with their name spans.
    #[must_use]
    pub(crate) const fn functions(&self) -> &HashMap<crate::syntax::function_name::FnName, Span> {
        &self.functions
    }
}

/// A selective import binding for one namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSymbol<Ns: NameNamespace> {
    pub(super) resolved: ResolvedName<Ns>,
    pub(super) span: Span,
    pub(super) visibility: BindableVisibility,
}

impl<Ns: NameNamespace> ImportedSymbol<Ns> {
    pub(super) const fn new(
        resolved: ResolvedName<Ns>,
        span: Span,
        visibility: BindableVisibility,
    ) -> Self {
        Self {
            resolved,
            span,
            visibility,
        }
    }

    /// Canonical target identity of the imported symbol.
    #[must_use]
    pub(super) const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Source span of the local import name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.span
    }

    /// Visibility of this selective import when the importing module is itself imported.
    #[must_use]
    pub(super) const fn visibility(&self) -> BindableVisibility {
        self.visibility
    }
}

impl<Ns: NameNamespace> ModuleSymbolLookup<Ns> for ImportedSymbol<Ns> {
    fn resolved(&self) -> &ResolvedName<Ns> {
        self.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Import scope for a single module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleScope {
    pub(super) module_aliases: HashMap<ModuleAliasName, ModuleAliasTarget>,
    pub(super) plugin_aliases: HashMap<ModuleAliasName, PluginAliasTarget>,
    pub(super) selected_decls: HashMap<DeclName, ImportedSymbol<DeclNameNamespace>>,
    pub(super) selected_dimensions: HashMap<DimName, ImportedSymbol<DimNameNamespace>>,
    pub(super) selected_units: HashMap<UnitName, ImportedSymbol<UnitNameNamespace>>,
    pub(super) selected_struct_types:
        HashMap<StructTypeName, ImportedSymbol<StructTypeNameNamespace>>,
    pub(super) selected_indexes: HashMap<IndexName, ImportedSymbol<IndexNameNamespace>>,
    pub(super) selected_constructors:
        HashMap<ConstructorName, ImportedSymbol<ConstructorNameNamespace>>,
}

impl ModuleScope {
    /// Module aliases introduced by whole-module imports/includes.
    #[must_use]
    pub const fn module_aliases(&self) -> &HashMap<ModuleAliasName, ModuleAliasTarget> {
        &self.module_aliases
    }

    /// Extern-plugin aliases introduced by `import plugin` declarations.
    #[must_use]
    pub const fn plugin_aliases(&self) -> &HashMap<ModuleAliasName, PluginAliasTarget> {
        &self.plugin_aliases
    }
}

#[derive(Debug, Clone)]
pub(super) enum ImportAddition {
    ModuleAlias {
        alias: Spanned<ModuleAliasName>,
        target: DagId,
        access: Access,
        role: ModuleAliasRole,
        visibility: BindableVisibility,
    },
    Decl {
        local: Spanned<DeclName>,
        target: ResolvedDeclName,
        visibility: BindableVisibility,
    },
    Dimension {
        local: Spanned<DimName>,
        target: ResolvedDimName,
        visibility: BindableVisibility,
    },
    Unit {
        local: Spanned<UnitName>,
        target: ResolvedUnitName,
        visibility: BindableVisibility,
    },
    StructType {
        local: Spanned<StructTypeName>,
        target: ResolvedStructTypeName,
        visibility: BindableVisibility,
    },
    Index {
        local: Spanned<IndexName>,
        target: ResolvedIndexName,
        visibility: BindableVisibility,
    },
    Constructor {
        local: Spanned<ConstructorName>,
        target: ResolvedConstructorName,
        visibility: BindableVisibility,
    },
}

impl ModuleScope {
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive typed dispatch installs every import-surface category"
    )]
    pub(super) fn apply_addition(
        &mut self,
        owner: &DagId,
        addition: ImportAddition,
    ) -> Result<(), ModuleResolveError> {
        match addition {
            ImportAddition::ModuleAlias {
                alias,
                target,
                access,
                role,
                visibility,
            } => {
                // Module aliases and plugin aliases share one qualifier
                // namespace: `alias.name` must have a single meaning.
                if let Some(first) = self.plugin_aliases.get(&alias.value) {
                    return Err(ModuleResolveError::DuplicateImportName {
                        owner: owner.clone(),
                        namespace: ModuleAliasNameNamespace::DISPLAY_NAME,
                        name: alias.value.to_string(),
                        first: first.span(),
                        duplicate: alias.span,
                    });
                }
                insert_module_alias(
                    owner,
                    &mut self.module_aliases,
                    alias,
                    target,
                    access,
                    role,
                    visibility,
                    ModuleAliasNameNamespace::DISPLAY_NAME,
                )
            }
            ImportAddition::Decl {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_decls,
                local,
                target,
                visibility,
                DeclNameNamespace::DISPLAY_NAME,
            ),
            ImportAddition::Dimension {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_dimensions,
                local,
                target,
                visibility,
                DimNameNamespace::DISPLAY_NAME,
            ),
            ImportAddition::Unit {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_units,
                local,
                target,
                visibility,
                UnitNameNamespace::DISPLAY_NAME,
            ),
            ImportAddition::StructType {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_struct_types,
                local,
                target,
                visibility,
                StructTypeNameNamespace::DISPLAY_NAME,
            ),
            ImportAddition::Index {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_indexes,
                local,
                target,
                visibility,
                IndexNameNamespace::DISPLAY_NAME,
            ),
            ImportAddition::Constructor {
                local,
                target,
                visibility,
            } => insert_imported_symbol(
                owner,
                &mut self.selected_constructors,
                local,
                target,
                visibility,
                ConstructorNameNamespace::DISPLAY_NAME,
            ),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "module aliases preserve target, access, role, visibility, and diagnostic context"
)]
fn insert_module_alias(
    owner: &DagId,
    map: &mut HashMap<ModuleAliasName, ModuleAliasTarget>,
    alias: Spanned<ModuleAliasName>,
    target: DagId,
    access: Access,
    role: ModuleAliasRole,
    visibility: BindableVisibility,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(&alias.value) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: namespace_name,
            name: alias.value.to_string(),
            first: first.span(),
            duplicate: alias.span,
        });
    }
    map.insert(
        alias.value,
        ModuleAliasTarget {
            target,
            span: alias.span,
            access,
            role,
            visibility,
        },
    );
    Ok(())
}

/// Register the plugin aliases declared by a module's `import plugin`
/// declarations into its scope.
///
/// Rejects duplicate aliases across plugin imports and duplicate function
/// names inside one plugin block. Collisions with module-import aliases are
/// caught when the module alias registers (imports register after modules).
pub(super) fn register_plugin_imports(
    owner: &DagId,
    scope: &mut ModuleScope,
    symbols: &ModuleSymbols,
    declarations: &[ast::Declaration],
) -> Result<(), ModuleResolveError> {
    for decl in declarations {
        let ast::DeclKind::PluginImport(plugin) = &decl.kind else {
            continue;
        };
        let alias_atom = plugin.alias.value.atom();
        let local_term_span = symbols
            .decls
            .get(&NameDef::classify(alias_atom.clone()))
            .map(ModuleDeclSymbol::span)
            .or_else(|| {
                symbols
                    .constructors
                    .get(&NameDef::classify(alias_atom.clone()))
                    .map(ModuleSymbolLookup::span)
            });
        if let Some(first) = local_term_span.or_else(|| {
            scope
                .plugin_aliases
                .get(&plugin.alias.value)
                .map(PluginAliasTarget::span)
        }) {
            return Err(ModuleResolveError::DuplicateImportName {
                owner: owner.clone(),
                namespace: "Term",
                name: plugin.alias.value.to_string(),
                first,
                duplicate: plugin.alias.span,
            });
        }
        let mut functions = HashMap::new();
        for function in &plugin.functions {
            if let Some(first) = functions.insert(function.name.value.clone(), function.name.span) {
                return Err(ModuleResolveError::DuplicateSymbol {
                    owner: owner.clone(),
                    namespace: crate::syntax::function_name::FnNameNamespace::DISPLAY_NAME,
                    name: function.name.value.to_string(),
                    first,
                    duplicate: function.name.span,
                });
            }
        }
        scope.plugin_aliases.insert(
            plugin.alias.value.clone(),
            PluginAliasTarget {
                path: crate::plugin_identity::PluginIdentity::resolve(
                    &plugin.path.value,
                    owner.package(),
                ),
                span: plugin.alias.span,
                functions,
            },
        );
    }
    Ok(())
}

fn insert_imported_symbol<Ns: NameNamespace>(
    owner: &DagId,
    map: &mut HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    local: Spanned<NameDef<Ns>>,
    target: ResolvedName<Ns>,
    visibility: BindableVisibility,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(&local.value) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: namespace_name,
            name: local.value.to_string(),
            first: first.span(),
            duplicate: local.span,
        });
    }
    map.insert(
        local.value,
        ImportedSymbol::new(target, local.span, visibility),
    );
    Ok(())
}
