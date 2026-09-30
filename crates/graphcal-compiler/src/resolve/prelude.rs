//! Implicit prelude type-system symbols visible without an import.
//!
//! The module resolver resolves source module aliases only. The Graphcal
//! prelude is different: its dimensions and units are implicitly in scope in
//! every module but still need a canonical owner once we cross into HIR. This
//! small typed scope declares those identities, so a prelude name, like every
//! other name, obtains its identity from the resolver.

use std::collections::HashSet;
use std::sync::LazyLock;

use crate::dag_id::DagId;
use crate::resolved_name::{ResolvedDimName, ResolvedName, ResolvedUnitName};
use crate::syntax::dimension::{DimName, UnitName, UnitRef};
use crate::syntax::names::{NameDef, NamePath};

use crate::semantic::prelude::{prelude_dag_id, prelude_dimension_names, prelude_unit_names};

use super::mint::ResolverMint;

/// The built-in Graphcal prelude type scope, built once per process.
#[must_use]
pub fn prelude_type_scope() -> &'static PreludeTypeScope {
    static GRAPHCAL: LazyLock<PreludeTypeScope> = LazyLock::new(|| {
        PreludeTypeScope::new(
            prelude_dag_id(),
            prelude_dimension_names().map(DimName::expect_valid),
            prelude_unit_names().map(UnitName::expect_valid),
        )
    });
    &GRAPHCAL
}

/// The dimensions and units a synthetic prelude module declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreludeTypeScope {
    owner: DagId,
    dimensions: HashSet<DimName>,
    units: HashSet<UnitName>,
}

impl PreludeTypeScope {
    /// Create a prelude type scope from its canonical owner, dimensions, and units.
    #[must_use]
    pub(crate) fn new(
        owner: DagId,
        dimensions: impl IntoIterator<Item = DimName>,
        units: impl IntoIterator<Item = UnitName>,
    ) -> Self {
        Self {
            owner,
            dimensions: dimensions.into_iter().collect(),
            units: units.into_iter().collect(),
        }
    }

    /// The canonical identity of the prelude dimension `name`, if the
    /// prelude declares it.
    #[must_use]
    pub fn dimension(&self, name: &DimName) -> Option<ResolvedDimName> {
        self.dimensions
            .contains(name)
            .then(|| ResolvedName::from_def(ResolverMint(()), self.owner.clone(), name.clone()))
    }

    /// The canonical identity of the prelude unit `name`, if the prelude
    /// declares it.
    #[must_use]
    pub fn unit(&self, name: &UnitName) -> Option<ResolvedUnitName> {
        self.units
            .contains(name)
            .then(|| ResolvedName::from_def(ResolverMint(()), self.owner.clone(), name.clone()))
    }

    /// Resolve an unqualified dimension path against the prelude.
    #[must_use]
    pub fn resolve_dimension_path(&self, path: &NamePath) -> Option<ResolvedDimName> {
        self.dimension(&NameDef::classify(path.as_bare()?.clone()))
    }

    /// Resolve an unqualified unit reference against the prelude.
    pub(crate) fn resolve_unit_ref(&self, reference: &UnitRef) -> Option<ResolvedUnitName> {
        if reference.is_qualified() {
            return None;
        }
        self.unit(reference.leaf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> PreludeTypeScope {
        PreludeTypeScope::new(
            DagId::root_in_package("test", "prelude"),
            [DimName::expect_valid("Length")],
            [UnitName::expect_valid("m")],
        )
    }

    #[test]
    fn declared_names_resolve_into_the_prelude_owner() {
        let scope = scope();
        let length = scope
            .dimension(&DimName::expect_valid("Length"))
            .expect("declared dimension");
        assert_eq!(length.owner(), &DagId::root_in_package("test", "prelude"));
        assert_eq!(length.as_str(), "Length");
        let metre = scope
            .resolve_unit_ref(&UnitRef::local(UnitName::expect_valid("m")))
            .expect("declared unit");
        assert_eq!(metre.owner(), length.owner());
        assert_eq!(
            scope.resolve_dimension_path(&NamePath::local(length.atom().clone())),
            Some(length)
        );
    }

    #[test]
    fn undeclared_or_qualified_names_do_not_resolve() {
        let scope = scope();
        assert_eq!(scope.dimension(&DimName::expect_valid("Mass")), None);
        assert_eq!(scope.unit(&UnitName::expect_valid("s")), None);
        assert_eq!(
            scope.resolve_unit_ref(&UnitRef::qualified(
                crate::syntax::non_empty::NonEmpty::singleton(
                    crate::syntax::names::NameAtom::parse("lib").unwrap()
                ),
                UnitName::expect_valid("m"),
            )),
            None
        );
    }
}
