//! The names an evaluation's output gives the declarations it reports.
//!
//! Evaluation tracks declarations by runtime identity. A reader of the
//! output knows them by the names the root's source gives them, so every
//! reason the output reports (a failed dependency, an unfinished formula)
//! names its declarations this way.

use graphcal_compiler::node_unavailable::{NodeUnavailable, ReportedName};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::module_name::ScopedName;

/// A declaration as an evaluation's output names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputDeclName {
    /// The name the root gives the declaration: its own declaration, a value
    /// an include site exposes, or a declaration of an instance or inline DAG
    /// below the root, qualified by their scopes (`inst::pending`).
    Root(ScopedName),
    /// A declaration of a module outside the root's subtree that the
    /// evaluation invoked. The root has no name for it, so it is reported by
    /// its identity.
    Invoked(ResolvedDeclName),
}

impl std::fmt::Display for OutputDeclName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Root(name) => name.fmt(formatter),
            Self::Invoked(identity) => identity.fmt(formatter),
        }
    }
}

impl ReportedName for OutputDeclName {}

/// Why a declaration the output reports has no value, naming the
/// declarations involved as the output names them.
pub type OutputUnavailable = NodeUnavailable<OutputDeclName>;

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::syntax::decl_name::DeclName;
    use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};

    use super::*;

    fn leaf(name: &str) -> DeclName {
        DeclName::expect_valid(name)
    }

    #[test]
    fn root_names_display_as_written() {
        let name = OutputDeclName::Root(ScopedName::in_scope(
            ScopeSegment::Named(ModuleAliasName::expect_valid("inst")),
            leaf("pending"),
        ));
        assert_eq!(name.to_string(), "inst::pending");
    }

    #[test]
    fn invoked_declarations_keep_their_identity() {
        let identity = ResolvedDeclName::for_test(DagId::root_in_package("test", "lib"), leaf("x"));
        let name = OutputDeclName::Invoked(identity.clone());
        assert_eq!(name.to_string(), identity.to_string());
    }
}
