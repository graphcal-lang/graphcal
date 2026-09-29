//! Typed relationships between reusable DAG templates and concrete instances.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dag_id::{DagId, InstanceId};
use crate::registry::index::FiniteIndex;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::UnitName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};

/// The declaration `name` of the template that `instance` instantiates.
#[must_use]
pub fn template_declaration(instance: &InstanceId, name: DeclName) -> ResolvedDeclName {
    ResolvedDeclName::from_def(instance.template().clone(), name)
}

/// The concrete copy that `instance` materializes of its template's
/// declaration `name`.
#[must_use]
pub fn instance_declaration(instance: &InstanceId, name: DeclName) -> ResolvedDeclName {
    ResolvedDeclName::from_def(instance.owner().clone(), name)
}

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

/// One edge in the explicit module-template/instance graph.
///
/// The fields are private so the record keeps its invariants by construction:
/// the specialization's template is the instance's template, and every
/// concrete value port is owned by the current concrete instance owner, so
/// re-parenting cannot leave a stale port or a stale template behind. The
/// instance's parent ([`InstanceId::parent`]) is the DAG whose instance list
/// holds this record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRecord {
    /// Concrete owner paired with the canonical template instantiated here.
    id: InstanceId,
    /// Applicative Static specialization shared independently of runtime values.
    specialization: StaticSpecializationId,
    /// Leaves of the template value declarations materialized by this instance.
    value_ports: HashSet<DeclName>,
}

impl InstanceRecord {
    /// Record the instance `id` of its template, specialized by
    /// `substitution`, materializing the template value declarations
    /// `value_ports`.
    #[must_use]
    pub fn new(
        id: InstanceId,
        substitution: StaticSubstitution,
        value_ports: impl IntoIterator<Item = DeclName>,
    ) -> Self {
        let specialization = StaticSpecializationId::new(id.template().clone(), substitution);
        Self {
            id,
            specialization,
            value_ports: value_ports.into_iter().collect(),
        }
    }

    /// Concrete instance identity.
    #[must_use]
    pub const fn id(&self) -> &InstanceId {
        &self.id
    }

    /// Applicative Static specialization of the template.
    #[must_use]
    pub const fn specialization(&self) -> &StaticSpecializationId {
        &self.specialization
    }

    /// Canonical Static substitution applied at the instance boundary.
    #[must_use]
    pub const fn substitution(&self) -> &StaticSubstitution {
        &self.specialization.substitution
    }

    /// The concrete declaration materializing the template value port
    /// `template_port`, if it is one of this instance's ports.
    #[must_use]
    pub fn value_port(&self, template_port: &ResolvedDeclName) -> Option<ResolvedDeclName> {
        let leaf = template_port.to_unowned_def_name();
        (template_port.owner() == self.id.template() && self.value_ports.contains(&leaf))
            .then(|| instance_declaration(&self.id, leaf))
    }

    /// Every concrete declaration materializing one of the template's value ports.
    pub fn concrete_value_ports(&self) -> impl Iterator<Item = ResolvedDeclName> + '_ {
        self.value_ports
            .iter()
            .map(|leaf| instance_declaration(&self.id, leaf.clone()))
    }

    /// Re-parent this instance under `parent`, the concrete owner of the
    /// enclosing specialized template, and compose its Static substitution
    /// with the enclosing one through `compose`.
    pub(crate) fn rebase(&mut self, parent: DagId, compose: impl FnOnce(&mut StaticSubstitution)) {
        self.id = InstanceId::new(parent, self.id.scope().clone(), self.id.template().clone());
        compose(&mut self.specialization.substitution);
    }
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
    pub expected_fail: Option<crate::assertion_expectation::ExpectedFail>,
}

/// One plot requested from an instance include site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlotProjection {
    pub target: ResolvedDeclName,
    pub exposed_name: ScopedName,
    pub visibility: crate::plot_visibility::PlotVisibility,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::module_name::ScopeSegment;

    #[test]
    fn template_and_instance_declarations_share_the_leaf() {
        let parent = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let instance = InstanceId::new(
            parent,
            ScopeSegment::Named(ModuleAliasName::expect_valid("inst")),
            template.clone(),
        );
        let name = DeclName::expect_valid("value");

        let source = template_declaration(&instance, name.clone());
        let copy = instance_declaration(&instance, name);
        assert_eq!(source.owner(), &template);
        assert_eq!(copy.owner(), instance.owner());
        assert_eq!(source.as_str(), copy.as_str());
    }

    fn named(alias: &str) -> ScopeSegment {
        ScopeSegment::Named(ModuleAliasName::expect_valid(alias))
    }

    fn dimension(owner: &DagId, name: &str) -> ResolvedDimName {
        ResolvedDimName::from_def(
            owner.clone(),
            crate::syntax::dimension::DimName::expect_valid(name),
        )
    }

    #[test]
    fn record_keeps_the_instance_template_as_its_specialization_template() {
        let parent = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let id = InstanceId::new(parent, named("inst"), template.clone());
        let record = InstanceRecord::new(id.clone(), StaticSubstitution::default(), []);

        assert_eq!(record.id(), &id);
        assert_eq!(record.specialization().template, template);
        assert_eq!(record.substitution(), &StaticSubstitution::default());
    }

    #[test]
    fn value_ports_map_template_declarations_to_the_current_owner() {
        let parent = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let other = DagId::root_in_package("test", "other");
        let id = InstanceId::new(parent, named("inst"), template);
        let port = DeclName::expect_valid("factor");
        let record = InstanceRecord::new(id.clone(), StaticSubstitution::default(), [port.clone()]);

        let template_port = template_declaration(&id, port.clone());
        assert_eq!(
            record.value_port(&template_port),
            Some(instance_declaration(&id, port.clone()))
        );
        assert_eq!(
            record.value_port(&template_declaration(
                &id,
                DeclName::expect_valid("missing")
            )),
            None
        );
        assert_eq!(
            record.value_port(&ResolvedDeclName::from_def(other, port.clone())),
            None
        );
        assert_eq!(
            record.concrete_value_ports().collect::<Vec<_>>(),
            vec![instance_declaration(&id, port)]
        );
    }

    #[test]
    fn rebase_moves_identity_and_ports_and_composes_the_substitution() {
        let template_owner = DagId::root_in_package("test", "outer");
        let template = DagId::root_in_package("test", "lib");
        let concrete_parent = DagId::root_in_package("test", "main").instance_child(named("outer"));
        let port = DeclName::expect_valid("factor");
        let source = dimension(&template, "Measure");
        let substitution = StaticSubstitution {
            dimensions: BTreeMap::from([(source.clone(), dimension(&template_owner, "Local"))]),
            ..StaticSubstitution::default()
        };
        let mut record = InstanceRecord::new(
            InstanceId::new(template_owner, named("inner"), template.clone()),
            substitution,
            [port.clone()],
        );

        let composed = dimension(&concrete_parent, "Bound");
        record.rebase(concrete_parent.clone(), |substitution| {
            for target in substitution.dimensions.values_mut() {
                *target = composed.clone();
            }
        });

        let expected = InstanceId::new(concrete_parent, named("inner"), template.clone());
        assert_eq!(record.id(), &expected);
        assert_eq!(record.specialization().template, template);
        assert_eq!(record.substitution().dimensions[&source], composed);
        assert_eq!(
            record.value_port(&template_declaration(&expected, port.clone())),
            Some(instance_declaration(&expected, port))
        );
    }
}
