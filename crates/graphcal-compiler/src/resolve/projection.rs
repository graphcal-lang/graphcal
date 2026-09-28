//! Selective-include projection of Static aliases onto effective bindings.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
    ResolvedUnitName,
};
use crate::syntax::ast::{
    BindableVisibility, ExprKind, ImportKind, InputBindingCategory, UnresolvedRef,
};
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexName;
use crate::syntax::type_name::{ConstructorName, ConstructorNameNamespace, StructTypeName};

use super::ModuleResolver;
use super::category::{SymbolTable, include_projection};
use super::error::{ModuleResolveError, NameCategory};
use super::imports::ExportLookup;
use super::scope::Access;
use super::symbols::{ConstructorSignature, Symbol};

impl ModuleResolver {
    /// Redirect selected Static aliases to supplied effective binding targets.
    ///
    /// Include selection normally points at the instantiated source declaration.
    /// A bound type, dimension, or declared index instead projects the concrete
    /// importer-side target, so its local alias must resolve to that identity.
    #[expect(
        clippy::too_many_lines,
        clippy::single_match_else,
        reason = "typed Static projection updates three disjoint resolver namespaces; explicit matches keep fallback identity construction visible"
    )]
    pub fn apply_include_static_projection_bindings(
        &mut self,
        owner: &DagId,
        source_target: Option<&DagId>,
        include: &ast::IncludeDecl,
    ) -> Result<(), ModuleResolveError> {
        let ImportKind::Selective(items) = &include.kind else {
            return Ok(());
        };
        let has_static_bindings = include
            .param_bindings
            .iter()
            .any(|binding| binding.category != InputBindingCategory::Unmarked);
        let has_dimension_bindings = include
            .param_bindings
            .iter()
            .any(|binding| binding.category == InputBindingCategory::Dimension);
        for item in items {
            if let Some(target) = source_target {
                for addition in self.import_item_additions(target, item, Access::CrossModule)? {
                    let Some(kind) = self.import_target_kind(&addition.target)? else {
                        continue;
                    };
                    if include_projection(kind).is_none() {
                        return Err(ModuleResolveError::IncludeItemNotProjectable {
                            owner: target.clone(),
                            name: item.name.name.atom().clone(),
                            kind,
                            span: item.name.span,
                        });
                    }
                }
            }
            let category = match item.namespace {
                ImportItemNamespace::Type => Some(InputBindingCategory::Type),
                ImportItemNamespace::Dimension => Some(InputBindingCategory::Dimension),
                ImportItemNamespace::Index => Some(InputBindingCategory::Index),
                ImportItemNamespace::Unit | ImportItemNamespace::Term => None,
            };
            let binding = category.and_then(|category| {
                include.param_bindings.iter().find(|binding| {
                    binding.category == category && binding.name.name == item.name.name
                })
            });
            let binding_path = binding.and_then(|binding| match &binding.value.kind {
                ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => Some(path.to_name_path()),
                _ => None,
            });
            let visibility = BindableVisibility::from(item.visibility);
            let local = item.local_name_atom().clone();
            let source = item.name.name.clone();
            match item.namespace {
                ImportItemNamespace::Type => {
                    let source_name = StructTypeName::classify(source.into_atom());
                    if binding_path.is_none() && has_static_bindings {
                        let Some(target) = source_target else {
                            continue;
                        };
                        let source_symbol = self
                            .module_symbols(target)?
                            .struct_types
                            .get(&source_name)
                            .ok_or_else(|| ModuleResolveError::UnknownName {
                                owner: target.clone(),
                                category: NameCategory::Table(SymbolTable::StructType),
                                name: source_name.atom().clone(),
                            })?;
                        let generic_params = source_symbol.data().clone();
                        let local_name = StructTypeName::classify(local);
                        self.scopes
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .selected_struct_types
                            .remove(&local_name);
                        self.modules
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .struct_types
                            .insert(
                                local_name,
                                Symbol::new(
                                    ResolvedStructTypeName::from_def(owner.clone(), source_name),
                                    visibility,
                                    item.local_span(),
                                    generic_params,
                                ),
                            );
                        continue;
                    }
                    let resolved = match binding_path {
                        Some(path) => self.resolve_struct_type_path(owner, &path)?,
                        None => {
                            let Some(target) = source_target else {
                                continue;
                            };
                            ResolvedStructTypeName::from_def(target.clone(), source_name)
                        }
                    };
                    self.scopes
                        .get_mut(owner)
                        .ok_or_else(|| ModuleResolveError::UnknownModule {
                            owner: owner.clone(),
                        })?
                        .selected_struct_types
                        .insert(
                            StructTypeName::classify(local),
                            Symbol::new(resolved, visibility, item.local_span(), ()),
                        );
                }
                ImportItemNamespace::Dimension => {
                    if binding_path.is_none() && has_dimension_bindings {
                        // A dimension defined over the instance's dimension
                        // ports (`QR = Q / Time`) is specialized by this
                        // include, so the projection is the importer's own
                        // declaration rather than the template's identity.
                        let local_name = DimName::classify(local);
                        self.scopes
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .selected_dimensions
                            .remove(&local_name);
                        self.modules
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .dimensions
                            .insert(
                                local_name.clone(),
                                Symbol::new(
                                    ResolvedDimName::from_def(owner.clone(), local_name),
                                    visibility,
                                    item.local_span(),
                                    (),
                                ),
                            );
                        continue;
                    }
                    let resolved = match binding_path {
                        Some(path) => self.resolve_dimension_path(owner, &path)?,
                        None => {
                            let Some(target) = source_target else {
                                continue;
                            };
                            ResolvedDimName::from_def(
                                target.clone(),
                                DimName::classify(source.into_atom()),
                            )
                        }
                    };
                    self.scopes
                        .get_mut(owner)
                        .ok_or_else(|| ModuleResolveError::UnknownModule {
                            owner: owner.clone(),
                        })?
                        .selected_dimensions
                        .insert(
                            DimName::classify(local),
                            Symbol::new(resolved, visibility, item.local_span(), ()),
                        );
                }
                ImportItemNamespace::Index => {
                    if binding.is_some() && binding_path.is_none() {
                        let local = IndexName::classify(local);
                        self.modules
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .indexes
                            .insert(
                                local.clone(),
                                Symbol::new(
                                    ResolvedIndexName::from_def(owner.clone(), local),
                                    visibility,
                                    item.local_span(),
                                    HashMap::new(),
                                ),
                            );
                        continue;
                    }
                    let resolved = match binding_path {
                        Some(path) => self.resolve_index_path(owner, &path)?,
                        None => {
                            let Some(target) = source_target else {
                                continue;
                            };
                            ResolvedIndexName::from_def(
                                target.clone(),
                                IndexName::classify(source.into_atom()),
                            )
                        }
                    };
                    self.scopes
                        .get_mut(owner)
                        .ok_or_else(|| ModuleResolveError::UnknownModule {
                            owner: owner.clone(),
                        })?
                        .selected_indexes
                        .insert(
                            IndexName::classify(local),
                            Symbol::new(resolved, visibility, item.local_span(), ()),
                        );
                }
                ImportItemNamespace::Unit => {
                    let Some(target) = source_target else {
                        continue;
                    };
                    let resolved = ResolvedUnitName::from_def(
                        target.clone(),
                        UnitName::classify(source.into_atom()),
                    );
                    self.scopes
                        .get_mut(owner)
                        .ok_or_else(|| ModuleResolveError::UnknownModule {
                            owner: owner.clone(),
                        })?
                        .selected_units
                        .insert(
                            UnitName::classify(local),
                            Symbol::new(resolved, visibility, item.local_span(), ()),
                        );
                }
                ImportItemNamespace::Term => {
                    let Some(source_target) = source_target else {
                        continue;
                    };
                    let source_constructor =
                        ConstructorName::classify(item.name.name.atom().clone());
                    let resolved = match self
                        .exported_symbol_for_import::<ConstructorNameNamespace>(
                            source_target,
                            source_constructor.atom(),
                            Access::CrossModule,
                        )? {
                        ExportLookup::Public(resolved) => resolved,
                        ExportLookup::Private | ExportLookup::Missing => continue,
                    };
                    let owner_type = self.constructor_owner_type(&resolved)?;
                    let source_symbol = self
                        .module_symbols(resolved.owner())?
                        .constructors
                        .get(&source_constructor)
                        .cloned()
                        .ok_or_else(|| ModuleResolveError::UnknownName {
                            owner: resolved.owner().clone(),
                            category: NameCategory::Table(SymbolTable::Constructor),
                            name: source_constructor.atom().clone(),
                        })?;
                    if include.param_bindings.iter().any(|binding| {
                        binding.category == InputBindingCategory::Type
                            && binding.name.name.atom() == owner_type.atom()
                    }) {
                        return Err(ModuleResolveError::ConstructorOwnerRebound {
                            owner: owner.clone(),
                            constructor: source_constructor,
                            owner_type,
                            span: item.name.span,
                        });
                    }
                    let has_specialized_owner = has_static_bindings
                        && items.iter().any(|candidate| {
                            candidate.namespace == ImportItemNamespace::Type
                                && candidate.name.name.atom() == owner_type.atom()
                        });
                    let resolved = if has_specialized_owner {
                        let resolved = ResolvedConstructorName::from_def(
                            owner.clone(),
                            resolved.to_unowned_def_name(),
                        );
                        self.modules
                            .get_mut(owner)
                            .ok_or_else(|| ModuleResolveError::UnknownModule {
                                owner: owner.clone(),
                            })?
                            .constructors
                            .insert(
                                resolved.to_unowned_def_name(),
                                Symbol::new(
                                    resolved.clone(),
                                    visibility,
                                    item.local_span(),
                                    ConstructorSignature {
                                        owner_type: owner_type.to_unowned_def_name(),
                                        generic_params: source_symbol.data().generic_params.clone(),
                                    },
                                ),
                            );
                        resolved
                    } else {
                        resolved
                    };
                    let scope = self.scopes.get_mut(owner).ok_or_else(|| {
                        ModuleResolveError::UnknownModule {
                            owner: owner.clone(),
                        }
                    })?;
                    let local = ConstructorName::classify(local);
                    let (span, visibility) = scope.selected_constructors.get(&local).map_or_else(
                        || (item.local_span(), visibility),
                        |existing| (existing.span(), existing.visibility()),
                    );
                    scope
                        .selected_constructors
                        .insert(local, Symbol::new(resolved, visibility, span, ()));
                }
            }
        }
        Ok(())
    }
}
