//! Typed relationships between reusable DAG templates and concrete instances.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dag_id::{DagId, InstanceId};
use crate::registry::index::FiniteIndex;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::dimension::{ResolvedDimName, UnitName};
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::type_name::ResolvedStructTypeName;

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
    /// Applicative Static specialization shared independently of runtime values.
    pub specialization: StaticSpecializationId,
    /// DAG or enclosing concrete instance that owns this instantiation site.
    pub parent_owner: DagId,
    /// Value and type-system substitutions applied at the instance boundary.
    pub bindings: InstanceBindingEnvironment,
}

/// One instance value exposed through the including DAG's source interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceValueProjection {
    /// Template declaration materialized by the instance.
    pub target: ResolvedDeclName,
    /// Source-visible name introduced in the including DAG.
    pub exposed_name: ScopedName,
}

/// One instance assertion exposed through the including DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceAssertionProjection {
    pub target: ResolvedDeclName,
    pub exposed_name: ScopedName,
    /// Include-site override resolved in the including DAG's lexical context.
    pub expected_fail: Option<crate::ir::resolve::ExpectedFail>,
}

/// One plot requested from an instance include site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlotProjection {
    pub target: ResolvedDeclName,
    pub exposed_name: ScopedName,
    pub hidden: bool,
}

/// One semantic include edge after importer-context value expressions are lowered.
#[derive(Debug, Clone)]
pub struct HirInstanceRecord {
    /// Concrete instance identity and typed Static substitution.
    pub instance: InstanceRecord,
    /// Display-only scope for private instance implementation values.
    pub debug_scope: ModuleAliasName,
    /// Explicit value-port bindings lowered in the importer's lexical context.
    pub value_bindings: HashMap<ResolvedDeclName, crate::hir::CheckedExpr>,
    /// Runtime-unit definitions materialized under this instance owner.
    pub runtime_unit_names: HashSet<UnitName>,
    /// Runtime values intentionally exposed by this include site.
    pub output_projections: Vec<InstanceValueProjection>,
    /// Assertions intentionally exposed by this include site.
    pub assertion_projections: Vec<InstanceAssertionProjection>,
    /// Plot declarations explicitly requested by this include site.
    pub plot_projections: Vec<InstancePlotProjection>,
    /// Ancestor template owners rebased by enclosing semantic instances.
    pub owner_rebases: HashMap<DagId, DagId>,
    /// V005 obligations retained only for unrebound parameter defaults.
    pub(crate) override_reconciliations: HashMap<
        ResolvedDeclName,
        Vec<crate::ir::override_reconciliation::PendingOverrideReconciliation>,
    >,
}
