//! Path resolution and symbol queries over a built [`ModuleResolver`].

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedDimName, ResolvedIndexName,
    ResolvedIndexVariant, ResolvedName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::ast::{IdentPath, ModulePath, UnitConstness};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::index_name::{IndexNameNamespace, IndexVariantName};
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::ModuleResolver;
use super::category::{DeclSymbolKind, SurfaceNameKind};
use super::error::ModuleResolveError;
use super::exports::{ExportedBinding, ExportedBindingTarget, ExportedImportItem};
use super::namespace::Namespace;
use super::scope::Access;
use super::symbols::{GenericParamSignature, Symbol};
use super::tables::SymbolTables;

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

        for (name, symbol) in public(&symbols.decls) {
            export(
                name,
                ExportedBindingTarget::Decl {
                    identity: symbol.resolved().clone(),
                    kind: *symbol.data(),
                },
            );
        }
        for (name, symbol) in public(&scope.selected_decls) {
            export(
                name,
                ExportedBindingTarget::Decl {
                    identity: symbol.resolved().clone(),
                    kind: self.decl_symbol_kind(symbol.resolved())?,
                },
            );
        }
        for (name, symbol) in public(&symbols.units) {
            export(
                name,
                ExportedBindingTarget::Unit {
                    identity: symbol.resolved().clone(),
                    constness: *symbol.data(),
                },
            );
        }
        for (name, symbol) in public(&scope.selected_units) {
            export(
                name,
                ExportedBindingTarget::Unit {
                    identity: symbol.resolved().clone(),
                    constness: self.unit_constness(symbol.resolved())?,
                },
            );
        }
        for (name, resolved) in public_targets::<ConstructorNameNamespace>(symbols, scope) {
            export(name, ExportedBindingTarget::Constructor(resolved.clone()));
        }
        for (name, resolved) in public_targets::<StructTypeNameNamespace>(symbols, scope) {
            export(name, ExportedBindingTarget::Type(resolved.clone()));
        }
        for (name, resolved) in public_targets::<DimNameNamespace>(symbols, scope) {
            export(name, ExportedBindingTarget::Dimension(resolved.clone()));
        }
        for (name, resolved) in public_targets::<IndexNameNamespace>(symbols, scope) {
            export(name, ExportedBindingTarget::Index(resolved.clone()));
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
        scope
            .selected_decls
            .iter()
            .map(|(local, imported)| {
                self.decl_symbol_kind(imported.resolved()).map(|kind| {
                    (kind == DeclSymbolKind::Dag).then(|| {
                        (
                            local.clone(),
                            imported
                                .resolved()
                                .owner()
                                .child(imported.resolved().as_str()),
                        )
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|entries| entries.into_iter().flatten().collect())
    }
    /// Resolve a syntactic declaration/value path to a canonical owner + leaf.
    ///
    /// Bare paths first search local declarations, then selective imports.
    /// Qualified paths resolve their qualifier through module aliases and then
    /// apply that alias boundary's visibility rule.
    pub fn resolve_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedDeclName, ModuleResolveError> {
        self.resolve_symbol_path::<DeclNameNamespace>(owner, path)
    }

    /// Resolve a declaration path and require that it names a const declaration.
    pub(crate) fn resolve_const_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedDeclName, ModuleResolveError> {
        let resolved = self.resolve_decl_path(owner, path)?;
        let actual = self.decl_symbol_kind(&resolved)?;
        if actual.is_const() {
            Ok(resolved)
        } else {
            Err(ModuleResolveError::UnexpectedDeclKind {
                name: resolved,
                expected: "const",
                actual,
            })
        }
    }

    /// Return the semantic kind of a resolved declaration symbol.
    pub fn decl_symbol_kind(
        &self,
        name: &ResolvedDeclName,
    ) -> Result<DeclSymbolKind, ModuleResolveError> {
        self.declared(name).copied()
    }

    pub(super) fn unit_constness(
        &self,
        name: &ResolvedUnitName,
    ) -> Result<UnitConstness, ModuleResolveError> {
        self.declared(name).copied()
    }

    pub(super) fn constructor_owner_type(
        &self,
        name: &ResolvedConstructorName,
    ) -> Result<ResolvedStructTypeName, ModuleResolveError> {
        self.declared(name).map(|signature| {
            ResolvedStructTypeName::from_def(name.owner().clone(), signature.owner_type.clone())
        })
    }

    /// Return whether an instantiated declaration may be referenced by its consumer.
    ///
    /// Parameters are explicit instance inputs even when they are not declared
    /// `pub`; other declaration kinds require public visibility.
    pub(crate) fn decl_symbol_is_instance_accessible(
        &self,
        name: &ResolvedDeclName,
    ) -> Result<bool, ModuleResolveError> {
        self.declaration(name).map(|symbol| {
            *symbol.data() == DeclSymbolKind::Param || symbol.visibility().is_public()
        })
    }

    /// Resolve a syntactic dimension path to a canonical owner + leaf.
    pub fn resolve_dimension_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedDimName, ModuleResolveError> {
        self.resolve_symbol_path::<DimNameNamespace>(owner, path)
    }

    /// Resolve a syntactic unit path to a canonical owner + leaf.
    pub fn resolve_unit_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedUnitName, ModuleResolveError> {
        self.resolve_symbol_path::<UnitNameNamespace>(owner, path)
    }

    /// Resolve a syntactic struct/tagged-union type path to a canonical owner + leaf.
    pub fn resolve_struct_type_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedStructTypeName, ModuleResolveError> {
        self.resolve_symbol_path::<StructTypeNameNamespace>(owner, path)
    }

    /// Return the source generic signature for a resolved user-defined type.
    pub(crate) fn struct_type_generic_params(
        &self,
        name: &ResolvedStructTypeName,
    ) -> Result<&[GenericParamSignature], ModuleResolveError> {
        self.declared(name).map(Vec::as_slice)
    }

    /// Resolve a syntactic tagged-union constructor path to a canonical owner + leaf.
    pub fn resolve_constructor_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedConstructorName, ModuleResolveError> {
        self.resolve_symbol_path::<ConstructorNameNamespace>(owner, path)
    }

    /// Return the owning type's source generic signature for a resolved constructor.
    pub(crate) fn constructor_generic_params(
        &self,
        name: &ResolvedConstructorName,
    ) -> Result<&[GenericParamSignature], ModuleResolveError> {
        self.declared(name)
            .map(|signature| signature.generic_params.as_slice())
    }

    /// Resolve a span-aware constructor path without losing source path shape at
    /// the caller boundary.
    pub(crate) fn resolve_constructor_ident_path(
        &self,
        owner: &DagId,
        path: &IdentPath,
    ) -> Result<ResolvedConstructorName, ModuleResolveError> {
        self.resolve_constructor_path(owner, &ident_path_to_name_path(path))
    }

    /// Find a source-visible spelling for a canonical index. Each candidate
    /// passes ordinary resolution, so aliases, visibility and shadowing obey
    /// the same rules as authored bindings. Absence means no offered spelling.
    #[must_use]
    pub fn source_index_path(&self, owner: &DagId, target: &ResolvedIndexName) -> Option<NamePath> {
        let mut candidates = Vec::new();
        if let Some(symbols) = self.modules.get(owner) {
            candidates.extend(
                symbols
                    .indexes
                    .keys()
                    .map(|name| NamePath::local(name.atom().clone())),
            );
        }
        if let Some(scope) = self.scopes.get(owner) {
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
                    .flat_map(|symbols| symbols.indexes.keys())
                    .chain(
                        self.scopes
                            .get(binding.target())
                            .into_iter()
                            .flat_map(|scope| scope.selected_indexes.keys()),
                    );
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
                .is_ok_and(|resolved| &resolved == target)
        })
    }

    /// Resolve a syntactic index path to a canonical owner + leaf.
    pub fn resolve_index_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedIndexName, ModuleResolveError> {
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
        let resolved_index = self.resolve_index_path(owner, index_path)?;
        if !self.declared(&resolved_index)?.contains_key(variant) {
            return Err(ModuleResolveError::UnknownIndexVariant {
                index: resolved_index,
                variant: variant.clone(),
            });
        }
        Ok(ResolvedIndexVariant::new(resolved_index, variant.clone()))
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
            self.modules
                .get(parent)
                .and_then(|symbols| symbols.decls.get(&NameDef::classify(head.atom().clone())))
                .filter(|symbol| *symbol.data() == DeclSymbolKind::Dag)
                .map(|_| parent.child(head.as_str()))
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
            Some(imported)
                if self.decl_symbol_kind(imported.resolved())? == DeclSymbolKind::Dag =>
            {
                Some((
                    imported
                        .resolved()
                        .owner()
                        .child(imported.resolved().as_str()),
                    Access::CrossModule,
                ))
            }
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
                        owner: owner.child(head.as_str()),
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
            target = target.child(segment.name.as_str());
            if !self.modules.contains_key(&target) {
                return Err(ModuleResolveError::UnknownModule { owner: target });
            }
            self.ensure_module_path_visible(&target, access)?;
        }
        Ok(target)
    }
    pub(super) fn resolve_symbol_path<Ns: SymbolTables>(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<Ns>, ModuleResolveError> {
        let Some((qualifier, leaf)) = path.qualifier_and_leaf() else {
            let atom = path.leaf();
            let name = NameDef::<Ns>::classify(atom.clone());
            if let Some(symbol) = Ns::declared(self.module_symbols(owner)?).get(&name) {
                return Ok(symbol.resolved().clone());
            }
            if let Some(imported) = Ns::selected(self.module_scope(owner)?).get(&name) {
                return Ok(imported.resolved().clone());
            }
            if let Some(actual) = self.visible_surface_kind(owner, Ns::NAMESPACE, atom, false)? {
                return Err(ModuleResolveError::WrongUniverseName {
                    owner: owner.clone(),
                    name: atom.to_string(),
                    expected: Ns::SURFACE_KIND,
                    actual,
                });
            }
            return Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: Ns::DISPLAY_NAME,
                name: atom.to_string(),
            });
        };

        let target_ref = self.resolve_module_qualifier(owner, qualifier)?;
        let requires_public = target_ref.access.requires_public();
        let leaf_name = NameDef::<Ns>::classify(leaf.clone());
        let found = Ns::declared(self.module_symbols(&target_ref.owner)?)
            .get(&leaf_name)
            .map(|symbol| (symbol.resolved(), symbol.visibility()))
            .or_else(|| {
                self.scopes
                    .get(&target_ref.owner)
                    .and_then(|scope| Ns::selected(scope).get(&leaf_name))
                    .map(|imported| (imported.resolved(), imported.visibility()))
            });
        if let Some((resolved, visibility)) = found {
            if requires_public && !visibility.is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    namespace: Ns::DISPLAY_NAME,
                    name: leaf.to_string(),
                });
            }
            return Ok(resolved.clone());
        }

        if let Some(actual) =
            self.visible_surface_kind(&target_ref.owner, Ns::NAMESPACE, leaf, requires_public)?
        {
            return Err(ModuleResolveError::WrongUniverseName {
                owner: target_ref.owner,
                name: path.display_path(),
                expected: Ns::SURFACE_KIND,
                actual,
            });
        }

        Err(ModuleResolveError::UnknownName {
            owner: target_ref.owner,
            namespace: Ns::DISPLAY_NAME,
            name: leaf.to_string(),
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
                        namespace: "dag alias",
                        name: segment.to_string(),
                    });
                }
                target = nested_alias.target().clone();
            } else {
                target = target.child(segment.as_str());
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

        let mut child = target.clone();
        loop {
            let Some(parent) = child.parent() else {
                return Ok(());
            };
            let Some(parent_symbols) = self.modules.get(&parent) else {
                // File-root path components are semantic package identity, not
                // source DAG declarations, and therefore carry no visibility.
                return Ok(());
            };
            let Some(symbol) = child
                .leaf()
                .spelling()
                // `DagSegment` spells source modules as text; classify the
                // leaf into the declaration namespace at this boundary.
                .and_then(|name| DeclName::try_new(name).ok())
                .and_then(|name| parent_symbols.decls.get(&name))
            else {
                // Synthetic include namespaces have a semantic parent but no
                // source `dag` declaration on that edge.
                return Ok(());
            };
            if *symbol.data() != DeclSymbolKind::Dag {
                return Ok(());
            }
            if !symbol.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: parent,
                    namespace: "dag",
                    name: child.leaf().to_string(),
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
/// re-exports) with the canonical target each denotes.
fn public_targets<'a, Ns: SymbolTables>(
    symbols: &'a super::symbols::ModuleSymbols,
    scope: &'a super::scope::ModuleScope,
) -> impl Iterator<Item = (&'a NameAtom, &'a ResolvedName<Ns>)> {
    public(Ns::declared(symbols))
        .map(|(name, symbol)| (name, symbol.resolved()))
        .chain(public(Ns::selected(scope)).map(|(name, symbol)| (name, symbol.resolved())))
}
