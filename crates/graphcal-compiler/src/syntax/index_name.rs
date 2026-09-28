//! Index and index-variant names.

use crate::syntax::names::{NameDef, NameNamespace, NamePath};

/// Index type namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexNameNamespace {}

impl NameNamespace for IndexNameNamespace {
    const DISPLAY_NAME: &'static str = "IndexName";
}

/// Index variant namespace marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexVariantNameNamespace {}

impl NameNamespace for IndexVariantNameNamespace {
    const DISPLAY_NAME: &'static str = "IndexVariantName";
}

/// Index variable namespace marker (extern signature `<I: Index>` binders).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexVarNameNamespace {}

impl NameNamespace for IndexVarNameNamespace {
    const DISPLAY_NAME: &'static str = "IndexVarName";
}

/// Name of an index type (e.g., `"Maneuver"`).
pub type IndexName = NameDef<IndexNameNamespace>;

/// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
pub type IndexVariantName = NameDef<IndexVariantNameNamespace>;

/// Typed key for one element of an indexed collection.
///
/// Named axes use declared variant names. Coordinate and `Fin(N)` axes use a
/// numeric position; `#N` is only its source/diagnostic rendering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexEntryKey {
    Named(IndexVariantName),
    Position(u64),
}

impl IndexEntryKey {
    #[must_use]
    pub const fn named(name: IndexVariantName) -> Self {
        Self::Named(name)
    }

    #[must_use]
    pub const fn position(position: u64) -> Self {
        Self::Position(position)
    }

    #[must_use]
    pub const fn as_named(&self) -> Option<&IndexVariantName> {
        match self {
            Self::Named(name) => Some(name),
            Self::Position(_) => None,
        }
    }
}

impl From<IndexVariantName> for IndexEntryKey {
    fn from(value: IndexVariantName) -> Self {
        Self::Named(value)
    }
}

impl std::fmt::Display for IndexEntryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(name) => name.fmt(f),
            Self::Position(position) => write!(f, "#{position}"),
        }
    }
}

/// Name of an index variable declared by an extern signature's `<I: Index>`
/// binder (parallel to [`crate::syntax::dimension::DimVarName`] for `<D: Dim>`).
pub type IndexVarName = NameDef<IndexVarNameNamespace>;

impl From<IndexName> for NamePath {
    fn from(name: IndexName) -> Self {
        Self::local(name.into_atom())
    }
}

impl IndexVariantName {
    /// Pair this variant with its index name for qualified rendering.
    #[must_use]
    pub fn qualified_by(&self, index: &IndexName) -> QualifiedIndexVariantName {
        QualifiedIndexVariantName::new(index.clone(), self.clone())
    }
}

/// A fully qualified index variant name, rendered as `Index#Variant`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedIndexVariantName {
    index: IndexName,
    variant: IndexVariantName,
}

impl QualifiedIndexVariantName {
    /// Create a qualified index variant name from its index and variant parts.
    #[must_use]
    const fn new(index: IndexName, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    /// The index/type part of the qualified variant.
    #[must_use]
    pub const fn index(&self) -> &IndexName {
        &self.index
    }

    /// The variant/constructor part of the qualified variant.
    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }
}

impl std::fmt::Display for QualifiedIndexVariantName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.index, self.variant)
    }
}
