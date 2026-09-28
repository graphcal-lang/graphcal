//! Per-module import scopes: module and plugin aliases plus selective imports.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::ast::{BindableVisibility, ImportKind, ModulePath};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::error::ModuleResolveError;
use super::namespace::{Namespace, Occupant};
use super::symbols::{ModuleSymbols, Symbol, occupant_in};

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
    path: crate::plugin_identity::PluginIdentity,
    span: Span,
    functions: HashMap<crate::syntax::function_name::FnName, Span>,
}

impl PluginAliasTarget {
    /// The plugin identity the alias refers to.
    #[must_use]
    pub(crate) const fn path(&self) -> &crate::plugin_identity::PluginIdentity {
        &self.path
    }

    /// The extern functions declared under this alias, with their name spans.
    #[must_use]
    pub(crate) const fn functions(&self) -> &HashMap<crate::syntax::function_name::FnName, Span> {
        &self.functions
    }
}

/// Import scope for a single module.
///
/// Selective imports are [`Symbol`]s without a payload: the local name binds
/// the target's canonical identity, whose payload stays with its declaration.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleScope {
    pub(super) module_aliases: HashMap<ModuleAliasName, ModuleAliasTarget>,
    pub(super) plugin_aliases: HashMap<ModuleAliasName, PluginAliasTarget>,
    pub(super) selected_decls: HashMap<DeclName, Symbol<DeclNameNamespace>>,
    pub(super) selected_dimensions: HashMap<DimName, Symbol<DimNameNamespace>>,
    pub(super) selected_units: HashMap<UnitName, Symbol<UnitNameNamespace>>,
    pub(super) selected_struct_types: HashMap<StructTypeName, Symbol<StructTypeNameNamespace>>,
    pub(super) selected_indexes: HashMap<IndexName, Symbol<IndexNameNamespace>>,
    pub(super) selected_constructors: HashMap<ConstructorName, Symbol<ConstructorNameNamespace>>,
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

    /// The import binding occupying `(namespace, atom)`, if any.
    pub(super) fn occupant(&self, namespace: Namespace, atom: &NameAtom) -> Option<Occupant> {
        match namespace {
            Namespace::Term => occupant_in(&self.selected_decls, atom)
                .or_else(|| occupant_in(&self.selected_constructors, atom))
                .or_else(|| {
                    let alias = ModuleAliasName::classify(atom.clone());
                    self.module_aliases
                        .get(&alias)
                        .map(|target| Occupant {
                            span: target.span,
                            visibility: target.visibility,
                            surface: None,
                        })
                        .or_else(|| {
                            self.plugin_aliases.get(&alias).map(|target| Occupant {
                                span: target.span,
                                visibility: BindableVisibility::Private,
                                surface: None,
                            })
                        })
                }),
            Namespace::Static => occupant_in(&self.selected_dimensions, atom)
                .or_else(|| occupant_in(&self.selected_struct_types, atom))
                .or_else(|| occupant_in(&self.selected_indexes, atom)),
            Namespace::Unit => occupant_in(&self.selected_units, atom),
        }
    }

    /// Install the names one `import` / `include` edge introduces.
    ///
    /// Each name must claim a free slot of the owner's collision unit: local
    /// declarations, earlier imports, and the edge's own earlier names all
    /// occupy it.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateImportName`] for the first
    /// addition whose slot is already occupied.
    pub(super) fn add_imports(
        &mut self,
        symbols: &ModuleSymbols,
        additions: Vec<ImportAddition>,
    ) -> Result<(), ModuleResolveError> {
        for addition in additions {
            let namespace = addition.target.namespace();
            let local = &addition.local;
            if let Some(first) = symbols
                .occupant(namespace, &local.value)
                .or_else(|| self.occupant(namespace, &local.value))
            {
                return Err(ModuleResolveError::DuplicateImportName {
                    owner: symbols.owner().clone(),
                    namespace,
                    name: local.value.clone(),
                    first: first.span,
                    duplicate: local.span,
                });
            }
            self.install(addition);
        }
        Ok(())
    }

    fn install(&mut self, addition: ImportAddition) {
        fn select<Ns: NameNamespace>(
            table: &mut HashMap<NameDef<Ns>, Symbol<Ns>>,
            local: Spanned<NameAtom>,
            target: ResolvedName<Ns>,
            visibility: BindableVisibility,
        ) {
            table.insert(
                NameDef::classify(local.value),
                Symbol::new(target, visibility, local.span, ()),
            );
        }

        let ImportAddition {
            local,
            visibility,
            target,
        } = addition;
        match target {
            ImportTarget::ModuleAlias {
                target,
                access,
                role,
            } => {
                self.module_aliases.insert(
                    ModuleAliasName::classify(local.value),
                    ModuleAliasTarget {
                        target,
                        span: local.span,
                        access,
                        role,
                        visibility,
                    },
                );
            }
            ImportTarget::Decl(target) => {
                select(&mut self.selected_decls, local, target, visibility);
            }
            ImportTarget::Dimension(target) => {
                select(&mut self.selected_dimensions, local, target, visibility);
            }
            ImportTarget::Unit(target) => {
                select(&mut self.selected_units, local, target, visibility);
            }
            ImportTarget::StructType(target) => {
                select(&mut self.selected_struct_types, local, target, visibility);
            }
            ImportTarget::Index(target) => {
                select(&mut self.selected_indexes, local, target, visibility);
            }
            ImportTarget::Constructor(target) => {
                select(&mut self.selected_constructors, local, target, visibility);
            }
        }
    }
}

/// One local name an `import` / `include` edge introduces.
#[derive(Debug, Clone)]
pub(super) struct ImportAddition {
    /// The local spelling and its source span.
    pub(super) local: Spanned<NameAtom>,
    /// Visibility of the local name when the owner is itself imported.
    pub(super) visibility: BindableVisibility,
    /// What the local name denotes.
    pub(super) target: ImportTarget,
}

/// What one import-introduced local name denotes.
#[derive(Debug, Clone)]
pub(super) enum ImportTarget {
    ModuleAlias {
        target: DagId,
        access: Access,
        role: ModuleAliasRole,
    },
    Decl(ResolvedDeclName),
    Dimension(ResolvedDimName),
    Unit(ResolvedUnitName),
    StructType(ResolvedStructTypeName),
    Index(ResolvedIndexName),
    Constructor(ResolvedConstructorName),
}

impl ImportTarget {
    /// The collision unit the local name occupies.
    pub(super) const fn namespace(&self) -> Namespace {
        match self {
            Self::ModuleAlias { .. } | Self::Decl(_) | Self::Constructor(_) => Namespace::Term,
            Self::Dimension(_) | Self::StructType(_) | Self::Index(_) => Namespace::Static,
            Self::Unit(_) => Namespace::Unit,
        }
    }
}

/// The module alias an `import` / module-form `include` binds: the explicit
/// `as` name, or else the path's last segment.
pub(super) fn module_alias(
    path: &ModulePath,
    alias: Option<&Spanned<ModuleAliasName>>,
) -> Spanned<ModuleAliasName> {
    alias.cloned().unwrap_or_else(|| {
        Spanned::new(
            ModuleAliasName::classify(path.leaf().name.atom().clone()),
            path.leaf().span,
        )
    })
}

/// Claim the Term slots of the aliases a module's declarations introduce and
/// register its plugin aliases.
///
/// Every `import` / module-form `include` alias (explicit `as` or the path's
/// last segment) and every `import plugin` alias occupies the declaring
/// module's Term namespace whether or not the loader resolves its target to a
/// registered module, so an alias and a local declaration of the same name are
/// rejected uniformly. The one exception is an alias that re-spells the local
/// `dag` its single-segment path names (`include d(...)`, `import d`): it
/// shares that declaration's name rather than introducing a second one.
///
/// Import and include aliases are installed later, when their loader-resolved
/// edge registers (see [`ModuleScope::add_imports`]); plugin aliases have no
/// edge and are installed here.
///
/// # Errors
///
/// Returns [`ModuleResolveError::DuplicateImportName`] when an alias collides
/// with a local declaration or an earlier plugin alias, and
/// [`ModuleResolveError::DuplicatePluginFunction`] for a function declared twice in
/// one plugin block.
pub(super) fn declare_aliases(
    scope: &mut ModuleScope,
    symbols: &ModuleSymbols,
    declarations: &[ast::Declaration],
) -> Result<(), ModuleResolveError> {
    let owner = symbols.owner();
    for decl in declarations {
        let (alias, respells_path) = match &decl.kind {
            ast::DeclKind::Import(ast::ImportDecl::Module { path, alias, .. })
            | ast::DeclKind::Include(ast::IncludeDecl {
                path,
                kind: ImportKind::Module { alias },
                ..
            }) => {
                let alias = module_alias(path, alias.as_ref());
                let respells_path =
                    path.segments().len() == 1 && path.leaf().name.atom() == alias.value.atom();
                (alias, respells_path)
            }
            ast::DeclKind::PluginImport(plugin) => (plugin.alias.clone(), false),
            _ => continue,
        };
        let plugin_alias = match &decl.kind {
            ast::DeclKind::PluginImport(_) => scope.plugin_aliases.get(&alias.value),
            _ => None,
        };
        let names_local_dag = respells_path && symbols.declares_dag(alias.value.atom());
        if let Some(first) = symbols
            .occupant(Namespace::Term, alias.value.atom())
            .filter(|_| !names_local_dag)
            .map(|occupant| occupant.span)
            .or_else(|| plugin_alias.map(|target| target.span))
        {
            return Err(ModuleResolveError::DuplicateImportName {
                owner: owner.clone(),
                namespace: Namespace::Term,
                name: alias.value.atom().clone(),
                first,
                duplicate: alias.span,
            });
        }
        if let ast::DeclKind::PluginImport(plugin) = &decl.kind {
            let mut functions = HashMap::new();
            for function in &plugin.functions {
                if let Some(first) =
                    functions.insert(function.name.value.clone(), function.name.span)
                {
                    return Err(ModuleResolveError::DuplicatePluginFunction {
                        owner: owner.clone(),
                        function: function.name.value.clone(),
                        first,
                        duplicate: function.name.span,
                    });
                }
            }
            scope.plugin_aliases.insert(
                alias.value,
                PluginAliasTarget {
                    path: crate::plugin_identity::PluginIdentity::resolve(
                        &plugin.path.value,
                        owner.package(),
                    ),
                    span: alias.span,
                    functions,
                },
            );
        }
    }
    Ok(())
}
