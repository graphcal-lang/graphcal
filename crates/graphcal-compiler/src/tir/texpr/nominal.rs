//! The nominal facts checking establishes for a typed tree: the constructor
//! a node applies, the constructor a match arm selects, and the nominal uses a
//! checked root makes.

use std::sync::Arc;

use crate::hir::nominal::ResolvedConstructor;
use crate::registry::applied_constructor::AppliedConstructor;
use crate::registry::checked_type::{
    CheckedGenericArg, Concrete, Concreteness, IndexTypeRef, Symbolic,
};
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use crate::syntax::index_name::IndexVariantName;
use crate::syntax::type_name::{ConstructorName, FieldName};

/// A checked constructor application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorApplication<V: Concreteness = Concrete> {
    /// The applied constructor and its owning definition. The definition's
    /// field annotations are the application's field constraints.
    pub constructor: ResolvedConstructor,
    /// The application at its runtime type, with each field's instantiated
    /// type; shared by every value the node builds.
    pub applied: Arc<AppliedConstructor<V>>,
}

impl<V: Concreteness> ConstructorApplication<V> {
    /// Definition identity used by field contracts, distinct from runtime owner.
    #[must_use]
    pub fn definition(&self) -> &ResolvedStructTypeName {
        self.constructor.owning_type()
    }

    /// The generic arguments of the application.
    #[must_use]
    pub fn generic_args(&self) -> &[CheckedGenericArg<V>] {
        self.applied.generic_args()
    }
}

/// The constructor a match arm selects, in the matching body's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorMatch {
    pub definition: ResolvedStructTypeName,
    pub runtime_type: ResolvedStructTypeName,
    pub constructor: ConstructorName,
}

/// A nominal use a checked root makes, as its definition names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NominalObservation {
    Field {
        identity: ResolvedStructTypeName,
        field: FieldName,
    },
    Constructor {
        identity: ResolvedStructTypeName,
        constructor: ResolvedConstructorName,
    },
    TypeArgument(ResolvedStructTypeName),
    IndexLabel {
        identity: IndexTypeRef<Symbolic>,
        variant: IndexVariantName,
    },
    IndexArgument(IndexTypeRef<Symbolic>),
}
