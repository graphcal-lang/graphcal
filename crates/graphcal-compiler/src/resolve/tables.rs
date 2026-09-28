//! Per-namespace access to a module's declaration and selective-import tables.

use std::collections::HashMap;

use crate::syntax::ast::UnitConstness;
use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::index_name::{IndexNameNamespace, IndexVariantName};
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::category::DeclSymbolKind;
use super::namespace::Namespaced;
use super::scope::ModuleScope;
use super::symbols::{ConstructorSignature, GenericParamSignature, ModuleSymbols, Table};

/// A namespace with one declaration table and one selective-import table per
/// module.
///
/// Resolution is written once over this trait; each namespace only names its
/// two tables and the payload its declarations carry.
pub trait NamespaceTables: Namespaced {
    /// Payload a local declaration in this namespace carries.
    type Declared: Clone;

    /// The module's own declarations in this namespace.
    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared>;

    /// The module's selective imports in this namespace.
    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared>;
}

impl NamespaceTables for DeclNameNamespace {
    type Declared = DeclSymbolKind;

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.decls
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_decls
    }
}

impl NamespaceTables for ConstructorNameNamespace {
    type Declared = ConstructorSignature;

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.constructors
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_constructors
    }
}

impl NamespaceTables for DimNameNamespace {
    type Declared = ();

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.dimensions
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_dimensions
    }
}

impl NamespaceTables for StructTypeNameNamespace {
    type Declared = Vec<GenericParamSignature>;

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.struct_types
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_struct_types
    }
}

impl NamespaceTables for IndexNameNamespace {
    type Declared = HashMap<IndexVariantName, Span>;

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.indexes
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_indexes
    }
}

impl NamespaceTables for UnitNameNamespace {
    type Declared = UnitConstness;

    fn declared(symbols: &ModuleSymbols) -> &Table<Self, Self::Declared> {
        &symbols.units
    }

    fn selected(scope: &ModuleScope) -> &Table<Self, Self::Declared> {
        &scope.selected_units
    }
}
