//! Module-aware symbol tables and name resolution scaffolding.
//!
//! This module is the first HIR/resolver-oriented layer after the syntax-first
//! name refactor. It does **not** rewrite the AST yet. Instead it builds typed
//! symbol tables for loaded DAG/module identities and resolves syntactic
//! [`NamePath`] / [`IdentPath`] references to canonical [`ResolvedName`] values.
//!
//! The important invariant is that source qualifier text is used only to look up
//! a module alias in the current module scope. The result of a successful lookup
//! carries the canonical [`DagId`] owner, not the textual alias.

use std::collections::HashMap;

use thiserror::Error;

use crate::dag_id::DagId;
use crate::desugar::resolved_ast as ast;
use crate::syntax::ast::{IdentPath, ImportItem, ImportItemNamespace, ImportKind, ModulePath};
use crate::syntax::names::{
    ConstructorName, DeclName, DimName, IndexName, IndexVariantName, ModuleAliasName, NameAtom,
    NameDef, NameNamespace, NamePath, ResolvedIndexVariant, ResolvedName, StructTypeName, UnitName,
    namespace,
};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};

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
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public | Self::PublicBind)
    }

    /// Returns whether the symbol can be rebound by include-time bindings.
    #[must_use]
    pub const fn is_bindable(self) -> bool {
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

/// Visibility rule applied by a module alias or selective import edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModuleAccess {
    /// Cross-module `import`: only public target symbols are accessible.
    PublicOnly,
    /// Instantiated `include`: the imported DAG body is being embedded, so
    /// private implementation declarations can be addressed by the include
    /// lowering boundary.
    IncludePrivate,
}

impl ModuleAccess {
    const fn requires_public(self) -> bool {
        matches!(self, Self::PublicOnly)
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
    pub const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Visibility of this symbol across module boundaries.
    #[must_use]
    pub const fn visibility(&self) -> SymbolVisibility {
        self.visibility
    }

    /// Source span of the definition-site name.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// Index symbol plus the variants declared by that index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIndexSymbol {
    symbol: ModuleSymbol<namespace::Index>,
    variants: HashMap<IndexVariantName, Span>,
}

impl ModuleIndexSymbol {
    /// Canonical resolved identity for the index type.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedName<namespace::Index> {
        self.symbol.resolved()
    }

    /// Visibility of the index declaration.
    #[must_use]
    pub const fn visibility(&self) -> SymbolVisibility {
        self.symbol.visibility()
    }

    /// Source span of the index definition-site name.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.symbol.span()
    }

    /// Variant names declared by this index, keyed by leaf name.
    #[must_use]
    pub const fn variants(&self) -> &HashMap<IndexVariantName, Span> {
        &self.variants
    }
}

/// Symbols declared by a single DAG/module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbols {
    owner: DagId,
    decls: HashMap<DeclName, ModuleSymbol<namespace::Decl>>,
    dimensions: HashMap<DimName, ModuleSymbol<namespace::Dim>>,
    units: HashMap<UnitName, ModuleSymbol<namespace::Unit>>,
    struct_types: HashMap<StructTypeName, ModuleSymbol<namespace::StructType>>,
    indexes: HashMap<IndexName, ModuleIndexSymbol>,
    constructors: HashMap<ConstructorName, ModuleSymbol<namespace::Constructor>>,
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
    pub fn from_declarations(
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
    pub const fn decls(&self) -> &HashMap<DeclName, ModuleSymbol<namespace::Decl>> {
        &self.decls
    }

    /// Dimension namespace symbols.
    #[must_use]
    pub const fn dimensions(&self) -> &HashMap<DimName, ModuleSymbol<namespace::Dim>> {
        &self.dimensions
    }

    /// Unit namespace symbols.
    #[must_use]
    pub const fn units(&self) -> &HashMap<UnitName, ModuleSymbol<namespace::Unit>> {
        &self.units
    }

    /// Struct/tagged-union type namespace symbols.
    #[must_use]
    pub const fn struct_types(
        &self,
    ) -> &HashMap<StructTypeName, ModuleSymbol<namespace::StructType>> {
        &self.struct_types
    }

    /// Index namespace symbols.
    #[must_use]
    pub const fn indexes(&self) -> &HashMap<IndexName, ModuleIndexSymbol> {
        &self.indexes
    }

    /// Tagged-union constructor namespace symbols.
    #[must_use]
    pub const fn constructors(
        &self,
    ) -> &HashMap<ConstructorName, ModuleSymbol<namespace::Constructor>> {
        &self.constructors
    }

    fn collect_declarations(
        &mut self,
        declarations: &[ast::Declaration],
    ) -> Result<(), ModuleResolveError> {
        for decl in declarations {
            match &decl.kind {
                ast::DeclKind::Param(p) => self.insert_decl(
                    &p.name,
                    SymbolVisibility::PublicBind,
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Node(n) => self.insert_decl(
                    &n.name,
                    SymbolVisibility::from(n.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::ConstNode(c) => self.insert_decl(
                    &c.name,
                    SymbolVisibility::from(c.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Assert(a) => self.insert_decl(
                    &a.name,
                    SymbolVisibility::from(a.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Plot(p) => self.insert_decl(
                    &p.name,
                    SymbolVisibility::from(p.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Figure(f) => self.insert_decl(
                    &f.name,
                    SymbolVisibility::from(f.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Layer(l) => self.insert_decl(
                    &l.name,
                    SymbolVisibility::from(l.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::Dag(d) => self.insert_decl(
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                    namespace::Decl::DISPLAY_NAME,
                )?,
                ast::DeclKind::BaseDimension(d) => self.insert_dimension(
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                    namespace::Dim::DISPLAY_NAME,
                )?,
                ast::DeclKind::Dimension(d) => self.insert_dimension(
                    &d.name,
                    SymbolVisibility::from(d.visibility),
                    namespace::Dim::DISPLAY_NAME,
                )?,
                ast::DeclKind::Unit(u) => self.insert_unit(
                    &u.name,
                    SymbolVisibility::from(u.visibility),
                    namespace::Unit::DISPLAY_NAME,
                )?,
                ast::DeclKind::Type(t) => {
                    let visibility = SymbolVisibility::from(t.visibility);
                    self.insert_struct_type(
                        &t.name,
                        visibility,
                        namespace::StructType::DISPLAY_NAME,
                    )?;
                    if let ast::TypeDeclBody::Constructors(members) = &t.body {
                        for member in members {
                            self.insert_constructor(
                                &member.name,
                                visibility,
                                namespace::Constructor::DISPLAY_NAME,
                            )?;
                        }
                    }
                }
                ast::DeclKind::Index(i) => self.insert_index(i)?,
                ast::DeclKind::Import(_) | ast::DeclKind::Include(_) => {}
                ast::DeclKind::Sugar(s) => never(*s),
            }
        }
        Ok(())
    }

    fn insert_decl(
        &mut self,
        name: &Spanned<DeclName>,
        visibility: SymbolVisibility,
        namespace_name: &'static str,
    ) -> Result<(), ModuleResolveError> {
        insert_symbol(
            &self.owner,
            &mut self.decls,
            name,
            visibility,
            namespace_name,
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
    ) -> Result<(), ModuleResolveError> {
        insert_symbol(
            &self.owner,
            &mut self.struct_types,
            name,
            visibility,
            namespace_name,
        )
    }

    fn insert_constructor(
        &mut self,
        name: &Spanned<ConstructorName>,
        visibility: SymbolVisibility,
        namespace_name: &'static str,
    ) -> Result<(), ModuleResolveError> {
        insert_symbol(
            &self.owner,
            &mut self.constructors,
            name,
            visibility,
            namespace_name,
        )
    }

    fn insert_index(&mut self, index: &ast::IndexDecl) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.indexes.get(index.name.value.as_str()) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: namespace::Index::DISPLAY_NAME,
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
                        namespace: namespace::IndexVariant::DISPLAY_NAME,
                        name: format!("{}.{}", index.name.value, variant.value),
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

/// A resolved module alias in one module's import scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleAliasTarget {
    target: DagId,
    span: Span,
    access: ModuleAccess,
}

impl ModuleAliasTarget {
    /// Canonical DAG/module targeted by the alias.
    #[must_use]
    pub const fn target(&self) -> &DagId {
        &self.target
    }

    /// Source span of the local alias name.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Visibility rule for names reached through this alias.
    #[must_use]
    pub const fn access(&self) -> ModuleAccess {
        self.access
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
    fn new(resolved: ResolvedName<Ns>, span: Span, visibility: SymbolVisibility) -> Self {
        Self {
            resolved,
            span,
            visibility,
        }
    }

    /// Canonical target identity of the imported symbol.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Source span of the local import name.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Visibility of this selective import when the importing module is itself imported.
    #[must_use]
    pub const fn visibility(&self) -> SymbolVisibility {
        self.visibility
    }
}

/// Import scope for a single module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleScope {
    module_aliases: HashMap<ModuleAliasName, ModuleAliasTarget>,
    selected_decls: HashMap<DeclName, ImportedSymbol<namespace::Decl>>,
    selected_dimensions: HashMap<DimName, ImportedSymbol<namespace::Dim>>,
    selected_units: HashMap<UnitName, ImportedSymbol<namespace::Unit>>,
    selected_struct_types: HashMap<StructTypeName, ImportedSymbol<namespace::StructType>>,
    selected_indexes: HashMap<IndexName, ImportedSymbol<namespace::Index>>,
    selected_constructors: HashMap<ConstructorName, ImportedSymbol<namespace::Constructor>>,
}

impl ModuleScope {
    /// Module aliases introduced by whole-module imports/includes.
    #[must_use]
    pub const fn module_aliases(&self) -> &HashMap<ModuleAliasName, ModuleAliasTarget> {
        &self.module_aliases
    }
}

#[derive(Debug, Clone)]
enum ImportAddition {
    ModuleAlias {
        alias: Spanned<ModuleAliasName>,
        target: DagId,
        access: ModuleAccess,
    },
    Decl {
        local: Spanned<DeclName>,
        target: ResolvedName<namespace::Decl>,
        visibility: SymbolVisibility,
    },
    Dimension {
        local: Spanned<DimName>,
        target: ResolvedName<namespace::Dim>,
        visibility: SymbolVisibility,
    },
    Unit {
        local: Spanned<UnitName>,
        target: ResolvedName<namespace::Unit>,
        visibility: SymbolVisibility,
    },
    StructType {
        local: Spanned<StructTypeName>,
        target: ResolvedName<namespace::StructType>,
        visibility: SymbolVisibility,
    },
    Index {
        local: Spanned<IndexName>,
        target: ResolvedName<namespace::Index>,
        visibility: SymbolVisibility,
    },
    Constructor {
        local: Spanned<ConstructorName>,
        target: ResolvedName<namespace::Constructor>,
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
        self.scopes.entry(owner.clone()).or_default();
        self.modules.insert(owner, symbols);
        Ok(())
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
        target: DagId,
    ) -> Result<(), ModuleResolveError> {
        self.register_import_with_access(owner, path, kind, target, ModuleAccess::PublicOnly)
    }

    /// Register one loader-resolved `include` edge in `owner`'s scope.
    ///
    /// Instantiated includes embed the dependency DAG body, so this boundary is
    /// allowed to address private implementation declarations in the target.
    /// Use [`Self::register_import`] for source `import` declarations, which
    /// enforce public visibility.
    pub fn register_include(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        kind: &ImportKind,
        target: DagId,
    ) -> Result<(), ModuleResolveError> {
        self.register_import_with_access(owner, path, kind, target, ModuleAccess::IncludePrivate)
    }

    fn register_import_with_access(
        &mut self,
        owner: &DagId,
        path: &ModulePath,
        kind: &ImportKind,
        target: DagId,
        access: ModuleAccess,
    ) -> Result<(), ModuleResolveError> {
        self.module_symbols(owner)?;
        self.module_symbols(&target)?;

        let additions = self.import_additions(path, kind, &target, access)?;
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
    pub fn resolve_decl_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::Decl>, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::decls, |scope| {
            &scope.selected_decls
        })
    }

    /// Resolve a syntactic dimension path to a canonical owner + leaf.
    pub fn resolve_dimension_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::Dim>, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::dimensions, |scope| {
            &scope.selected_dimensions
        })
    }

    /// Resolve a syntactic unit path to a canonical owner + leaf.
    pub fn resolve_unit_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::Unit>, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::units, |scope| {
            &scope.selected_units
        })
    }

    /// Resolve a syntactic struct/tagged-union type path to a canonical owner + leaf.
    pub fn resolve_struct_type_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::StructType>, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::struct_types, |scope| {
            &scope.selected_struct_types
        })
    }

    /// Resolve a syntactic tagged-union constructor path to a canonical owner + leaf.
    pub fn resolve_constructor_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::Constructor>, ModuleResolveError> {
        self.resolve_symbol_path(owner, path, ModuleSymbols::constructors, |scope| {
            &scope.selected_constructors
        })
    }

    /// Resolve a span-aware constructor path without losing source path shape at
    /// the caller boundary.
    pub fn resolve_constructor_ident_path(
        &self,
        owner: &DagId,
        path: &IdentPath,
    ) -> Result<ResolvedName<namespace::Constructor>, ModuleResolveError> {
        self.resolve_constructor_path(owner, &ident_path_to_name_path(path))
    }

    /// Resolve a syntactic index path to a canonical owner + leaf.
    pub fn resolve_index_path(
        &self,
        owner: &DagId,
        path: &NamePath,
    ) -> Result<ResolvedName<namespace::Index>, ModuleResolveError> {
        if let Some(atom) = path.as_bare() {
            let local = self.module_symbols(owner)?;
            if let Some(symbol) = local.indexes.get(atom.as_str()) {
                return Ok(symbol.resolved().clone());
            }
            let scope = self.module_scope(owner)?;
            if let Some(imported) = scope.selected_indexes.get(atom.as_str()) {
                return Ok(imported.resolved().clone());
            }
            return Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: namespace::Index::DISPLAY_NAME,
                name: atom.to_string(),
            });
        }

        let (qualifier, leaf) = path
            .qualifier_and_leaf()
            .expect("qualified path has a qualifier");
        let target_ref = self.resolve_module_qualifier(owner, qualifier)?;
        let target = self.module_symbols(&target_ref.owner)?;
        if let Some(symbol) = target.indexes.get(leaf.as_str()) {
            if target_ref.access.requires_public() && !symbol.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    namespace: namespace::Index::DISPLAY_NAME,
                    name: leaf.to_string(),
                });
            }
            return Ok(symbol.resolved().clone());
        }

        let target_scope = self.module_scope(&target_ref.owner)?;
        if let Some(imported) = target_scope.selected_indexes.get(leaf.as_str()) {
            if target_ref.access.requires_public() && !imported.visibility().is_public() {
                return Err(ModuleResolveError::PrivateName {
                    owner: target_ref.owner,
                    namespace: namespace::Index::DISPLAY_NAME,
                    name: leaf.to_string(),
                });
            }
            return Ok(imported.resolved().clone());
        }

        Err(ModuleResolveError::UnknownName {
            owner: target_ref.owner,
            namespace: namespace::Index::DISPLAY_NAME,
            name: leaf.to_string(),
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
    pub fn resolve_index_variant_parts(
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
                namespace: namespace::Index::DISPLAY_NAME,
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

    /// Resolve a source inline-DAG/module path to its canonical [`DagId`].
    ///
    /// Single-segment paths name inline DAG children of `owner`. Qualified
    /// paths use the first segment as a module alias and append the remaining
    /// segments to the alias target. The returned identity is canonical; source
    /// qualifier text is not carried beyond this resolver boundary.
    pub fn resolve_module_path(
        &self,
        owner: &DagId,
        path: &ModulePath,
    ) -> Result<DagId, ModuleResolveError> {
        if let [leaf] = path.segments() {
            let target = owner.child(leaf.name.as_str());
            if self.modules.contains_key(&target) {
                return Ok(target);
            }
            return Err(ModuleResolveError::UnknownModule { owner: target });
        }

        let atoms = path
            .segments()
            .iter()
            .map(|segment| segment.name.clone())
            .collect::<Vec<_>>();
        self.resolve_module_qualifier(owner, &atoms)
            .map(|resolved| resolved.owner)
    }

    fn import_additions(
        &self,
        path: &ModulePath,
        kind: &ImportKind,
        target: &DagId,
        access: ModuleAccess,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        match kind {
            ImportKind::Module { alias } => {
                let alias = alias.clone().map_or_else(
                    || {
                        Spanned::new(
                            ModuleAliasName::from_atom(path.leaf().name.clone()),
                            path.leaf().span,
                        )
                    },
                    |alias| alias,
                );
                Ok(vec![ImportAddition::ModuleAlias {
                    alias,
                    target: target.clone(),
                    access,
                }])
            }
            ImportKind::Selective(items) => items
                .iter()
                .map(|item| self.import_item_additions(target, item, access))
                .collect::<Result<Vec<_>, _>>()
                .map(|chunks| chunks.into_iter().flatten().collect()),
        }
    }

    fn import_item_additions(
        &self,
        target: &DagId,
        item: &ImportItem,
        access: ModuleAccess,
    ) -> Result<Vec<ImportAddition>, ModuleResolveError> {
        let source_atom = &item.name.name;
        let local_atom = item
            .alias
            .as_ref()
            .map_or_else(|| item.name.name.clone(), |alias| alias.name.clone());
        let local_span = item.local_span();
        let local_visibility = if item.is_pub {
            SymbolVisibility::Public
        } else {
            SymbolVisibility::Private
        };

        match item.namespace {
            ImportItemNamespace::Type => {
                match self.exported_symbol_for_import(
                    target,
                    source_atom,
                    access,
                    ModuleSymbols::struct_types,
                    |scope| &scope.selected_struct_types,
                )? {
                    ExportLookup::Public(target_name) => Ok(vec![ImportAddition::StructType {
                        local: Spanned::new(StructTypeName::from_atom(local_atom), local_span),
                        target: target_name,
                        visibility: local_visibility,
                    }]),
                    ExportLookup::Private => Err(ModuleResolveError::PrivateName {
                        owner: target.clone(),
                        namespace: namespace::StructType::DISPLAY_NAME,
                        name: source_atom.to_string(),
                    }),
                    ExportLookup::Missing => Err(ModuleResolveError::UnknownName {
                        owner: target.clone(),
                        namespace: namespace::StructType::DISPLAY_NAME,
                        name: source_atom.to_string(),
                    }),
                }
            }
            ImportItemNamespace::Default => {
                let mut additions = Vec::new();
                let mut saw_private = false;

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
                        visibility: local_visibility,
                    }),
                    ExportLookup::Private => saw_private = true,
                    ExportLookup::Missing => {}
                }
                match self.exported_symbol_for_import(
                    target,
                    source_atom,
                    access,
                    ModuleSymbols::dimensions,
                    |scope| &scope.selected_dimensions,
                )? {
                    ExportLookup::Public(target_name) => {
                        additions.push(ImportAddition::Dimension {
                            local: Spanned::new(DimName::from_atom(local_atom.clone()), local_span),
                            target: target_name,
                            visibility: local_visibility,
                        })
                    }
                    ExportLookup::Private => saw_private = true,
                    ExportLookup::Missing => {}
                }
                match self.exported_symbol_for_import(
                    target,
                    source_atom,
                    access,
                    ModuleSymbols::units,
                    |scope| &scope.selected_units,
                )? {
                    ExportLookup::Public(target_name) => additions.push(ImportAddition::Unit {
                        local: Spanned::new(UnitName::from_atom(local_atom.clone()), local_span),
                        target: target_name,
                        visibility: local_visibility,
                    }),
                    ExportLookup::Private => saw_private = true,
                    ExportLookup::Missing => {}
                }
                match self.exported_index_for_import(target, source_atom, access)? {
                    ExportLookup::Public(target_name) => additions.push(ImportAddition::Index {
                        local: Spanned::new(IndexName::from_atom(local_atom.clone()), local_span),
                        target: target_name,
                        visibility: local_visibility,
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
                    ExportLookup::Public(target_name) => {
                        additions.push(ImportAddition::Constructor {
                            local: Spanned::new(ConstructorName::from_atom(local_atom), local_span),
                            target: target_name,
                            visibility: local_visibility,
                        })
                    }
                    ExportLookup::Private => saw_private = true,
                    ExportLookup::Missing => {}
                }

                if !additions.is_empty() {
                    Ok(additions)
                } else if saw_private {
                    Err(ModuleResolveError::PrivateName {
                        owner: target.clone(),
                        namespace: "default import namespace",
                        name: source_atom.to_string(),
                    })
                } else {
                    Err(ModuleResolveError::UnknownName {
                        owner: target.clone(),
                        namespace: "default import namespace",
                        name: source_atom.to_string(),
                    })
                }
            }
        }
    }

    fn exported_symbol_for_import<Ns: NameNamespace>(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: ModuleAccess,
        local_symbols: fn(&ModuleSymbols) -> &HashMap<NameDef<Ns>, ModuleSymbol<Ns>>,
        selected_symbols: fn(&ModuleScope) -> &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    ) -> Result<ExportLookup<Ns>, ModuleResolveError> {
        let target_symbols = self.module_symbols(target)?;
        match exported_symbol(local_symbols(target_symbols), atom, access) {
            ExportLookup::Missing => {}
            found => return Ok(found),
        }

        let target_scope = self.module_scope(target)?;
        Ok(exported_imported_symbol(
            selected_symbols(target_scope),
            atom,
            access,
        ))
    }

    fn exported_index_for_import(
        &self,
        target: &DagId,
        atom: &NameAtom,
        access: ModuleAccess,
    ) -> Result<ExportLookup<namespace::Index>, ModuleResolveError> {
        let target_symbols = self.module_symbols(target)?;
        match exported_index_symbol(target_symbols.indexes(), atom, access) {
            ExportLookup::Missing => {}
            found => return Ok(found),
        }

        let target_scope = self.module_scope(target)?;
        Ok(exported_imported_symbol(
            &target_scope.selected_indexes,
            atom,
            access,
        ))
    }

    fn resolve_symbol_path<Ns: NameNamespace>(
        &self,
        owner: &DagId,
        path: &NamePath,
        local_symbols: fn(&ModuleSymbols) -> &HashMap<NameDef<Ns>, ModuleSymbol<Ns>>,
        selected_symbols: fn(&ModuleScope) -> &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    ) -> Result<ResolvedName<Ns>, ModuleResolveError> {
        if let Some(atom) = path.as_bare() {
            let local = self.module_symbols(owner)?;
            if let Some(symbol) = local_symbols(local).get(atom.as_str()) {
                return Ok(symbol.resolved().clone());
            }
            let scope = self.module_scope(owner)?;
            if let Some(imported) = selected_symbols(scope).get(atom.as_str()) {
                return Ok(imported.resolved().clone());
            }
            return Err(ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: Ns::DISPLAY_NAME,
                name: atom.to_string(),
            });
        }

        let (qualifier, leaf) = path
            .qualifier_and_leaf()
            .expect("qualified path has a qualifier");
        let target_ref = self.resolve_module_qualifier(owner, qualifier)?;
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

        Err(ModuleResolveError::UnknownName {
            owner: target_ref.owner,
            namespace: Ns::DISPLAY_NAME,
            name: leaf.to_string(),
        })
    }

    fn resolve_module_qualifier(
        &self,
        owner: &DagId,
        qualifier: &[NameAtom],
    ) -> Result<ResolvedModuleQualifier, ModuleResolveError> {
        let (head, rest) = qualifier
            .split_first()
            .expect("qualified path has non-empty qualifier");
        let scope = self.module_scope(owner)?;
        let alias = ModuleAliasName::from_atom(head.clone());
        let alias_target = scope.module_aliases.get(alias.as_str()).ok_or_else(|| {
            ModuleResolveError::UnknownModuleAlias {
                owner: owner.clone(),
                alias,
            }
        })?;
        let target = rest
            .iter()
            .fold(alias_target.target.clone(), |owner, segment| {
                owner.child(segment.as_str())
            });
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

    fn module_scope(&self, owner: &DagId) -> Result<&ModuleScope, ModuleResolveError> {
        self.scopes
            .get(owner)
            .ok_or_else(|| ModuleResolveError::UnknownModule {
                owner: owner.clone(),
            })
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
            } => insert_module_alias(
                owner,
                &mut self.module_aliases,
                alias,
                target,
                access,
                namespace::ModuleAlias::DISPLAY_NAME,
            ),
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
                namespace::Decl::DISPLAY_NAME,
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
                namespace::Dim::DISPLAY_NAME,
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
                namespace::Unit::DISPLAY_NAME,
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
                namespace::StructType::DISPLAY_NAME,
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
                namespace::Index::DISPLAY_NAME,
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
                namespace::Constructor::DISPLAY_NAME,
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
        },
    );
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

fn exported_symbol<Ns: NameNamespace>(
    map: &HashMap<NameDef<Ns>, ModuleSymbol<Ns>>,
    atom: &NameAtom,
    access: ModuleAccess,
) -> ExportLookup<Ns> {
    map.get(atom.as_str())
        .map_or(ExportLookup::Missing, |symbol| {
            if !access.requires_public() || symbol.visibility().is_public() {
                ExportLookup::Public(symbol.resolved().clone())
            } else {
                ExportLookup::Private
            }
        })
}

fn exported_imported_symbol<Ns: NameNamespace>(
    map: &HashMap<NameDef<Ns>, ImportedSymbol<Ns>>,
    atom: &NameAtom,
    access: ModuleAccess,
) -> ExportLookup<Ns> {
    map.get(atom.as_str())
        .map_or(ExportLookup::Missing, |symbol| {
            if !access.requires_public() || symbol.visibility().is_public() {
                ExportLookup::Public(symbol.resolved().clone())
            } else {
                ExportLookup::Private
            }
        })
}

fn exported_index_symbol(
    map: &HashMap<IndexName, ModuleIndexSymbol>,
    atom: &NameAtom,
    access: ModuleAccess,
) -> ExportLookup<namespace::Index> {
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
    NamePath::new(
        NonEmpty::try_from_vec(
            path.segments()
                .iter()
                .map(|ident| ident.name.clone())
                .collect(),
        )
        .expect("IdentPath is non-empty"),
    )
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
        index: ResolvedName<namespace::Index>,
        variant: IndexVariantName,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn resolved_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw);
        crate::syntax::name_resolve::resolve_name_refs(desugared)
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

    fn atom(s: &str) -> NameAtom {
        NameAtom::parse(s).unwrap()
    }

    fn path(segments: &[&str]) -> NamePath {
        let atoms = segments.iter().map(|s| atom(s)).collect::<Vec<_>>();
        NamePath::new(NonEmpty::try_from_vec(atoms).unwrap())
    }

    #[test]
    fn resolves_qualified_index_variant_to_canonical_owner() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub index Phase = { Burn, Coast };");
        let main = resolved_source("import lib as physics;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, lib_id.clone())
            .unwrap();

        let resolved = resolver
            .resolve_index_variant_path(&main_id, &path(&["physics", "Phase", "Burn"]))
            .unwrap();

        assert_eq!(resolved.index().owner(), &lib_id);
        assert_eq!(resolved.index().as_str(), "Phase");
        assert_eq!(resolved.variant().as_str(), "Burn");
    }

    #[test]
    fn selective_type_alias_resolves_to_original_owner_and_leaf() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub type Vec3 { Vec3 }");
        let main = resolved_source("import lib.{ type Vec3 as Vector };");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, lib_id.clone())
            .unwrap();

        let resolved = resolver
            .resolve_struct_type_path(&main_id, &path(&["Vector"]))
            .unwrap();

        assert_eq!(resolved.owner(), &lib_id);
        assert_eq!(resolved.as_str(), "Vec3");
    }

    #[test]
    fn qualified_private_type_is_rejected() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("type Secret { Secret }");
        let main = resolved_source("import lib as hidden;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, lib_id.clone())
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
    fn qualified_constructor_resolves_to_canonical_owner() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub type BurnKind { Impulsive, Coast }");
        let main = resolved_source("import lib as mission;");
        let (import_path, import_kind) = first_import(&main);

        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        resolver
            .register_import(&main_id, import_path, import_kind, lib_id.clone())
            .unwrap();

        let resolved = resolver
            .resolve_constructor_path(&main_id, &path(&["mission", "Impulsive"]))
            .unwrap();

        assert_eq!(resolved.owner(), &lib_id);
        assert_eq!(resolved.as_str(), "Impulsive");
    }

    #[test]
    fn selective_pub_reexport_resolves_to_original_owner() {
        let leaf_id = DagId::root("leaf");
        let middle_id = DagId::root("middle");
        let main_id = DagId::root("main");
        let leaf = resolved_source("pub dim Acceleration = Length / Time^2;");
        let middle = resolved_source("import leaf.{ pub Acceleration };");
        let main = resolved_source("import middle.{ Acceleration };");
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
            .register_import(
                &middle_id,
                middle_import_path,
                middle_import_kind,
                leaf_id.clone(),
            )
            .unwrap();
        resolver
            .register_import(&main_id, main_import_path, main_import_kind, middle_id)
            .unwrap();

        let resolved = resolver
            .resolve_dimension_path(&main_id, &path(&["Acceleration"]))
            .unwrap();

        assert_eq!(resolved.owner(), &leaf_id);
        assert_eq!(resolved.as_str(), "Acceleration");
    }
}
