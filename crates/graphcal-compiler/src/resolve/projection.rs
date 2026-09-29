//! Selective-include projection of Static aliases onto effective bindings.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::registry::index::FiniteIndex;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use crate::syntax::ast::{
    BindableVisibility, ExprKind, ImportKind, InputBindingCategory, UnresolvedRef,
};
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::{IndexName, IndexNameNamespace};
use crate::syntax::names::NameAtom;
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::ModuleResolver;
use super::category::SymbolTable;
use super::error::{ModuleResolveError, NameCategory};
use super::imports::ExportLookup;
use super::scope::Access;
use super::symbols::{DimensionPortBinding, DimensionProjection, StaticProjection, Symbol};
use super::tables::NamespaceTables;

impl ModuleResolver {
    /// Redirect selected Static aliases to supplied effective binding targets.
    ///
    /// Include selection normally points at the instantiated source declaration.
    /// A bound type, dimension, or declared index instead projects the concrete
    /// importer-side target, so its local alias must resolve to that identity.
    #[expect(
        clippy::too_many_lines,
        reason = "typed Static projection updates three disjoint resolver namespaces; explicit matches keep fallback identity construction visible"
    )]
    pub(super) fn apply_include_static_projection_bindings(
        &mut self,
        owner: &DagId,
        template: &DagId,
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
            let source = item.name.name.atom();
            let local_span = item.local_span();
            match item.namespace {
                ImportItemNamespace::Type => {
                    let source_name = StructTypeName::classify(source.clone());
                    if binding_path.is_none() && has_static_bindings {
                        let source_symbol = self
                            .module_symbols(template)?
                            .struct_types
                            .get(&source_name)
                            .ok_or_else(|| ModuleResolveError::UnknownName {
                                owner: template.clone(),
                                category: NameCategory::Table(SymbolTable::StructType),
                                name: source_name.atom().clone(),
                            })?;
                        let generic_params = source_symbol.data().clone();
                        let local_name = StructTypeName::classify(local);
                        let projection = StaticProjection::new(
                            ResolvedStructTypeName::from_def(
                                super::mint::ResolverMint(()),
                                template.clone(),
                                source_name.clone(),
                            ),
                            include.instance_scope(),
                        );
                        let entry = self.entry_mut(owner)?;
                        entry.scope.selected_struct_types.remove(&local_name);
                        entry
                            .symbols
                            .struct_type_projections
                            .insert(local_name.clone(), projection);
                        entry.symbols.struct_types.insert(
                            local_name,
                            Symbol::new(
                                ResolvedStructTypeName::from_def(
                                    super::mint::ResolverMint(()),
                                    owner.clone(),
                                    source_name,
                                ),
                                visibility,
                                local_span,
                                generic_params,
                            ),
                        );
                        continue;
                    }
                    let selected = match binding_path {
                        Some(path) => self
                            .resolve_struct_type_path(owner, &path)?
                            .rebind(visibility, local_span),
                        None => self
                            .template_export::<StructTypeNameNamespace>(template, source)?
                            .rebind(visibility, local_span),
                    };
                    self.entry_mut(owner)?
                        .scope
                        .selected_struct_types
                        .insert(StructTypeName::classify(local), selected);
                }
                ImportItemNamespace::Dimension => {
                    if binding_path.is_none() && has_dimension_bindings {
                        // A dimension defined over the instance's dimension
                        // ports (`QR = Q / Time`) is specialized by this
                        // include, so the projection is the importer's own
                        // declaration rather than the template's identity.
                        let local_name = DimName::classify(local);
                        let projection = DimensionProjection::new(
                            StaticProjection::new(
                                ResolvedDimName::from_def(
                                    super::mint::ResolverMint(()),
                                    template.clone(),
                                    DimName::classify(source.clone()),
                                ),
                                include.instance_scope(),
                            ),
                            dimension_port_bindings(include),
                        );
                        let entry = self.entry_mut(owner)?;
                        entry.scope.selected_dimensions.remove(&local_name);
                        entry
                            .symbols
                            .dimension_projections
                            .insert(local_name.clone(), projection);
                        entry.symbols.dimensions.insert(
                            local_name.clone(),
                            Symbol::new(
                                ResolvedDimName::from_def(
                                    super::mint::ResolverMint(()),
                                    owner.clone(),
                                    local_name,
                                ),
                                visibility,
                                local_span,
                                (),
                            ),
                        );
                        continue;
                    }
                    let selected = match binding_path {
                        Some(path) => self
                            .resolve_dimension_path(owner, &path)?
                            .rebind(visibility, local_span),
                        None => self
                            .template_export::<DimNameNamespace>(template, source)?
                            .rebind(visibility, local_span),
                    };
                    self.entry_mut(owner)?
                        .scope
                        .selected_dimensions
                        .insert(DimName::classify(local), selected);
                }
                ImportItemNamespace::Index => {
                    if let Some(binding) = binding
                        && binding_path.is_none()
                    {
                        let local = IndexName::classify(local);
                        let finite = finite_index_binding(&binding.value);
                        let symbols = &mut self.entry_mut(owner)?.symbols;
                        if let Some(finite) = finite {
                            symbols
                                .finite_index_projections
                                .insert(local.clone(), finite);
                        }
                        symbols.indexes.insert(
                            local.clone(),
                            Symbol::new(
                                ResolvedIndexName::from_def(
                                    super::mint::ResolverMint(()),
                                    owner.clone(),
                                    local,
                                ),
                                visibility,
                                local_span,
                                HashMap::new(),
                            ),
                        );
                        continue;
                    }
                    let selected = match binding_path {
                        Some(path) => self
                            .resolve_index_path(owner, &path)?
                            .rebind(visibility, local_span),
                        None => self
                            .template_export::<IndexNameNamespace>(template, source)?
                            .rebind(visibility, local_span),
                    };
                    self.entry_mut(owner)?
                        .scope
                        .selected_indexes
                        .insert(IndexName::classify(local), selected);
                }
                ImportItemNamespace::Unit => {
                    let selected = self
                        .template_export::<UnitNameNamespace>(template, source)?
                        .rebind(visibility, local_span);
                    self.entry_mut(owner)?
                        .scope
                        .selected_units
                        .insert(UnitName::classify(local), selected);
                }
                ImportItemNamespace::Term => {
                    let source_constructor = ConstructorName::classify(source.clone());
                    let constructor = match self
                        .exported_symbol_for_import::<ConstructorNameNamespace>(
                            template,
                            source,
                            Access::CrossModule,
                        )? {
                        ExportLookup::Public(constructor) => constructor,
                        ExportLookup::Private | ExportLookup::Missing => continue,
                    };
                    let owner_type = &constructor.data().owner_type;
                    if include.param_bindings.iter().any(|binding| {
                        binding.category == InputBindingCategory::Type
                            && binding.name.name.atom() == owner_type.atom()
                    }) {
                        return Err(ModuleResolveError::ConstructorOwnerRebound {
                            owner: owner.clone(),
                            constructor: source_constructor,
                            owner_type: ResolvedStructTypeName::from_def(
                                super::mint::ResolverMint(()),
                                constructor.resolved().owner().clone(),
                                owner_type.clone(),
                            ),
                            span: item.name.span,
                        });
                    }
                    let has_specialized_owner = has_static_bindings
                        && items.iter().any(|candidate| {
                            candidate.namespace == ImportItemNamespace::Type
                                && candidate.name.name.atom() == owner_type.atom()
                        });
                    let constructor = if has_specialized_owner {
                        let specialized = Symbol::new(
                            ResolvedConstructorName::from_def(
                                super::mint::ResolverMint(()),
                                owner.clone(),
                                constructor.resolved().to_unowned_def_name(),
                            ),
                            visibility,
                            local_span,
                            constructor.data().clone(),
                        );
                        self.entry_mut(owner)?.symbols.constructors.insert(
                            specialized.resolved().to_unowned_def_name(),
                            specialized.clone(),
                        );
                        specialized
                    } else {
                        constructor
                    };
                    let scope = &mut self.entry_mut(owner)?.scope;
                    let local = ConstructorName::classify(local);
                    let (span, visibility) = scope
                        .selected_constructors
                        .get(&local)
                        .map_or((local_span, visibility), |existing| {
                            (existing.span(), existing.visibility())
                        });
                    scope
                        .selected_constructors
                        .insert(local, constructor.rebind(visibility, span));
                }
            }
        }
        Ok(())
    }

    /// The binding `template` publicly exports as `atom` in one namespace.
    fn template_export<Ns: NamespaceTables>(
        &self,
        template: &DagId,
        atom: &NameAtom,
    ) -> Result<Symbol<Ns, Ns::Declared>, ModuleResolveError> {
        match self.exported_symbol_for_import::<Ns>(template, atom, Access::CrossModule)? {
            ExportLookup::Public(symbol) => Ok(symbol),
            ExportLookup::Private => Err(ModuleResolveError::PrivateName {
                owner: template.clone(),
                category: NameCategory::Table(Ns::TABLE),
                name: atom.clone(),
            }),
            ExportLookup::Missing => Err(ModuleResolveError::UnknownName {
                owner: template.clone(),
                category: NameCategory::Table(Ns::TABLE),
                name: atom.clone(),
            }),
        }
    }
}

/// The include's `dim Port: Target` bindings whose target is a name path.
///
/// A non-path target is not a dimension; the include's binding validation
/// reports it, and the port stays opaque in the projection.
fn dimension_port_bindings(include: &ast::IncludeDecl) -> Vec<DimensionPortBinding> {
    include
        .param_bindings
        .iter()
        .filter(|binding| binding.category == InputBindingCategory::Dimension)
        .filter_map(|binding| match &binding.value.kind {
            ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => Some(DimensionPortBinding::new(
                DimName::classify(binding.name.name.atom().clone()),
                path.to_name_path(),
            )),
            _ => None,
        })
        .collect()
}

/// The structural index an index binding value names, when it is a concrete
/// `Fin(N)`.
///
/// Any other value is rejected by the include's binding validation.
fn finite_index_binding(value: &ast::Expr) -> Option<FiniteIndex> {
    match value.index_binding_arg()? {
        ast::IndexExpr::Finite { cardinality, .. } => {
            FiniteIndex::try_from_u64(concrete_nat_value(&cardinality)?).ok()
        }
        ast::IndexExpr::Name(_) | ast::IndexExpr::BareNat(_) => None,
    }
}

/// Evaluate a variable-free natural-number expression, or `None` when it has
/// a variable or overflows.
fn concrete_nat_value(expr: &ast::NatExpr) -> Option<u64> {
    match expr {
        ast::NatExpr::Literal(value, _) => Some(*value),
        ast::NatExpr::Var(_) => None,
        ast::NatExpr::Add(operands, _) => operands.iter().try_fold(0_u64, |sum, operand| {
            sum.checked_add(concrete_nat_value(operand)?)
        }),
        ast::NatExpr::Mul(operands, _) => operands.iter().try_fold(1_u64, |product, operand| {
            product.checked_mul(concrete_nat_value(operand)?)
        }),
    }
}
