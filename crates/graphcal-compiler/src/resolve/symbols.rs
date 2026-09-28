//! Per-module declaration symbol tables collected from a desugared AST.

use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::resolved_name::ResolvedName;
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
}

/// One symbol table: leaf name to binding.
pub(super) type Table<Ns, X = ()> = HashMap<NameDef<Ns>, Symbol<Ns, X>>;

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

/// The identity and generic signature of a constructor's owning type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConstructorSignature {
    pub(super) owner_type: StructTypeName,
    pub(super) generic_params: Vec<GenericParamSignature>,
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
}

impl ModuleSymbols {
    /// Build a module symbol table from a declaration list.
    ///
    /// The `owner` is the canonical DAG/module identity assigned by the loader.
    /// The declarations are not modified; this is a pure collection pass.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateSymbol`] when two definitions
    /// occupy the same slot of the module's collision unit, or one index
    /// declares a variant twice.
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
        for decl in declarations {
            symbols.collect_declaration(&decl.kind)?;
        }
        Ok(symbols)
    }

    /// The canonical owner for this table.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// Value/declaration namespace symbols.
    #[must_use]
    pub(crate) const fn decls(
        &self,
    ) -> &HashMap<DeclName, Symbol<DeclNameNamespace, DeclSymbolKind>> {
        &self.decls
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
                namespace: Ns::NAMESPACE.label(),
                name: name.value.to_string(),
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
