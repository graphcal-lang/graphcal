//! Registration of loader-resolved `import` / `include` edges into scopes.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::ResolvedName;
use crate::syntax::ast::{BindableVisibility, ImportItem, ImportKind, ModulePath, Visibility};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::ModuleResolver;
use super::category::{ExportedImportItemKind, include_projection};
use super::error::ModuleResolveError;
use super::namespace::{
    ExclusiveNameKind, ExclusiveNameOccupancy, FlatNamespace, ResolvableNamespace,
};
use super::scope::{Access, ImportAddition, ImportedSymbol, ModuleAliasRole, ModuleScope};
use super::symbols::{ModuleSymbolLookup, ModuleSymbols};

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
            Access::CrossModule,
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
            Access::CrossModule,
            ModuleAliasRole::IncludedInstance,
        )
    }
    fn register_import_with_access(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        tail: ImportTail<'_>,
        target: &DagId,
        access: Access,
        role: ModuleAliasRole,
    ) -> Result<(), ModuleResolveError> {
        self.module_symbols(owner)?;
        self.module_symbols(target)?;
        if tail.requires_visible_module_path() {
            self.ensure_module_path_visible(target, access)?;
        }

        let additions = self.import_additions(path, tail, target, access, role)?;
        self.check_import_exclusive_name_collisions(owner, &additions)?;
        let scope =
            self.scopes
                .get_mut(owner)
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: owner.clone(),
                })?;
        for addition in additions {
            scope.apply_addition(owner, addition)?;
        }
        Ok(())
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
                let alias = alias.cloned().unwrap_or_else(|| {
                    Spanned::new(
                        ModuleAliasName::classify(path.leaf().name.atom().clone()),
                        path.leaf().span,
                    )
                });
                Ok(vec![ImportAddition::ModuleAlias {
                    alias,
                    target: target.clone(),
                    access,
                    role,
                    visibility: BindableVisibility::from(visibility),
                }])
            }
            ImportTail::Selective(items) => items
                .iter()
                .map(|item| {
                    let additions = self.import_item_additions(target, item, access)?;
                    if role == ModuleAliasRole::IncludedInstance {
                        for addition in &additions {
                            let Some(kind) = self.import_addition_kind(addition)? else {
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

    pub(super) fn import_addition_kind(
        &self,
        addition: &ImportAddition,
    ) -> Result<Option<ExportedImportItemKind>, ModuleResolveError> {
        match addition {
            ImportAddition::ModuleAlias { .. } => Ok(None),
            ImportAddition::Decl { target, .. } => Ok(Some(ExportedImportItemKind::Decl(
                self.decl_symbol_kind(target)?,
            ))),
            ImportAddition::Dimension { .. } => Ok(Some(ExportedImportItemKind::Dimension)),
            ImportAddition::Unit { target, .. } => Ok(Some(ExportedImportItemKind::Unit(
                self.unit_constness(target)?,
            ))),
            ImportAddition::StructType { .. } => Ok(Some(ExportedImportItemKind::Type)),
            ImportAddition::Index { .. } => Ok(Some(ExportedImportItemKind::Index)),
            ImportAddition::Constructor { .. } => Ok(Some(ExportedImportItemKind::Constructor)),
        }
    }

    fn check_import_exclusive_name_collisions(
        &self,
        owner: &DagId,
        additions: &[ImportAddition],
    ) -> Result<(), ModuleResolveError> {
        let local = self.module_symbols(owner)?;
        let scope = self.module_scope(owner)?;
        let mut occupied = self.exclusive_name_occupancy(owner)?;

        check_same_namespace_import_collisions(owner, local, scope, additions)?;
        check_import_addition_exclusive_names(owner, &mut occupied, additions)
    }

    fn exclusive_name_occupancy(
        &self,
        owner: &DagId,
    ) -> Result<ExclusiveNameOccupancy, ModuleResolveError> {
        let local = self.module_symbols(owner)?;
        let scope = self.module_scope(owner)?;
        let mut occupied = HashMap::new();

        seed_exclusive_names(&mut occupied, &local.decls, ExclusiveNameKind::Value);
        seed_exclusive_names(
            &mut occupied,
            &local.dimensions,
            ExclusiveNameKind::Dimension,
        );
        seed_exclusive_names(
            &mut occupied,
            &local.struct_types,
            ExclusiveNameKind::StructType,
        );
        seed_exclusive_names(&mut occupied, &local.indexes, ExclusiveNameKind::Index);
        seed_exclusive_names(
            &mut occupied,
            &local.constructors,
            ExclusiveNameKind::Constructor,
        );
        seed_exclusive_names(
            &mut occupied,
            &scope.selected_decls,
            ExclusiveNameKind::Value,
        );
        seed_exclusive_names(
            &mut occupied,
            &scope.selected_dimensions,
            ExclusiveNameKind::Dimension,
        );
        seed_exclusive_names(
            &mut occupied,
            &scope.selected_struct_types,
            ExclusiveNameKind::StructType,
        );
        seed_exclusive_names(
            &mut occupied,
            &scope.selected_indexes,
            ExclusiveNameKind::Index,
        );
        seed_exclusive_names(
            &mut occupied,
            &scope.selected_constructors,
            ExclusiveNameKind::Constructor,
        );
        for (alias, target) in &scope.module_aliases {
            occupied.insert((FlatNamespace::Term, alias.atom().clone()), target.span());
        }
        for (alias, target) in &scope.plugin_aliases {
            occupied.insert((FlatNamespace::Term, alias.atom().clone()), target.span());
        }

        Ok(occupied)
    }

    pub(super) fn import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: Access,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let local_atom = item
            .alias
            .as_ref()
            .map_or_else(|| item.name.name.clone(), |alias| alias.name.clone());
        let local_span = item.local_span();
        let visibility = BindableVisibility::from(item.visibility);

        let additions = match item.namespace {
            ImportItemNamespace::Term => {
                return self.term_import_item_additions(
                    target,
                    item,
                    access,
                    local_atom.into_atom(),
                    visibility,
                );
            }
            ImportItemNamespace::Type => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    item.name.name.atom(),
                    access,
                    ModuleSymbols::struct_types,
                    |scope| &scope.selected_struct_types,
                    StructTypeNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::StructType {
                    local: Spanned::new(
                        StructTypeName::classify(local_atom.into_atom()),
                        local_span,
                    ),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Dimension => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    item.name.name.atom(),
                    access,
                    ModuleSymbols::dimensions,
                    |scope| &scope.selected_dimensions,
                    DimNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Dimension {
                    local: Spanned::new(DimName::classify(local_atom.into_atom()), local_span),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Unit => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    item.name.name.atom(),
                    access,
                    ModuleSymbols::units,
                    |scope| &scope.selected_units,
                    UnitNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Unit {
                    local: Spanned::new(UnitName::classify(local_atom.into_atom()), local_span),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Index => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    item.name.name.atom(),
                    access,
                    ModuleSymbols::indexes,
                    |scope| &scope.selected_indexes,
                    IndexNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Index {
                    local: Spanned::new(IndexName::classify(local_atom.into_atom()), local_span),
                    target: target_name,
                    visibility,
                }
            }
        };
        Ok(vec![additions])
    }

    fn term_import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: Access,
        local_atom: NameAtom,
        visibility: BindableVisibility,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let mut additions = Vec::new();
        let mut saw_private = false;
        let source_atom = &item.name.name;
        let local_span = item.local_span();

        match self.exported_symbol_for_import(
            target,
            source_atom.atom(),
            access,
            ModuleSymbols::decls,
            |scope| &scope.selected_decls,
        )? {
            ExportLookup::Public(target_name) => additions.push(ImportAddition::Decl {
                local: Spanned::new(DeclName::classify(local_atom.clone()), local_span),
                target: target_name,
                visibility,
            }),
            ExportLookup::Private => saw_private = true,
            ExportLookup::Missing => {}
        }
        match self.exported_symbol_for_import(
            target,
            source_atom.atom(),
            access,
            ModuleSymbols::constructors,
            |scope| &scope.selected_constructors,
        )? {
            ExportLookup::Public(target_name) => additions.push(ImportAddition::Constructor {
                local: Spanned::new(ConstructorName::classify(local_atom), local_span),
                target: target_name,
                visibility,
            }),
            ExportLookup::Private => saw_private = true,
            ExportLookup::Missing => {}
        }

        match (additions.is_empty(), saw_private) {
            (false, _) => Ok(additions),
            (true, true) => Err(ModuleResolveError::PrivateName {
                owner: target.clone(),
                namespace: "term import namespace",
                name: source_atom.to_string(),
            }),
            (true, false) => self
                .exported_import_item_categories(target, source_atom.atom(), access)?
                .map_or_else(
                    || {
                        Err(ModuleResolveError::UnknownName {
                            owner: target.clone(),
                            namespace: "term import namespace",
                            name: source_atom.to_string(),
                        })
                    },
                    |alternatives| {
                        Err(ModuleResolveError::WrongImportCategory {
                            owner: target.clone(),
                            mismatch: ImportItemCategoryMismatch::new(
                                source_atom.atom().clone(),
                                item.namespace,
                                alternatives,
                            ),
                            span: item.name.span,
                        })
                    },
                ),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "generic import lookup carries typed namespace accessors and diagnostics"
    )]
    fn required_exported_symbol_for_import<Ns, S>(
        &self,
        target: &DagId,
        source_atom: &NameAtom,
        access: Access,
        local_symbols: fn(&ModuleSymbols) -> &HashMap<NameDef<Ns>, S>,
        selected_symbols: fn(&ModuleScope) -> &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
        namespace_name: &'static str,
        expected: ImportItemNamespace,
        span: Span,
    ) -> Result<ResolvedName<Ns>, ModuleResolveError>
    where
        Ns: ResolvableNamespace,
        S: ModuleSymbolLookup<Ns>,
    {
        match self.exported_symbol_for_import(
            target,
            source_atom,
            access,
            local_symbols,
            selected_symbols,
        )? {
            ExportLookup::Public(target_name) => Ok(target_name),
            ExportLookup::Private => Err(ModuleResolveError::PrivateName {
                owner: target.clone(),
                namespace: namespace_name,
                name: source_atom.to_string(),
            }),
            ExportLookup::Missing => self
                .exported_import_item_categories(target, source_atom, access)?
                .map_or_else(
                    || {
                        Err(ModuleResolveError::UnknownName {
                            owner: target.clone(),
                            namespace: namespace_name,
                            name: source_atom.to_string(),
                        })
                    },
                    |alternatives| {
                        Err(ModuleResolveError::WrongImportCategory {
                            owner: target.clone(),
                            mismatch: ImportItemCategoryMismatch::new(
                                source_atom.clone(),
                                expected,
                                alternatives,
                            ),
                            span,
                        })
                    },
                ),
        }
    }

    pub(super) fn exported_symbol_for_import<Ns, S>(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: Access,
        local_symbols: fn(&ModuleSymbols) -> &HashMap<NameDef<Ns>, S>,
        selected_symbols: fn(&ModuleScope) -> &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    ) -> Result<ExportLookup<Ns>, ModuleResolveError>
    where
        Ns: ResolvableNamespace,
        S: ModuleSymbolLookup<Ns>,
    {
        let target_symbols = self.module_symbols(target)?;
        match exported_symbol(local_symbols(target_symbols), atom, access) {
            ExportLookup::Missing => {}
            found => return Ok(found),
        }

        let target_scope = self.module_scope(target)?;
        Ok(exported_symbol(
            selected_symbols(target_scope),
            atom,
            access,
        ))
    }

    fn exported_import_item_categories(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: Access,
    ) -> Result<Option<NonEmpty<ImportItemNamespace>>, ModuleResolveError> {
        let mut categories = Vec::new();
        macro_rules! probe {
            ($category:expr, $local:expr, $selected:expr) => {
                if matches!(
                    self.exported_symbol_for_import(target, atom, access, $local, $selected)?,
                    ExportLookup::Public(_)
                ) && !categories.contains(&$category)
                {
                    categories.push($category);
                }
            };
        }

        probe!(ImportItemNamespace::Term, ModuleSymbols::decls, |scope| {
            &scope.selected_decls
        });
        probe!(
            ImportItemNamespace::Term,
            ModuleSymbols::constructors,
            |scope| &scope.selected_constructors
        );
        probe!(
            ImportItemNamespace::Type,
            ModuleSymbols::struct_types,
            |scope| &scope.selected_struct_types
        );
        probe!(
            ImportItemNamespace::Dimension,
            ModuleSymbols::dimensions,
            |scope| &scope.selected_dimensions
        );
        probe!(ImportItemNamespace::Unit, ModuleSymbols::units, |scope| {
            &scope.selected_units
        });
        probe!(
            ImportItemNamespace::Index,
            ModuleSymbols::indexes,
            |scope| { &scope.selected_indexes }
        );

        Ok(NonEmpty::try_from_vec(categories).ok())
    }
}

fn seed_exclusive_names<Ns, S>(
    occupied: &mut ExclusiveNameOccupancy,
    symbols: &HashMap<NameDef<Ns>, S>,
    kind: ExclusiveNameKind,
) where
    Ns: NameNamespace,
    S: ModuleSymbolLookup<Ns>,
{
    for (name, symbol) in symbols {
        occupied
            .entry((kind.namespace(), name.atom().clone()))
            .or_insert_with(|| symbol.span());
    }
}

fn check_import_addition_exclusive_names(
    owner: &DagId,
    occupied: &mut ExclusiveNameOccupancy,
    additions: &[ImportAddition],
) -> Result<(), ModuleResolveError> {
    for addition in additions {
        match addition {
            ImportAddition::Decl { local, .. } => register_import_exclusive_name(
                owner,
                occupied,
                local.value.atom(),
                ExclusiveNameKind::Value,
                local.span,
            )?,
            ImportAddition::Dimension { local, .. } => register_import_exclusive_name(
                owner,
                occupied,
                local.value.atom(),
                ExclusiveNameKind::Dimension,
                local.span,
            )?,
            ImportAddition::StructType { local, .. } => register_import_exclusive_name(
                owner,
                occupied,
                local.value.atom(),
                ExclusiveNameKind::StructType,
                local.span,
            )?,
            ImportAddition::Index { local, .. } => register_import_exclusive_name(
                owner,
                occupied,
                local.value.atom(),
                ExclusiveNameKind::Index,
                local.span,
            )?,
            ImportAddition::Constructor { local, .. } => register_import_exclusive_name(
                owner,
                occupied,
                local.value.atom(),
                ExclusiveNameKind::Constructor,
                local.span,
            )?,
            ImportAddition::ModuleAlias { alias, .. } => register_import_exclusive_name(
                owner,
                occupied,
                alias.value.atom(),
                ExclusiveNameKind::Value,
                alias.span,
            )?,
            ImportAddition::Unit { .. } => {}
        }
    }
    Ok(())
}

fn check_same_namespace_import_collisions(
    owner: &DagId,
    local: &ModuleSymbols,
    scope: &ModuleScope,
    additions: &[ImportAddition],
) -> Result<(), ModuleResolveError> {
    for addition in additions {
        match addition {
            ImportAddition::Unit { local: name, .. } => check_import_collision_in_namespace(
                owner,
                name,
                &local.units,
                &scope.selected_units,
                UnitNameNamespace::DISPLAY_NAME,
            )?,
            ImportAddition::Constructor { local: name, .. } => check_import_collision_in_namespace(
                owner,
                name,
                &local.constructors,
                &scope.selected_constructors,
                ConstructorNameNamespace::DISPLAY_NAME,
            )?,
            ImportAddition::ModuleAlias { .. }
            | ImportAddition::Decl { .. }
            | ImportAddition::Dimension { .. }
            | ImportAddition::StructType { .. }
            | ImportAddition::Index { .. } => {}
        }
    }
    Ok(())
}

fn check_import_collision_in_namespace<Ns, S>(
    owner: &DagId,
    name: &Spanned<NameDef<Ns>>,
    local: &HashMap<NameDef<Ns>, S>,
    selected: &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError>
where
    Ns: NameNamespace,
    S: ModuleSymbolLookup<Ns>,
{
    if let Some(first) = local.get(&name.value) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: namespace_name,
            name: name.value.to_string(),
            first: first.span(),
            duplicate: name.span,
        });
    }
    if let Some(first) = selected.get(&name.value) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: namespace_name,
            name: name.value.to_string(),
            first: first.span(),
            duplicate: name.span,
        });
    }
    Ok(())
}

fn register_import_exclusive_name(
    owner: &DagId,
    occupied: &mut ExclusiveNameOccupancy,
    atom: &NameAtom,
    kind: ExclusiveNameKind,
    span: Span,
) -> Result<(), ModuleResolveError> {
    let namespace = kind.namespace();
    let slot = (namespace, atom.clone());
    if let Some(first) = occupied.get(&slot) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: match namespace {
                FlatNamespace::Static => "Static",
                FlatNamespace::Term => "Term",
            },
            name: atom.to_string(),
            first: *first,
            duplicate: span,
        });
    }
    occupied.insert(slot, span);
    Ok(())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ExportLookup<Ns: NameNamespace> {
    Public(ResolvedName<Ns>),
    Private,
    Missing,
}

fn exported_symbol<Ns, S>(
    map: &HashMap<NameDef<Ns>, S>,
    atom: &NameAtom,
    access: Access,
) -> ExportLookup<Ns>
where
    Ns: NameNamespace,
    S: ModuleSymbolLookup<Ns>,
{
    map.get(&NameDef::classify(atom.clone()))
        .map_or(ExportLookup::Missing, |symbol| {
            if !access.requires_public() || symbol.visibility().is_public() {
                ExportLookup::Public(symbol.resolved().clone())
            } else {
                ExportLookup::Private
            }
        })
}
