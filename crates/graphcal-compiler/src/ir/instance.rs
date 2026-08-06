//! Typed relationships between reusable DAG templates and concrete instances.

use std::collections::{HashMap, HashSet};

use crate::dag_id::{DagId, InstanceId};
use crate::registry::index::FiniteIndex;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::dimension::ResolvedDimName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::ResolvedStructTypeName;

/// Canonical importer-side target of one instance index binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceIndexBindingTarget {
    /// A declared index owned by the importing DAG.
    Declared(ResolvedIndexName),
    /// A structural finite index supplied directly at the instance boundary.
    Finite(FiniteIndex),
}

/// Typed substitution environment connecting one template to a concrete instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceBindingEnvironment {
    /// Every template value port mapped to its concrete declaration record.
    pub value_ports: HashMap<ResolvedDeclName, ResolvedDeclName>,
    /// Template value ports explicitly bound at the include/call site.
    pub explicitly_bound_values: HashSet<ResolvedDeclName>,
    /// Canonical index substitutions.
    pub indexes: HashMap<ResolvedIndexName, InstanceIndexBindingTarget>,
    /// Canonical nominal-type substitutions.
    pub types: HashMap<ResolvedStructTypeName, ResolvedStructTypeName>,
    /// Canonical dimension substitutions.
    pub dimensions: HashMap<ResolvedDimName, ResolvedDimName>,
}

/// One edge in the explicit module-template/instance graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRecord {
    /// Concrete owner paired with the canonical template instantiated here.
    pub id: InstanceId,
    /// DAG or enclosing concrete instance that owns this instantiation site.
    pub parent_owner: DagId,
    /// Value and type-system substitutions applied at the instance boundary.
    pub bindings: InstanceBindingEnvironment,
}
