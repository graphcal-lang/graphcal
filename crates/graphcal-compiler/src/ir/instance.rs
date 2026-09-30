//! Typed relationships between reusable DAG templates and concrete instances.

use std::collections::{HashMap, HashSet};

use crate::dag_id::{DagId, InstanceId};
use crate::hir::expr::LocalDecl;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::UnitName;
use crate::syntax::module_name::ScopedName;

use super::static_substitution::{StaticSpecializationId, StaticSubstitution};

pub mod frame;
pub mod identity;
pub(crate) mod mint;

pub use self::identity::{instance_declaration, template_declaration, template_reference};

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

    /// The source-level name under which the including DAG exposes
    /// `projection` of this instance: the name a selective item binds, or
    /// the template's name qualified by this instance's scope.
    #[must_use]
    pub fn exposed_name(&self, projection: &impl InstanceProjection) -> ScopedName {
        match projection.exposure() {
            ProjectionExposure::Selected(name) => ScopedName::local(name.clone()),
            ProjectionExposure::Member => {
                ScopedName::in_scope(self.id.scope().clone(), projection.target().leaf().clone())
            }
        }
    }

    /// Every concrete declaration materializing one of the template's value ports.
    pub fn concrete_value_ports(&self) -> impl Iterator<Item = ResolvedDeclName> + '_ {
        self.value_ports
            .iter()
            .map(|leaf| instance_declaration(&self.id, leaf.clone()))
    }

    /// The frame the instance runs its template's bodies in, inside the DAG
    /// that runs in `parent`: the only way to build an instance frame.
    ///
    /// `template_edges` are the instances the template itself includes, as
    /// the template records them; `runtime_units` are the runtime units this
    /// instance materializes. Only the instance materialization in
    /// [`crate::tir::typed`] holds the [`InstanceFrameMint`], so no other code
    /// can build a frame for an instance.
    ///
    /// [`InstanceFrameMint`]: crate::tir::typed::frame_mint::InstanceFrameMint
    #[must_use]
    pub(crate) fn frame<'a>(
        &self,
        _: crate::tir::typed::frame_mint::InstanceFrameMint,
        parent: &frame::InstanceFrame,
        template_edges: impl IntoIterator<Item = &'a InstanceId>,
        runtime_units: impl IntoIterator<Item = UnitName>,
    ) -> frame::InstanceFrame {
        frame::InstanceFrame::instance(
            parent,
            &self.id,
            &self.specialization,
            template_edges,
            runtime_units,
        )
    }

    /// Re-parent this instance under `parent`, the concrete owner of the
    /// enclosing specialized template, and compose its Static substitution
    /// with the enclosing one through `compose`.
    pub(crate) fn rebase(&mut self, parent: DagId, compose: impl FnOnce(&mut StaticSubstitution)) {
        self.id = InstanceId::new(parent, self.id.scope().clone(), self.id.template().clone());
        compose(&mut self.specialization.substitution);
    }
}

/// Where an include site exposes one projected value or assertion in the
/// including DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionExposure {
    /// A selective include item (`include lib(..)::{x as y}`): the including
    /// DAG binds the projection directly under this name.
    Selected(DeclName),
    /// A member of a whole-module include (`include lib(..) as l`), reached
    /// through the include's instance scope under the template's own name
    /// (`l::x`).
    Member,
}

impl ProjectionExposure {
    /// The name a selective include item binds, or `None` for a member
    /// reached through the include's instance scope.
    #[must_use]
    pub const fn selected(&self) -> Option<&DeclName> {
        match self {
            Self::Selected(name) => Some(name),
            Self::Member => None,
        }
    }
}

/// An include-site projection of a value or an assertion: a template
/// declaration and where the including DAG exposes it.
pub trait InstanceProjection {
    /// Template declaration materialized by the instance, as the template
    /// names it; the instance's frame resolves it.
    fn target(&self) -> &LocalDecl;

    /// Where the including DAG exposes the projection.
    fn exposure(&self) -> &ProjectionExposure;
}

/// One instance value exposed through the including DAG's source interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceValueProjection {
    /// Template declaration materialized by the instance, as the template
    /// names it; the instance's frame resolves it.
    pub target: LocalDecl,
    /// Where the including DAG exposes the value.
    pub exposure: ProjectionExposure,
}

/// One instance assertion exposed through the including DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceAssertionProjection {
    /// Template assertion, as the template names it.
    pub target: LocalDecl,
    /// Where the including DAG exposes the assertion.
    pub exposure: ProjectionExposure,
    /// Include-site override resolved in the including DAG's lexical context.
    pub expected_fail: Option<crate::assertion_expectation::ExpectedFail>,
}

impl InstanceProjection for InstanceValueProjection {
    fn target(&self) -> &LocalDecl {
        &self.target
    }

    fn exposure(&self) -> &ProjectionExposure {
        &self.exposure
    }
}

impl InstanceProjection for InstanceAssertionProjection {
    fn target(&self) -> &LocalDecl {
        &self.target
    }

    fn exposure(&self) -> &ProjectionExposure {
        &self.exposure
    }
}

/// One plot requested from an instance include site.
///
/// Only a selective include item requests a plot, so the plot is always
/// bound directly in the including DAG, under `alias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlotProjection {
    /// Template plot (or plot the template forwards from one of its own
    /// instances), as the template names it.
    pub target: LocalDecl,
    /// The name the including DAG binds the plot under.
    pub alias: DeclName,
    pub visibility: crate::plot_visibility::PlotVisibility,
}

/// One semantic include edge after importer-context value expressions are lowered.
#[derive(Debug, Clone)]
pub struct HirInstanceRecord {
    /// Concrete instance identity and typed Static substitution.
    pub instance: InstanceRecord,
    /// Explicit value-port bindings lowered in the importer's lexical context.
    pub value_bindings: HashMap<ResolvedDeclName, crate::hir::CheckedExpr>,
    /// Runtime-unit definitions materialized under this instance owner.
    pub runtime_unit_names: HashSet<UnitName>,
    /// Runtime values intentionally exposed by this include site.
    ///
    /// The projections' targets are handles for the instance's frame; outside
    /// the compiler they are read already resolved through
    /// [`CheckedInstance`](crate::tir::typed::CheckedInstance).
    pub(crate) output_projections: Vec<InstanceValueProjection>,
    /// Assertions intentionally exposed by this include site.
    pub(crate) assertion_projections: Vec<InstanceAssertionProjection>,
    /// Plot declarations explicitly requested by this include site.
    pub(crate) plot_projections: Vec<InstancePlotProjection>,
    /// V005 obligations retained only for unrebound parameter defaults.
    pub(crate) override_reconciliations:
        HashMap<ResolvedDeclName, Vec<crate::ir::override_reconciliation::OverrideReconciliation>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolved_name::ResolvedDimName;
    use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};
    use std::collections::BTreeMap;

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
        ResolvedDimName::for_test(
            owner.clone(),
            crate::syntax::dimension::DimName::expect_valid(name),
        )
    }

    #[test]
    fn projections_are_exposed_under_the_selected_name_or_the_instance_scope() {
        let parent = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let id = InstanceId::new(parent, named("inst"), template);
        let record = InstanceRecord::new(id.clone(), StaticSubstitution::default(), []);
        let target = template_reference(&id, DeclName::expect_valid("output"));

        let member = InstanceValueProjection {
            target: target.clone(),
            exposure: ProjectionExposure::Member,
        };
        assert_eq!(member.exposure.selected(), None);
        assert_eq!(
            record.exposed_name(&member),
            ScopedName::in_scope(named("inst"), DeclName::expect_valid("output"))
        );

        let selected = InstanceAssertionProjection {
            target,
            exposure: ProjectionExposure::Selected(DeclName::expect_valid("renamed")),
            expected_fail: None,
        };
        assert_eq!(
            selected.exposure.selected(),
            Some(&DeclName::expect_valid("renamed"))
        );
        assert_eq!(
            record.exposed_name(&selected),
            ScopedName::local(DeclName::expect_valid("renamed"))
        );
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
            record.value_port(&ResolvedDeclName::for_test(other, port.clone())),
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
