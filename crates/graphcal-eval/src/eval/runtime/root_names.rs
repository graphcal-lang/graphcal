//! Source-level names of the runtime identities of the root DAG's closure,
//! as the root reports them.
//!
//! A declaration the root exposes (its own, or a projection of an include
//! site) has the name the root's source gives it. Any other declaration of
//! the closure belongs to a semantic instance and is named by the instance
//! scopes below the root, then its own leaf (`l2::reciprocal`,
//! `outer::inner::x`); anonymous include scopes are given display names at
//! the project boundary, which also supplies them to [`RootNames`] so the
//! reasons an evaluation reports name declarations the same way.

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::ir::instance::ExposedValueBody;
use graphcal_compiler::node_unavailable::NodeUnavailable;
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::non_empty::NonEmpty;

use graphcal_compiler::display::include_scope_names::{IncludeScopeNames, name_include_scopes};

use crate::eval::output_decl_name::{OutputDeclName, OutputUnavailable};
use crate::execution_plan::ExecPlan;

/// `name`, written in `dag`, as the root `root` names it: qualified by the
/// scopes of `dag` below the root.
///
/// Returns `None` when `dag` is neither the root nor a module below it.
pub(super) fn qualified_below(root: &DagId, dag: &DagId, name: &ScopedName) -> Option<ScopedName> {
    let path = dag.scopes_below(root)?;
    let qualifier = path
        .into_iter()
        .chain(name.qualifier().iter().cloned())
        .collect::<Vec<_>>();
    Some(ScopedName::from_parts(
        NonEmpty::try_from_vec(qualifier).ok(),
        name.leaf().clone(),
    ))
}

/// The name of `declaration`, a declaration of a semantic instance in the
/// root's closure, qualified by the instance scopes below the root.
///
/// # Errors
///
/// Returns an internal error when the declaration's owner is not below the
/// root: every declaration the root evaluates is its own or an instance's.
pub(super) fn instance_member_name(
    root: &DagId,
    declaration: &ResolvedDeclName,
    src: SourceId,
) -> Result<ScopedName, SemanticError> {
    member_name(root, declaration).ok_or_else(|| {
        SemanticError::internal_error(
            format!("declaration `{declaration}` is not a member of an instance below the root"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
}

fn member_name(root: &DagId, declaration: &ResolvedDeclName) -> Option<ScopedName> {
    let owner = declaration.owner();
    (owner != root)
        .then(|| qualified_below(root, owner, &ScopedName::local(declaration.leaf().clone())))
        .flatten()
}

/// Source-level names of the runtime declarations the root DAG exposes, in
/// deterministic order: root declarations in source order, then the output
/// and assertion projections of each root semantic instance in record order.
///
/// Declarations private to a semantic instance have no root source name and
/// are absent; [`instance_member_name`] names them.
pub(super) fn root_source_names(plan: &ExecPlan<'_>) -> Vec<(ResolvedDeclName, ScopedName)> {
    let root = plan.root();
    let own = root.scope().dag().declarations().map(|entry| {
        (
            entry.identity().clone(),
            ScopedName::local(entry.name().clone()),
        )
    });
    let projected = root.semantic_instances().iter().flat_map(|planned| {
        let instance = planned.instance();
        let record = &instance.record().instance;
        instance
            .output_projections()
            .map(|resolved| (resolved.target, record.exposed_name(resolved.projection)))
            .chain(
                instance
                    .assertion_projections()
                    .map(|resolved| (resolved.target, record.exposed_name(resolved.projection))),
            )
    });
    own.chain(projected).collect()
}

/// The names the root gives the declarations an evaluation reports, for
/// renaming the reasons it reports from runtime identities.
pub(super) struct RootNames<'p> {
    root: &'p DagId,
    /// Values an include site exposes under a name of the root's own that
    /// no root declaration materializes.
    exposed: HashMap<ResolvedDeclName, ScopedName>,
    include_scopes: &'p IncludeScopeNames,
}

impl<'p> RootNames<'p> {
    /// The names the root of `plan` gives its declarations, with private
    /// include scopes named by `include_scopes`.
    pub(super) fn new(plan: &'p ExecPlan<'_>, include_scopes: &'p IncludeScopeNames) -> Self {
        let exposed = plan
            .root()
            .semantic_instances()
            .iter()
            .flat_map(|planned| {
                let instance = planned.instance();
                let record = &instance.record().instance;
                instance
                    .output_projections()
                    // A value materialized as a local alias is named by that
                    // alias's own declaration, not by the instance's.
                    .filter(|resolved| resolved.projection.body() == ExposedValueBody::Instance)
                    .map(|resolved| (resolved.target, record.exposed_name(resolved.projection)))
            })
            .collect();
        Self {
            root: plan.tir().root_dag_id(),
            exposed,
            include_scopes,
        }
    }

    /// The output name of `declaration`: the name the root exposes it under,
    /// else its name qualified by the scopes below the root (none for the
    /// root's own declarations), else (a declaration of an invoked module
    /// outside the root's subtree) its identity.
    pub(super) fn name(&self, declaration: &ResolvedDeclName) -> OutputDeclName {
        self.exposed
            .get(declaration)
            .cloned()
            .or_else(|| {
                qualified_below(
                    self.root,
                    declaration.owner(),
                    &ScopedName::local(declaration.leaf().clone()),
                )
            })
            .map_or_else(
                || OutputDeclName::Invoked(declaration.clone()),
                |name| OutputDeclName::Root(name_include_scopes(&name, self.include_scopes)),
            )
    }

    /// `reason`, naming its declarations as the output does.
    pub(super) fn present(&self, reason: &NodeUnavailable) -> OutputUnavailable {
        reason.map_names(|declaration| self.name(declaration))
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::syntax::decl_name::DeclName;
    use graphcal_compiler::syntax::module_name::{ModuleAliasName, ScopeSegment};

    use super::*;

    fn named(alias: &str) -> ScopeSegment {
        ScopeSegment::Named(ModuleAliasName::expect_valid(alias))
    }

    fn leaf(name: &str) -> DeclName {
        DeclName::expect_valid(name)
    }

    #[test]
    fn instance_members_are_qualified_by_every_instance_scope_below_the_root() {
        let root = DagId::root_in_package("test", "main");
        let outer = root.instance_child(named("outer"));
        let inner = outer.instance_child(named("inner"));
        let member = |owner: &DagId| {
            member_name(&root, &ResolvedDeclName::for_test(owner.clone(), leaf("x")))
        };
        assert_eq!(
            member(&outer),
            Some(ScopedName::in_scope(named("outer"), leaf("x")))
        );
        assert_eq!(
            member(&inner),
            Some(ScopedName::qualified(
                NonEmpty::new(named("outer"), vec![named("inner")]),
                leaf("x")
            ))
        );
        // The root's own declarations and modules outside the root are not
        // instance members.
        assert_eq!(member(&root), None);
        assert_eq!(member(&DagId::root_in_package("test", "other")), None);
    }

    #[test]
    fn names_written_in_a_module_keep_their_own_qualifier() {
        let root = DagId::root_in_package("test", "main");
        let outer = root.instance_child(named("outer"));
        let exposed = ScopedName::in_scope(named("inst"), leaf("ok"));
        assert_eq!(
            qualified_below(&root, &outer, &exposed),
            Some(ScopedName::qualified(
                NonEmpty::new(named("outer"), vec![named("inst")]),
                leaf("ok")
            ))
        );
        assert_eq!(
            qualified_below(&root, &root, &exposed),
            Some(exposed.clone())
        );
        assert_eq!(
            qualified_below(&root, &DagId::root_in_package("test", "other"), &exposed),
            None
        );
    }
}
