//! Canonical identities of lexically scoped generic parameters.
//!
//! Generic parameters are not module-level symbols, so they are never
//! represented as `ResolvedName<GenericParam>`. Their identity is the owning
//! generic scope plus the parameter leaf name. HIR lowering mints these
//! identities; every later representation of a generic form (type-level Nat
//! polynomials, TIR type expressions, substitutions) is keyed by them, so two
//! same-spelled parameters of different owners can never be confused.

use crate::plugin_identity::ExternFnKey;
use crate::resolved_name::ResolvedStructTypeName;
use crate::syntax::type_name::GenericParamName;

/// The accepted number of generic arguments: every parameter up to the last
/// one without a default is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericArgArity {
    required: usize,
    max: usize,
}

impl GenericArgArity {
    /// Exactly `count` arguments.
    #[must_use]
    pub const fn exactly(count: usize) -> Self {
        Self {
            required: count,
            max: count,
        }
    }

    /// The arity of a declared generic parameter list, given whether each
    /// parameter, in order, has a default.
    #[must_use]
    pub fn of_defaults(has_default: impl IntoIterator<Item = bool>) -> Self {
        let (required, max) = has_default.into_iter().enumerate().fold(
            (0, 0),
            |(required, _), (index, defaulted)| {
                let position = index.saturating_add(1);
                (if defaulted { required } else { position }, position)
            },
        );
        Self { required, max }
    }

    /// Whether `got` arguments are accepted.
    #[must_use]
    pub const fn accepts(self, got: usize) -> bool {
        self.required <= got && got <= self.max
    }
}

impl std::fmt::Display for GenericArgArity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.required == self.max {
            write!(f, "{}", self.max)
        } else {
            write!(f, "{}..{}", self.required, self.max)
        }
    }
}

/// Canonical identity for a generic parameter in a lexical generic scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GenericParamId {
    owner: GenericParamOwner,
    pub name: GenericParamName,
}

impl GenericParamId {
    /// Create a generic parameter identity from its owner and leaf name.
    #[must_use]
    pub(crate) const fn new(owner: GenericParamOwner, name: GenericParamName) -> Self {
        Self { owner, name }
    }

    /// The lexical scope that owns this parameter.
    #[must_use]
    pub(crate) const fn owner(&self) -> &GenericParamOwner {
        &self.owner
    }
}

/// Renders the source-facing leaf spelling of the parameter.
impl std::fmt::Display for GenericParamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

/// The lexical scope that owns a generic parameter list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GenericParamOwner {
    /// Generic parameter on a user-defined `type` declaration.
    Type(ResolvedStructTypeName),
    /// Dimension or index binder of an extern plugin function signature.
    ExternFn(ExternFnKey),
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{GenericParamId, GenericParamOwner};
    use crate::dag_id::DagId;
    use crate::resolved_name::ResolvedName;
    use crate::syntax::type_name::{GenericParamName, StructTypeName};

    /// A generic parameter of a test-only `type T` owned by the root module.
    pub fn type_param(name: &str) -> GenericParamId {
        GenericParamId::new(
            GenericParamOwner::Type(ResolvedName::for_test(
                DagId::root_in_package("test", "main"),
                StructTypeName::expect_valid("T"),
            )),
            GenericParamName::expect_valid(name),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::type_param;
    use super::*;
    use crate::dag_id::DagId;
    use crate::resolved_name::ResolvedName;
    use crate::syntax::type_name::StructTypeName;

    #[test]
    fn same_spelled_parameters_of_different_owners_are_distinct() {
        let other = GenericParamId::new(
            GenericParamOwner::Type(ResolvedName::for_test(
                DagId::root_in_package("test", "main"),
                StructTypeName::expect_valid("U"),
            )),
            GenericParamName::expect_valid("N"),
        );
        assert_ne!(type_param("N"), other);
        assert_eq!(type_param("N").to_string(), other.to_string());
        assert_eq!(type_param("N").to_string(), "N");
    }
}
