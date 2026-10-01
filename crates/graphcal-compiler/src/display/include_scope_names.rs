//! Readable names for the private scopes of anonymous (selective) include
//! instances.
//!
//! A selective include introduces no module alias, so its instance scope is an
//! opaque [`IncludeInstanceId`] that source can never spell. The project names
//! each such scope once; every output boundary renders names through it.

use std::collections::HashMap;

use crate::syntax::module_name::{IncludeInstanceId, ModuleAliasName, ScopeSegment, ScopedName};
use crate::syntax::non_empty::NonEmpty;

/// Readable names for the private scopes of anonymous (selective) include
/// instances, which source can never spell.
pub type IncludeScopeNames = HashMap<IncludeInstanceId, ModuleAliasName>;

/// `name` with its leading private include scope replaced by the readable
/// name `include_scopes` gives it, if any.
#[must_use]
pub fn name_include_scopes(name: &ScopedName, include_scopes: &IncludeScopeNames) -> ScopedName {
    let Some(owner) = name.owner() else {
        return name.clone();
    };
    let ScopeSegment::IncludeInstance(first) = owner.first() else {
        return name.clone();
    };
    let Some(display) = include_scopes.get(first) else {
        return name.clone();
    };
    ScopedName::qualified(
        NonEmpty::new(
            ScopeSegment::Named(display.clone()),
            owner.as_slice()[1..].to_vec(),
        ),
        name.leaf().clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::decl_name::DeclName;

    fn leaf(name: &str) -> DeclName {
        DeclName::expect_valid(name)
    }

    #[test]
    fn private_include_scopes_take_their_readable_name() {
        let private = IncludeInstanceId::at_source_offset(7);
        let names = IncludeScopeNames::from([(private, ModuleAliasName::expect_valid("lib"))]);
        let inner = ScopeSegment::Named(ModuleAliasName::expect_valid("inner"));
        let scoped = ScopedName::qualified(
            NonEmpty::new(ScopeSegment::IncludeInstance(private), vec![inner.clone()]),
            leaf("x"),
        );
        assert_eq!(
            name_include_scopes(&scoped, &names),
            ScopedName::qualified(
                NonEmpty::new(
                    ScopeSegment::Named(ModuleAliasName::expect_valid("lib")),
                    vec![inner]
                ),
                leaf("x")
            )
        );
        let unnamed = ScopedName::in_scope(
            ScopeSegment::IncludeInstance(IncludeInstanceId::at_source_offset(9)),
            leaf("x"),
        );
        assert_eq!(name_include_scopes(&unnamed, &names), unnamed);
        let local = ScopedName::local(leaf("x"));
        assert_eq!(name_include_scopes(&local, &names), local);
    }
}
