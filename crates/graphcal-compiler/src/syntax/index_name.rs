//! Index and index-variant names.

use crate::syntax::names::{NameDef, NameNamespace, NamePath, ResolvedName};

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

/// Module-resolved index name.
pub type ResolvedIndexName = ResolvedName<IndexNameNamespace>;

/// Name of an index variant (e.g., `"Departure"`, `"Correction"`).
pub type IndexVariantName = NameDef<IndexVariantNameNamespace>;

/// Name of an index variable declared by an extern signature's `<I: Index>`
/// binder (parallel to [`crate::syntax::dimension::DimVarName`] for `<D: Dim>`).
pub type IndexVarName = NameDef<IndexVarNameNamespace>;

impl From<IndexName> for NamePath {
    fn from(name: IndexName) -> Self {
        Self::local(name.into_atom())
    }
}

impl IndexVariantName {
    /// Build the variant name for the `n`-th step of a range index
    /// (`#0`, `#1`, …). Centralises the `"#"`-prefix format so registry,
    /// parser, and evaluator can't disagree on it.
    #[must_use]
    pub fn range_step(n: impl std::fmt::Display) -> Self {
        Self::expect_valid(format!("#{n}"))
    }

    /// Pair this variant with its index name for qualified rendering.
    #[must_use]
    pub fn qualified_by(&self, index: &IndexName) -> QualifiedIndexVariantName {
        QualifiedIndexVariantName::new(index.clone(), self.clone())
    }
}

/// A fully qualified index variant name, rendered as `Index.Variant`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedIndexVariantName {
    index: IndexName,
    variant: IndexVariantName,
}

impl QualifiedIndexVariantName {
    /// Create a qualified index variant name from its index and variant parts.
    #[must_use]
    pub const fn new(index: IndexName, variant: IndexVariantName) -> Self {
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
        write!(f, "{}.{}", self.index, self.variant)
    }
}

/// A fully resolved index variant reference.
///
/// Index variants are owned by an index declaration rather than directly by a
/// DAG/module. This type therefore resolves the index itself to a canonical
/// owner, then stores the variant as a leaf in that index's variant set.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResolvedIndexVariant {
    index: ResolvedIndexName,
    variant: IndexVariantName,
}

impl ResolvedIndexVariant {
    /// Create a resolved index-variant reference from its resolved index and
    /// variant leaf.
    #[must_use]
    pub const fn new(index: ResolvedIndexName, variant: IndexVariantName) -> Self {
        Self { index, variant }
    }

    /// The resolved index that owns this variant.
    #[must_use]
    pub const fn index(&self) -> &ResolvedIndexName {
        &self.index
    }

    /// The variant leaf inside [`Self::index`].
    #[must_use]
    pub const fn variant(&self) -> &IndexVariantName {
        &self.variant
    }

    /// Consume this value and return its typed parts.
    #[must_use]
    pub fn into_parts(self) -> (ResolvedIndexName, IndexVariantName) {
        (self.index, self.variant)
    }
}

impl std::fmt::Debug for ResolvedIndexVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedIndexVariant")
            .field("index", &self.index)
            .field("variant", &self.variant)
            .finish()
    }
}

impl std::fmt::Display for ResolvedIndexVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.index, self.variant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_index_variant_carries_resolved_index_owner() {
        let index = ResolvedIndexName::from_def(
            crate::dag_id::DagId::root_in_package("test", "mission"),
            IndexName::expect_valid("Phase"),
        );
        let variant = ResolvedIndexVariant::new(index, IndexVariantName::expect_valid("Burn"));

        assert_eq!(variant.index().owner().to_string(), "mission");
        assert_eq!(variant.index().as_str(), "Phase");
        assert_eq!(variant.variant().as_str(), "Burn");
        assert_eq!(variant.to_string(), "mission.Phase.Burn");
    }
}
