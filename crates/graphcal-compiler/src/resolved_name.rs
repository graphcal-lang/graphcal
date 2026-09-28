//! Module-resolved names: a canonical [`DagId`] owner plus a namespaced leaf.
//!
//! A <code>[ResolvedName]&lt;Ns&gt;</code> is a reference that has passed
//! module-aware resolution. It lives beside [`DagId`] rather than with the
//! syntax-level name infrastructure in [`crate::syntax::names`]: source names
//! ([`NameDef`], [`NamePath`](crate::syntax::names::NamePath)) are upstream of
//! module identity, while a resolved name is defined by it.
//!
//! The per-namespace aliases ([`ResolvedDeclName`], [`ResolvedIndexName`], …)
//! pair the namespace markers owned by the syntax modules with this type.

use std::marker::PhantomData;

use crate::dag_id::DagId;
use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::dimension::{DimNameNamespace, UnitNameNamespace};
use crate::syntax::index_name::{IndexNameNamespace, IndexVariantName};
use crate::syntax::names::{NameAtom, NameDef, NameNamespace};
use crate::syntax::type_name::{
    ConstructorName, ConstructorNameNamespace, StructTypeNameNamespace,
};

/// A fully resolved reference in a semantic namespace.
///
/// Unlike [`NamePath`](crate::syntax::names::NamePath), this no longer stores
/// source qualifier text. The `owner` is the canonical DAG/module identity
/// chosen by module-aware resolution; `name` is the declaration leaf inside
/// that owner.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResolvedName<Ns: NameNamespace> {
    owner: DagId,
    name: NameAtom,
    _ns: PhantomData<Ns>,
}

impl<Ns: NameNamespace> std::fmt::Debug for ResolvedName<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedName")
            .field("namespace", &Ns::DISPLAY_NAME)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .finish()
    }
}

impl<Ns: NameNamespace> ResolvedName<Ns> {
    /// Construct a resolved name from its canonical owner and leaf atom.
    #[must_use]
    pub(crate) const fn new(owner: DagId, name: NameAtom) -> Self {
        Self {
            owner,
            name,
            _ns: PhantomData,
        }
    }

    /// Resolve an existing definition-site name into a canonical owner.
    #[must_use]
    pub(crate) fn from_def(owner: DagId, name: NameDef<Ns>) -> Self {
        Self::new(owner, name.into_atom())
    }

    /// Construct an identity outside module resolution, for tests that
    /// name a declaration directly (including one that does not exist).
    ///
    /// Production code outside the compiler obtains identities from the
    /// module resolver only.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub fn for_test(owner: DagId, name: NameDef<Ns>) -> Self {
        Self::from_def(owner, name)
    }

    /// The same owner with another leaf in the same namespace, e.g. the
    /// identity a declaration would take if it were renamed.
    #[must_use]
    pub fn with_leaf(&self, name: NameDef<Ns>) -> Self {
        Self::new(self.owner.clone(), name.into_atom())
    }

    /// The canonical DAG/module that owns this name.
    #[must_use]
    pub const fn owner(&self) -> &DagId {
        &self.owner
    }

    /// The leaf atom inside [`Self::owner`].
    #[must_use]
    pub const fn atom(&self) -> &NameAtom {
        &self.name
    }

    /// The leaf string inside [`Self::owner`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.name.as_str()
    }

    /// Return the unowned definition-site leaf in the same namespace.
    ///
    /// This deliberately drops the canonical owner. Use it only at explicit
    /// standalone registry, diagnostic, or serialization boundaries that
    /// cannot yet carry [`ResolvedName`] itself.
    #[must_use]
    pub fn to_unowned_def_name(&self) -> NameDef<Ns> {
        NameDef::classify(self.name.clone())
    }

    /// Consume this value and return the canonical owner plus leaf atom.
    #[must_use]
    pub fn into_parts(self) -> (DagId, NameAtom) {
        (self.owner, self.name)
    }
}

impl<Ns: NameNamespace> std::fmt::Display for ResolvedName<Ns> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.owner, self.name)
    }
}

/// Module-resolved declaration name.
pub type ResolvedDeclName = ResolvedName<DeclNameNamespace>;

/// Module-resolved dimension name.
pub type ResolvedDimName = ResolvedName<DimNameNamespace>;

/// Module-resolved unit name.
pub type ResolvedUnitName = ResolvedName<UnitNameNamespace>;

/// Module-resolved struct/tagged-union type name.
pub type ResolvedStructTypeName = ResolvedName<StructTypeNameNamespace>;

/// Module-resolved tagged-union constructor name.
pub type ResolvedConstructorName = ResolvedName<ConstructorNameNamespace>;

/// Module-resolved index name.
pub type ResolvedIndexName = ResolvedName<IndexNameNamespace>;

impl ResolvedDeclName {
    /// The `dag` declaration that names an inline DAG module in its parent,
    /// or `None` for a file root or a concrete instance.
    #[must_use]
    pub fn naming_inline_dag(dag: &DagId) -> Option<Self> {
        let name = dag.leaf().inline_dag()?.clone();
        Some(Self::from_def(dag.parent()?, name))
    }
}

impl ResolvedStructTypeName {
    /// The constructor `member` of this type, which is declared beside the
    /// type in the same module.
    #[must_use]
    pub fn constructor(&self, member: ConstructorName) -> ResolvedConstructorName {
        ResolvedName::new(self.owner.clone(), member.into_atom())
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
    pub(crate) const fn new(index: ResolvedIndexName, variant: IndexVariantName) -> Self {
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
    pub(crate) fn into_parts(self) -> (ResolvedIndexName, IndexVariantName) {
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
        write!(f, "{}#{}", self.index, self.variant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::index_name::IndexName;
    use crate::syntax::non_empty::NonEmpty;

    #[test]
    fn resolved_name_carries_canonical_owner_and_leaf() {
        let resolved = ResolvedDeclName::from_def(
            DagId::new("test", NonEmpty::new("helpers", vec!["mass"])),
            DeclName::expect_valid("dry_mass"),
        );

        assert_eq!(resolved.owner().to_string(), "helpers.mass");
        assert_eq!(resolved.as_str(), "dry_mass");
        assert_eq!(resolved.to_string(), "helpers.mass.dry_mass");
        assert_eq!(
            resolved.to_unowned_def_name(),
            DeclName::expect_valid("dry_mass")
        );
    }

    #[test]
    fn resolved_index_variant_carries_resolved_index_owner() {
        let index = ResolvedIndexName::from_def(
            DagId::root_in_package("test", "mission"),
            IndexName::expect_valid("Phase"),
        );
        let variant = ResolvedIndexVariant::new(index, IndexVariantName::expect_valid("Burn"));

        assert_eq!(variant.index().owner().to_string(), "mission");
        assert_eq!(variant.index().as_str(), "Phase");
        assert_eq!(variant.variant().as_str(), "Burn");
        assert_eq!(variant.to_string(), "mission.Phase#Burn");
    }

    #[test]
    fn derived_identities_keep_the_owner() {
        let owner = DagId::root_in_package("test", "mission");
        let node = ResolvedDeclName::from_def(owner.clone(), DeclName::expect_valid("mass"));
        let renamed = node.with_leaf(DeclName::expect_valid("dry_mass"));
        assert_eq!(renamed.owner(), &owner);
        assert_eq!(renamed.as_str(), "dry_mass");

        let choice = ResolvedStructTypeName::from_def(
            owner.clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Choice"),
        );
        let pick = choice.constructor(ConstructorName::expect_valid("Pick"));
        assert_eq!(pick.owner(), &owner);
        assert_eq!(pick.as_str(), "Pick");
    }

    #[test]
    fn inline_dags_are_named_by_a_declaration_of_their_parent() {
        let file = DagId::root_in_package("test", "mission");
        let inline = file.inline_dag_child(DeclName::expect_valid("stage"));
        assert_eq!(
            ResolvedDeclName::naming_inline_dag(&inline),
            Some(ResolvedDeclName::from_def(
                file.clone(),
                DeclName::expect_valid("stage")
            ))
        );
        assert_eq!(ResolvedDeclName::naming_inline_dag(&file), None);
        let instance = file.instance_child(crate::syntax::module_name::ScopeSegment::Named(
            crate::syntax::module_name::ModuleAliasName::expect_valid("stage"),
        ));
        assert_eq!(ResolvedDeclName::naming_inline_dag(&instance), None);
    }
}
