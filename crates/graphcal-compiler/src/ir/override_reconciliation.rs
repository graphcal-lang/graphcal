//! Include-override reconciliation obligations of unrebound param defaults.
//!
//! Include assembly knows which bindable type/index declarations an include
//! replaced, with both sides already canonical. Canonical expression ownership
//! is only available after HIR/type resolution, so these records carry the
//! canonical overrides across that phase boundary.

use std::sync::Arc;

use miette::NamedSource;

use crate::dag_id::DagId;
use crate::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use crate::registry::checked_type::IndexTypeRef;
use crate::resolved_name::{ResolvedDeclName, ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::span::Span;
use crate::syntax::type_name::StructTypeName;

/// One include whose unrebound param default must remain independent of the
/// bindable nominal declarations replaced by that include.
#[derive(Debug, Clone)]
pub struct OverrideReconciliation {
    pub(crate) source_decl: ResolvedDeclName,
    pub(crate) targets: Vec<OverrideTarget>,
    pub(crate) src: NamedSource<Arc<String>>,
    pub(crate) include_span: Span,
}

impl OverrideReconciliation {
    /// The obligation of `orphan_decl` (declared in `source_owner`) under the
    /// index and type overrides of one include's canonical `substitution`.
    #[must_use]
    pub(crate) fn new(
        orphan_decl: DeclName,
        source_owner: &DagId,
        substitution: &StaticSubstitution,
        src: NamedSource<Arc<String>>,
        include_span: Span,
    ) -> Self {
        let targets =
            substitution
                .indexes
                .iter()
                .map(|(source, replacement)| OverrideTarget::Index {
                    overridden: source.to_unowned_def_name(),
                    source: source.clone(),
                    replacement: match replacement {
                        InstanceIndexBindingTarget::Declared(target) => {
                            IndexTypeRef::from_resolved(target.clone())
                        }
                        InstanceIndexBindingTarget::Finite(index) => {
                            IndexTypeRef::from_finite_index(*index)
                        }
                    },
                })
                .chain(substitution.types.iter().map(|(source, replacement)| {
                    OverrideTarget::Type {
                        overridden: source.to_unowned_def_name(),
                        source: source.clone(),
                        replacement: replacement.clone(),
                    }
                }))
                .collect();
        Self {
            source_decl: ResolvedDeclName::from_def(source_owner.clone(), orphan_decl),
            targets,
            src,
            include_span,
        }
    }

    /// The unrebound param that must be re-bound at the include site.
    #[must_use]
    pub(crate) fn orphan_decl(&self) -> DeclName {
        self.source_decl.to_unowned_def_name()
    }
}

/// A canonical nominal override that an unrebound param default must not use.
#[derive(Debug, Clone)]
pub enum OverrideTarget {
    Index {
        overridden: IndexName,
        source: ResolvedIndexName,
        replacement: IndexTypeRef,
    },
    Type {
        overridden: StructTypeName,
        source: ResolvedStructTypeName,
        replacement: ResolvedStructTypeName,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::index::FiniteIndex;
    use crate::resolved_name::ResolvedDimName;
    use crate::syntax::dimension::DimName;

    #[test]
    fn obligations_cover_index_and_type_overrides_only() {
        let template = DagId::root_in_package("test", "lib");
        let importer = DagId::root_in_package("test", "main");
        let axis = ResolvedIndexName::from_def(template.clone(), IndexName::expect_valid("Axis"));
        let slot = ResolvedStructTypeName::from_def(
            template.clone(),
            StructTypeName::expect_valid("Slot"),
        );
        let cell = ResolvedStructTypeName::from_def(
            importer.clone(),
            StructTypeName::expect_valid("Cell"),
        );
        let mut substitution = StaticSubstitution::default();
        substitution.indexes.insert(
            axis.clone(),
            InstanceIndexBindingTarget::Finite(FiniteIndex::try_from_u64(3).unwrap()),
        );
        substitution.types.insert(slot.clone(), cell.clone());
        substitution.dimensions.insert(
            ResolvedDimName::from_def(template.clone(), DimName::expect_valid("Q")),
            ResolvedDimName::from_def(importer, DimName::expect_valid("Length")),
        );

        let reconciliation = OverrideReconciliation::new(
            DeclName::expect_valid("fallback"),
            &template,
            &substitution,
            NamedSource::new("main.gcl", Arc::new(String::new())),
            Span::new(0, 0),
        );

        assert_eq!(reconciliation.targets.len(), 2);
        assert!(reconciliation.targets.iter().any(|target| matches!(
            target,
            OverrideTarget::Index { overridden, source, .. }
                if source == &axis && overridden.as_str() == "Axis"
        )));
        assert!(reconciliation.targets.iter().any(|target| matches!(
            target,
            OverrideTarget::Type { source, replacement, .. }
                if source == &slot && replacement == &cell
        )));
        assert_eq!(
            reconciliation.source_decl,
            ResolvedDeclName::from_def(template, DeclName::expect_valid("fallback"))
        );
        assert_eq!(reconciliation.orphan_decl().as_str(), "fallback");
    }
}
