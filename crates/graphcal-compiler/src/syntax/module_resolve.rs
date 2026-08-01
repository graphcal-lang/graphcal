//! Module-aware symbol tables backing HIR name resolution.
//!
//! This module is the first HIR/resolver-oriented layer after the syntax-first
//! name refactor. It does **not** rewrite the AST yet. Instead it builds typed
//! symbol tables for loaded DAG/module identities and resolves syntactic
//! [`NamePath`] / [`IdentPath`] references to canonical [`ResolvedName`] values.
//!
//! The important invariant is that source spelling is used only to look up a
//! scoped symbol or DAG-module binding. Imported aliases may name a file root or
//! inline DAG directly, and local DAGs may qualify their children. Every
//! successful lookup carries the canonical [`DagId`] owner, not textual path
//! conventions.

use crate::syntax::decl_name::{DeclNameNamespace, ResolvedDeclName};
use crate::syntax::dimension::{
    DimNameNamespace, ResolvedDimName, ResolvedUnitName, UnitNameNamespace,
};
use crate::syntax::index_name::{IndexNameNamespace, IndexVariantNameNamespace, ResolvedIndexName};
use crate::syntax::type_name::{
    ConstructorNameNamespace, ResolvedConstructorName, ResolvedStructTypeName,
    StructTypeNameNamespace,
};
use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::syntax::ast::{IdentPath, ImportItem, ImportKind, ModulePath};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitName};
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::index_name::{IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::module_name::{ModuleAliasName, ModuleAliasNameNamespace};
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, NamePath, ResolvedName};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, StructTypeName};

/// Visibility of a symbol across module boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolVisibility {
    /// Visible only inside the owning module.
    Private,
    /// Publicly visible to importers.
    Public,
    /// Public and bindable by include-time type/index/dimension bindings.
    PublicBind,
}

impl SymbolVisibility {
    /// Returns whether the symbol is visible outside its owning module.
    #[must_use]
    const fn is_public(self) -> bool {
        matches!(self, Self::Public | Self::PublicBind)
    }

    /// Returns whether the symbol can be rebound by include-time bindings.
    #[must_use]
    pub(crate) const fn is_bindable(self) -> bool {
        matches!(self, Self::PublicBind)
    }
}

impl From<ast::Visibility> for SymbolVisibility {
    fn from(visibility: ast::Visibility) -> Self {
        match visibility {
            ast::Visibility::Private => Self::Private,
            ast::Visibility::Public => Self::Public,
        }
    }
}

impl From<ast::BindableVisibility> for SymbolVisibility {
    fn from(visibility: ast::BindableVisibility) -> Self {
        match visibility {
            ast::BindableVisibility::Private => Self::Private,
            ast::BindableVisibility::Public => Self::Public,
            ast::BindableVisibility::PublicBind => Self::PublicBind,
        }
    }
}

/// Semantic kind of a value/declaration namespace symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclSymbolKind {
    Const,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
    Dag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ExclusiveNameKind {
    Value,
    Dimension,
    StructType,
    Index,
    Constructor,
}

impl ExclusiveNameKind {
    /// Whether two namespaces can collide at a source use site.
    ///
    /// Values, dimensions, types, and indexes share sort-inferred positions
    /// and are mutually exclusive. Constructors are term-sorted and therefore
    /// collide with values, but may coexist with their owning type (and with
    /// names confined to other sort-fixed positions).
    fn conflicts_with(self, other: Self) -> bool {
        match (self, other) {
            (Self::Constructor, Self::Value) | (Self::Value, Self::Constructor) => true,
            (Self::Constructor, _) | (_, Self::Constructor) => false,
            _ => self != other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExclusiveNameBinding {
    kind: ExclusiveNameKind,
    span: Span,
}

impl DeclSymbolKind {
    /// Returns whether this declaration can be referenced from const-like
    /// expression positions.
    #[must_use]
    pub(crate) const fn is_const(self) -> bool {
        matches!(self, Self::Const)
    }
}

impl std::fmt::Display for DeclSymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Const => "const",
            Self::Param => "param",
            Self::Node => "node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
            Self::Dag => "dag",
        };
        f.write_str(label)
    }
}

/// Visibility rule applied by a module alias or selective import edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleAccess {
    /// Cross-module import/include boundary: only public target symbols are accessible.
    PublicOnly,
}

impl ModuleAccess {
    const fn requires_public(self) -> bool {
        matches!(self, Self::PublicOnly)
    }
}

/// Semantic role of a module alias introduced by an import or include.
///
/// An imported module names a reusable DAG blueprint, so its alias may be
/// invoked directly (`@alias(args).out`) or used to reach a child DAG
/// (`@alias.child(args).out`). An included instance is already instantiated;
/// its alias is only a namespace for the selected instance's members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleAliasRole {
    /// Alias of a reusable file or inline-DAG module introduced by `import`.
    ImportedDag,
    /// Namespace of an already-instantiated DAG introduced by `include`.
    IncludedInstance,
}

impl ModuleAliasRole {
    const fn is_callable(self) -> bool {
        matches!(self, Self::ImportedDag)
    }
}

/// A declaration symbol in one semantic namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbol<Ns: NameNamespace> {
    resolved: ResolvedName<Ns>,
    visibility: SymbolVisibility,
    span: Span,
}

impl<Ns: NameNamespace> ModuleSymbol<Ns> {
    fn new(owner: &DagId, name: NameDef<Ns>, visibility: SymbolVisibility, span: Span) -> Self {
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
    const fn visibility(&self) -> SymbolVisibility {
        self.visibility
    }

    /// Source span of the definition-site name.
    #[must_use]
    const fn span(&self) -> Span {
        self.span
    }
}

trait ModuleSymbolLookup<Ns: NameNamespace> {
    fn resolved(&self) -> &ResolvedName<Ns>;
    fn visibility(&self) -> SymbolVisibility;
    fn span(&self) -> Span;
}

impl<Ns: NameNamespace> ModuleSymbolLookup<Ns> for ModuleSymbol<Ns> {
    fn resolved(&self) -> &ResolvedName<Ns> {
        self.resolved()
    }

    fn visibility(&self) -> SymbolVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Value/declaration symbol plus its semantic declaration kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDeclSymbol {
    symbol: ModuleSymbol<DeclNameNamespace>,
    kind: DeclSymbolKind,
}

impl ModuleDeclSymbol {
    fn new(
        owner: &DagId,
        name: DeclName,
        visibility: SymbolVisibility,
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
    const fn resolved(&self) -> &ResolvedDeclName {
        self.symbol.resolved()
    }

    /// Visibility of this declaration across module boundaries.
    #[must_use]
    const fn visibility(&self) -> SymbolVisibility {
        self.symbol.visibility()
    }

    /// Source span of the definition-site name.
    #[must_use]
    const fn span(&self) -> Span {
        self.symbol.span()
    }

    /// Semantic declaration kind.
    #[must_use]
    const fn kind(&self) -> DeclSymbolKind {
        self.kind
    }
}

impl ModuleSymbolLookup<DeclNameNamespace> for ModuleDeclSymbol {
    fn resolved(&self) -> &ResolvedDeclName {
        self.resolved()
    }

    fn visibility(&self) -> SymbolVisibility {
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
    symbol: ModuleSymbol<StructTypeNameNamespace>,
    generic_params: Vec<GenericParamSignature>,
}

impl ModuleTypeSymbol {
    fn generic_params(&self) -> &[GenericParamSignature] {
        &self.generic_params
    }

    pub(crate) const fn resolved(&self) -> &ResolvedStructTypeName {
        self.symbol.resolved()
    }
}

impl ModuleSymbolLookup<StructTypeNameNamespace> for ModuleTypeSymbol {
    fn resolved(&self) -> &ResolvedStructTypeName {
        self.symbol.resolved()
    }

    fn visibility(&self) -> SymbolVisibility {
        self.symbol.visibility()
    }

    fn span(&self) -> Span {
        self.symbol.span()
    }
}

/// Constructor symbol plus the generic signature of its owning type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleConstructorSymbol {
    symbol: ModuleSymbol<ConstructorNameNamespace>,
    generic_params: Vec<GenericParamSignature>,
}

impl ModuleConstructorSymbol {
    fn generic_params(&self) -> &[GenericParamSignature] {
        &self.generic_params
    }
}

impl ModuleSymbolLookup<ConstructorNameNamespace> for ModuleConstructorSymbol {
    fn resolved(&self) -> &ResolvedConstructorName {
        self.symbol.resolved()
    }

    fn visibility(&self) -> SymbolVisibility {
        self.symbol.visibility()
    }

    fn span(&self) -> Span {
        self.symbol.span()
    }
}

/// Index symbol plus the variants declared by that index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIndexSymbol {
    symbol: ModuleSymbol<IndexNameNamespace>,
    variants: HashMap<IndexVariantName, Span>,
}

impl ModuleIndexSymbol {
    /// Canonical resolved identity for the index type.
    #[must_use]
    const fn resolved(&self) -> &ResolvedIndexName {
        self.symbol.resolved()
    }

    /// Visibility of the index declaration.
    #[must_use]
    pub(crate) const fn visibility(&self) -> SymbolVisibility {
        self.symbol.visibility()
    }

    /// Source span of the index definition-site name.
    #[must_use]
    const fn span(&self) -> Span {
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

    fn visibility(&self) -> SymbolVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Symbols declared by a single DAG/module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbols {
    owner: DagId,
    decls: HashMap<DeclName, ModuleDeclSymbol>,
    dimensions: HashMap<DimName, ModuleSymbol<DimNameNamespace>>,
    units: HashMap<UnitName, ModuleSymbol<UnitNameNamespace>>,
    struct_types: HashMap<StructTypeName, ModuleTypeSymbol>,
    indexes: HashMap<IndexName, ModuleIndexSymbol>,
    constructors: HashMap<ConstructorName, ModuleConstructorSymbol>,
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
    fn from_declarations(
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
    const fn dimensions(&self) -> &HashMap<DimName, ModuleSymbol<DimNameNamespace>> {
        &self.dimensions
    }

    /// Unit namespace symbols.
    #[must_use]
    const fn units(&self) -> &HashMap<UnitName, ModuleSymbol<UnitNameNamespace>> {
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
    const fn constructors(&self) -> &HashMap<ConstructorName, ModuleConstructorSymbol> {
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
                    SymbolVisibility::PublicBind,
                    DeclSymbolKind::Param,
                )?,
                ast::DeclKind::Node(n) => self.insert_value_decl(
                    &mut exclusive_names,
                    &n.name,
                    SymbolVisibility::from(n.visibility),
                    DeclSymbolKind::Node,
                )?,
                ast::DeclKind::ConstNode(c) => self.insert_value_decl(
                    &mut exclusive_names,
                    &c.name,
                    SymbolVisibility::from(c.visibility),
                    DeclSymbolKind::Const,
                )?,
                ast::DeclKind::Assert(a) => self.insert_value_decl(
                    &mut exclusive_names,
                    &a.name,
                    SymbolVisibility::from(a.visibility),
                    DeclSymbolKind::Assert,
                )?,
                ast::DeclKind::Plot(p) => self.insert_value_decl(
                    &mut exclusive_names,
                    &p.name,
                    SymbolVisibility::from(p.visibility),
                    DeclSymbolKind::Plot,
                )?,
                ast::DeclKind::Figure(f) => self.insert_value_decl(
                    &mut exclusive_names,
                    &f.name,
                    SymbolVisibility::from(f.visibility),
                    DeclSymbolKind::Figure,
                )?,
                ast::DeclKind::Layer(l) => self.insert_value_decl(
                    &mut exclusive_names,
                    &l.name,
                    SymbolVisibility::from(l.visibility),
                    DeclSymbolKind::Layer,
                )?,
                ast::DeclKind::Dag(d) => self.insert_value_decl(
                    &mut exclusive_names,
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                    DeclSymbolKind::Dag,
                )?,
                ast::DeclKind::BaseDimension(d) => self.insert_dimension_decl(
                    &mut exclusive_names,
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                )?,
                ast::DeclKind::Dimension(d) => self.insert_dimension_decl(
                    &mut exclusive_names,
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                )?,
                ast::DeclKind::Unit(u) => self.insert_unit(
                    &u.name,
                    SymbolVisibility::from(u.visibility),
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
                    reason = "post-desugar Sugar payload is uninhabited by phase invariant"
                )]
                ast::DeclKind::Sugar(s) => never(*s),
            }
        }
        Ok(())
    }

    fn insert_value_decl(
        &mut self,
        exclusive_names: &mut HashMap<NameAtom, ExclusiveNameBinding>,
        name: &Spanned<DeclName>,
        visibility: SymbolVisibility,
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
        exclusive_names: &mut HashMap<NameAtom, ExclusiveNameBinding>,
        name: &Spanned<DimName>,
        visibility: SymbolVisibility,
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
        exclusive_names: &mut HashMap<NameAtom, ExclusiveNameBinding>,
        type_decl: &ast::TypeDecl,
    ) -> Result<(), ModuleResolveError> {
        let visibility = SymbolVisibility::from(type_decl.visibility);
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
        exclusive_names: &mut HashMap<NameAtom, ExclusiveNameBinding>,
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
        occupied: &mut HashMap<NameAtom, ExclusiveNameBinding>,
        atom: &NameAtom,
        kind: ExclusiveNameKind,
        span: Span,
    ) -> Result<(), ModuleResolveError> {
        match occupied.get(atom) {
            Some(first) if first.kind.conflicts_with(kind) => {
                Err(ModuleResolveError::DuplicateSymbol {
                    owner: self.owner.clone(),
                    namespace: "name",
                    name: atom.to_string(),
                    first: first.span,
                    duplicate: span,
                })
            }
            _ => {
                let should_replace_constructor = occupied
                    .get(atom)
                    .is_some_and(|binding| binding.kind == ExclusiveNameKind::Constructor)
                    && kind != ExclusiveNameKind::Constructor;
                if should_replace_constructor {
                    occupied.insert(atom.clone(), ExclusiveNameBinding { kind, span });
                } else {
                    occupied
                        .entry(atom.clone())
                        .or_insert(ExclusiveNameBinding { kind, span });
                }
                Ok(())
            }
        }
    }

    fn insert_decl(
        &mut self,
        name: &Spanned<DeclName>,
        visibility: SymbolVisibility,
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
        visibility: SymbolVisibility,
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
        visibility: SymbolVisibility,
        namespace_name: &'static str,
    ) -> Result<(), ModuleResolveError> {
        insert_symbol(
            &self.owner,
            &mut self.units,
            name,
            visibility,
            namespace_name,
        )
    }

    fn insert_struct_type(
        &mut self,
        name: &Spanned<StructTypeName>,
        visibility: SymbolVisibility,
        namespace_name: &'static str,
        generic_params: Vec<GenericParamSignature>,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.struct_types.get(name.value.as_str()) {
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
        visibility: SymbolVisibility,
        namespace_name: &'static str,
        generic_params: Vec<GenericParamSignature>,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.constructors.get(name.value.as_str()) {
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
                generic_params,
            },
        );
        Ok(())
    }

    fn insert_index(&mut self, index: &ast::IndexDecl) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.indexes.get(index.name.value.as_str()) {
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
                    SymbolVisibility::from(index.visibility),
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
    visibility: SymbolVisibility,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(name.value.as_str()) {
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
    visibility: SymbolVisibility,
    namespace_name: &'static str,
    kind: DeclSymbolKind,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(name.value.as_str()) {
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

/// A resolved module alias in one module's import scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAliasTarget {
    target: DagId,
    span: Span,
    access: ModuleAccess,
    role: ModuleAliasRole,
}

impl ModuleAliasTarget {
    /// Canonical DAG/module targeted by the alias.
    #[must_use]
    pub const fn target(&self) -> &DagId {
        &self.target
    }

    /// Source span of the local alias name.
    #[must_use]
    const fn span(&self) -> Span {
        self.span
    }

    /// Visibility rule for names reached through this alias.
    #[must_use]
    pub const fn access(&self) -> ModuleAccess {
        self.access
    }

    /// Whether this alias names an imported DAG or an included instance.
    #[must_use]
    pub const fn role(&self) -> ModuleAliasRole {
        self.role
    }
}

/// An extern-plugin alias registered in one module's import scope by
/// `import plugin "path" as alias { ... }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAliasTarget {
    path: crate::syntax::plugin::PluginPath,
    span: Span,
    functions: HashMap<crate::syntax::function_name::FnName, Span>,
}

impl PluginAliasTarget {
    /// The plugin identity the alias refers to.
    #[must_use]
    pub(crate) const fn path(&self) -> &crate::syntax::plugin::PluginPath {
        &self.path
    }

    /// Source span of the local alias name.
    #[must_use]
    const fn span(&self) -> Span {
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
    resolved: ResolvedName<Ns>,
    span: Span,
    visibility: SymbolVisibility,
}

impl<Ns: NameNamespace> ImportedSymbol<Ns> {
    const fn new(resolved: ResolvedName<Ns>, span: Span, visibility: SymbolVisibility) -> Self {
        Self {
            resolved,
            span,
            visibility,
        }
    }

    /// Canonical target identity of the imported symbol.
    #[must_use]
    const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Source span of the local import name.
    #[must_use]
    const fn span(&self) -> Span {
        self.span
    }

    /// Visibility of this selective import when the importing module is itself imported.
    #[must_use]
    const fn visibility(&self) -> SymbolVisibility {
        self.visibility
    }
}

impl<Ns: NameNamespace> ModuleSymbolLookup<Ns> for ImportedSymbol<Ns> {
    fn resolved(&self) -> &ResolvedName<Ns> {
        self.resolved()
    }

    fn visibility(&self) -> SymbolVisibility {
        self.visibility()
    }

    fn span(&self) -> Span {
        self.span()
    }
}

/// Import scope for a single module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleScope {
    module_aliases: HashMap<ModuleAliasName, ModuleAliasTarget>,
    plugin_aliases: HashMap<ModuleAliasName, PluginAliasTarget>,
    selected_decls: HashMap<DeclName, ImportedSymbol<DeclNameNamespace>>,
    selected_dimensions: HashMap<DimName, ImportedSymbol<DimNameNamespace>>,
    selected_units: HashMap<UnitName, ImportedSymbol<UnitNameNamespace>>,
    selected_struct_types: HashMap<StructTypeName, ImportedSymbol<StructTypeNameNamespace>>,
    selected_indexes: HashMap<IndexName, ImportedSymbol<IndexNameNamespace>>,
    selected_constructors: HashMap<ConstructorName, ImportedSymbol<ConstructorNameNamespace>>,
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

/// Semantic category of one exported symbol as seen by import tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportedImportItemKind {
    Decl(DeclSymbolKind),
    Constructor,
    Dimension,
    Unit,
    Type,
    Index,
}

impl ExportedImportItemKind {
    /// Selective-import namespace required for this symbol.
    #[must_use]
    pub const fn namespace(self) -> ImportItemNamespace {
        match self {
            Self::Decl(_) | Self::Constructor => ImportItemNamespace::Term,
            Self::Dimension => ImportItemNamespace::Dimension,
            Self::Unit => ImportItemNamespace::Unit,
            Self::Type => ImportItemNamespace::Type,
            Self::Index => ImportItemNamespace::Index,
        }
    }

    const fn sort_rank(self) -> u8 {
        match self.namespace() {
            ImportItemNamespace::Term => 0,
            ImportItemNamespace::Type => 1,
            ImportItemNamespace::Dimension => 2,
            ImportItemNamespace::Unit => 3,
            ImportItemNamespace::Index => 4,
        }
    }
}

/// One public symbol rendered in the exact category used by selective imports.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportedImportItem {
    pub name: NameAtom,
    pub kind: ExportedImportItemKind,
}

impl ExportedImportItem {
    /// Canonical source spelling that can be pasted into an import brace list.
    #[must_use]
    pub fn render(&self) -> String {
        self.kind.namespace().render_item(self.name.as_str())
    }
}

/// Surface category for diagnostics that cross namespace boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceNameKind {
    Value,
    Dimension,
    Unit,
    Type,
    Index,
    IndexLabel,
    Constructor,
}

impl std::fmt::Display for SurfaceNameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Value => "a value",
            Self::Dimension => "a dimension",
            Self::Unit => "a unit",
            Self::Type => "a type",
            Self::Index => "an index",
            Self::IndexLabel => "an index label",
            Self::Constructor => "a constructor",
        };
        f.write_str(text)
    }
}

trait ResolvableNamespace: NameNamespace {
    const SURFACE_KIND: SurfaceNameKind;
}

impl ResolvableNamespace for DeclNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Value;
}

impl ResolvableNamespace for DimNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Dimension;
}

impl ResolvableNamespace for UnitNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Unit;
}

impl ResolvableNamespace for StructTypeNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Type;
}

impl ResolvableNamespace for IndexNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Index;
}

impl ResolvableNamespace for ConstructorNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Constructor;
}

#[derive(Debug, Clone)]
enum ImportAddition {
    ModuleAlias {
        alias: Spanned<ModuleAliasName>,
        target: DagId,
        access: ModuleAccess,
        role: ModuleAliasRole,
    },
    Decl {
        local: Spanned<DeclName>,
        target: ResolvedDeclName,
        visibility: SymbolVisibility,
    },
    Dimension {
        local: Spanned<DimName>,
        target: ResolvedDimName,
        visibility: SymbolVisibility,
    },
    Unit {
        local: Spanned<UnitName>,
        target: ResolvedUnitName,
        visibility: SymbolVisibility,
    },
    StructType {
        local: Spanned<StructTypeName>,
        target: ResolvedStructTypeName,
        visibility: SymbolVisibility,
    },
    Index {
        local: Spanned<IndexName>,
        target: ResolvedIndexName,
        visibility: SymbolVisibility,
    },
    Constructor {
        local: Spanned<ConstructorName>,
        target: ResolvedConstructorName,
        visibility: SymbolVisibility,
    },
}

/// Project-wide module resolver backed by canonical [`DagId`] identities.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleResolver {
    modules: HashMap<DagId, ModuleSymbols>,
    scopes: HashMap<DagId, ModuleScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedModuleQualifier {
    owner: DagId,
    access: ModuleAccess,
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
        register_plugin_imports(&owner, scope, declarations)?;
        self.modules.insert(owner, symbols);
        Ok(())
    }

    /// Look up an extern-plugin alias visible from `owner`.
    ///
    /// Plugin imports are file-level declarations; inline `dag` children see
    /// the enclosing file's aliases, so the lookup walks up the owner chain.
    #[must_use]
    pub(crate) fn plugin_alias(&self, owner: &DagId, alias: &str) -> Option<&PluginAliasTarget> {
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

    /// Borrow all module import scopes.
    #[must_use]
    pub const fn scopes(&self) -> &HashMap<DagId, ModuleScope> {
        &self.scopes
    }

    /// List the module's public surface in canonical selective-import categories.
    ///
    /// Native declarations and selective re-exports are indistinguishable here:
    /// callers receive the local exported spelling and the marker required to
    /// import it. Whole-module re-export chains have already been expanded into
    /// the module scope by import registration.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] if `owner` or a re-exported declaration's
    /// canonical owner is missing.
    pub fn exported_import_items(
        &self,
        owner: &DagId,
    ) -> Result<Vec<ExportedImportItem>, ModuleResolveError> {
        let symbols = self.module_symbols(owner)?;
        let scope = self.module_scope(owner)?;
        let mut items = Vec::new();

        for (name, symbol) in &symbols.decls {
            if symbol.visibility().is_public() {
                items.push(ExportedImportItem {
                    name: name.atom().clone(),
                    kind: ExportedImportItemKind::Decl(symbol.kind()),
                });
            }
        }
        for (name, symbol) in &scope.selected_decls {
            if symbol.visibility().is_public() {
                items.push(ExportedImportItem {
                    name: name.atom().clone(),
                    kind: ExportedImportItemKind::Decl(self.decl_symbol_kind(symbol.resolved())?),
                });
            }
        }

        macro_rules! push_namespace {
            ($local:expr, $selected:expr, $kind:expr) => {
                for (name, symbol) in $local {
                    if symbol.visibility().is_public() {
                        items.push(ExportedImportItem {
                            name: name.atom().clone(),
                            kind: $kind,
                        });
                    }
                }
                for (name, symbol) in $selected {
                    if symbol.visibility().is_public() {
                        items.push(ExportedImportItem {
                            name: name.atom().clone(),
                            kind: $kind,
                        });
                    }
                }
            };
        }

        push_namespace!(
            &symbols.constructors,
            &scope.selected_constructors,
            ExportedImportItemKind::Constructor
        );
        push_namespace!(
            &symbols.struct_types,
            &scope.selected_struct_types,
            ExportedImportItemKind::Type
        );
        push_namespace!(
            &symbols.dimensions,
            &scope.selected_dimensions,
            ExportedImportItemKind::Dimension
        );
        push_namespace!(
            &symbols.units,
            &scope.selected_units,
            ExportedImportItemKind::Unit
        );
        push_namespace!(
            &symbols.indexes,
            &scope.selected_indexes,
            ExportedImportItemKind::Index
        );

        items.sort_by(|left, right| {
            left.kind
                .sort_rank()
                .cmp(&right.kind.sort_rank())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(items)
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

    /// Register one loader-resolved `import` edge in `owner`'s scope.
    ///
    /// `path` and `kind` come from the source AST. `target` is the canonical
    /// module identity chosen by the loader for that path. This function never
    /// re-resolves filesystem paths.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError`] if either module is unknown, an imported
    /// item is missing/private, or the import introduces a duplicate local name.
    pub fn register_import(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        kind: &ImportKind,
        target: &DagId,
    ) -> Result<(), ModuleResolveError> {
        self.register_import_with_access(
            owner,
            path,
            kind,
            target,
            ModuleAccess::PublicOnly,
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
            kind,
            target,
            ModuleAccess::PublicOnly,
            ModuleAliasRole::IncludedInstance,
        )
    }

    /// Make an instantiated include's own indexes resolvable in the importer.
    ///
    /// An instantiated `include` inlines the dependency's declaration bodies
    /// into the importer (see `ir::lower::merge_dependency`). Those bodies
    /// reference the dependency's own indexes by their bare names (`for s:
    /// Step`, `T[Step]`, `Step.A`), which are not bound to any importer symbol.
    /// The dependency's declarations live in the synthetic include module
    /// `source`; copy each of its index symbols — re-homed onto the importer so
    /// they resolve against the flat merged registry that backs the importer's
    /// declarations — into the importer's own symbol table, variants included.
    ///
    /// Indexes named in `bound` (the include's index bindings/overrides) are
    /// skipped: a bound index is rewritten to the importer's replacement before
    /// resolution, so the dependency's original name must not shadow it.
    /// Any remaining same-name collision is rejected instead of silently
    /// choosing whichever include was linked first.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::UnknownModule`] if either module is absent
    /// from the resolver, or [`ModuleResolveError::DuplicateSymbol`] if an
    /// unbound dependency index collides with an index already visible in the
    /// importer.
    pub fn inline_instantiated_include_indexes(
        &mut self,
        importer: &DagId,
        source: &DagId,
        bound: &HashSet<IndexName>,
    ) -> Result<(), ModuleResolveError> {
        let source_symbols =
            self.modules
                .get(source)
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: source.clone(),
                })?;
        let injected: Vec<ModuleIndexSymbol> = source_symbols
            .indexes
            .iter()
            .filter(|(name, _)| !bound.contains(*name))
            .map(|(name, symbol)| ModuleIndexSymbol {
                symbol: ModuleSymbol::new(
                    importer,
                    name.clone(),
                    symbol.visibility(),
                    symbol.span(),
                ),
                variants: symbol.variants().clone(),
            })
            .collect();
        let target =
            self.modules
                .get_mut(importer)
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: importer.clone(),
                })?;
        for symbol in injected {
            let name = IndexName::from_atom(symbol.resolved().atom().clone());
            if let Some(first) = target.indexes.get(name.as_str()) {
                return Err(ModuleResolveError::DuplicateSymbol {
                    owner: importer.clone(),
                    namespace: IndexNameNamespace::DISPLAY_NAME,
                    name: name.to_string(),
                    first: first.span(),
                    duplicate: symbol.span(),
                });
            }
            target.indexes.insert(name, symbol);
        }
        Ok(())
    }

    fn register_import_with_access(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        kind: &ImportKind,
        target: &DagId,
        access: ModuleAccess,
        role: ModuleAliasRole,
    ) -> Result<(), ModuleResolveError> {
        self.module_symbols(owner)?;
        self.module_symbols(target)?;

        let additions = self.import_additions(path, kind, target, access, role)?;
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

    /// Resolve a syntactic declaration/value path to a canonical owner + leaf.
    ///
    /// Bare paths first search local declarations, then selective imports.
    /// Qualified paths resolve their qualifier through module aliases and then
    /// apply that alias boundary's visibility rule.
    pub(crate) fn resolve_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedDeclName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::decls, |scope| {
            &scope.selected_decls
        })
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
    pub(crate) fn decl_symbol_kind(
        &self,
        name: &ResolvedDeclName,
    ) -> Result<DeclSymbolKind, ModuleResolveError> {
        let symbols = self.module_symbols(name.owner())?;
        let def_name = DeclName::from_atom(name.atom().clone());
        symbols
            .decls
            .get(def_name.as_str())
            .map(ModuleDeclSymbol::kind)
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: name.owner().clone(),
                namespace: DeclNameNamespace::DISPLAY_NAME,
                name: name.as_str().to_string(),
            })
    }

    /// Resolve a syntactic dimension path to a canonical owner + leaf.
    pub(crate) fn resolve_dimension_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedDimName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::dimensions, |scope| {
            &scope.selected_dimensions
        })
    }

    /// Resolve a syntactic unit path to a canonical owner + leaf.
    pub fn resolve_unit_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedUnitName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::units, |scope| {
            &scope.selected_units
        })
    }

    /// Resolve a syntactic struct/tagged-union type path to a canonical owner + leaf.
    pub(crate) fn resolve_struct_type_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedStructTypeName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::struct_types, |scope| {
            &scope.selected_struct_types
        })
    }

    /// Return the source generic signature for a resolved user-defined type.
    pub(crate) fn struct_type_generic_params(
        &self,
        name: &ResolvedStructTypeName,
    ) -> Result<&[GenericParamSignature], ModuleResolveError> {
        let symbols = self.module_symbols(name.owner())?;
        symbols
            .struct_types
            .get(name.as_str())
            .map(ModuleTypeSymbol::generic_params)
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: name.owner().clone(),
                namespace: StructTypeNameNamespace::DISPLAY_NAME,
                name: name.as_str().to_string(),
            })
    }

    /// Resolve a syntactic tagged-union constructor path to a canonical owner + leaf.
    pub(crate) fn resolve_constructor_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedConstructorName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::constructors, |scope| {
            &scope.selected_constructors
        })
    }

    /// Return the owning type's source generic signature for a resolved constructor.
    pub(crate) fn constructor_generic_params(
        &self,
        name: &ResolvedConstructorName,
    ) -> Result<&[GenericParamSignature], ModuleResolveError> {
        let symbols = self.module_symbols(name.owner())?;
        symbols
            .constructors
            .get(name.as_str())
            .map(ModuleConstructorSymbol::generic_params)
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: name.owner().clone(),
                namespace: ConstructorNameNamespace::DISPLAY_NAME,
                name: name.as_str().to_string(),
            })
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

    /// Resolve a syntactic index path to a canonical owner + leaf.
    pub(crate) fn resolve_index_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedIndexName, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::indexes, |scope| {
            &scope.selected_indexes
        })
    }

    /// Resolve a syntactic index-variant path such as `Index.Variant` or
    /// `module.Index.Variant`.
    pub fn resolve_index_variant_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedIndexVariant, ModuleResolveError> {
        let (index_segments, variant_atom) = path.split_last();
        let index_path = name_path_from_slice(index_segments).ok_or_else(|| {
            ModuleResolveError::ExpectedIndexVariantPath {
                owner: owner.clone(),
                path: path.display_path(),
            }
        })?;
        self.resolve_index_variant_parts(
            owner,
            &index_path,
            &IndexVariantName::from_atom(variant_atom.clone()),
        )
    }

    /// Resolve an already-split index path plus variant leaf to a canonical
    /// index-variant identity.
    ///
    /// This is the HIR-facing form for parser positions that preserve the
    /// index path and variant leaf separately (map keys, index arguments, and
    /// match labels). It avoids reconstructing a dotted string or re-parsing
    /// source text just to validate the variant against the canonical index.
    pub(crate) fn resolve_index_variant_parts(
        &self,
        owner: &DagId,
        index_path: &NamePath,
        variant: &IndexVariantName,
    ) -> Result<ResolvedIndexVariant, ModuleResolveError> {
        let resolved_index = self.resolve_index_path(owner, index_path)?;
        let index_owner = resolved_index.owner().clone();
        let index_name = IndexName::from_atom(resolved_index.atom().clone());
        let target_symbols = self.module_symbols(&index_owner)?;
        let index_symbol = target_symbols
            .indexes
            .get(index_name.as_str())
            .ok_or_else(|| ModuleResolveError::UnknownName {
                owner: index_owner.clone(),
                namespace: IndexNameNamespace::DISPLAY_NAME,
                name: index_name.to_string(),
            })?;
        if !index_symbol.variants.contains_key(variant.as_str()) {
            return Err(ModuleResolveError::UnknownIndexVariant {
                index: resolved_index,
                variant: variant.clone(),
            });
        }
        Ok(ResolvedIndexVariant::new(resolved_index, variant.clone()))
    }

    /// Resolve a bare variant leaf by searching local and selectively imported
    /// indexes in the current module scope.
    pub(crate) fn resolve_bare_index_variant(
        &self,
        owner: &DagId,
        variant: &IndexVariantName,
    ) -> Result<ResolvedIndexVariant, ModuleResolveError> {
        let local = self.module_symbols(owner)?;
        let scope = self.module_scope(owner)?;
        let mut candidates = Vec::new();

        for symbol in local.indexes.values() {
            if symbol.variants().contains_key(variant.as_str()) {
                let resolved = symbol.resolved().clone();
                if !candidates.contains(&resolved) {
                    candidates.push(resolved);
                }
            }
        }
        for imported in scope.selected_indexes.values() {
            let resolved = imported.resolved();
            let index_owner = resolved.owner().clone();
            let index_name = IndexName::from_atom(resolved.atom().clone());
            let target_symbols = self.module_symbols(&index_owner)?;
            let Some(symbol) = target_symbols.indexes.get(index_name.as_str()) else {
                continue;
            };
            if symbol.variants().contains_key(variant.as_str()) && !candidates.contains(resolved) {
                candidates.push(resolved.clone());
            }
        }
        candidates.sort();
        match candidates.as_slice() {
            [] => Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: IndexVariantNameNamespace::DISPLAY_NAME,
                name: variant.to_string(),
            }),
            [index] => Ok(ResolvedIndexVariant::new(index.clone(), variant.clone())),
            _ => Err(ModuleResolveError::AmbiguousIndexVariant {
                owner: owner.clone(),
                variant: variant.clone(),
                indexes: candidates,
            }),
        }
    }

    /// Resolve a source DAG/module call path to its canonical [`DagId`].
    ///
    /// The first segment names a reusable DAG module in the caller's scope. It
    /// may be a local inline DAG, a sibling DAG visible from an inline body, or
    /// an imported module alias. That binding is itself callable regardless of
    /// whether its canonical target is a file root or an inline DAG; remaining
    /// path segments descend through child DAG modules uniformly.
    pub(crate) fn resolve_module_path(
        &self,
        owner: &DagId,
        path: &ModulePath,
    ) -> Result<DagId, ModuleResolveError> {
        let head = &path.segments.first().name;
        let local_target = {
            let child = owner.child(head.as_str());
            if self.modules.contains_key(&child) {
                Some(child)
            } else {
                // Only inline DAGs have a semantic parent module in the
                // resolver. A file root's `DagId` also has filesystem-like
                // parent segments, but sibling files never enter scope without
                // an explicit import.
                owner
                    .parent()
                    .filter(|parent| self.modules.contains_key(parent))
                    .and_then(|parent| {
                        let sibling = parent.child(head.as_str());
                        self.modules.contains_key(&sibling).then_some(sibling)
                    })
            }
        };

        let scope = self.module_scope(owner)?;
        let alias = ModuleAliasName::from_atom(head.clone());
        let alias_binding = scope.module_aliases.get(alias.as_str());
        let imported_alias_target = alias_binding
            .filter(|binding| binding.role.is_callable())
            .map(|binding| (binding.target.clone(), Some(binding.access)));
        let selected_name = DeclName::from_atom(head.clone());
        let selected_target = match scope.selected_decls.get(selected_name.as_str()) {
            Some(imported)
                if self.decl_symbol_kind(imported.resolved())? == DeclSymbolKind::Dag =>
            {
                Some((
                    imported
                        .resolved()
                        .owner()
                        .child(imported.resolved().as_str()),
                    Some(ModuleAccess::PublicOnly),
                ))
            }
            Some(_) | None => None,
        };
        let candidates = local_target
            .map(|target| (target, None))
            .into_iter()
            .chain(selected_target)
            .chain(imported_alias_target)
            .collect::<Vec<_>>();

        let (mut target, imported_access) = match candidates.as_slice() {
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

        if let Some(access) = imported_access {
            self.ensure_module_visible(&target, access)?;
        }
        for segment in path.segments().iter().skip(1) {
            target = target.child(segment.name.as_str());
            if !self.modules.contains_key(&target) {
                return Err(ModuleResolveError::UnknownModule { owner: target });
            }
            if let Some(access) = imported_access {
                self.ensure_module_visible(&target, access)?;
            }
        }
        Ok(target)
    }

    fn import_additions(
        &self,
        path: &ModulePath,
        kind: &ImportKind,
        target: &DagId,
        access: ModuleAccess,
        role: ModuleAliasRole,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        match kind {
            ImportKind::Module { alias } => {
                let alias = alias.clone().unwrap_or_else(|| {
                    Spanned::new(
                        ModuleAliasName::from_atom(path.leaf().name.clone()),
                        path.leaf().span,
                    )
                });
                Ok(vec![ImportAddition::ModuleAlias {
                    alias,
                    target: target.clone(),
                    access,
                    role,
                }])
            }
            ImportKind::Selective(items) => items
                .iter()
                .map(|item| self.import_item_additions(target, item, access))
                .collect::<Result<Vec<_>, _>>()
                .map(|chunks| chunks.into_iter().flatten().collect()),
        }
    }

    fn check_import_exclusive_name_collisions(
        &self,
        owner: &DagId,
        additions: &[ImportAddition],
    ) -> Result<(), ModuleResolveError> {
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

        check_same_namespace_import_collisions(owner, local, scope, additions)?;
        check_import_addition_exclusive_names(owner, &mut occupied, additions)
    }

    fn import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: ModuleAccess,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let local_atom = item
            .alias
            .as_ref()
            .map_or_else(|| item.name.name.clone(), |alias| alias.name.clone());
        let local_span = item.local_span();
        let visibility = if item.is_pub {
            SymbolVisibility::Public
        } else {
            SymbolVisibility::Private
        };

        let additions = match item.namespace {
            ImportItemNamespace::Term => {
                return self
                    .term_import_item_additions(target, item, access, local_atom, visibility);
            }
            ImportItemNamespace::Type => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    &item.name.name,
                    access,
                    ModuleSymbols::struct_types,
                    |scope| &scope.selected_struct_types,
                    StructTypeNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::StructType {
                    local: Spanned::new(StructTypeName::from_atom(local_atom), local_span),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Dimension => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    &item.name.name,
                    access,
                    ModuleSymbols::dimensions,
                    |scope| &scope.selected_dimensions,
                    DimNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Dimension {
                    local: Spanned::new(DimName::from_atom(local_atom), local_span),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Unit => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    &item.name.name,
                    access,
                    ModuleSymbols::units,
                    |scope| &scope.selected_units,
                    UnitNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Unit {
                    local: Spanned::new(UnitName::from_atom(local_atom), local_span),
                    target: target_name,
                    visibility,
                }
            }
            ImportItemNamespace::Index => {
                let target_name = self.required_exported_symbol_for_import(
                    target,
                    &item.name.name,
                    access,
                    ModuleSymbols::indexes,
                    |scope| &scope.selected_indexes,
                    IndexNameNamespace::DISPLAY_NAME,
                    item.namespace,
                    item.name.span,
                )?;
                ImportAddition::Index {
                    local: Spanned::new(IndexName::from_atom(local_atom), local_span),
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
        access: ModuleAccess,
        local_atom: NameAtom,
        visibility: SymbolVisibility,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let mut additions = Vec::new();
        let mut saw_private = false;
        let source_atom = &item.name.name;
        let local_span = item.local_span();

        match self.exported_symbol_for_import(
            target,
            source_atom,
            access,
            ModuleSymbols::decls,
            |scope| &scope.selected_decls,
        )? {
            ExportLookup::Public(target_name) => additions.push(ImportAddition::Decl {
                local: Spanned::new(DeclName::from_atom(local_atom.clone()), local_span),
                target: target_name,
                visibility,
            }),
            ExportLookup::Private => saw_private = true,
            ExportLookup::Missing => {}
        }
        match self.exported_symbol_for_import(
            target,
            source_atom,
            access,
            ModuleSymbols::constructors,
            |scope| &scope.selected_constructors,
        )? {
            ExportLookup::Public(target_name) => additions.push(ImportAddition::Constructor {
                local: Spanned::new(ConstructorName::from_atom(local_atom), local_span),
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
                .exported_import_item_categories(target, source_atom, access)?
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
                                source_atom.clone(),
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
        access: ModuleAccess,
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

    fn exported_symbol_for_import<Ns, S>(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: ModuleAccess,
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
        access: ModuleAccess,
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

    fn resolve_symbol_path<Ns, S>(
        &self,
        owner: &DagId,
        path: &NamePath,
        local_symbols: fn(&ModuleSymbols) -> &HashMap<NameDef<Ns>, S>,
        selected_symbols: fn(&ModuleScope) -> &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    ) -> Result<ResolvedName<Ns>, ModuleResolveError>
    where
        Ns: ResolvableNamespace,
        S: ModuleSymbolLookup<Ns>,
    {
        if let Some(atom) = path.as_bare() {
            let local = self.module_symbols(owner)?;
            if let Some(symbol) = local_symbols(local).get(atom.as_str()) {
                return Ok(symbol.resolved().clone());
            }
            let scope = self.module_scope(owner)?;
            if let Some(imported) = selected_symbols(scope).get(atom.as_str()) {
                return Ok(imported.resolved().clone());
            }
            if let Some(actual) = self.visible_surface_kind_for_bare_name(owner, atom)? {
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
        }

        let (qualifier, leaf) = path.split_last();
        let target_ref = match self.resolve_module_qualifier(owner, qualifier) {
            Ok(target_ref) => target_ref,
            Err(
                err @ (ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownModule { .. }),
            ) => {
                if let Some(actual) = self.visible_index_variant_path_kind(owner, path)? {
                    return Err(ModuleResolveError::WrongUniverseName {
                        owner: owner.clone(),
                        name: path.display_path(),
                        expected: Ns::SURFACE_KIND,
                        actual,
                    });
                }
                return Err(err);
            }
            Err(err) => return Err(err),
        };
        let target = self.module_symbols(&target_ref.owner)?;
        if let Some(symbol) = local_symbols(target).get(leaf.as_str()) {
            if target_ref.access.requires_public() && !symbol.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    namespace: Ns::DISPLAY_NAME,
                    name: leaf.to_string(),
                });
            }
            return Ok(symbol.resolved().clone());
        }

        let target_scope = self.module_scope(&target_ref.owner)?;
        if let Some(imported) = selected_symbols(target_scope).get(leaf.as_str()) {
            if target_ref.access.requires_public() && !imported.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    namespace: Ns::DISPLAY_NAME,
                    name: leaf.to_string(),
                });
            }
            return Ok(imported.resolved().clone());
        }

        if let Some(actual) =
            self.visible_surface_kind_for_qualified_leaf(&target_ref, leaf, path)?
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

    fn visible_surface_kind_for_bare_name(
        &self,
        owner: &DagId,
        atom: &NameAtom,
    ) -> Result<Option<SurfaceNameKind>, ModuleResolveError> {
        let local = self.module_symbols(owner)?;
        if let Some(kind) = surface_kind_in_local_symbols(local, atom, false) {
            return Ok(Some(kind));
        }
        let scope = self.module_scope(owner)?;
        Ok(surface_kind_in_scope(scope, atom, false))
    }

    fn visible_surface_kind_for_qualified_leaf(
        &self,
        target_ref: &ResolvedModuleQualifier,
        leaf: &NameAtom,
        path: &NamePath,
    ) -> Result<Option<SurfaceNameKind>, ModuleResolveError> {
        if let Some(kind) = self.visible_index_variant_path_kind(&target_ref.owner, path)? {
            return Ok(Some(kind));
        }

        let target = self.module_symbols(&target_ref.owner)?;
        if let Some(kind) =
            surface_kind_in_local_symbols(target, leaf, target_ref.access.requires_public())
        {
            return Ok(Some(kind));
        }
        let target_scope = self.module_scope(&target_ref.owner)?;
        Ok(surface_kind_in_scope(
            target_scope,
            leaf,
            target_ref.access.requires_public(),
        ))
    }

    fn visible_index_variant_path_kind(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<Option<SurfaceNameKind>, ModuleResolveError> {
        match self.resolve_index_variant_path(owner, path) {
            Ok(_) => Ok(Some(SurfaceNameKind::IndexLabel)),
            Err(
                ModuleResolveError::UnknownName { .. }
                | ModuleResolveError::UnknownIndexVariant { .. }
                | ModuleResolveError::ExpectedIndexVariantPath { .. }
                | ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownModule { .. },
            ) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn resolve_module_qualifier(
        &self,
        owner: &DagId,
        qualifier: &[NameAtom],
    ) -> Result<ResolvedModuleQualifier, ModuleResolveError> {
        let Some((head, rest)) = qualifier.split_first() else {
            return Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: "module",
                name: String::new(),
            });
        };
        let scope = self.module_scope(owner)?;
        let alias = ModuleAliasName::from_atom(head.clone());
        let alias_target = scope.module_aliases.get(alias.as_str()).ok_or_else(|| {
            ModuleResolveError::UnknownModuleAlias {
                owner: owner.clone(),
                alias,
            }
        })?;
        // Descend segment by segment, enforcing dag visibility at every
        // step: `lib.helper.symbol` must be rejected when `helper` is a
        // private dag, exactly like `resolve_module_path` rejects
        // `lib.helper` — previously only the symbol's own visibility was
        // checked, never the modules on the path.
        let mut target = alias_target.target.clone();
        for segment in rest {
            target = target.child(segment.as_str());
            if !self.modules.contains_key(&target) {
                return Err(ModuleResolveError::UnknownModule { owner: target });
            }
            self.ensure_module_visible(&target, alias_target.access)?;
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

    fn ensure_module_visible(
        &self,
        target: &DagId,
        access: ModuleAccess,
    ) -> Result<(), ModuleResolveError> {
        if !access.requires_public() {
            return Ok(());
        }
        let Some(parent) = target.parent() else {
            return Ok(());
        };
        let Some(parent_symbols) = self.modules.get(&parent) else {
            return Ok(());
        };
        let Some(symbol) = parent_symbols.decls.get(target.name()) else {
            return Ok(());
        };
        if symbol.kind() == DeclSymbolKind::Dag && !symbol.visibility().is_public() {
            return Err(ModuleResolveError::PrivateName {
                owner: parent,
                namespace: "dag",
                name: target.name().to_string(),
            });
        }
        Ok(())
    }
}

impl ModuleScope {
    fn apply_addition(
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
            } => {
                // Module aliases and plugin aliases share one qualifier
                // namespace: `alias.name` must have a single meaning.
                if let Some(first) = self.plugin_aliases.get(alias.value.as_str()) {
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

fn insert_module_alias(
    owner: &DagId,
    map: &mut HashMap<ModuleAliasName, ModuleAliasTarget>,
    alias: Spanned<ModuleAliasName>,
    target: DagId,
    access: ModuleAccess,
    role: ModuleAliasRole,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(alias.value.as_str()) {
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
fn register_plugin_imports(
    owner: &DagId,
    scope: &mut ModuleScope,
    declarations: &[ast::Declaration],
) -> Result<(), ModuleResolveError> {
    for decl in declarations {
        let ast::DeclKind::PluginImport(plugin) = &decl.kind else {
            continue;
        };
        if let Some(first) = scope.plugin_aliases.get(plugin.alias.value.as_str()) {
            return Err(ModuleResolveError::DuplicateImportName {
                owner: owner.clone(),
                namespace: ModuleAliasNameNamespace::DISPLAY_NAME,
                name: plugin.alias.value.to_string(),
                first: first.span(),
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
                path: plugin.path.value.clone(),
                span: plugin.alias.span,
                functions,
            },
        );
    }
    Ok(())
}

fn surface_kind_in_local_symbols(
    symbols: &ModuleSymbols,
    atom: &NameAtom,
    requires_public: bool,
) -> Option<SurfaceNameKind> {
    macro_rules! probe {
        ($map:expr, $kind:expr) => {
            if let Some(symbol) = $map.get(atom.as_str())
                && (!requires_public || symbol.visibility().is_public())
            {
                return Some($kind);
            }
        };
    }

    probe!(symbols.decls, SurfaceNameKind::Value);
    probe!(symbols.dimensions, SurfaceNameKind::Dimension);
    probe!(symbols.units, SurfaceNameKind::Unit);
    probe!(symbols.struct_types, SurfaceNameKind::Type);
    probe!(symbols.indexes, SurfaceNameKind::Index);
    probe!(symbols.constructors, SurfaceNameKind::Constructor);
    None
}

fn surface_kind_in_scope(
    scope: &ModuleScope,
    atom: &NameAtom,
    requires_public: bool,
) -> Option<SurfaceNameKind> {
    macro_rules! probe {
        ($map:expr, $kind:expr) => {
            if let Some(symbol) = $map.get(atom.as_str())
                && (!requires_public || symbol.visibility().is_public())
            {
                return Some($kind);
            }
        };
    }

    probe!(scope.selected_decls, SurfaceNameKind::Value);
    probe!(scope.selected_dimensions, SurfaceNameKind::Dimension);
    probe!(scope.selected_units, SurfaceNameKind::Unit);
    probe!(scope.selected_struct_types, SurfaceNameKind::Type);
    probe!(scope.selected_indexes, SurfaceNameKind::Index);
    probe!(scope.selected_constructors, SurfaceNameKind::Constructor);
    None
}

fn seed_exclusive_names<Ns, S>(
    occupied: &mut HashMap<NameAtom, ExclusiveNameBinding>,
    symbols: &HashMap<NameDef<Ns>, S>,
    kind: ExclusiveNameKind,
) where
    Ns: NameNamespace,
    S: ModuleSymbolLookup<Ns>,
{
    for (name, symbol) in symbols {
        occupied
            .entry(name.atom().clone())
            .and_modify(|binding| {
                if binding.kind == ExclusiveNameKind::Constructor
                    && kind != ExclusiveNameKind::Constructor
                {
                    *binding = ExclusiveNameBinding {
                        kind,
                        span: symbol.span(),
                    };
                }
            })
            .or_insert_with(|| ExclusiveNameBinding {
                kind,
                span: symbol.span(),
            });
    }
}

fn check_import_addition_exclusive_names(
    owner: &DagId,
    occupied: &mut HashMap<NameAtom, ExclusiveNameBinding>,
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
            ImportAddition::ModuleAlias { .. } | ImportAddition::Unit { .. } => {}
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
    if let Some(first) = local.get(name.value.as_str()) {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: namespace_name,
            name: name.value.to_string(),
            first: first.span(),
            duplicate: name.span,
        });
    }
    if let Some(first) = selected.get(name.value.as_str()) {
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
    occupied: &mut HashMap<NameAtom, ExclusiveNameBinding>,
    atom: &NameAtom,
    kind: ExclusiveNameKind,
    span: Span,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = occupied.get(atom)
        && (first.kind == kind || first.kind.conflicts_with(kind))
    {
        return Err(ModuleResolveError::DuplicateImportName {
            owner: owner.clone(),
            namespace: "name",
            name: atom.to_string(),
            first: first.span,
            duplicate: span,
        });
    }
    let should_replace_constructor = occupied
        .get(atom)
        .is_some_and(|binding| binding.kind == ExclusiveNameKind::Constructor)
        && kind != ExclusiveNameKind::Constructor;
    if should_replace_constructor {
        occupied.insert(atom.clone(), ExclusiveNameBinding { kind, span });
    } else {
        occupied
            .entry(atom.clone())
            .or_insert(ExclusiveNameBinding { kind, span });
    }
    Ok(())
}

fn insert_imported_symbol<Ns: NameNamespace>(
    owner: &DagId,
    map: &mut HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    local: Spanned<NameDef<Ns>>,
    target: ResolvedName<Ns>,
    visibility: SymbolVisibility,
    namespace_name: &'static str,
) -> Result<(), ModuleResolveError> {
    if let Some(first) = map.get(local.value.as_str()) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExportLookup<Ns: NameNamespace> {
    Public(ResolvedName<Ns>),
    Private,
    Missing,
}

fn exported_symbol<Ns, S>(
    map: &HashMap<NameDef<Ns>, S>,
    atom: &NameAtom,
    access: ModuleAccess,
) -> ExportLookup<Ns>
where
    Ns: NameNamespace,
    S: ModuleSymbolLookup<Ns>,
{
    map.get(atom.as_str())
        .map_or(ExportLookup::Missing, |symbol| {
            if !access.requires_public() || symbol.visibility().is_public() {
                ExportLookup::Public(symbol.resolved().clone())
            } else {
                ExportLookup::Private
            }
        })
}

fn name_path_from_slice(segments: &[NameAtom]) -> Option<NamePath> {
    NonEmpty::try_from_vec(segments.to_vec())
        .ok()
        .map(NamePath::new)
}

fn ident_path_to_name_path(path: &IdentPath) -> NamePath {
    let segments = path.segments();
    NamePath::new(NonEmpty::new(
        segments[0].name.clone(),
        segments[1..]
            .iter()
            .map(|ident| ident.name.clone())
            .collect(),
    ))
}

/// Errors produced while building or using module-aware symbol tables.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModuleResolveError {
    /// A module was added twice.
    #[error("duplicate module `{owner}`")]
    DuplicateModule { owner: DagId },
    /// No symbol table exists for a canonical module identity.
    #[error("unknown module `{owner}`")]
    UnknownModule { owner: DagId },
    /// A module qualifier's first segment is not an alias in the current module.
    #[error("module alias `{alias}` is not in scope of `{owner}`")]
    UnknownModuleAlias {
        owner: DagId,
        alias: ModuleAliasName,
    },
    /// A call path is ambiguous between a local DAG and an imported module alias.
    #[error("DAG name `{name}` is ambiguous in `{owner}`")]
    AmbiguousCallableModule {
        owner: DagId,
        name: ModuleAliasName,
        targets: Vec<DagId>,
    },
    /// An include alias denotes an existing instance, not a reusable DAG blueprint.
    #[error("included instance `{alias}` is not callable in `{owner}`")]
    IncludedInstanceNotCallable {
        owner: DagId,
        alias: ModuleAliasName,
    },
    /// Duplicate definition in one namespace.
    #[error("duplicate {namespace} `{name}` in module `{owner}`")]
    DuplicateSymbol {
        owner: DagId,
        namespace: &'static str,
        name: String,
        first: Span,
        duplicate: Span,
    },
    /// Duplicate local import/alias in one namespace.
    #[error("duplicate imported {namespace} `{name}` in module `{owner}`")]
    DuplicateImportName {
        owner: DagId,
        namespace: &'static str,
        name: String,
        first: Span,
        duplicate: Span,
    },
    /// A name was not found in the requested namespace.
    #[error("unknown {namespace} `{name}` in module `{owner}`")]
    UnknownName {
        owner: DagId,
        namespace: &'static str,
        name: String,
    },
    /// A selective import name exists, but not under the marked category.
    #[error("in module `{owner}`, {mismatch}")]
    WrongImportCategory {
        owner: DagId,
        mismatch: ImportItemCategoryMismatch,
        span: Span,
    },
    /// A name exists, but in a semantic universe that is not valid here.
    #[error("in module `{owner}`, `{name}` is {actual}, not {expected}")]
    WrongUniverseName {
        owner: DagId,
        name: String,
        expected: SurfaceNameKind,
        actual: SurfaceNameKind,
    },
    /// A name exists but has the wrong declaration kind for the use site.
    #[error("expected {expected} declaration `{name}`, found {actual}")]
    UnexpectedDeclKind {
        name: ResolvedDeclName,
        expected: &'static str,
        actual: DeclSymbolKind,
    },
    /// A name exists but is not public across module boundaries.
    #[error("private {namespace} `{name}` in module `{owner}`")]
    PrivateName {
        owner: DagId,
        namespace: &'static str,
        name: String,
    },
    /// A path did not have enough segments to denote `Index.Variant`.
    #[error("expected index-variant path in module `{owner}`, got `{path}`")]
    ExpectedIndexVariantPath { owner: DagId, path: String },
    /// The index exists, but the requested variant is absent.
    #[error("unknown variant `{variant}` for index `{index}`")]
    UnknownIndexVariant {
        index: ResolvedIndexName,
        variant: IndexVariantName,
    },
    /// A bare variant exists on more than one visible index.
    #[error("ambiguous index label `{variant}` in module `{owner}`; qualify it with an index name")]
    AmbiguousIndexVariant {
        owner: DagId,
        variant: IndexVariantName,
        indexes: Vec<ResolvedIndexName>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::Ident;
    use crate::syntax::parser::Parser;

    fn desugared_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        crate::syntax::desugar::desugar_multi_decls_in_file(raw)
    }

    fn first_import(file: &ast::File) -> (&ModulePath, &ImportKind) {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Import(import) => Some((&import.path, &import.kind)),
                _ => None,
            })
            .expect("source should contain an import")
    }

    fn imports(file: &ast::File) -> Vec<(&ModulePath, &ImportKind)> {
        file.declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                ast::DeclKind::Import(import) => Some((&import.path, &import.kind)),
                _ => None,
            })
            .collect()
    }

    fn first_include(file: &ast::File) -> (&ModulePath, &ImportKind) {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Include(include) => Some((&include.path, &include.kind)),
                _ => None,
            })
            .expect("source should contain an include")
    }

    fn atom(s: &str) -> NameAtom {
        NameAtom::parse(s).unwrap()
    }

    fn path(segments: &[&str]) -> NamePath {
        let atoms = segments.iter().map(|s| atom(s)).collect::<Vec<_>>();
        NamePath::new(NonEmpty::try_from_vec(atoms).unwrap())
    }

    fn module_path(segments: &[&str]) -> ModulePath {
        let idents = segments
            .iter()
            .map(|s| Ident {
                name: atom(s),
                span: Span::new(0, 0),
            })
            .collect::<Vec<_>>();
        ModulePath {
            segments: NonEmpty::try_from_vec(idents).unwrap(),
            span: Span::new(0, 0),
        }
    }

    fn first_dag(file: &ast::File) -> &ast::DagDecl {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Dag(dag) => Some(dag),
                _ => None,
            })
            .expect("source should contain a dag")
    }

    #[test]
    fn local_type_index_name_collision_is_rejected() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("type M { Mk(v: Dimensionless) }\npub index M = { A, B };");

        let err = ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::DuplicateSymbol {
                owner: err_owner,
                namespace: "name",
                name,
                ..
            } if err_owner == owner && name == "M"
        ));
    }

    #[test]
    fn local_dimension_type_name_collision_is_rejected() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("dim M = Length;\ntype M { Mk(v: Dimensionless) }");

        let err = ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::DuplicateSymbol {
                owner: err_owner,
                namespace: "name",
                name,
                ..
            } if err_owner == owner && name == "M"
        ));
    }

    #[test]
    fn alias_reimport_of_same_index_does_not_ambiguous_bare_variant() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub index Phase = { Burn, Coast };");
        let main = desugared_source("import lib.{ index Phase, index Phase as P };");
        let imports = imports(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, imports[0].0, imports[0].1, &lib_id)
            .unwrap();

        let index_variant = resolver
            .resolve_bare_index_variant(&main_id, &IndexVariantName::expect_valid("Burn"))
            .unwrap();
        assert_eq!(index_variant.index().as_str(), "Phase");
        assert_eq!(index_variant.variant().as_str(), "Burn");
    }

    #[test]
    fn same_named_type_and_constructor_remain_distinct() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("type T { T }");

        let symbols = ModuleSymbols::from_declarations(owner, &file.declarations).unwrap();

        assert!(symbols.struct_types().contains_key("T"));
        assert!(symbols.constructors().contains_key("T"));
    }

    #[test]
    fn exported_surface_uses_canonical_import_item_spellings() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source(
            "pub const node JPY: Dimensionless = 1.0;\n\
             pub base unit JPY: Dimensionless;\n\
             pub type Student { Student }\n\
             pub dim Information = Dimensionless;\n\
             pub index Category = { A };",
        );
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();

        let rendered = resolver
            .exported_import_items(&owner)
            .unwrap()
            .iter()
            .map(ExportedImportItem::render)
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            [
                "JPY",
                "Student",
                "type Student",
                "dim Information",
                "unit JPY",
                "index Category",
            ]
        );
    }

    #[test]
    fn local_value_constructor_name_collision_is_rejected_in_either_order() {
        let owner = DagId::root_in_package("test", "main");
        for source in [
            "type Choice { Red }\nconst node Red: Dimensionless = 1.0;",
            "const node Red: Dimensionless = 1.0;\ntype Choice { Red }",
        ] {
            let file = desugared_source(source);
            let err =
                ModuleSymbols::from_declarations(owner.clone(), &file.declarations).unwrap_err();
            assert!(matches!(
                err,
                ModuleResolveError::DuplicateSymbol {
                    owner: err_owner,
                    namespace: "name",
                    name,
                    ..
                } if err_owner == owner && name == "Red"
            ));
        }
    }

    #[test]
    fn unit_import_colliding_with_local_unit_is_rejected() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub base unit m: Dimensionless;");
        let main = desugared_source("base unit m: Dimensionless;\nimport lib.{ unit m };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateImportName {
                owner,
                namespace: "UnitName",
                name,
                ..
            } if owner == main_id && name == "m"
        ));
    }

    #[test]
    fn constructor_import_colliding_with_local_constructor_is_rejected() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type Foreign { Mk }");
        let main = desugared_source("type Local { Mk }\nimport lib.{ Mk };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateImportName {
                owner,
                namespace: "ConstructorName",
                name,
                ..
            } if owner == main_id && name == "Mk"
        ));
    }

    #[test]
    fn imported_value_constructor_collisions_are_rejected_in_either_direction() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");

        for (lib_source, main_source) in [
            (
                "pub type Foreign { Red }",
                "const node Red: Dimensionless = 1.0;\nimport lib.{ Red };",
            ),
            (
                "pub const node Red: Dimensionless = 1.0;",
                "type Local { Red }\nimport lib.{ Red };",
            ),
        ] {
            let lib = desugared_source(lib_source);
            let main = desugared_source(main_source);
            let (import_path, import_kind) = first_import(&main);
            let mut resolver = ModuleResolver::default();
            resolver
                .add_module(lib_id.clone(), &lib.declarations)
                .unwrap();
            resolver
                .add_module(main_id.clone(), &main.declarations)
                .unwrap();

            let err = resolver
                .register_import(&main_id, import_path, import_kind, &lib_id)
                .unwrap_err();
            assert!(matches!(
                err,
                ModuleResolveError::DuplicateImportName {
                    owner,
                    namespace: "name",
                    name,
                    ..
                } if owner == main_id && name == "Red"
            ));
        }
    }

    #[test]
    fn inline_include_index_collision_is_rejected() {
        let main_id = DagId::root_in_package("test", "main");
        let first_id = main_id.child("first");
        let second_id = main_id.child("second");
        let main = desugared_source("");
        let first = desugared_source("pub index Step = { A };");
        let second = desugared_source("pub index Step = { B };");
        let bound = HashSet::new();

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .add_module(first_id.clone(), &first.declarations)
            .unwrap();
        resolver
            .add_module(second_id.clone(), &second.declarations)
            .unwrap();
        resolver
            .inline_instantiated_include_indexes(&main_id, &first_id, &bound)
            .unwrap();

        let err = resolver
            .inline_instantiated_include_indexes(&main_id, &second_id, &bound)
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::DuplicateSymbol {
                owner,
                namespace: "IndexName",
                name,
                ..
            } if owner == main_id && name == "Step"
        ));
    }

    #[test]
    fn ambiguous_bare_variant_candidates_are_sorted() {
        let a_id = DagId::root_in_package("test", "a");
        let z_id = DagId::root_in_package("test", "z");
        let main_id = DagId::root_in_package("test", "main");
        let a = desugared_source("pub index AIndex = { Shared };");
        let z = desugared_source("pub index ZIndex = { Shared };");
        let main = desugared_source("import z.{ index ZIndex };\nimport a.{ index AIndex };");
        let imports = imports(&main);

        let mut resolver = ModuleResolver::default();
        resolver.add_module(z_id.clone(), &z.declarations).unwrap();
        resolver.add_module(a_id.clone(), &a.declarations).unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, imports[0].0, imports[0].1, &z_id)
            .unwrap();
        resolver
            .register_import(&main_id, imports[1].0, imports[1].1, &a_id)
            .unwrap();

        let err = resolver
            .resolve_bare_index_variant(&main_id, &IndexVariantName::expect_valid("Shared"))
            .unwrap_err();
        let ModuleResolveError::AmbiguousIndexVariant { indexes, .. } = err else {
            panic!("expected ambiguous variant, got {err:?}");
        };
        let mut sorted = indexes.clone();
        sorted.sort();
        assert_eq!(indexes, sorted);
    }

    #[test]
    fn selective_import_cross_universe_name_collision_is_rejected() {
        let type_lib_id = DagId::root_in_package("test", "type_lib");
        let index_lib_id = DagId::root_in_package("test", "index_lib");
        let main_id = DagId::root_in_package("test", "main");
        let type_lib = desugared_source("pub type M { Mk(v: Dimensionless) }");
        let index_lib = desugared_source("pub index M = { A, B };");
        let main = desugared_source(
            "import type_lib.{ type M };
             import index_lib.{ index M };",
        );
        let imports = imports(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(type_lib_id.clone(), &type_lib.declarations)
            .unwrap();
        resolver
            .add_module(index_lib_id.clone(), &index_lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, imports[0].0, imports[0].1, &type_lib_id)
            .unwrap();
        let err = resolver
            .register_import(&main_id, imports[1].0, imports[1].1, &index_lib_id)
            .unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::DuplicateImportName {
                owner,
                namespace: "name",
                name,
                ..
            } if owner == main_id && name == "M"
        ));
    }

    #[test]
    fn resolves_qualified_index_variant_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub index Phase = { Burn, Coast };");
        let main = desugared_source("import lib as physics;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let resolved_name = resolver
            .resolve_index_variant_path(&main_id, &path(&["physics", "Phase", "Burn"]))
            .unwrap();

        assert_eq!(resolved_name.index().owner(), &lib_id);
        assert_eq!(resolved_name.index().as_str(), "Phase");
        assert_eq!(resolved_name.variant().as_str(), "Burn");
    }

    #[test]
    fn selective_type_alias_resolves_to_original_owner_and_leaf() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type Vec3 { Vec3 }");
        let main = desugared_source("import lib.{ type Vec3 as Vector };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let resolved_name = resolver
            .resolve_struct_type_path(&main_id, &path(&["Vector"]))
            .unwrap();

        assert_eq!(resolved_name.owner(), &lib_id);
        assert_eq!(resolved_name.as_str(), "Vec3");
    }

    #[test]
    fn type_import_in_child_dag_does_not_import_same_named_constructor() {
        let main_id = DagId::root_in_package("test", "main");
        let child_id = main_id.child("build_transfer");
        let main = desugared_source(
            "pub type TransferResult { TransferResult }
             dag build_transfer {
                 import main.{ type TransferResult };
             }",
        );
        let dag = first_dag(&main);
        let import = dag
            .body
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Import(import) => Some((&import.path, &import.kind)),
                _ => None,
            })
            .expect("dag body should contain an import");

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver.add_module(child_id.clone(), &dag.body).unwrap();
        resolver
            .register_import(&child_id, import.0, import.1, &main_id)
            .unwrap();

        let resolved_type = resolver
            .resolve_struct_type_path(&child_id, &path(&["TransferResult"]))
            .unwrap();
        assert_eq!(resolved_type.owner(), &main_id);
        assert_eq!(resolved_type.as_str(), "TransferResult");

        let err = resolver
            .resolve_constructor_path(&child_id, &path(&["TransferResult"]))
            .unwrap_err();
        assert!(matches!(
            err,
            ModuleResolveError::WrongUniverseName {
                owner,
                name,
                expected: SurfaceNameKind::Constructor,
                actual: SurfaceNameKind::Type,
                ..
            } if owner == child_id && name == "TransferResult"
        ));
    }

    #[test]
    fn type_marker_importing_index_reports_wrong_universe() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub index M = { A };");
        let main = desugared_source("import lib.{ type M };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap_err();
        assert!(err.to_string().contains("did you mean `index M`?"));

        assert!(matches!(
            err,
            ModuleResolveError::WrongImportCategory {
                owner,
                mismatch,
                ..
            } if owner == lib_id
                && mismatch.name().as_str() == "M"
                && mismatch.expected() == ImportItemNamespace::Type
                && mismatch.alternatives().as_slice() == [ImportItemNamespace::Index]
        ));
    }

    #[test]
    fn bare_importing_type_reports_wrong_category() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type Foo { MkFoo }");
        let main = desugared_source("import lib.{ Foo };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap_err();
        assert!(err.to_string().contains("did you mean `type Foo`?"));

        assert!(matches!(
            err,
            ModuleResolveError::WrongImportCategory {
                owner,
                mismatch,
                ..
            } if owner == lib_id
                && mismatch.name().as_str() == "Foo"
                && mismatch.expected() == ImportItemNamespace::Term
                && mismatch.alternatives().as_slice() == [ImportItemNamespace::Type]
        ));
    }

    #[test]
    fn wrong_import_category_lists_all_legal_same_name_alternatives() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source(
            "pub const node JPY: Dimensionless = 1.0;\n\
             pub base unit JPY: Dimensionless;",
        );
        let main = desugared_source("import lib.{ dim JPY };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("did you mean `JPY` or `unit JPY`?")
        );
        assert!(matches!(
            err,
            ModuleResolveError::WrongImportCategory { mismatch, .. }
                if mismatch.alternatives().as_slice()
                    == [ImportItemNamespace::Term, ImportItemNamespace::Unit]
        ));
    }

    #[test]
    fn qualified_private_type_is_rejected() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("type Secret { Secret }");
        let main = desugared_source("import lib as hidden;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let err = resolver
            .resolve_struct_type_path(&main_id, &path(&["hidden", "Secret"]))
            .unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::PrivateName {
                owner,
                namespace: "StructTypeName",
                name,
            } if owner == lib_id && name == "Secret"
        ));
    }

    #[test]
    fn include_selective_private_decl_is_rejected() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("node hidden: Dimensionless = 1.0;");
        let main = desugared_source("include lib().{ hidden };");
        let (include_path, include_kind) = first_include(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();

        let err = resolver
            .register_include(&main_id, include_path, include_kind, &lib_id)
            .unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::PrivateName {
                owner,
                namespace: _,
                name,
            } if owner == lib_id && name == "hidden"
        ));
    }

    #[test]
    fn loaded_sibling_file_is_not_callable_without_an_import() {
        let main_id = DagId::new("test", NonEmpty::new("src", vec!["pkg", "main"]));
        let sibling_id = DagId::new("test", NonEmpty::new("src", vec!["pkg", "library"]));
        let empty = desugared_source("");
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(main_id.clone(), &empty.declarations)
            .unwrap();
        resolver
            .add_module(sibling_id, &empty.declarations)
            .unwrap();

        assert!(matches!(
            resolver.resolve_module_path(&main_id, &module_path(&["library"])),
            Err(ModuleResolveError::UnknownModule { .. })
        ));
    }

    #[test]
    fn imported_file_module_alias_is_callable_as_its_exact_target() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub node result: Dimensionless = 1.0;");
        let main = desugared_source("import lib;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        assert_eq!(
            resolver
                .resolve_module_path(&main_id, &module_path(&["lib"]))
                .unwrap(),
            lib_id
        );
    }

    #[test]
    fn imported_inline_dag_alias_is_callable_as_its_exact_target() {
        let lib_id = DagId::root_in_package("test", "lib");
        let helper_id = lib_id.child("helper");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source(
            "pub dag helper {
                pub node result: Dimensionless = 1.0;
            }",
        );
        let helper = first_dag(&lib);
        let main = desugared_source("import lib.helper as imported;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver.add_module(lib_id, &lib.declarations).unwrap();
        resolver
            .add_module(helper_id.clone(), &helper.body)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &helper_id)
            .unwrap();

        assert_eq!(
            resolver
                .resolve_module_path(&main_id, &module_path(&["imported"]))
                .unwrap(),
            helper_id
        );
    }

    #[test]
    fn direct_alias_of_private_inline_dag_is_rejected_when_called() {
        let lib_id = DagId::root_in_package("test", "lib");
        let helper_id = lib_id.child("helper");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("dag helper { pub node result: Dimensionless = 1.0; }");
        let helper = first_dag(&lib);
        let main = desugared_source("import lib.helper as imported;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(helper_id.clone(), &helper.body)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &helper_id)
            .unwrap();

        assert!(matches!(
            resolver.resolve_module_path(&main_id, &module_path(&["imported"])),
            Err(ModuleResolveError::PrivateName {
                owner,
                namespace: "dag",
                name,
            }) if owner == lib_id && name == "helper"
        ));
    }

    #[test]
    fn selectively_imported_dag_alias_is_callable() {
        let lib_id = DagId::root_in_package("test", "lib");
        let helper_id = lib_id.child("helper");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub dag helper { pub node result: Dimensionless = 1.0; }");
        let helper = first_dag(&lib);
        let main = desugared_source("import lib.{helper as imported};");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(helper_id.clone(), &helper.body)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        assert_eq!(
            resolver
                .resolve_module_path(&main_id, &module_path(&["imported"]))
                .unwrap(),
            helper_id
        );
    }

    #[test]
    fn local_inline_dag_can_qualify_its_nested_child() {
        let main_id = DagId::root_in_package("test", "main");
        let outer_id = main_id.child("outer");
        let inner_id = outer_id.child("inner");
        let main = desugared_source("dag outer { dag inner {} }");
        let outer = first_dag(&main);
        let inner = outer
            .body
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Dag(dag) => Some(dag),
                _ => None,
            })
            .expect("outer should contain inner");

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver.add_module(outer_id, &outer.body).unwrap();
        resolver.add_module(inner_id.clone(), &inner.body).unwrap();

        assert_eq!(
            resolver
                .resolve_module_path(&main_id, &module_path(&["outer", "inner"]))
                .unwrap(),
            inner_id
        );
    }

    #[test]
    fn included_instance_alias_is_not_callable() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub node result: Dimensionless = 1.0;");
        let main = desugared_source("include lib() as instance;");
        let (include_path, include_kind) = first_include(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_include(&main_id, include_path, include_kind, &lib_id)
            .unwrap();

        assert!(matches!(
            resolver.resolve_module_path(&main_id, &module_path(&["instance"])),
            Err(ModuleResolveError::IncludedInstanceNotCallable { alias, .. })
                if alias.as_str() == "instance"
        ));
    }

    #[test]
    fn local_dag_and_imported_module_alias_are_ambiguous_when_called() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let local_id = main_id.child("shared");
        let lib = desugared_source("pub node result: Dimensionless = 1.0;");
        let main = desugared_source("dag shared {} import lib as shared;");
        let local = first_dag(&main);
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver.add_module(local_id, &local.body).unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        assert!(matches!(
            resolver.resolve_module_path(&main_id, &module_path(&["shared"])),
            Err(ModuleResolveError::AmbiguousCallableModule { targets, .. })
                if targets.len() == 2
        ));
    }

    #[test]
    fn qualified_private_dag_path_is_rejected() {
        let lib_id = DagId::root_in_package("test", "lib");
        let helper_id = lib_id.child("helper");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source(
            "dag helper {
                pub node shown: Dimensionless = 1.0;
            }",
        );
        let main = desugared_source("import lib as lib;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(helper_id, &first_dag(&lib).body)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let err = resolver
            .resolve_module_path(&main_id, &module_path(&["lib", "helper"]))
            .unwrap_err();

        assert!(matches!(
            err,
            ModuleResolveError::PrivateName {
                owner,
                namespace: "dag",
                name,
            } if owner == lib_id && name == "helper"
        ));
    }

    #[test]
    fn qualified_symbol_path_through_private_dag_is_rejected() {
        // Regression: `resolve_symbol_path` resolved the qualifier without
        // the dag-visibility check that `resolve_module_path` enforces, so
        // `lib.helper.shown` resolved even though `helper` is a private dag.
        let lib_id = DagId::root_in_package("test", "lib");
        let helper_id = lib_id.child("helper");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source(
            "dag helper {
                pub node shown: Dimensionless = 1.0;
            }",
        );
        let main = desugared_source("import lib as lib;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(helper_id, &first_dag(&lib).body)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let err = resolver
            .resolve_decl_path(&main_id, &path(&["lib", "helper", "shown"]))
            .unwrap_err();

        assert!(
            matches!(
                err,
                ModuleResolveError::PrivateName {
                    ref owner,
                    namespace: "dag",
                    ref name,
                } if *owner == lib_id && name == "helper"
            ),
            "expected PrivateName for dag `helper`, got: {err:?}"
        );
    }

    #[test]
    fn qualified_constructor_resolves_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type BurnKind { Impulsive, Coast }");
        let main = desugared_source("import lib as mission;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, &lib_id)
            .unwrap();

        let resolved_name = resolver
            .resolve_constructor_path(&main_id, &path(&["mission", "Impulsive"]))
            .unwrap();

        assert_eq!(resolved_name.owner(), &lib_id);
        assert_eq!(resolved_name.as_str(), "Impulsive");
    }

    #[test]
    fn selective_pub_reexport_resolves_to_original_owner() {
        let leaf_id = DagId::root_in_package("test", "leaf");
        let middle_id = DagId::root_in_package("test", "middle");
        let main_id = DagId::root_in_package("test", "main");
        let leaf = desugared_source("pub dim Acceleration = Length / Time^2;");
        let middle = desugared_source("import leaf.{ pub dim Acceleration };");
        let main = desugared_source("import middle.{ dim Acceleration };");
        let (middle_import_path, middle_import_kind) = first_import(&middle);
        let (main_import_path, main_import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(leaf_id.clone(), &leaf.declarations)
            .unwrap();
        resolver
            .add_module(middle_id.clone(), &middle.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&middle_id, middle_import_path, middle_import_kind, &leaf_id)
            .unwrap();
        resolver
            .register_import(&main_id, main_import_path, main_import_kind, &middle_id)
            .unwrap();
        assert_eq!(
            resolver
                .exported_import_items(&middle_id)
                .unwrap()
                .iter()
                .map(ExportedImportItem::render)
                .collect::<Vec<_>>(),
            ["dim Acceleration"]
        );

        let resolved_name = resolver
            .resolve_dimension_path(&main_id, &path(&["Acceleration"]))
            .unwrap();

        assert_eq!(resolved_name.owner(), &leaf_id);
        assert_eq!(resolved_name.as_str(), "Acceleration");
    }
}
