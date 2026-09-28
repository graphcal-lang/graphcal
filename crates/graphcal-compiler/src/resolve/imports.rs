//! Registration of loader-resolved `import` / `include` edges into scopes.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::ResolvedName;
use crate::syntax::ast::{BindableVisibility, ImportItem, ImportKind, ModulePath, Visibility};
use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::index_name::IndexNameNamespace;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Spanned;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::ModuleResolver;
use super::category::{ExportedImportItemKind, include_projection};
use super::error::ModuleResolveError;
use super::scope::{Access, ImportAddition, ImportTarget, ModuleAliasRole, module_alias};
use super::symbols::Symbol;
use super::tables::SymbolTables;

impl ModuleResolver {
    /// Register one loader-resolved `import` edge in `owner`'s scope.
    ///
    /// `import` comes from the source AST. `target` is the canonical module
    /// identity chosen by the loader for its path. This function never
    /// re-resolves filesystem paths.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] if either module is unknown, an imported
    /// item is missing/private, or the import introduces a duplicate local name.
    pub fn register_import(
        &mut self,
        owner: &DagId,
        import: &ast::ImportDecl,
        target: &DagId,
    ) -> Result<(), ModuleResolveError> {
        self.register_import_with_access(
            owner,
            import.path(),
            ImportTail::of_import(import),
            target,
            ModuleAliasRole::ImportedDag,
        )
    }

    /// Register one loader-resolved `include` edge in `owner`'s scope.
    ///
    /// Instantiated includes embed the dependency DAG body, but the source-level
    /// names introduced by the include are still a cross-module boundary and
    /// must preserve public visibility.
    pub fn register_include(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        kind: &ImportKind,
        target: &DagId,
    ) -> Result<(), ModuleResolveError> {
        self.register_import_with_access(
            owner,
            path,
            ImportTail::of_include(kind),
            target,
            ModuleAliasRole::IncludedInstance,
        )
    }

    fn register_import_with_access(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        tail: ImportTail<'_>,
        target: &DagId,
        role: ModuleAliasRole,
    ) -> Result<(), ModuleResolveError> {
        // Every import/include edge crosses a module boundary.
        let access = Access::CrossModule;
        self.module_symbols(owner)?;
        self.module_symbols(target)?;
        if tail.requires_visible_module_path() {
            self.ensure_module_path_visible(target, access)?;
        }

        let additions = self.import_additions(path, tail, target, access, role)?;
        let symbols = self
            .modules
            .get(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })?;
        self.scopes
            .get_mut(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })?
            .add_imports(symbols, additions)
    }

    fn import_additions(
        &self,
        path: &ModulePath,
        tail: ImportTail<'_>,
        target: &DagId,
        access: Access,
        role: ModuleAliasRole,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        match tail {
            ImportTail::Module { alias, visibility } => {
                let alias = module_alias(path, alias);
                Ok(vec![ImportAddition {
                    local: Spanned::new(alias.value.into_atom(), alias.span),
                    visibility: BindableVisibility::from(visibility),
                    target: ImportTarget::ModuleAlias {
                        target: target.clone(),
                        access,
                        role,
                    },
                }])
            }
            ImportTail::Selective(items) => items
                .iter()
                .map(|item| {
                    let additions = self.import_item_additions(target, item, access)?;
                    if role == ModuleAliasRole::IncludedInstance {
                        for addition in &additions {
                            let Some(kind) = self.import_target_kind(&addition.target)? else {
                                continue;
                            };
                            if include_projection(kind).is_none() {
                                return Err(ModuleResolveError::IncludeItemNotProjectable {
                                    owner: target.clone(),
                                    name: item.name.name.to_string(),
                                    kind,
                                    span: item.name.span,
                                });
                            }
                        }
                    }
                    Ok(additions)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|chunks| chunks.into_iter().flatten().collect()),
        }
    }

    /// Selective-import category of an import target; module aliases have none.
    pub(super) fn import_target_kind(
        &self,
        target: &ImportTarget,
    ) -> Result<Option<ExportedImportItemKind>, ModuleResolveError> {
        Ok(Some(match target {
            ImportTarget::ModuleAlias { .. } => return Ok(None),
            ImportTarget::Decl(target) => ExportedImportItemKind::Decl(*self.declared(target)?),
            ImportTarget::Unit(target) => ExportedImportItemKind::Unit(*self.declared(target)?),
            ImportTarget::Dimension(_) => ExportedImportItemKind::Dimension,
            ImportTarget::StructType(_) => ExportedImportItemKind::Type,
            ImportTarget::Index(_) => ExportedImportItemKind::Index,
            ImportTarget::Constructor(_) => ExportedImportItemKind::Constructor,
        }))
    }

    pub(super) fn import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: Access,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let addition = |target| ImportAddition {
            local: Spanned::new(item.local_name_atom().clone(), item.local_span()),
            visibility: BindableVisibility::from(item.visibility),
            target,
        };
        let source = item.name.name.atom();
        let target = match item.namespace {
            ImportItemNamespace::Term => {
                return self.term_import_item_additions(target, item, access);
            }
            ImportItemNamespace::Type => ImportTarget::StructType(
                self.required_exported_symbol_for_import::<StructTypeNameNamespace>(
                    target, source, access, item,
                )?,
            ),
            ImportItemNamespace::Dimension => ImportTarget::Dimension(
                self.required_exported_symbol_for_import::<DimNameNamespace>(
                    target, source, access, item,
                )?,
            ),
            ImportItemNamespace::Unit => ImportTarget::Unit(
                self.required_exported_symbol_for_import::<UnitNameNamespace>(
                    target, source, access, item,
                )?,
            ),
            ImportItemNamespace::Index => ImportTarget::Index(
                self.required_exported_symbol_for_import::<IndexNameNamespace>(
                    target, source, access, item,
                )?,
            ),
        };
        Ok(vec![addition(target)])
    }

    fn term_import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: Access,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let addition = |target| ImportAddition {
            local: Spanned::new(item.local_name_atom().clone(), item.local_span()),
            visibility: BindableVisibility::from(item.visibility),
            target,
        };
        let source_atom = &item.name.name;
        let decl = self.exported_symbol_for_import::<DeclNameNamespace>(
            target,
            source_atom.atom(),
            access,
        )?;
        let constructor = self.exported_symbol_for_import::<ConstructorNameNamespace>(
            target,
            source_atom.atom(),
            access,
        )?;
        let saw_private = decl == ExportLookup::Private || constructor == ExportLookup::Private;
        let additions = decl
            .public()
            .map(ImportTarget::Decl)
            .into_iter()
            .chain(constructor.public().map(ImportTarget::Constructor))
            .map(addition)
            .collect::<Vec<_>>();

        match (additions.is_empty(), saw_private) {
            (false, _) => Ok(additions),
            (true, true) => Err(ModuleResolveError::PrivateName {
                owner: target.clone(),
                namespace: "term import namespace",
                name: source_atom.to_string(),
            }),
            (true, false) => Err(self
                .exported_import_item_categories(target, source_atom.atom(), access)?
                .map_or_else(
                    || ModuleResolveError::UnknownName {
                        owner: target.clone(),
                        namespace: "term import namespace",
                        name: source_atom.to_string(),
                    },
                    |alternatives| ModuleResolveError::WrongImportCategory {
                        owner: target.clone(),
                        mismatch: ImportItemCategoryMismatch::new(
                            source_atom.atom().clone(),
                            item.namespace,
                            alternatives,
                        ),
                        span: item.name.span,
                    },
                )),
        }
    }

    fn required_exported_symbol_for_import<Ns: SymbolTables>(
        &self,
        target: &DagId,
        source_atom: &NameAtom,
        access: Access,
        item: &ImportItem,
    ) -> Result<ResolvedName<Ns>, ModuleResolveError> {
        match self.exported_symbol_for_import::<Ns>(target, source_atom, access)? {
            ExportLookup::Public(target_name) => Ok(target_name),
            ExportLookup::Private => Err(ModuleResolveError::PrivateName {
                owner: target.clone(),
                namespace: Ns::DISPLAY_NAME,
                name: source_atom.to_string(),
            }),
            ExportLookup::Missing => Err(self
                .exported_import_item_categories(target, source_atom, access)?
                .map_or_else(
                    || ModuleResolveError::UnknownName {
                        owner: target.clone(),
                        namespace: Ns::DISPLAY_NAME,
                        name: source_atom.to_string(),
                    },
                    |alternatives| ModuleResolveError::WrongImportCategory {
                        owner: target.clone(),
                        mismatch: ImportItemCategoryMismatch::new(
                            source_atom.clone(),
                            item.namespace,
                            alternatives,
                        ),
                        span: item.name.span,
                    },
                )),
        }
    }

    /// Look `atom` up on `target`'s public surface in one namespace: its own
    /// declarations first, then its selective (re-)exports.
    pub(super) fn exported_symbol_for_import<Ns: SymbolTables>(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: Access,
    ) -> Result<ExportLookup<Ns>, ModuleResolveError> {
        match exported_symbol(Ns::declared(self.module_symbols(target)?), atom, access) {
            ExportLookup::Missing => Ok(exported_symbol(
                Ns::selected(self.module_scope(target)?),
                atom,
                access,
            )),
            found => Ok(found),
        }
    }

    fn exported_import_item_categories(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: Access,
    ) -> Result<Option<NonEmpty<ImportItemNamespace>>, ModuleResolveError> {
        let exported = [
            (
                ImportItemNamespace::Term,
                self.exported_symbol_for_import::<DeclNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
            (
                ImportItemNamespace::Term,
                self.exported_symbol_for_import::<ConstructorNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
            (
                ImportItemNamespace::Type,
                self.exported_symbol_for_import::<StructTypeNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
            (
                ImportItemNamespace::Dimension,
                self.exported_symbol_for_import::<DimNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
            (
                ImportItemNamespace::Unit,
                self.exported_symbol_for_import::<UnitNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
            (
                ImportItemNamespace::Index,
                self.exported_symbol_for_import::<IndexNameNamespace>(target, atom, access)?
                    .is_public(),
            ),
        ];
        let mut categories = Vec::new();
        for (category, public) in exported {
            if public && !categories.contains(&category) {
                categories.push(category);
            }
        }
        Ok(NonEmpty::try_from_vec(categories).ok())
    }
}

/// The names one `import` / `include` edge introduces, with the visibility
/// its declaration form assigns them.
#[derive(Debug, Clone, Copy)]
enum ImportTail<'a> {
    /// A module alias: a whole-DAG import or a module-form include.
    Module {
        alias: Option<&'a Spanned<ModuleAliasName>>,
        visibility: Visibility,
    },
    /// Selected items, each carrying its own visibility.
    Selective(&'a [ImportItem]),
}

impl<'a> ImportTail<'a> {
    fn of_import(import: &'a ast::ImportDecl) -> Self {
        match import {
            ast::ImportDecl::Module {
                visibility, alias, ..
            } => Self::Module {
                alias: alias.as_ref(),
                visibility: *visibility,
            },
            ast::ImportDecl::Selective { items, .. } => Self::Selective(items),
        }
    }

    /// An include's module alias names a private instance; its selected
    /// items carry their own visibility.
    fn of_include(kind: &'a ImportKind) -> Self {
        match kind {
            ImportKind::Module { alias } => Self::Module {
                alias: alias.as_ref(),
                visibility: Visibility::Private,
            },
            ImportKind::Selective(items) => Self::Selective(items),
        }
    }

    /// Selecting items, or re-exporting the alias, reaches through the target
    /// module path, so that path must be visible to the importer.
    const fn requires_visible_module_path(self) -> bool {
        match self {
            Self::Selective(_) => true,
            Self::Module { visibility, .. } => visibility.is_public(),
        }
    }
}

/// Outcome of looking a name up on a module's public surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExportLookup<Ns: NameNamespace> {
    Public(ResolvedName<Ns>),
    Private,
    Missing,
}

impl<Ns: NameNamespace> ExportLookup<Ns> {
    fn public(self) -> Option<ResolvedName<Ns>> {
        match self {
            Self::Public(resolved) => Some(resolved),
            Self::Private | Self::Missing => None,
        }
    }

    const fn is_public(&self) -> bool {
        matches!(self, Self::Public(_))
    }
}

fn exported_symbol<Ns: NameNamespace, X>(
    table: &HashMap<NameDef<Ns>, Symbol<Ns, X>>,
    atom: &NameAtom,
    access: Access,
) -> ExportLookup<Ns> {
    table
        .get(&NameDef::classify(atom.clone()))
        .map_or(ExportLookup::Missing, |symbol| {
            if !access.requires_public() || symbol.visibility().is_public() {
                ExportLookup::Public(symbol.resolved().clone())
            } else {
                ExportLookup::Private
            }
        })
}
