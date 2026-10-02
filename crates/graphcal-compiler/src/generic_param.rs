//! Canonical identities of lexically scoped generic parameters.
//!
//! Generic parameters are not module-level symbols, so they are never
//! represented as `ResolvedName<GenericParam>`. Their identity is the owning
//! generic scope plus the parameter leaf name. HIR lowering mints these
//! identities; every later representation of a generic form (type-level Nat
//! polynomials, TIR type expressions, substitutions) is keyed by them, so two
//! same-spelled parameters of different owners can never be confused.

use crate::plugin_identity::ExternFnKey;
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use crate::syntax::ast::GenericConstraint;
use crate::syntax::type_name::GenericParamName;

/// Render accepted generic constraints as `A or B` at the diagnostic boundary.
pub(crate) fn render_accepted_constraints(accepted: &[GenericConstraint]) -> String {
    accepted
        .iter()
        .map(|constraint| constraint.as_str())
        .collect::<Vec<_>>()
        .join(" or ")
}

/// The generic type or constructor a generic argument list is applied to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericApplicationTarget {
    /// A user-declared generic struct type.
    StructType(ResolvedStructTypeName),
    /// A constructor of a user-declared generic type.
    Constructor(ResolvedConstructorName),
    /// The built-in `Complex<D>` type.
    Complex,
    /// The built-in `Key<I>` type.
    Key,
}

impl std::fmt::Display for GenericApplicationTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StructType(name) => f.write_str(name.as_str()),
            Self::Constructor(name) => f.write_str(name.as_str()),
            Self::Complex => f.write_str("Complex"),
            Self::Key => f.write_str("Key"),
        }
    }
}

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

    /// The parameters an application of `got` leading arguments leaves to
    /// their defaults, each paired with its default, in order.
    ///
    /// `params` pairs each declared parameter, in order, with its default.
    /// An application is accepted exactly when every parameter after its
    /// arguments has a default, so the defaults it needs always exist.
    ///
    /// # Errors
    ///
    /// Returns the arity of `params` when `got` arguments are not accepted.
    pub fn defaulted_tail<P, D>(
        params: impl IntoIterator<Item = (P, Option<D>)>,
        got: usize,
    ) -> Result<Vec<(P, D)>, Self> {
        let params = params.into_iter().collect::<Vec<_>>();
        let arity = Self::of_defaults(params.iter().map(|(_, default)| default.is_some()));
        if got > params.len() {
            return Err(arity);
        }
        params
            .into_iter()
            .skip(got)
            .map(|(param, default)| default.map(|default| (param, default)).ok_or(arity))
            .collect()
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

    #[test]
    fn an_accepted_application_leaves_only_defaulted_parameters() {
        let params = [("A", None), ("B", Some(2)), ("C", Some(3))];
        assert_eq!(
            GenericArgArity::defaulted_tail(params, 1),
            Ok(vec![("B", 2), ("C", 3)])
        );
        assert_eq!(GenericArgArity::defaulted_tail(params, 3), Ok(Vec::new()));
        let arity = GenericArgArity::of_defaults([false, true, true]);
        assert_eq!(GenericArgArity::defaulted_tail(params, 0), Err(arity));
        assert_eq!(GenericArgArity::defaulted_tail(params, 4), Err(arity));
        // A parameter without a default after a defaulted one is required.
        let interleaved = [("A", Some(1)), ("B", None)];
        assert_eq!(
            GenericArgArity::defaulted_tail(interleaved, 1),
            Err(GenericArgArity::exactly(2))
        );
        assert_eq!(
            GenericArgArity::defaulted_tail(interleaved, 2),
            Ok(Vec::new())
        );
    }
}
