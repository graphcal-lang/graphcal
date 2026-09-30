//! Unit handles: unit references as a checked body spells them.

use crate::ir::instance::mint::FrameAccess;
use crate::resolved_name::ResolvedUnitName;
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;

/// A unit reference inside a checked body, relative to the DAG that runs the
/// body.
///
/// An instance materializes its own copy of each runtime unit of its
/// template, with its own scale, while the template body naming the unit is
/// shared. The handle keeps the unit as the defining template names it and
/// offers no public accessor for it: the unit whose scale applies is obtained
/// only through the frame of the DAG running the body (outside the compiler,
/// as a term of a [`ScopedUnitExpr`](crate::tir::typed::scoped_node::ScopedUnitExpr)).
///
/// The checker reads the facts every copy of the unit shares (its dimension
/// and the constness of its scale) from the definition through
/// `LocalUnit::static_definition`, which is visible only inside the
/// compiler.
///
/// IDE trees ([`Tolerant`](crate::hir::Tolerant)) are never run and name the
/// source definition directly
/// ([`ResolvedUnitRef`](super::ResolvedUnitRef)); refining one into a
/// complete tree is where handles are made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUnit {
    spelling: SyntaxUnitRef,
    definition: ResolvedUnitName,
}

impl LocalUnit {
    /// The handle for the unit reference `spelling`, which resolved to
    /// `definition` in its body's module.
    #[must_use]
    pub(crate) const fn new(spelling: SyntaxUnitRef, definition: ResolvedUnitName) -> Self {
        Self {
            spelling,
            definition,
        }
    }

    /// A handle naming an arbitrary definition, for tests outside the
    /// compiler.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub const fn for_test(spelling: SyntaxUnitRef, definition: ResolvedUnitName) -> Self {
        Self::new(spelling, definition)
    }

    /// The source spelling, including any module alias, for diagnostics and
    /// display labels.
    #[must_use]
    pub const fn spelling(&self) -> &SyntaxUnitRef {
        &self.spelling
    }

    /// The unit definition, for the facts every materialized copy of it
    /// shares: its dimension and the constness of its scale.
    #[must_use]
    pub(crate) const fn static_definition(&self) -> &ResolvedUnitName {
        &self.definition
    }

    /// The unit definition the defining template names, for an instance
    /// frame to resolve.
    #[must_use]
    pub(crate) const fn definition(&self, _: FrameAccess) -> &ResolvedUnitName {
        &self.definition
    }
}

/// Renders the source spelling.
impl std::fmt::Display for LocalUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.spelling.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_id::DagId;
    use crate::syntax::dimension::UnitName;

    #[test]
    fn handles_render_their_spelling_and_keep_the_definition_for_the_checker() {
        let definition = ResolvedUnitName::for_test(
            DagId::root_in_package("test", "lib"),
            UnitName::expect_valid("tick"),
        );
        let spelling = SyntaxUnitRef::local(UnitName::expect_valid("tick"));
        let handle = LocalUnit::new(spelling.clone(), definition.clone());
        assert_eq!(handle.spelling(), &spelling);
        assert_eq!(handle.to_string(), spelling.to_string());
        assert_eq!(handle.static_definition(), &definition);
    }
}
