//! Path resolution and symbol queries over a built [`ModuleResolver`].

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::resolved_name::{ResolvedIndexName, ResolvedIndexVariant, ResolvedStaticName};
use crate::syntax::ast::{IdentPath, ModulePath, UnitConstness};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::index_name::{IndexNameNamespace, IndexVariantName};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::category::{DeclSymbolKind, SurfaceNameKind};
use super::error::{ExpectedDeclKind, ModuleResolveError, NameCategory};
use super::exports::{ExportedBinding, ExportedBindingTarget, ExportedImportItem};
use super::namespace::Namespace;
use super::scope::Access;
use super::symbols::{ConstructorSignature, GenericParamSignature, Symbol, SymbolRef};
use super::tables::NamespaceTables;
use super::{ModuleEntry, ModuleResolver};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedModuleQualifier {
    owner: DagId,
    access: Access,
}

impl ModuleResolver {
    /// List the module's public surface as source spelling plus typed canonical target.
    ///
    /// Native declarations and selective re-exports are indistinguishable here.
    /// Imports expose only items explicitly marked `pub` in their selective list;
    /// namespaced imports never widen this surface.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] if `owner` or a re-exported declaration's
    /// canonical owner is missing.
    pub fn exported_bindings(
        &self,
        owner: &DagId,
    ) -> Result<Vec<ExportedBinding>, ModuleResolveError> {
        let symbols = self.module_symbols(owner)?;
        let scope = self.module_scope(owner)?;
        let mut bindings = Vec::new();
        let mut export = |name: &NameAtom, target| {
            bindings.push(ExportedBinding {
                name: name.clone(),
                target,
            });
        };

        for (name, symbol) in public_symbols::<DeclNameNamespace>(symbols, scope) {
            export(
                name,
                ExportedBindingTarget::Decl {
                    identity: symbol.resolved().clone(),
                    kind: *symbol.data(),
                },
            );
        }
        for (name, symbol) in public_symbols::<UnitNameNamespace>(symbols, scope) {
            export(
                name,
                ExportedBindingTarget::Unit {
                    identity: symbol.resolved().clone(),
                    constness: *symbol.data(),
                },
            );
        }
        for (name, symbol) in public_symbols::<ConstructorNameNamespace>(symbols, scope) {
            export(
                name,
                ExportedBindingTarget::Constructor(symbol.resolved().clone()),
            );
        }
        for (name, symbol) in public_symbols::<StructTypeNameNamespace>(symbols, scope) {
            export(name, ExportedBindingTarget::Type(symbol.resolved().clone()));
        }
        for (name, symbol) in public_symbols::<DimNameNamespace>(symbols, scope) {
            export(
                name,
                ExportedBindingTarget::Dimension(symbol.resolved().clone()),
            );
        }
        for (name, symbol) in public_symbols::<IndexNameNamespace>(symbols, scope) {
            export(
                name,
                ExportedBindingTarget::Index(symbol.resolved().clone()),
            );
        }

        bindings.sort_by(|left, right| {
            left.target
                .kind()
                .sort_rank()
                .cmp(&right.target.kind().sort_rank())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(bindings)
    }

    /// Render the typed exported bindings in selective-import categories.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] when [`Self::exported_bindings`] cannot
    /// resolve a canonical target.
    pub fn exported_import_items(
        &self,
        owner: &DagId,
    ) -> Result<Vec<ExportedImportItem>, ModuleResolveError> {
        self.exported_bindings(owner).map(|bindings| {
            bindings
                .into_iter()
                .map(|binding| ExportedImportItem {
                    name: binding.name,
                    kind: binding.target.kind(),
                })
                .collect()
        })
    }

    /// Return selectively imported DAG bindings as local name → canonical DAG.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] if a selected declaration's canonical
    /// owner is missing from the project-wide resolver.
    pub fn selected_dag_imports(
        &self,
        owner: &DagId,
    ) -> Result<Vec<(DeclName, DagId)>, ModuleResolveError> {
        let scope = self.module_scope(owner)?;
        Ok(scope
            .selected_decls
            .iter()
            .filter(|(_, imported)| *imported.data() == DeclSymbolKind::Dag)
            .map(|(local, imported)| {
                (
                    local.clone(),
                    imported
                        .resolved()
                        .owner()
                        .inline_dag_child(imported.resolved().to_unowned_def_name()),
                )
            })
            .collect())
    }
    /// Resolve a syntactic declaration/value path to the symbol it denotes.
    ///
    /// Bare paths first search local declarations, then selective imports.
    /// Qualified paths resolve their qualifier through module aliases and then
    /// apply that alias boundary's visibility rule.
    pub fn resolve_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, DeclNameNamespace, DeclSymbolKind>, ModuleResolveError> {
        self.resolve_symbol_path::<DeclNameNamespace>(owner, path)
    }

    /// Resolve a declaration path and require that it names a const declaration.
    pub(crate) fn resolve_const_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, DeclNameNamespace, DeclSymbolKind>, ModuleResolveError> {
        let symbol = self.resolve_decl_path(owner, path)?;
        let actual = *symbol.kind();
        if actual.is_const() {
            Ok(symbol)
        } else {
            Err(ModuleResolveError::UnexpectedDeclKind {
                name: symbol.into_resolved(),
                expected: ExpectedDeclKind::Const,
                actual,
            })
        }
    }

    /// Resolve a syntactic dimension path to the symbol it denotes.
    pub fn resolve_dimension_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, DimNameNamespace, ()>, ModuleResolveError> {
        self.resolve_symbol_path::<DimNameNamespace>(owner, path)
    }

    /// Resolve a syntactic unit path to the symbol it denotes.
    pub fn resolve_unit_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, UnitNameNamespace, UnitConstness>, ModuleResolveError> {
        self.resolve_symbol_path::<UnitNameNamespace>(owner, path)
    }

    /// Resolve a syntactic path in the Static slot, whichever of a
    /// dimension, type, or index occupies it.
    ///
    /// # Errors
    ///
    /// Returns the index lookup's [`ModuleResolveError`] when no Static
    /// symbol resolves.
    pub fn resolve_static_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedStaticName, ModuleResolveError> {
        self.resolve_dimension_path(owner, path)
            .map(|symbol| ResolvedStaticName::Dimension(symbol.into_resolved()))
            .or_else(|_| {
                self.resolve_struct_type_path(owner, path)
                    .map(|symbol| ResolvedStaticName::Type(symbol.into_resolved()))
            })
            .or_else(|_| {
                self.resolve_index_path(owner, path)
                    .map(|symbol| ResolvedStaticName::Index(symbol.into_resolved()))
            })
    }

    /// Resolve a syntactic struct/tagged-union type path to the symbol it
    /// denotes, whose kind is the type's source generic signature.
    pub fn resolve_struct_type_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<
        SymbolRef<'_, StructTypeNameNamespace, Vec<GenericParamSignature>>,
        ModuleResolveError,
    > {
        self.resolve_symbol_path::<StructTypeNameNamespace>(owner, path)
    }

    /// Resolve a syntactic tagged-union constructor path to the symbol it
    /// denotes, whose kind is its owning type's signature.
    pub fn resolve_constructor_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, ConstructorNameNamespace, ConstructorSignature>, ModuleResolveError>
    {
        self.resolve_symbol_path::<ConstructorNameNamespace>(owner, path)
    }

    /// Resolve a span-aware constructor path without losing source path shape at
    /// the caller boundary.
    pub(crate) fn resolve_constructor_ident_path(
        &self,
        owner: &DagId,
        path: &IdentPath,
    ) -> Result<SymbolRef<'_, ConstructorNameNamespace, ConstructorSignature>, ModuleResolveError>
    {
        self.resolve_constructor_path(owner, &ident_path_to_name_path(path))
    }

    /// Find a source-visible spelling for a canonical index. Each candidate
    /// passes ordinary resolution, so aliases, visibility and shadowing obey
    /// the same rules as authored bindings. Absence means no offered spelling.
    #[must_use]
    pub fn source_index_path(&self, owner: &DagId, target: &ResolvedIndexName) -> Option<NamePath> {
        let mut candidates = Vec::new();
        if let Some(symbols) = self.symbols(owner) {
            candidates.extend(
                symbols
                    .indexes
                    .keys()
                    .map(|name| NamePath::local(name.atom().clone())),
            );
        }
        if let Some(scope) = self.modules.get(owner).map(ModuleEntry::scope) {
            candidates.extend(
                scope
                    .selected_indexes
                    .keys()
                    .map(|name| NamePath::local(name.atom().clone())),
            );
            for (alias, binding) in &scope.module_aliases {
                let names = self
                    .modules
                    .get(binding.target())
                    .into_iter()
                    .flat_map(|entry| {
                        entry
                            .symbols
                            .indexes
                            .keys()
                            .chain(entry.scope.selected_indexes.keys())
                    });
                candidates.extend(names.map(|name| {
                    NamePath::qualified(
                        crate::syntax::non_empty::NonEmpty::singleton(alias.atom().clone()),
                        name.atom().clone(),
                    )
                }));
            }
        }
        candidates.sort();
        candidates.into_iter().find(|candidate| {
            self.resolve_index_path(owner, candidate)
                .is_ok_and(|resolved| resolved.resolved() == target)
        })
    }

    /// Resolve a syntactic index path to a canonical owner + leaf.
    pub fn resolve_index_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<
        SymbolRef<'_, IndexNameNamespace, HashMap<IndexVariantName, Span>>,
        ModuleResolveError,
    > {
        self.resolve_symbol_path::<IndexNameNamespace>(owner, path)
    }

    /// Resolve an already-split index path plus variant leaf to a canonical
    /// index-variant identity.
    ///
    /// This is the HIR-facing form for parser positions that preserve the
    /// index path and variant leaf separately (map keys, index arguments, and
    /// match labels). It avoids reconstructing a dotted string or re-parsing
    /// source text just to validate the variant against the canonical index.
    pub fn resolve_index_variant_parts(
        &self,
        owner: &DagId,
        index_path: &NamePath,
        variant: &IndexVariantName,
    ) -> Result<ResolvedIndexVariant, ModuleResolveError> {
        let index = self.resolve_index_path(owner, index_path)?;
        if !index.kind().contains_key(variant) {
            return Err(ModuleResolveError::UnknownIndexVariant {
                index: index.into_resolved(),
                variant: variant.clone(),
            });
        }
        Ok(ResolvedIndexVariant::new(
            index.into_resolved(),
            variant.clone(),
        ))
    }

    /// Resolve a source DAG/module call path to its canonical [`DagId`].
    ///
    /// The first segment names a reusable DAG module in the caller's scope. It
    /// may be a local inline DAG, a sibling DAG visible from an inline body, or
    /// an imported module alias. That binding is itself callable regardless of
    /// whether its canonical target is a file root or an inline DAG; remaining
    /// path segments descend through child DAG modules uniformly.
    pub fn resolve_module_path(
        &self,
        owner: &DagId,
        path: &ModulePath,
    ) -> Result<DagId, ModuleResolveError> {
        let head = &path.segments.first().name;
        // Project-wide module registration is not lexical visibility. Only an
        // actual `dag` declaration in the current/parent source module creates
        // an implicit local callable; loaded files remain unavailable unless an
        // import binds them. In particular, `include module() as alias` binds
        // only `alias`, never the source module's leaf name.
        let declared_dag_child = |parent: &DagId| {
            self.symbols(parent)
                .and_then(|symbols| symbols.decls.get(&NameDef::classify(head.atom().clone())))
                .filter(|symbol| *symbol.data() == DeclSymbolKind::Dag)
                .map(|symbol| parent.inline_dag_child(symbol.resolved().to_unowned_def_name()))
                .filter(|child| self.modules.contains_key(child))
        };
        let local_target = declared_dag_child(owner).or_else(|| {
            owner
                .parent()
                .and_then(|parent| declared_dag_child(&parent))
        });

        let scope = self.module_scope(owner)?;
        let alias = ModuleAliasName::classify(head.atom().clone());
        let alias_binding = scope.module_aliases.get(&alias);
        let imported_alias_target = alias_binding
            .filter(|binding| binding.role.is_callable())
            .map(|binding| (binding.target.clone(), binding.access));
        let selected_name = DeclName::classify(head.atom().clone());
        let selected_target = match scope.selected_decls.get(&selected_name) {
            Some(imported) if *imported.data() == DeclSymbolKind::Dag => Some((
                imported
                    .resolved()
                    .owner()
                    .inline_dag_child(imported.resolved().to_unowned_def_name()),
                Access::CrossModule,
            )),
            Some(_) | None => None,
        };
        let candidates = local_target
            .map(|target| (target, Access::Local))
            .into_iter()
            .chain(selected_target)
            .chain(imported_alias_target)
            .fold(
                Vec::<(DagId, Access)>::new(),
                |mut candidates, (target, access)| {
                    match candidates
                        .iter_mut()
                        .find(|(registered, _)| registered == &target)
                    {
                        Some((_, registered_access)) if access == Access::Local => {
                            *registered_access = Access::Local;
                        }
                        Some(_) => {}
                        None => candidates.push((target, access)),
                    }
                    candidates
                },
            );

        let (mut target, access) = match candidates.as_slice() {
            [(target, access)] => (target.clone(), *access),
            [] => {
                if alias_binding.is_some() {
                    return Err(ModuleResolveError::IncludedInstanceNotCallable {
                        owner: owner.clone(),
                        alias,
                    });
                }
                if path.segments().len() == 1 {
                    return Err(ModuleResolveError::UnknownModule {
                        owner: owner.inline_dag_child(DeclName::classify(head.atom().clone())),
                    });
                }
                return Err(ModuleResolveError::UnknownModuleAlias {
                    owner: owner.clone(),
                    alias,
                });
            }
            _ => {
                return Err(ModuleResolveError::AmbiguousCallableModule {
                    owner: owner.clone(),
                    name: alias,
                    targets: candidates
                        .iter()
                        .map(|(target, _access)| target.clone())
                        .collect(),
                });
            }
        };

        self.ensure_module_path_visible(&target, access)?;
        for segment in path.segments().iter().skip(1) {
            target = target.inline_dag_child(DeclName::classify(segment.name.atom().clone()));
            if !self.modules.contains_key(&target) {
                return Err(ModuleResolveError::UnknownModule { owner: target });
            }
            self.ensure_module_path_visible(&target, access)?;
        }
        Ok(target)
    }
    pub(super) fn resolve_symbol_path<Ns: NamespaceTables>(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<SymbolRef<'_, Ns, Ns::Declared>, ModuleResolveError> {
        let Some((qualifier, leaf)) = path.qualifier_and_leaf() else {
            let atom = path.leaf();
            let name = NameDef::<Ns>::classify(atom.clone());
            if let Some(symbol) = Ns::declared(self.module_symbols(owner)?).get(&name) {
                return Ok(SymbolRef::new(symbol));
            }
            if let Some(imported) = Ns::selected(self.module_scope(owner)?).get(&name) {
                return Ok(SymbolRef::new(imported));
            }
            if let Some(actual) = self.visible_surface_kind(owner, Ns::NAMESPACE, atom, false)? {
                return Err(ModuleResolveError::WrongUniverseName {
                    owner: owner.clone(),
                    name: path.clone(),
                    expected: Ns::SURFACE_KIND,
                    actual,
                });
            }
            return Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                category: NameCategory::Table(Ns::TABLE),
                name: atom.clone(),
            });
        };

        let target_ref = self.resolve_module_qualifier(owner, qualifier)?;
        let requires_public = target_ref.access.requires_public();
        let leaf_name = NameDef::<Ns>::classify(leaf.clone());
        let found = Ns::declared(self.module_symbols(&target_ref.owner)?)
            .get(&leaf_name)
            .or_else(|| {
                self.modules
                    .get(&target_ref.owner)
                    .and_then(|entry| Ns::selected(&entry.scope).get(&leaf_name))
            });
        if let Some(symbol) = found {
            if requires_public && !symbol.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    category: NameCategory::Table(Ns::TABLE),
                    name: leaf.clone(),
                });
            }
            return Ok(SymbolRef::new(symbol));
        }

        if let Some(actual) =
            self.visible_surface_kind(&target_ref.owner, Ns::NAMESPACE, leaf, requires_public)?
        {
            return Err(ModuleResolveError::WrongUniverseName {
                owner: target_ref.owner,
                name: path.clone(),
                expected: Ns::SURFACE_KIND,
                actual,
            });
        }

        Err(ModuleResolveError::UnknownName {
            owner: target_ref.owner,
            category: NameCategory::Table(Ns::TABLE),
            name: leaf.clone(),
        })
    }

    /// The surface category of whatever occupies `(namespace, atom)` in
    /// `owner`, for wrong-universe diagnostics. Private occupants are skipped
    /// when the lookup `requires_public`.
    fn visible_surface_kind(
        &self,
        owner: &DagId,
        namespace: Namespace,
        atom: &NameAtom,
        requires_public: bool,
    ) -> Result<Option<SurfaceNameKind>, ModuleResolveError> {
        let visible = |occupant: &super::namespace::Occupant| {
            !requires_public || occupant.visibility.is_public()
        };
        let local = self
            .module_symbols(owner)?
            .occupant(namespace, atom)
            .filter(visible);
        let occupant = match local {
            Some(occupant) => Some(occupant),
            None => self
                .module_scope(owner)?
                .occupant(namespace, atom)
                .filter(visible),
        };
        Ok(occupant.and_then(|occupant| occupant.surface))
    }

    fn resolve_module_qualifier(
        &self,
        owner: &DagId,
        qualifier: &NonEmpty<NameAtom>,
    ) -> Result<ResolvedModuleQualifier, ModuleResolveError> {
        let (head, rest) = (qualifier.first(), &qualifier.as_slice()[1..]);
        let scope = self.module_scope(owner)?;
        let alias = ModuleAliasName::classify(head.clone());
        let alias_target = scope.module_aliases.get(&alias).ok_or_else(|| {
            ModuleResolveError::UnknownModuleAlias {
                owner: owner.clone(),
                alias,
            }
        })?;
        // Validate the alias's target before descending. This is essential
        // when the alias points directly at a private inline DAG and `rest` is
        // empty. The helper also checks every declared DAG ancestor, so a
        // public child under a private parent cannot be used as an access
        // tunnel.
        let mut target = alias_target.target.clone();
        self.ensure_module_path_visible(&target, alias_target.access)?;
        for segment in rest {
            let nested_alias = self
                .module_scope(&target)?
                .module_aliases
                .get(&NameDef::classify(segment.clone()));
            if let Some(nested_alias) = nested_alias {
                if alias_target.access.requires_public() && !nested_alias.visibility().is_public() {
                    return Err(ModuleResolveError::PrivateName {
                        owner: target,
                        category: NameCategory::DagAlias,
                        name: segment.clone(),
                    });
                }
                target = nested_alias.target().clone();
            } else {
                target = target.inline_dag_child(DeclName::classify(segment.clone()));
                if !self.modules.contains_key(&target) {
                    return Err(ModuleResolveError::UnknownModule { owner: target });
                }
            }
            self.ensure_module_path_visible(&target, alias_target.access)?;
        }
        if self.modules.contains_key(&target) {
            Ok(ResolvedModuleQualifier {
                owner: target,
                access: alias_target.access,
            })
        } else {
            Err(ModuleResolveError::UnknownModule { owner: target })
        }
    }
    /// Definition/import span occupying `(namespace, name)` in `owner`.
    pub(crate) fn visible_span(
        &self,
        owner: &DagId,
        namespace: Namespace,
        name: &NameAtom,
    ) -> Result<Option<Span>, ModuleResolveError> {
        let local = self.module_symbols(owner)?.occupant(namespace, name);
        let occupant = match local {
            Some(occupant) => Some(occupant),
            None => self.module_scope(owner)?.occupant(namespace, name),
        };
        Ok(occupant.map(|occupant| occupant.span))
    }

    pub(super) fn ensure_module_path_visible(
        &self,
        target: &DagId,
        access: Access,
    ) -> Result<(), ModuleResolveError> {
        if !access.requires_public() {
            return Ok(());
        }

        // Only inline-DAG edges carry a `dag` declaration's visibility. The
        // walk stops at a file root (file-path components are package
        // identity, not declarations) and at a concrete include instance
        // (its namespace has no source `dag` declaration on that edge).
        let mut child = target.clone();
        loop {
            let Some(name) = child.leaf().inline_dag() else {
                return Ok(());
            };
            let Some(parent) = child.parent() else {
                return Ok(());
            };
            let Some(symbol) = self
                .symbols(&parent)
                .and_then(|parent_symbols| parent_symbols.decls.get(name))
            else {
                return Ok(());
            };
            if *symbol.data() != DeclSymbolKind::Dag {
                return Ok(());
            }
            if !symbol.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: parent,
                    category: NameCategory::Dag,
                    name: name.atom().clone(),
                });
            }
            child = parent;
        }
    }
}

fn ident_path_to_name_path(path: &IdentPath) -> NamePath {
    path.to_name_path()
}

/// Publicly visible bindings of one table.
fn public<Ns: NameNamespace, X>(
    table: &HashMap<NameDef<Ns>, Symbol<Ns, X>>,
) -> impl Iterator<Item = (&NameAtom, &Symbol<Ns, X>)> {
    table
        .iter()
        .filter(|(_, symbol)| symbol.visibility().is_public())
        .map(|(name, symbol)| (name.atom(), symbol))
}

/// Public spellings of one namespace (declarations, then selective
/// re-exports) with the binding each denotes.
fn public_symbols<'a, Ns: NamespaceTables>(
    symbols: &'a super::symbols::ModuleSymbols,
    scope: &'a super::scope::ModuleScope,
) -> impl Iterator<Item = (&'a NameAtom, &'a Symbol<Ns, Ns::Declared>)> {
    public(Ns::declared(symbols)).chain(public(Ns::selected(scope)))
}
