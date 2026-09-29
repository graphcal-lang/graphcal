//! Identities of the declarations specialization materializes.
//!
//! Instantiating a DAG template copies its declarations into a concrete
//! instance owner, and an include site exposes instance declarations under
//! aliases in the including DAG. The resolver declares neither, so these
//! functions are the only constructors of such identities outside the
//! resolver.

use crate::dag_id::{DagId, InstanceId};
use crate::hir::expr::LocalDecl;
use crate::resolved_name::{ResolvedDeclName, ResolvedName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::{NameDef, NameNamespace};

use super::mint::SpecializationMint;

/// The declaration `name` of the template that `instance` instantiates.
#[must_use]
pub fn template_declaration<Ns: NameNamespace>(
    instance: &InstanceId,
    name: NameDef<Ns>,
) -> ResolvedName<Ns> {
    ResolvedName::specialized(SpecializationMint(()), instance.template().clone(), name)
}

/// A reference to the declaration `name` of the template that `instance`
/// instantiates, as the template names it: the instance's frame resolves it
/// to the instance's copy.
#[must_use]
pub fn template_reference(instance: &InstanceId, name: DeclName) -> LocalDecl {
    LocalDecl::new(template_declaration(instance, name))
}

/// The concrete copy that `instance` materializes of its template's
/// declaration `name`.
#[must_use]
pub fn instance_declaration<Ns: NameNamespace>(
    instance: &InstanceId,
    name: NameDef<Ns>,
) -> ResolvedName<Ns> {
    ResolvedName::specialized(SpecializationMint(()), instance.owner().clone(), name)
}

/// The concrete copy of `declaration` in `owner`, the concrete counterpart of
/// the template-side owner of `declaration` under an enclosing specialization.
#[must_use]
pub(crate) fn rebased_declaration<Ns: NameNamespace>(
    declaration: &ResolvedName<Ns>,
    owner: &DagId,
) -> ResolvedName<Ns> {
    ResolvedName::specialized(
        SpecializationMint(()),
        owner.clone(),
        declaration.to_unowned_def_name(),
    )
}

/// The declaration an include site introduces in the including DAG `parent`
/// to expose an instance value, assertion, or plot as `exposed`.
#[must_use]
pub(crate) fn projection_alias(parent: &DagId, exposed: DeclName) -> ResolvedDeclName {
    ResolvedName::specialized(SpecializationMint(()), parent.clone(), exposed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::dimension::UnitName;
    use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};

    fn instance() -> InstanceId {
        InstanceId::new(
            DagId::root_in_package("test", "main"),
            ScopeSegment::Named(ModuleAliasName::expect_valid("inst")),
            DagId::root_in_package("test", "lib"),
        )
    }

    #[test]
    fn instance_identities_keep_namespace_and_leaf() {
        let instance = instance();
        let unit = UnitName::expect_valid("tick");
        let template = template_declaration(&instance, unit.clone());
        let copy = instance_declaration(&instance, unit);
        assert_eq!(template.owner(), instance.template());
        assert_eq!(copy.owner(), instance.owner());
        assert_eq!(template.leaf(), copy.leaf());
    }

    #[test]
    fn rebasing_moves_only_the_owner() {
        let instance = instance();
        let template = template_declaration(&instance, DeclName::expect_valid("x"));
        let rebased = rebased_declaration(&template, instance.owner());
        assert_eq!(
            rebased,
            instance_declaration(&instance, DeclName::expect_valid("x"))
        );
    }

    #[test]
    fn projection_aliases_live_in_the_including_dag() {
        let parent = DagId::root_in_package("test", "main");
        let alias = projection_alias(&parent, DeclName::expect_valid("speed"));
        assert_eq!(alias.owner(), &parent);
        assert_eq!(alias.as_str(), "speed");
    }
}
