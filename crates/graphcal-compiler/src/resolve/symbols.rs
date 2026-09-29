//! Per-module declaration symbol tables collected from a desugared AST.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::registry::index::FiniteIndex;
use crate::resolved_name::ResolvedName;
use crate::syntax::ast::{BindableVisibility, UnitConstness};
use crate::syntax::decl_name::{DeclName, DeclNameNamespace};
use crate::syntax::dimension::{DimName, DimNameNamespace, UnitName, UnitNameNamespace};
use crate::syntax::index_name::{IndexName, IndexNameNamespace, IndexVariantName};
use crate::syntax::names::{NameAtom, NameDef, NameNamespace, NamePath};
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeName, StructTypeNameNamespace,
};

use super::category::DeclSymbolKind;
use super::error::ModuleResolveError;
use super::namespace::{Namespace, Namespaced, Occupant};

/// One name visible in a module, bound to its canonical target.
///
/// `resolved` is the canonical identity the name denotes; `span` and
/// `visibility` belong to the binding site (a declaration, or the local name a
/// selective import introduces). `data` is the namespace-specific payload a
/// declaration carries, such as a declaration's kind or a type's generic
/// signature; a selective import carries none (`X = ()`), because its
/// target's payload lives with the target's own declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol<Ns: NameNamespace, X = ()> {
    resolved: ResolvedName<Ns>,
    visibility: BindableVisibility,
    span: Span,
    data: X,
}

impl<Ns: NameNamespace, X> Symbol<Ns, X> {
    /// Bind a name to an already-resolved canonical target.
    pub(super) const fn new(
        resolved: ResolvedName<Ns>,
        visibility: BindableVisibility,
        span: Span,
        data: X,
    ) -> Self {
        Self {
            resolved,
            visibility,
            span,
            data,
        }
    }

    /// Canonical resolved identity for this symbol.
    #[must_use]
    pub(crate) const fn resolved(&self) -> &ResolvedName<Ns> {
        &self.resolved
    }

    /// Visibility of this binding across module boundaries.
    #[must_use]
    pub(crate) const fn visibility(&self) -> BindableVisibility {
        self.visibility
    }

    /// Source span of the binding-site name.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        self.span
    }

    /// Namespace-specific payload of this binding.
    #[must_use]
    pub(crate) const fn data(&self) -> &X {
        &self.data
    }

    /// The same canonical target and payload, bound at another site.
    ///
    /// A selective import binds its local name to exactly what the source
    /// module's binding denotes, including the payload of the target's
    /// declaration.
    pub(super) fn rebind(&self, visibility: BindableVisibility, span: Span) -> Self
    where
        X: Clone,
    {
        Self::new(self.resolved.clone(), visibility, span, self.data.clone())
    }
}

/// A name resolved to the symbol it denotes, with the facts the resolver
/// knows about its target.
///
/// `Kind` is the payload of the target's declaration (for example a
/// declaration's [`DeclSymbolKind`] or a type's generic signature), so a
/// consumer never looks the resolved name up again to learn it.
#[derive(Debug)]
pub struct SymbolRef<'r, Ns: NameNamespace, Kind> {
    symbol: &'r Symbol<Ns, Kind>,
}

impl<Ns: NameNamespace, Kind> Clone for SymbolRef<'_, Ns, Kind> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Ns: NameNamespace, Kind> Copy for SymbolRef<'_, Ns, Kind> {}

impl<'r, Ns: NameNamespace, Kind> SymbolRef<'r, Ns, Kind> {
    pub(super) const fn new(symbol: &'r Symbol<Ns, Kind>) -> Self {
        Self { symbol }
    }

    /// Canonical identity of the target.
    #[must_use]
    pub const fn resolved(self) -> &'r ResolvedName<Ns> {
        &self.symbol.resolved
    }

    /// The payload of the target's declaration.
    #[must_use]
    pub const fn kind(self) -> &'r Kind {
        &self.symbol.data
    }

    /// Visibility of the binding the name was resolved through.
    #[must_use]
    pub const fn visibility(self) -> BindableVisibility {
        self.symbol.visibility
    }

    /// Source span of the binding the name was resolved through.
    #[must_use]
    pub const fn span(self) -> Span {
        self.symbol.span
    }

    /// The canonical identity of the target, owned.
    #[must_use]
    pub fn into_resolved(self) -> ResolvedName<Ns> {
        self.symbol.resolved.clone()
    }

    /// The same target and payload, bound at another site.
    pub(super) fn rebind(self, visibility: BindableVisibility, span: Span) -> Symbol<Ns, Kind>
    where
        Kind: Clone,
    {
        self.symbol.rebind(visibility, span)
    }
}

impl SymbolRef<'_, DeclNameNamespace, DeclSymbolKind> {
    /// Whether an instantiated declaration may be referenced by its consumer.
    ///
    /// Parameters are explicit instance inputs even when they are not declared
    /// `pub`; other declaration kinds require public visibility.
    #[must_use]
    pub fn is_instance_accessible(self) -> bool {
        *self.kind() == DeclSymbolKind::Param || self.visibility().is_public()
    }
}

/// One symbol table: leaf name to binding.
pub type Table<Ns, X = ()> = HashMap<NameDef<Ns>, Symbol<Ns, X>>;

/// Source signature of one generic parameter, retained by name resolution so
/// HIR can sort application arguments after resolving the callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamSignature {
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

/// The identity and generic signature of a constructor's owning type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorSignature {
    pub(super) owner_type: StructTypeName,
    pub(super) generic_params: Vec<GenericParamSignature>,
}

impl ConstructorSignature {
    /// Source signature of the owning type's generic parameters.
    #[must_use]
    pub(crate) fn generic_params(&self) -> &[GenericParamSignature] {
        &self.generic_params
    }
}

/// Symbols declared by a single DAG/module.
///
/// Every table is keyed by the leaf a declaration introduces. The tables
/// partition the module's collision unit ([`Namespace`]): no two of them hold
/// the same leaf in the same namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSymbols {
    pub(super) owner: DagId,
    pub(super) decls: HashMap<DeclName, Symbol<DeclNameNamespace, DeclSymbolKind>>,
    pub(super) dimensions: HashMap<DimName, Symbol<DimNameNamespace>>,
    pub(super) units: HashMap<UnitName, Symbol<UnitNameNamespace, UnitConstness>>,
    pub(super) struct_types:
        HashMap<StructTypeName, Symbol<StructTypeNameNamespace, Vec<GenericParamSignature>>>,
    pub(super) indexes:
        HashMap<IndexName, Symbol<IndexNameNamespace, HashMap<IndexVariantName, Span>>>,
    pub(super) constructors:
        HashMap<ConstructorName, Symbol<ConstructorNameNamespace, ConstructorSignature>>,
    /// Definition sources of dimension symbols a selective include projects
    /// as this module's own declarations; they have no source declaration.
    pub(super) dimension_projections: HashMap<DimName, DimensionProjection>,
    /// Index symbols a selective include projects as this module's own
    /// declarations because their port is bound to a structural `Fin(N)`.
    pub(super) finite_index_projections: HashMap<IndexName, FiniteIndex>,
}

/// A template dimension that a selective include specializes through its
/// dimension bindings (`include lib(dim Q: Length)::{dim QR as R}`).
///
/// The projected symbol is owned by the including module: `QR = Q / Time`
/// denotes a different dimension in every configured instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionProjection {
    template: ResolvedName<DimNameNamespace>,
    ports: Vec<DimensionPortBinding>,
}

impl DimensionProjection {
    pub(super) const fn new(
        template: ResolvedName<DimNameNamespace>,
        ports: Vec<DimensionPortBinding>,
    ) -> Self {
        Self { template, ports }
    }

    /// The template's own dimension the projection specializes.
    #[must_use]
    pub const fn template(&self) -> &ResolvedName<DimNameNamespace> {
        &self.template
    }

    /// The include's dimension bindings, as template port to importer path.
    #[must_use]
    pub fn ports(&self) -> &[DimensionPortBinding] {
        &self.ports
    }
}

/// One `dim Port: Target` binding of an include, with the target spelled in
/// the including module's scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionPortBinding {
    port: DimName,
    target: NamePath,
}

impl DimensionPortBinding {
    pub(super) const fn new(port: DimName, target: NamePath) -> Self {
        Self { port, target }
    }

    /// The bound dimension port of the template.
    #[must_use]
    pub const fn port(&self) -> &DimName {
        &self.port
    }

    /// The importer-side dimension the port is bound to, as written.
    #[must_use]
    pub const fn target(&self) -> &NamePath {
        &self.target
    }
}

impl ModuleSymbols {
    /// Build a module symbol table, failing on the first declaration that
    /// cannot be recorded.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateSymbol`] when two definitions
    /// occupy the same slot of the module's collision unit, or
    /// [`ModuleResolveError::DuplicateIndexVariant`] when one index declares a
    /// variant twice.
    #[cfg(test)]
    pub(super) fn from_declarations(
        owner: DagId,
        declarations: &[ast::Declaration],
    ) -> Result<Self, ModuleResolveError> {
        let (symbols, errors) = Self::collect(owner, declarations);
        errors.into_iter().next().map_or(Ok(symbols), Err)
    }

    /// Build a module symbol table from a declaration list, skipping each
    /// declaration that cannot be recorded (a duplicate) and returning its
    /// error; every other declaration is still collected.
    ///
    /// The `owner` is the canonical DAG/module identity assigned by the loader.
    /// The declarations are not modified; this is a pure collection pass.
    pub(super) fn collect(
        owner: DagId,
        declarations: &[ast::Declaration],
    ) -> (Self, Vec<ModuleResolveError>) {
        let mut symbols = Self {
            owner,
            decls: HashMap::new(),
            dimensions: HashMap::new(),
            units: HashMap::new(),
            struct_types: HashMap::new(),
            indexes: HashMap::new(),
            constructors: HashMap::new(),
            dimension_projections: HashMap::new(),
            finite_index_projections: HashMap::new(),
        };
        let errors = declarations
            .iter()
            .filter_map(|decl| symbols.collect_declaration(&decl.kind).err())
            .collect();
        (symbols, errors)
    }

    /// The canonical owner for this table.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Dimension namespace symbols.
    #[must_use]
    pub(crate) const fn dimensions(&self) -> &HashMap<DimName, Symbol<DimNameNamespace>> {
        &self.dimensions
    }

    /// Unit namespace symbols.
    #[must_use]
    pub(crate) const fn units(
        &self,
    ) -> &HashMap<UnitName, Symbol<UnitNameNamespace, UnitConstness>> {
        &self.units
    }

    /// Struct/tagged-union type namespace symbols.
    #[must_use]
    pub(crate) const fn struct_types(
        &self,
    ) -> &HashMap<StructTypeName, Symbol<StructTypeNameNamespace, Vec<GenericParamSignature>>> {
        &self.struct_types
    }

    /// Index namespace symbols, each with the variants its index declares.
    #[must_use]
    pub(crate) const fn indexes(
        &self,
    ) -> &HashMap<IndexName, Symbol<IndexNameNamespace, HashMap<IndexVariantName, Span>>> {
        &self.indexes
    }

    /// The include projection a dimension symbol of this module denotes, when
    /// it is a projected specialization rather than a source declaration.
    #[must_use]
    pub(crate) fn dimension_projection(&self, name: &DimName) -> Option<&DimensionProjection> {
        self.dimension_projections.get(name)
    }

    /// The structural index an index symbol of this module denotes, when it
    /// is an include projection of a port bound to `Fin(N)`.
    #[must_use]
    pub(crate) fn finite_index_projection(&self, name: &IndexName) -> Option<FiniteIndex> {
        self.finite_index_projections.get(name).copied()
    }

    /// The local declaration occupying `(namespace, atom)`, if any.
    pub(super) fn occupant(&self, namespace: Namespace, atom: &NameAtom) -> Option<Occupant> {
        match namespace {
            Namespace::Term => {
                occupant_in(&self.decls, atom).or_else(|| occupant_in(&self.constructors, atom))
            }
            Namespace::Static => occupant_in(&self.dimensions, atom)
                .or_else(|| occupant_in(&self.struct_types, atom))
                .or_else(|| occupant_in(&self.indexes, atom)),
            Namespace::Unit => occupant_in(&self.units, atom),
        }
    }

    /// Whether this module declares a `dag` named `atom`.
    pub(super) fn declares_dag(&self, atom: &NameAtom) -> bool {
        self.decls
            .get(&NameDef::classify(atom.clone()))
            .is_some_and(|symbol| *symbol.data() == DeclSymbolKind::Dag)
    }

    fn collect_declaration(&mut self, kind: &ast::DeclKind) -> Result<(), ModuleResolveError> {
        let decl = |symbols: &mut Self, name, visibility, kind| {
            symbols.declare(|symbols| &mut symbols.decls, name, visibility, kind)
        };
        match kind {
            ast::DeclKind::Param(p) => decl(
                self,
                &p.name,
                BindableVisibility::PublicBind,
                DeclSymbolKind::Param,
            ),
            ast::DeclKind::Node(n) => {
                decl(self, &n.name, n.visibility.into(), DeclSymbolKind::Node)
            }
            ast::DeclKind::ConstNode(c) => {
                decl(self, &c.name, c.visibility.into(), DeclSymbolKind::Const)
            }
            ast::DeclKind::Assert(a) => {
                decl(self, &a.name, a.visibility.into(), DeclSymbolKind::Assert)
            }
            ast::DeclKind::Plot(p) => {
                decl(self, &p.name, p.visibility.into(), DeclSymbolKind::Plot)
            }
            ast::DeclKind::Figure(f) => {
                decl(self, &f.name, f.visibility.into(), DeclSymbolKind::Figure)
            }
            ast::DeclKind::Layer(l) => {
                decl(self, &l.name, l.visibility.into(), DeclSymbolKind::Layer)
            }
            ast::DeclKind::Dag(d) => decl(self, &d.name, d.visibility.into(), DeclSymbolKind::Dag),
            ast::DeclKind::BaseDimension(d) => self.declare(
                |symbols| &mut symbols.dimensions,
                &d.name,
                d.visibility.into(),
                (),
            ),
            ast::DeclKind::Dimension(d) => {
                self.declare(|symbols| &mut symbols.dimensions, &d.name, d.visibility, ())
            }
            ast::DeclKind::Unit(u) => self.declare(
                |symbols| &mut symbols.units,
                &u.name,
                u.visibility.into(),
                u.constness,
            ),
            ast::DeclKind::Type(t) => self.declare_type(t),
            ast::DeclKind::Index(i) => self.declare_index(i),
            // Import, include, and plugin aliases live in the module scope
            // (see `scope::declare_aliases`), not the symbol table.
            ast::DeclKind::Import(_)
            | ast::DeclKind::PluginImport(_)
            | ast::DeclKind::Include(_) => Ok(()),
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            ast::DeclKind::Sugar(s) => never(*s),
        }
    }

    fn declare_type(&mut self, type_decl: &ast::TypeDecl) -> Result<(), ModuleResolveError> {
        let generic_params = type_decl
            .generic_params
            .iter()
            .map(GenericParamSignature::from_param)
            .collect::<Vec<_>>();
        self.declare(
            |symbols| &mut symbols.struct_types,
            &type_decl.name,
            type_decl.visibility,
            generic_params.clone(),
        )?;
        if let ast::TypeDeclBody::Constructors(members) = &type_decl.body {
            for member in members {
                self.declare(
                    |symbols| &mut symbols.constructors,
                    &member.name,
                    type_decl.visibility,
                    ConstructorSignature {
                        owner_type: type_decl.name.value.clone(),
                        generic_params: generic_params.clone(),
                    },
                )?;
            }
        }
        Ok(())
    }

    fn declare_index(&mut self, index: &ast::IndexDecl) -> Result<(), ModuleResolveError> {
        let mut variants = HashMap::new();
        if let ast::IndexDeclKind::Named { variants: declared } = &index.kind {
            for variant in declared {
                if let Some(first) = variants.insert(variant.value.clone(), variant.span) {
                    return Err(ModuleResolveError::DuplicateIndexVariant {
                        owner: self.owner.clone(),
                        variant: variant.value.qualified_by(&index.name.value),
                        first,
                        duplicate: variant.span,
                    });
                }
            }
        }
        self.declare(
            |symbols| &mut symbols.indexes,
            &index.name,
            index.visibility,
            variants,
        )
    }

    /// Claim the name's slot and record the declaration in its table.
    fn declare<Ns: Namespaced, X>(
        &mut self,
        table: fn(&mut Self) -> &mut Table<Ns, X>,
        name: &Spanned<NameDef<Ns>>,
        visibility: BindableVisibility,
        data: X,
    ) -> Result<(), ModuleResolveError> {
        if let Some(first) = self.occupant(Ns::NAMESPACE, name.value.atom()) {
            return Err(ModuleResolveError::DuplicateSymbol {
                owner: self.owner.clone(),
                namespace: Ns::NAMESPACE,
                name: name.value.atom().clone(),
                first: first.span,
                duplicate: name.span,
            });
        }
        let symbol = Symbol::new(
            ResolvedName::from_def(self.owner.clone(), name.value.clone()),
            visibility,
            name.span,
            data,
        );
        table(self).insert(name.value.clone(), symbol);
        Ok(())
    }
}

/// The binding of `atom` in one symbol table, as a collision-unit occupant.
pub(super) fn occupant_in<Ns: Namespaced, X>(
    table: &Table<Ns, X>,
    atom: &NameAtom,
) -> Option<Occupant> {
    table
        .get(&NameDef::classify(atom.clone()))
        .map(|symbol| Occupant {
            span: symbol.span,
            visibility: symbol.visibility,
            surface: Some(Ns::SURFACE_KIND),
        })
}
