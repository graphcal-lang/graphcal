//! Namespaces the resolver checks names in: collision slots and lookup universes.

use std::collections::HashMap;

use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::index_name::IndexNameNamespace;
use crate::syntax::names::{NameAtom, NameNamespace};
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorNameNamespace, StructTypeNameNamespace};

use super::category::SurfaceNameKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum FlatNamespace {
    Static,
    Term,
}

impl std::fmt::Display for FlatNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Static => "Static",
            Self::Term => "Term",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ExclusiveNameKind {
    Value,
    Dimension,
    StructType,
    Index,
    Constructor,
}

impl ExclusiveNameKind {
    pub(super) const fn namespace(self) -> FlatNamespace {
        match self {
            Self::Value | Self::Constructor => FlatNamespace::Term,
            Self::Dimension | Self::StructType | Self::Index => FlatNamespace::Static,
        }
    }
}

/// Span of the first binding that occupies each exclusive name slot.
pub(super) type ExclusiveNameOccupancy = HashMap<(FlatNamespace, NameAtom), Span>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LookupNamespace {
    Static,
    Term,
    Unit,
}

pub(super) trait ResolvableNamespace: NameNamespace {
    const SURFACE_KIND: SurfaceNameKind;
    const LOOKUP_NAMESPACE: LookupNamespace;
}

impl ResolvableNamespace for DeclNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Value;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Term;
}

impl ResolvableNamespace for DimNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Dimension;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Static;
}

impl ResolvableNamespace for UnitNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Unit;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Unit;
}

impl ResolvableNamespace for StructTypeNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Type;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Static;
}

impl ResolvableNamespace for IndexNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Index;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Static;
}

impl ResolvableNamespace for ConstructorNameNamespace {
    const SURFACE_KIND: SurfaceNameKind = SurfaceNameKind::Constructor;
    const LOOKUP_NAMESPACE: LookupNamespace = LookupNamespace::Term;
}
