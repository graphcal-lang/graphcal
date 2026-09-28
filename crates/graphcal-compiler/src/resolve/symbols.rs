//! Per-module declaration symbol tables collected from a desugared AST.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::{
    ResolvedConstructorName, ResolvedDeclName, ResolvedIndexName, ResolvedName,
    ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::ast::{BindableVisibility, UnitConstness};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::index_name::{
    IndexName, IndexNameNamespace, IndexVariantName, IndexVariantNameNamespace,
};
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::category::DeclSymbolKind;
use super::error::ModuleResolveError;
use super::namespace::{ExclusiveNameKind, ExclusiveNameOccupancy, FlatNamespace};

/// A declaration symbol in one semantic namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbol<Ns: NameNamespace> {
    pub(super) resolved: ResolvedName<Ns>,
    pub(super) visibility: BindableVisibility,
    pub(super) span: Span,
}

impl<Ns: NameNamespace> ModuleSymbol<Ns> {
    pub(super) fn new(
        owner: &DagId,
        name: NameDef<Ns>,
        visibility: BindableVisibility,
        span: Span,
    ) -> Self {
        Self {
            resolved: ResolvedName::from_def(owner.clone(), name),
            visibility,
            span,
        }
    }

    /// Canonical resolved identity for this symbol.
    #[must_use]
    pub(crate) const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Visibility of this symbol across module boundaries.
    #[must_use]
    pub(super) const fn visibility(&self) -> BindableVisibility {
        self.visibility
    }

    /// Source span of the definition-site name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.span
    }
}

pub(super) trait ModuleSymbolLookup<Ns: NameNamespace> {
    fn resolved(&self) -> &ResolvedName<Ns>;
    fn visibility(&self) -> BindableVisibility;
    fn span(&self) -> Span;
}

impl<Ns: NameNamespace> ModuleSymbolLookup<Ns> for ModuleSymbol<Ns> {
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

/// Value/declaration symbol plus its semantic declaration kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDeclSymbol {
    pub(super) symbol: ModuleSymbol<DeclNameNamespace>,
    pub(super) kind: DeclSymbolKind,
}

impl ModuleDeclSymbol {
    pub(super) fn new(
        owner: &DagId,
        name: DeclName,
        visibility: BindableVisibility,
        span: Span,
        kind: DeclSymbolKind,
    ) -> Self {
        Self {
            symbol: ModuleSymbol::new(owner, name, visibility, span),
            kind,
        }
    }

    /// Canonical resolved identity for this declaration.
    #[must_use]
    pub(super) const fn resolved(&self) -> &ResolvedDeclName {
        self.symbol.resolved()
    }

    /// Visibility of this declaration across module boundaries.
    #[must_use]
    pub(super) const fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    /// Source span of the definition-site name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.symbol.span()
    }

    /// Semantic declaration kind.
    #[must_use]
    pub(super) const fn kind(&self) -> DeclSymbolKind {
        self.kind
    }
}

impl ModuleSymbolLookup<DeclNameNamespace> for ModuleDeclSymbol {
    fn resolved(&self) -> &ResolvedDeclName {
        self.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Source signature of one generic parameter, retained by name resolution so
/// HIR can sort application arguments after resolving the callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenericParamSignature {
    pub(crate) name: crate::syntax::type_name::GenericParamName,
    pub(crate) constraint: ast::GenericConstraint,
    pub(crate) has_default: bool,
}

impl GenericParamSignature {
    fn from_param(param: &ast::GenericParam) -> Self {
        Self {
            name: param.name.value.clone(),
            constraint: param.constraint,
            has_default: param.default.is_some(),
        }
    }
}

/// Type symbol plus its declared generic-parameter signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleTypeSymbol {
    pub(super) symbol: ModuleSymbol<StructTypeNameNamespace>,
    pub(super) generic_params: Vec<GenericParamSignature>,
}

impl ModuleTypeSymbol {
    pub(super) fn generic_params(&self) -> &[GenericParamSignature] {
        &self.generic_params
    }

    pub(crate) const fn resolved(&self) -> &ResolvedStructTypeName {
        self.symbol.resolved()
    }

    pub(crate) const fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    pub(crate) const fn span(&self) -> Span {
        self.symbol.span()
    }
}

impl ModuleSymbolLookup<StructTypeNameNamespace> for ModuleTypeSymbol {
    fn resolved(&self) -> &ResolvedStructTypeName {
        self.symbol.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    fn span(&self) -> Span {
        self.symbol.span()
    }
}

/// Unit symbol plus whether its scale is compile-time or instance-specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleUnitSymbol {
    pub(super) symbol: ModuleSymbol<UnitNameNamespace>,
    pub(super) constness: UnitConstness,
}

impl ModuleUnitSymbol {
    pub(super) const fn constness(&self) -> UnitConstness {
        self.constness
    }
}

impl ModuleSymbolLookup<UnitNameNamespace> for ModuleUnitSymbol {
    fn resolved(&self) -> &ResolvedUnitName {
        self.symbol.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    fn span(&self) -> Span {
        self.symbol.span()
    }
}

/// Constructor symbol plus the identity and generic signature of its owning type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleConstructorSymbol {
    pub(super) symbol: ModuleSymbol<ConstructorNameNamespace>,
    pub(super) owner_type: StructTypeName,
    pub(super) generic_params: Vec<GenericParamSignature>,
}

impl ModuleConstructorSymbol {
    pub(super) const fn owner_type(&self) -> &StructTypeName {
        &self.owner_type
    }

    pub(super) fn generic_params(&self) -> &[GenericParamSignature] {
        &self.generic_params
    }
}

impl ModuleSymbolLookup<ConstructorNameNamespace> for ModuleConstructorSymbol {
    fn resolved(&self) -> &ResolvedConstructorName {
        self.symbol.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    fn span(&self) -> Span {
        self.symbol.span()
    }
}

/// Index symbol plus the variants declared by that index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIndexSymbol {
    pub(super) symbol: ModuleSymbol<IndexNameNamespace>,
    pub(super) variants: HashMap<IndexVariantName, Span>,
}

impl ModuleIndexSymbol {
    /// Canonical resolved identity for the index type.
    #[must_use]
    pub(crate) const fn resolved(&self) -> &ResolvedIndexName {
        self.symbol.resolved()
    }

    /// Visibility of the index declaration.
    #[must_use]
    pub(crate) const fn visibility(&self) -> BindableVisibility {
        self.symbol.visibility()
    }

    /// Source span of the index definition-site name.
    #[must_use]
    pub(super) const fn span(&self) -> Span {
        self.symbol.span()
    }

    /// Variant names declared by this index, keyed by leaf name.
    #[must_use]
    pub(crate) const fn variants(&self) -> &HashMap<IndexVariantName, Span> {
        &self.variants
    }
}

impl ModuleSymbolLookup<IndexNameNamespace> for ModuleIndexSymbol {
    fn resolved(&self) -> &ResolvedIndexName {
        self.resolved()
    }

    fn visibility(&self) -> BindableVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Symbols declared by a single DAG/module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbols {
    pub(super) owner: DagId,
    pub(super) decls: HashMap<DeclName, ModuleDeclSymbol>,
    pub(super) dimensions: HashMap<DimName, ModuleSymbol<DimNameNamespace>>,
    pub(super) units: HashMap<UnitName, ModuleUnitSymbol>,
    pub(super) struct_types: HashMap<StructTypeName, ModuleTypeSymbol>,
    pub(super) indexes: HashMap<IndexName, ModuleIndexSymbol>,
    pub(super) constructors: HashMap<ConstructorName, ModuleConstructorSymbol>,
}

impl ModuleSymbols {
    /// Build a module symbol table from a declaration list.
    ///
    /// The `owner` is the canonical DAG/module identity assigned by the loader.
    /// The declarations are not modified; this is a pure collection pass.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateSymbol`] when two definitions in
    /// the same namespace share a leaf name.
    pub(super) fn from_declarations(
        owner: DagId,
        declarations: &[ast::Declaration],
    ) -> Result<Self, ModuleResolveError> {
        let mut symbols = Self {
            owner,
            decls: HashMap::new(),
            dimensions: HashMap::new(),
            units: HashMap::new(),
            struct_types: HashMap::new(),
            indexes: HashMap::new(),
            constructors: HashMap::new(),
        };

        symbols.collect_declarations(declarations)?;
        Ok(symbols)
    }

    /// The canonical owner for this table.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Value/declaration namespace symbols.
    #[must_use]
    pub(crate) const fn decls(&self) -> &HashMap<DeclName, ModuleDeclSymbol> {
        &self.decls
    }

    /// Dimension namespace symbols.
    #[must_use]
    pub(crate) const fn dimensions(&self) -> &HashMap<DimName, ModuleSymbol<DimNameNamespace>> {
        &self.dimensions
    }

    /// Unit namespace symbols.
    #[must_use]
    pub(crate) const fn units(&self) -> &HashMap<UnitName, ModuleUnitSymbol> {
        &self.units
    }

    /// Struct/tagged-union type namespace symbols.
    #[must_use]
    pub(crate) const fn struct_types(&self) -> &HashMap<StructTypeName, ModuleTypeSymbol> {
        &self.struct_types
    }

    /// Index namespace symbols.
    #[must_use]
    pub(crate) const fn indexes(&self) -> &HashMap<IndexName, ModuleIndexSymbol> {
        &self.indexes
    }

    /// Tagged-union constructor namespace symbols.
    #[must_use]
    pub(super) const fn constructors(&self) -> &HashMap<ConstructorName, ModuleConstructorSymbol> {
        &self.constructors
    }

    fn collect_declarations(
        &mut self,
        declarations: &[ast::Declaration],
    ) -> Result<(), ModuleResolveError> {
        let mut exclusive_names = HashMap::new();
        for decl in declarations {
            match &decl.kind {
                ast::DeclKind::Param(p) => self.insert_value_decl(
                    &mut exclusive_names,
                    &p.name,
                    BindableVisibility::PublicBind,
                    DeclSymbolKind::Param,
                )?,
                ast::DeclKind::Node(n) => self.insert_value_decl(
                    &mut exclusive_names,
                    &n.name,
                    BindableVisibility::from(n.visibility),
                    DeclSymbolKind::Node,
                )?,
                ast::DeclKind::ConstNode(c) => self.insert_value_decl(
                    &mut exclusive_names,
                    &c.name,
                    BindableVisibility::from(c.visibility),
                    DeclSymbolKind::Const,
                )?,
                ast::DeclKind::Assert(a) => self.insert_value_decl(
                    &mut exclusive_names,
                    &a.name,
                    BindableVisibility::from(a.visibility),
                    DeclSymbolKind::Assert,
                )?,
                ast::DeclKind::Plot(p) => self.insert_value_decl(
                    &mut exclusive_names,
                    &p.name,
                    BindableVisibility::from(p.visibility),
                    DeclSymbolKind::Plot,
                )?,
                ast::DeclKind::Figure(f) => self.insert_value_decl(
                    &mut exclusive_names,
                    &f.name,
                    BindableVisibility::from(f.visibility),
                    DeclSymbolKind::Figure,
                )?,
                ast::DeclKind::Layer(l) => self.insert_value_decl(
                    &mut exclusive_names,
                    &l.name,
                    BindableVisibility::from(l.visibility),
                    DeclSymbolKind::Layer,
                )?,
                ast::DeclKind::Dag(d) => self.insert_value_decl(
                    &mut exclusive_names,
                    &d.name,
                    BindableVisibility::from(d.visibility),
                    DeclSymbolKind::Dag,
                )?,
                ast::DeclKind::BaseDimension(d) => self.insert_dimension_decl(
                    &mut exclusive_names,
                    &d.name,
                    BindableVisibility::from(d.visibility),
                )?,
                ast::DeclKind::Dimension(d) => {
                    self.insert_dimension_decl(&mut exclusive_names, &d.name, d.visibility)?;
                }
                ast::DeclKind::Unit(u) => self.insert_unit(
                    &u.name,
                    BindableVisibility::from(u.visibility),
                    u.constness,
                    UnitNameNamespace::DISPLAY_NAME,
                )?,
                ast::DeclKind::Type(t) => self.insert_type_decl(&mut exclusive_names, t)?,
                ast::DeclKind::Index(i) => self.insert_index_decl(&mut exclusive_names, i)?,
                // Plugin imports register their alias into the module scope
                // (see `register_plugin_imports`), not the symbol table.
                ast::DeclKind::Import(_)
                | ast::DeclKind::PluginImport(_)
                | ast::DeclKind::Include(_) => {}
                #[expect(
                    clippy::uninhabited_references,
                    reason = "Sugar(Infallible) proves this arm unreachable"
                )]
                ast::DeclKind::Sugar(s) => never(*s),
            }
        }
        Ok(())
    }

    fn insert_value_decl(
        &mut self,
        exclusive_names: &mut ExclusiveNameOccupancy,
        name: &Spanned<DeclName>,
        visibility: BindableVisibility,
        kind: DeclSymbolKind,
    ) -> Result<(), ModuleResolveError> {
        self.insert_exclusive_name(
            exclusive_names,
            name.value.atom(),
            ExclusiveNameKind::Value,
            name.span,
        )?;
        self.insert_decl(name, visibility, DeclNameNamespace::DISPLAY_NAME, kind)
    }

    fn insert_dimension_decl(
        &mut self,
        exclusive_names: &mut ExclusiveNameOccupancy,
        name: &Spanned<DimName>,
        visibility: BindableVisibility,
    ) -> Result<(), ModuleResolveError> {
        self.insert_exclusive_name(
            exclusive_names,
            name.value.atom(),
            ExclusiveNameKind::Dimension,
            name.span,
        )?;
        self.insert_dimension(name, visibility, DimNameNamespace::DISPLAY_NAME)
    }

    fn insert_type_decl(
        &mut self,
        exclusive_names: &mut ExclusiveNameOccupancy,
        type_decl: &ast::TypeDecl,
    ) -> Result<(), ModuleResolveError> {
        let visibility = type_decl.visibility;
        self.insert_exclusive_name(
            exclusive_names,
            type_decl.name.value.atom(),
            ExclusiveNameKind::StructType,
            type_decl.name.span,
        )?;
        let generic_params = type_decl
            .generic_params
            .iter()
            .map(GenericParamSignature::from_param)
            .collect::<Vec<_>>();
        self.insert_struct_type(
            &type_decl.name,
            visibility,
            StructTypeNameNamespace::DISPLAY_NAME,
            generic_params.clone(),
        )?;
        if let ast::TypeDeclBody::Constructors(members) = &type_decl.body {
            for member in members {
                self.insert_exclusive_name(
                    exclusive_names,
                    member.name.value.atom(),
                    ExclusiveNameKind::Constructor,
                    member.name.span,
                )?;
                self.insert_constructor(
                    &member.name,
                    &type_decl.name.value,
                    visibility,
                    ConstructorNameNamespace::DISPLAY_NAME,
                    generic_params.clone(),
                )?;
            }
        }
        Ok(())
    }

    fn insert_index_decl(
        &mut self,
        exclusive_names: &mut ExclusiveNameOccupancy,
        index: &ast::IndexDecl,
    ) -> Result<(), ModuleResolveError> {
        self.insert_exclusive_name(
            exclusive_names,
            index.name.value.atom(),
            ExclusiveNameKind::Index,
            index.name.span,
        )?;
        self.insert_index(index)
    }

    fn insert_exclusive_name(
        &self,
        occupied: &mut ExclusiveNameOccupancy,
        atom: &NameAtom,
        kind: ExclusiveNameKind,
        span: Span,
    ) -> Result<(), ModuleResolveError> {
        let namespace = kind.namespace();
        let slot = (namespace, atom.clone());
        match occupied.entry(slot) {
            Entry::Occupied(entry) => Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: match namespace {
                    FlatNamespace::Static => "Static",
                    FlatNamespace::Term => "Term",
                },
                name: atom.to_string(),
                first: *entry.get(),
                duplicate: span,
            }),
            Entry::Vacant(entry) => {
                entry.insert(span);
                Ok(())
            }
        }
    }

    fn insert_decl(
        &mut self,
        name: &Spanned<DeclName>,
        visibility: BindableVisibility,
        namespace_name: &'static str,
        kind: DeclSymbolKind,
    ) -> Result<(), ModuleResolveError> {
        insert_decl_symbol(
            &self.owner,
            &mut self.decls,
            name,
            visibility,
            namespace_name,
            kind,
        )
    }

    fn insert_dimension(
        &mut self,
        name: &Spanned<DimName>,
        visibility: BindableVisibility,
        namespace_name: &'static str,
    ) -> Result<(), ModuleResolveError> {
        insert_symbol(
            &self.owner,
            &mut self.dimensions,
            name,
            visibility,
            namespace_name,
        )
    }

    fn insert_unit(
        &mut self,
        name: &Spanned<UnitName>,
        visibility: BindableVisibility,
        constness: UnitConstness,
        namespace_name: &'static str,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.units.get(&name.value) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: namespace_name,
                name: name.value.to_string(),
                first: first.span(),
                duplicate: name.span,
            });
        }
        self.units.insert(
            name.value.clone(),
            ModuleUnitSymbol {
                symbol: ModuleSymbol::new(&self.owner, name.value.clone(), visibility, name.span),
                constness,
            },
        );
        Ok(())
    }

    fn insert_struct_type(
        &mut self,
        name: &Spanned<StructTypeName>,
        visibility: BindableVisibility,
        namespace_name: &'static str,
        generic_params: Vec<GenericParamSignature>,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.struct_types.get(&name.value) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: namespace_name,
                name: name.value.to_string(),
                first: first.span(),
                duplicate: name.span,
            });
        }
        self.struct_types.insert(
            name.value.clone(),
            ModuleTypeSymbol {
                symbol: ModuleSymbol::new(&self.owner, name.value.clone(), visibility, name.span),
                generic_params,
            },
        );
        Ok(())
    }

    fn insert_constructor(
        &mut self,
        name: &Spanned<ConstructorName>,
        owner_type: &StructTypeName,
        visibility: BindableVisibility,
        namespace_name: &'static str,
        generic_params: Vec<GenericParamSignature>,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.constructors.get(&name.value) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: namespace_name,
                name: name.value.to_string(),
                first: first.span(),
                duplicate: name.span,
            });
        }
        self.constructors.insert(
            name.value.clone(),
            ModuleConstructorSymbol {
                symbol: ModuleSymbol::new(&self.owner, name.value.clone(), visibility, name.span),
                owner_type: owner_type.clone(),
                generic_params,
            },
        );
        Ok(())
    }

    fn insert_index(&mut self, index: &ast::IndexDecl) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.indexes.get(&index.name.value) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: IndexNameNamespace::DISPLAY_NAME,
                name: index.name.value.to_string(),
                first: first.span(),
                duplicate: index.name.span,
            });
        }

        let mut variants = HashMap::new();
        if let ast::IndexDeclKind::Named { variants: declared } = &index.kind {
            for variant in declared {
                if let Some(first) = variants.insert(variant.value.clone(), variant.span) {
                    return Err(ModuleResolveError::DuplicateSymbol {
                        owner: self.owner.clone(),
                        namespace: IndexVariantNameNamespace::DISPLAY_NAME,
                        name: variant.value.qualified_by(&index.name.value).to_string(),
                        first,
                        duplicate: variant.span,
                    });
                }
            }
        }

        self.indexes.insert(
            index.name.value.clone(),
            ModuleIndexSymbol {
                symbol: ModuleSymbol::new(
                    &self.owner,
                    index.name.value.clone(),
                    index.visibility,
                    index.name.span,
                ),
                variants,
            },
        );
        Ok(())
    }
}

fn insert_symbol<Ns: NameNamespace>(
    owner: &DagId,
    map: &mut HashMap<NameDef<Ns>, ModuleSymbol<Ns>>,
    name: &Spanned<NameDef<Ns>>,
    visibility: BindableVisibility,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(&name.value) {
        return Err(ModuleResolveError::DuplicateSymbol {
            owner: owner.clone(),
            namespace: namespace_name,
            name: name.value.to_string(),
            first: first.span(),
            duplicate: name.span,
        });
    }
    map.insert(
        name.value.clone(),
        ModuleSymbol::new(owner, name.value.clone(), visibility, name.span),
    );
    Ok(())
}

fn insert_decl_symbol(
    owner: &DagId,
    map: &mut HashMap<DeclName, ModuleDeclSymbol>,
    name: &Spanned<DeclName>,
    visibility: BindableVisibility,
    namespace_name: &'static str,
    kind: DeclSymbolKind,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(&name.value) {
        return Err(ModuleResolveError::DuplicateSymbol {
            owner: owner.clone(),
            namespace: namespace_name,
            name: name.value.to_string(),
            first: first.span(),
            duplicate: name.span,
        });
    }
    map.insert(
        name.value.clone(),
        ModuleDeclSymbol::new(owner, name.value.clone(), visibility, name.span, kind),
    );
    Ok(())
}
