//! Canonical Static substitution of one reusable DAG template.
//!
//! Pure data shared by include elaboration, semantic instance records, and
//! TIR specialization.

use std::collections::BTreeMap;

use crate::dag_id::DagId;
use crate::resolved_name::{ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName};
use crate::semantic::index_def::FiniteIndex;

/// Canonical importer-side target of one instance index binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InstanceIndexBindingTarget {
    /// A declared index owned by the importing DAG.
    Declared(ResolvedIndexName),
    /// A structural finite index supplied directly at the instance boundary.
    Finite(FiniteIndex),
}

/// Canonical applicative substitution for one reusable DAG template.
///
/// Ordered maps make equality and hashing independent of include-site spelling
/// and binding order. Runtime value bindings are deliberately absent: they do
/// not change static specialization identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StaticSubstitution {
    pub indexes: BTreeMap<ResolvedIndexName, InstanceIndexBindingTarget>,
    pub types: BTreeMap<ResolvedStructTypeName, ResolvedStructTypeName>,
    pub dimensions: BTreeMap<ResolvedDimName, ResolvedDimName>,
}

/// Applicative identity shared by instances with equal Static bindings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StaticSpecializationId {
    pub template: DagId,
    pub substitution: StaticSubstitution,
}

impl StaticSpecializationId {
    #[must_use]
    pub const fn new(template: DagId, substitution: StaticSubstitution) -> Self {
        Self {
            template,
            substitution,
        }
    }
}
