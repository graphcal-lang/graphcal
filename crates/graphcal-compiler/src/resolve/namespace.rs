//! The unit of collision checking and name lookup.

use crate::syntax::ast::BindableVisibility;
use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::index_name::IndexNameNamespace;
use crate::syntax::names::NameNamespace;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::category::{SurfaceNameKind, SymbolTable};

/// The unit of collision checking and name lookup.
///
/// Every name a module can see occupies exactly one slot `(Namespace, leaf)`.
/// Two names in one slot are a duplicate, whether they come from local
/// declarations, `import` / `include` / plugin aliases, or selective imports.
/// The finer symbol tables (declarations and constructors in [`Self::Term`];
/// dimensions, types, and indexes in [`Self::Static`]) only partition a slot
/// for lookup; they are never separate collision units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// Values and declarations, constructors, and module / plugin aliases.
    Term,
    /// Dimensions, struct / tagged-union types, and indexes.
    Static,
    /// Units.
    Unit,
}

impl std::fmt::Display for Namespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Term => "Term",
            Self::Static => "Static",
            Self::Unit => "Unit",
        })
    }
}

impl Namespace {
    /// The namespace a selective-import category introduces names into: the
    /// type, dimension, and index categories share Static.
    #[must_use]
    pub const fn of(category: ImportItemNamespace) -> Self {
        match category {
            ImportItemNamespace::Term => Self::Term,
            ImportItemNamespace::Unit => Self::Unit,
            ImportItemNamespace::Type
            | ImportItemNamespace::Dimension
            | ImportItemNamespace::Index => Self::Static,
        }
    }
}

/// A symbol-table namespace marker together with the slot its names occupy
/// and the category diagnostics report for them.
pub trait Namespaced: NameNamespace {
    /// The collision / lookup unit of names in this namespace.
    const NAMESPACE: Namespace;
    /// The category reported when a name is found in the wrong universe.
    const SURFACE_KIND: SurfaceNameKind;
    /// The symbol table holding names of this namespace.
    const TABLE: SymbolTable;
}

impl Namespaced for DeclNameNamespace {
    const NAMESPACE: Namespace = Namespace::Term;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Value;
    const TABLE: SymbolTable = SymbolTable::Decl;
}

impl Namespaced for ConstructorNameNamespace {
    const NAMESPACE: Namespace = Namespace::Term;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Constructor;
    const TABLE: SymbolTable = SymbolTable::Constructor;
}

impl Namespaced for DimNameNamespace {
    const NAMESPACE: Namespace = Namespace::Static;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Dimension;
    const TABLE: SymbolTable = SymbolTable::Dimension;
}

impl Namespaced for StructTypeNameNamespace {
    const NAMESPACE: Namespace = Namespace::Static;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Type;
    const TABLE: SymbolTable = SymbolTable::StructType;
}

impl Namespaced for IndexNameNamespace {
    const NAMESPACE: Namespace = Namespace::Static;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Index;
    const TABLE: SymbolTable = SymbolTable::Index;
}

impl Namespaced for UnitNameNamespace {
    const NAMESPACE: Namespace = Namespace::Unit;
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Unit;
    const TABLE: SymbolTable = SymbolTable::Unit;
}

/// The binding that occupies one slot of a module's collision unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Occupant {
    /// Source span of the occupying definition or import.
    pub(super) span: Span,
    /// Visibility of the occupying binding across module boundaries.
    pub(super) visibility: BindableVisibility,
    /// Category reported by wrong-universe diagnostics; module and plugin
    /// aliases have none.
    pub(super) surface: Option<SurfaceNameKind>,
}
