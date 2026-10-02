//! The checked view of one Static substitution that specialization reads.
//!
//! A [`StaticSubstitution`] is the identity of a specialization (it is
//! ordered and hashed inside a
//! [`StaticSpecializationId`](crate::ir::static_substitution::StaticSpecializationId)),
//! so it names its dimension targets and nothing more. Specializing a
//! dimension needs each target's definition; [`CompleteSubstitution`] is the
//! view built once, at one fallible construction point, in which every
//! dimension target is resolved. It is never part of an identity.

use std::collections::BTreeMap;

use crate::dimension::Dimension;
use crate::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName};
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;

use super::ProjectTypeStore;

/// A Static substitution whose every dimension target has a definition.
#[derive(Debug, Clone)]
pub struct CompleteSubstitution<'s> {
    substitution: &'s StaticSubstitution,
    dimensions: BTreeMap<ResolvedDimName, Dimension>,
}

/// Why a Static substitution has no complete view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableDimensionTarget {
    /// The bound dimension port.
    pub port: ResolvedDimName,
    /// The target the project type store does not define.
    pub target: ResolvedDimName,
}

impl UnavailableDimensionTarget {
    /// Render the failure: every include binding names a dimension the
    /// project defines, so a missing target is a compiler bug.
    #[must_use]
    pub fn into_graphcal(self, src: SourceId) -> SemanticError {
        SemanticError::internal_error(
            format!(
                "semantic specialization dimension target `{}` of `{}` is unavailable",
                self.target, self.port
            ),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::WholeFile,
        )
    }
}

impl<'s> CompleteSubstitution<'s> {
    /// Resolve every dimension target of `substitution` in `types`.
    ///
    /// # Errors
    ///
    /// Returns the first bound port whose target `types` does not define.
    pub fn try_new(
        substitution: &'s StaticSubstitution,
        types: &ProjectTypeStore,
    ) -> Result<Self, UnavailableDimensionTarget> {
        let dimensions = substitution
            .dimensions
            .iter()
            .map(|(port, target)| {
                types
                    .get_dimension(target)
                    .map(|dimension| (port.clone(), dimension.clone()))
                    .ok_or_else(|| UnavailableDimensionTarget {
                        port: port.clone(),
                        target: target.clone(),
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            substitution,
            dimensions,
        })
    }

    /// The definition replacing the dimension port `port`, if bound.
    #[must_use]
    pub fn dimension(&self, port: &ResolvedDimName) -> Option<&Dimension> {
        self.dimensions.get(port)
    }

    /// The target replacing the index port `port`, if bound.
    #[must_use]
    pub fn index(&self, port: &ResolvedIndexName) -> Option<&'s InstanceIndexBindingTarget> {
        self.substitution.indexes.get(port)
    }

    /// The nominal type replacing the type port `port`, if bound.
    #[must_use]
    pub fn nominal(&self, port: &ResolvedStructTypeName) -> Option<&'s ResolvedStructTypeName> {
        self.substitution.types.get(port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::dimension::DimName;

    #[test]
    fn every_bound_dimension_target_is_resolved_or_reported() {
        let template = crate::dag_id::DagId::root_in_package("test", "lib");
        let port = ResolvedDimName::for_test(template.clone(), DimName::expect_valid("Q"));
        let unbound = ResolvedDimName::for_test(template, DimName::expect_valid("R"));
        let prelude = crate::ir::prelude_definitions::prelude_definitions().unwrap();
        let (target, mass) = prelude
            .dimensions()
            .find(|(identity, _)| identity.atom().as_str() == "Mass")
            .map(|(identity, dimension)| (identity.clone(), dimension.clone()))
            .unwrap();
        let mut substitution = StaticSubstitution::default();
        substitution.dimensions.insert(port.clone(), target.clone());

        assert_eq!(
            CompleteSubstitution::try_new(&substitution, &ProjectTypeStore::default()).unwrap_err(),
            UnavailableDimensionTarget {
                port: port.clone(),
                target,
            }
        );

        let mut types = ProjectTypeStore::default();
        types.insert_graphcal_prelude().unwrap();
        let complete = CompleteSubstitution::try_new(&substitution, &types).unwrap();
        assert_eq!(complete.dimension(&port), Some(&mass));
        assert_eq!(complete.dimension(&unbound), None);
    }
}
