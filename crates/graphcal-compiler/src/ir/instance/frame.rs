//! Instance frames: how the bodies a DAG runs name its declarations.
//!
//! A template body is shared (`Arc`) by the canonical template and by every
//! instance of it, so a declaration reference in the body
//! ([`LocalDecl`]) names the declaration the defining template sees. An
//! [`InstanceFrame`] belongs to the DAG that runs the body and maps those
//! references to that DAG's own declarations: a canonical DAG runs its bodies
//! as written, while an instance re-owns everything its template (and the
//! templates enclosing it) owns under the concrete instance owners.

use std::collections::{HashMap, HashSet};

use crate::dag_id::{DagId, InstanceId};
use crate::hir::expr::{LocalDecl, ResolvedUnitRef};
use crate::resolved_name::{
    ResolvedDeclName, ResolvedName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::syntax::dimension::UnitName;
use crate::syntax::names::NameNamespace;

use super::identity::{instance_declaration, rebased_declaration};
use super::mint::FrameAccess;
use crate::ir::static_substitution::StaticSpecializationId;
use crate::tir::typed::canonical_frame::CanonicalFrameMint;

/// The frame a DAG runs its bodies in: the only way to turn a body handle
/// into a declaration identity.
///
/// A frame is built exactly once per DAG, together with the DAG itself: the
/// canonical frame when a module or inline DAG is type-resolved, an instance
/// frame from the instance record when a semantic include edge is
/// materialized. It is never chosen by the code that runs a body; that code
/// reads the frame of the DAG it runs.
#[derive(Debug, Clone)]
pub struct InstanceFrame {
    kind: FrameKind,
}

#[derive(Debug, Clone)]
enum FrameKind {
    /// A canonical DAG runs the bodies it defines: references name their
    /// definitions.
    Canonical,
    /// A semantic instance runs the bodies of its template.
    Instance(Box<InstanceBinding>),
}

#[derive(Debug, Clone)]
struct InstanceBinding {
    /// The template and the Static substitution of this instance.
    specialization: StaticSpecializationId,
    /// Template-side owner → concrete owner, for this instance's template,
    /// the instances the template includes, and every enclosing instance.
    owners: HashMap<DagId, DagId>,
    /// Runtime units materialized under the concrete owners of this frame.
    units: HashSet<ResolvedUnitName>,
}

impl InstanceFrame {
    /// The frame of a canonical DAG, which runs its bodies as written.
    ///
    /// Only the canonical DAG type resolver holds the
    /// [`CanonicalFrameMint`], so an instance's bodies cannot be run in a
    /// canonical frame built elsewhere.
    #[must_use]
    pub(crate) const fn canonical(_: CanonicalFrameMint) -> Self {
        Self {
            kind: FrameKind::Canonical,
        }
    }

    /// The frame of the instance `id`, specialized by `specialization`,
    /// inside the DAG that runs in `parent`.
    ///
    /// Reached only through
    /// [`InstanceRecord::frame`](crate::ir::instance::InstanceRecord::frame),
    /// whose record keeps `id` and `specialization` consistent.
    /// `template_edges` are the instances the template itself includes, as
    /// the template records them; `runtime_units` are the runtime units this
    /// instance materializes.
    #[must_use]
    pub(in crate::ir::instance) fn instance<'a>(
        parent: &Self,
        id: &InstanceId,
        specialization: &StaticSpecializationId,
        template_edges: impl IntoIterator<Item = &'a InstanceId>,
        runtime_units: impl IntoIterator<Item = UnitName>,
    ) -> Self {
        let owner = id.owner();
        let (mut owners, mut units) = match &parent.kind {
            FrameKind::Canonical => (HashMap::new(), HashSet::new()),
            FrameKind::Instance(binding) => (binding.owners.clone(), binding.units.clone()),
        };
        owners.insert(id.template().clone(), owner.clone());
        owners.extend(template_edges.into_iter().map(|edge| {
            (
                edge.owner().clone(),
                owner.instance_child(edge.scope().clone()),
            )
        }));
        units.extend(
            runtime_units
                .into_iter()
                .map(|name| instance_declaration(id, name)),
        );
        Self {
            kind: FrameKind::Instance(Box::new(InstanceBinding {
                specialization: specialization.clone(),
                owners,
                units,
            })),
        }
    }

    /// The declaration `handle` denotes when this frame's DAG runs the body
    /// holding it.
    #[must_use]
    pub fn resolve(&self, handle: &LocalDecl) -> ResolvedDeclName {
        self.rebase(handle.definition(FrameAccess(())))
    }

    /// The unit whose scale `unit` has when this frame's DAG runs the body
    /// holding it: the instance's own copy of a runtime unit it materializes,
    /// otherwise the unit definition itself.
    #[must_use]
    pub fn resolve_unit(&self, unit: &ResolvedUnitRef) -> ResolvedUnitName {
        let definition = unit.resolved();
        match &self.kind {
            FrameKind::Canonical => definition.clone(),
            FrameKind::Instance(binding) => {
                let rebased = self.rebase(definition);
                if binding.units.contains(&rebased) {
                    rebased
                } else {
                    definition.clone()
                }
            }
        }
    }

    /// The nominal type `source` stands for in this frame, after the
    /// instance's Static type substitution.
    #[must_use]
    pub fn struct_type(&self, source: &ResolvedStructTypeName) -> ResolvedStructTypeName {
        self.specialization()
            .and_then(|specialization| specialization.substitution.types.get(source))
            .cloned()
            .unwrap_or_else(|| source.clone())
    }

    /// Whether this frame belongs to a semantic instance.
    #[must_use]
    pub const fn is_instance(&self) -> bool {
        matches!(self.kind, FrameKind::Instance(_))
    }

    /// The template and Static substitution of an instance frame.
    #[must_use]
    pub fn specialization(&self) -> Option<&StaticSpecializationId> {
        match &self.kind {
            FrameKind::Canonical => None,
            FrameKind::Instance(binding) => Some(&binding.specialization),
        }
    }

    /// The concrete owner this frame's DAG uses for declarations the
    /// template side owns by `owner`.
    #[must_use]
    pub(crate) fn owner<'a>(&'a self, owner: &'a DagId) -> &'a DagId {
        match &self.kind {
            FrameKind::Canonical => owner,
            FrameKind::Instance(binding) => binding.owners.get(owner).unwrap_or(owner),
        }
    }

    /// The identity this frame's DAG uses for the template-side identity
    /// `definition`, when instantiation re-keys the template's tables.
    #[must_use]
    pub(crate) fn rebase<Ns: NameNamespace>(
        &self,
        definition: &ResolvedName<Ns>,
    ) -> ResolvedName<Ns> {
        match &self.kind {
            FrameKind::Canonical => definition.clone(),
            FrameKind::Instance(binding) => binding.owners.get(definition.owner()).map_or_else(
                || definition.clone(),
                |owner| rebased_declaration(definition, owner),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::static_substitution::StaticSubstitution;

    /// An instance of `template` under `parent`, as an instance record keeps it.
    struct Record {
        id: InstanceId,
        specialization: StaticSpecializationId,
    }

    impl Record {
        fn new(id: InstanceId, substitution: StaticSubstitution) -> Self {
            let specialization = StaticSpecializationId::new(id.template().clone(), substitution);
            Self { id, specialization }
        }

        fn frame<'a>(
            &self,
            parent: &InstanceFrame,
            template_edges: impl IntoIterator<Item = &'a InstanceId>,
            runtime_units: impl IntoIterator<Item = UnitName>,
        ) -> InstanceFrame {
            InstanceFrame::instance(
                parent,
                &self.id,
                &self.specialization,
                template_edges,
                runtime_units,
            )
        }
    }
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::module_name::{ModuleAliasName, ScopeSegment};

    fn scope(alias: &str) -> ScopeSegment {
        ScopeSegment::Named(ModuleAliasName::expect_valid(alias))
    }

    fn decl(owner: &DagId, name: &str) -> ResolvedDeclName {
        ResolvedDeclName::for_test(owner.clone(), DeclName::expect_valid(name))
    }

    fn handle(owner: &DagId, name: &str) -> LocalDecl {
        LocalDecl::new(decl(owner, name))
    }

    struct Fixture {
        main: DagId,
        lib: DagId,
        leaf: DagId,
        external: DagId,
        /// `main` includes `lib` as `inst`.
        inst: Record,
        /// `lib` includes `leaf` as `inner`, as `lib` records it.
        inner_in_lib: InstanceId,
    }

    fn fixture() -> Fixture {
        let main = DagId::root_in_package("test", "main");
        let lib = DagId::root_in_package("test", "lib");
        let leaf = DagId::root_in_package("test", "leaf");
        let external = DagId::root_in_package("test", "other");
        let inst = Record::new(
            InstanceId::new(main.clone(), scope("inst"), lib.clone()),
            StaticSubstitution::default(),
        );
        let inner_in_lib = InstanceId::new(lib.clone(), scope("inner"), leaf.clone());
        Fixture {
            main,
            lib,
            leaf,
            external,
            inst,
            inner_in_lib,
        }
    }

    #[test]
    fn canonical_frames_run_bodies_as_written() {
        let fixture = fixture();
        let frame = InstanceFrame::canonical(CanonicalFrameMint::for_test());
        assert!(!frame.is_instance());
        assert!(frame.specialization().is_none());
        assert_eq!(
            frame.resolve(&handle(&fixture.lib, "x")),
            decl(&fixture.lib, "x")
        );
        assert_eq!(frame.owner(&fixture.lib), &fixture.lib);
    }

    #[test]
    fn instance_frames_reown_template_and_included_instance_declarations() {
        let fixture = fixture();
        let frame = fixture.inst.frame(
            &InstanceFrame::canonical(CanonicalFrameMint::for_test()),
            [&fixture.inner_in_lib],
            [],
        );
        let owner = fixture.inst.id.owner();
        assert!(frame.is_instance());
        assert_eq!(
            frame
                .specialization()
                .map(|specialization| &specialization.template),
            Some(&fixture.lib)
        );
        assert_eq!(frame.resolve(&handle(&fixture.lib, "x")), decl(owner, "x"));
        assert_eq!(
            frame.resolve(&handle(fixture.inner_in_lib.owner(), "y")),
            decl(&owner.instance_child(scope("inner")), "y")
        );
        // Declarations of other DAGs are shared by every instance.
        assert_eq!(
            frame.resolve(&handle(&fixture.external, "z")),
            decl(&fixture.external, "z")
        );
        assert_eq!(frame.owner(&fixture.lib), owner);
        assert_eq!(frame.owner(&fixture.main), &fixture.main);
    }

    #[test]
    fn nested_instance_frames_keep_the_enclosing_instances_owners() {
        let fixture = fixture();
        let outer = fixture.inst.frame(
            &InstanceFrame::canonical(CanonicalFrameMint::for_test()),
            [&fixture.inner_in_lib],
            [],
        );
        let outer_owner = fixture.inst.id.owner().clone();
        let inner = Record::new(
            InstanceId::new(outer_owner.clone(), scope("inner"), fixture.leaf.clone()),
            StaticSubstitution::default(),
        );
        let frame = inner.frame(&outer, [], []);
        // A value binding lowered in `lib` runs in the nested instance and
        // reads the enclosing instance's declarations.
        assert_eq!(
            frame.resolve(&handle(&fixture.lib, "x")),
            decl(&outer_owner, "x")
        );
        assert_eq!(
            frame.resolve(&handle(&fixture.leaf, "y")),
            decl(inner.id.owner(), "y")
        );
    }

    #[test]
    fn runtime_units_resolve_to_the_materialized_copy_only() {
        let fixture = fixture();
        let unit = UnitName::expect_valid("tick");
        let frame = fixture.inst.frame(
            &InstanceFrame::canonical(CanonicalFrameMint::for_test()),
            [],
            [unit.clone()],
        );
        let definition = ResolvedUnitName::for_test(fixture.lib.clone(), unit.clone());
        let materialized = instance_declaration(&fixture.inst.id, unit);
        let reference = |resolved: ResolvedUnitName| {
            ResolvedUnitRef::new(
                crate::syntax::dimension::UnitRef::local(resolved.leaf().clone()),
                resolved,
            )
        };
        assert_eq!(frame.resolve_unit(&reference(definition)), materialized);
        let static_unit = ResolvedUnitName::for_test(fixture.lib, UnitName::expect_valid("meter"));
        assert_eq!(
            frame.resolve_unit(&reference(static_unit.clone())),
            static_unit
        );
        assert_eq!(
            InstanceFrame::canonical(CanonicalFrameMint::for_test())
                .resolve_unit(&reference(materialized.clone())),
            materialized
        );
    }

    #[test]
    fn struct_types_follow_the_static_substitution() {
        let fixture = fixture();
        let port = crate::resolved_name::ResolvedStructTypeName::for_test(
            fixture.lib.clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Payload"),
        );
        let bound = crate::resolved_name::ResolvedStructTypeName::for_test(
            fixture.main.clone(),
            crate::syntax::type_name::StructTypeName::expect_valid("Point"),
        );
        let mut substitution = StaticSubstitution::default();
        substitution.types.insert(port.clone(), bound.clone());
        let record = Record::new(
            InstanceId::new(fixture.main.clone(), scope("inst"), fixture.lib),
            substitution,
        );
        let frame = record.frame(
            &InstanceFrame::canonical(CanonicalFrameMint::for_test()),
            [],
            [],
        );
        assert_eq!(frame.struct_type(&port), bound);
        assert_eq!(
            InstanceFrame::canonical(CanonicalFrameMint::for_test()).struct_type(&port),
            port
        );
    }
}
