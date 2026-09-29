//! Inputs that scope one expression-lowering run.

use crate::resolved_name::{ResolvedDeclName, ResolvedUnitName};
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;
use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::registry::time_zone::TimeZoneRegistry;
use crate::registry::types::UnitRegistry;
use crate::syntax::module_name::ScopedName;

use crate::hir::lower::ModuleScope;

/// Context required to lower one expression tree into HIR.
#[derive(Debug, Clone, Copy)]
pub struct ExprLoweringContext<'a> {
    pub(super) scope: ModuleScope<'a>,
    pub(super) time_zones: &'a TimeZoneRegistry,
    pub(super) overlay: BindingOverlay<'a>,
}

impl<'a> ExprLoweringContext<'a> {
    /// Create a source-only expression-lowering context: every reference
    /// resolves through the module resolver and the implicit prelude.
    #[must_use]
    pub const fn new(scope: ModuleScope<'a>, time_zones: &'a TimeZoneRegistry) -> Self {
        Self::with_overlay(scope, time_zones, BindingOverlay::None)
    }

    /// Create an expression-lowering context whose references are resolved
    /// through `overlay` before the module resolver.
    #[must_use]
    pub const fn with_overlay(
        scope: ModuleScope<'a>,
        time_zones: &'a TimeZoneRegistry,
        overlay: BindingOverlay<'a>,
    ) -> Self {
        Self {
            scope,
            time_zones,
            overlay,
        }
    }
}

/// Bindings that take precedence over source resolution while lowering.
///
/// Source lowering (editor analysis, runtime binding expressions) resolves
/// every reference through the module resolver. Frozen IR bodies additionally
/// see bindings that exist only after collection: registry-synthesized units,
/// include projections, and concrete include-instance identities.
#[derive(Debug, Clone, Copy)]
pub enum BindingOverlay<'a> {
    /// No overlay: source resolution only.
    None,
    /// The owner's frozen unit registry, for registry-synthesized units that
    /// have no source module symbol (nominal type bodies).
    RegistryUnits(&'a UnitRegistry),
    /// The complete freeze-time overlay of one frozen IR.
    Frozen(FrozenBindings<'a>),
}

/// The freeze-time binding overlay of one frozen IR.
#[derive(Debug, Clone, Copy)]
pub struct FrozenBindings<'a> {
    /// The frozen unit scope used for registry-created synthetic bindings
    /// that do not have a source module symbol.
    pub unit_registry: &'a UnitRegistry,
    /// Source-visible unit projections with their concrete semantic identities.
    pub unit_bindings: &'a HashMap<SyntaxUnitRef, ResolvedUnitName>,
    /// Canonical declaration bindings for declarations already visible in the
    /// lowered IR, such as prefixed dependency entries and DAG self-imports.
    pub decl_bindings: &'a HashMap<ScopedName, ResolvedDeclName>,
    /// The concrete-instance to source-template identity map used to validate
    /// instance member access without flattening either identity.
    pub instance_templates: &'a HashMap<DagId, DagId>,
}

impl<'a> BindingOverlay<'a> {
    /// The concrete identity an include projection gives `reference`.
    pub(super) fn unit_binding(self, reference: &SyntaxUnitRef) -> Option<&'a ResolvedUnitName> {
        match self {
            Self::Frozen(frozen) => frozen.unit_bindings.get(reference),
            Self::None | Self::RegistryUnits(_) => None,
        }
    }

    /// The canonical declaration `name` is bound to, if the overlay binds it.
    pub(super) fn decl_binding(self, name: &ScopedName) -> Option<&'a ResolvedDeclName> {
        match self {
            Self::Frozen(frozen) => frozen.decl_bindings.get(name),
            Self::None | Self::RegistryUnits(_) => None,
        }
    }

    /// The source template of the concrete include instance `instance`.
    pub(super) fn instance_template(self, instance: &DagId) -> Option<&'a DagId> {
        match self {
            Self::Frozen(frozen) => frozen.instance_templates.get(instance),
            Self::None | Self::RegistryUnits(_) => None,
        }
    }

    /// Resolve a bare unit that only the owner's frozen unit registry defines.
    pub(super) fn resolve_registry_unit_ref(
        self,
        owner: &DagId,
        reference: &SyntaxUnitRef,
    ) -> Option<ResolvedUnitName> {
        let registry = match self {
            Self::RegistryUnits(registry) => registry,
            Self::Frozen(frozen) => frozen.unit_registry,
            Self::None => return None,
        };
        if reference.is_qualified() {
            return None;
        }
        registry.get_unit(reference)?;
        Some(ResolvedUnitName::from_def(
            owner.clone(),
            reference.leaf().clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::types::RegistryBuilder;
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::dimension::UnitName;
    use crate::syntax::names::NameAtom;
    use crate::syntax::non_empty::NonEmpty;

    fn unit(name: &str) -> SyntaxUnitRef {
        SyntaxUnitRef::local(UnitName::expect_valid(name))
    }

    fn prelude_units() -> UnitRegistry {
        let mut builder = RegistryBuilder::new();
        crate::registry::prelude::load_prelude(&mut builder).unwrap();
        builder.build().units
    }

    #[test]
    fn frozen_overlay_answers_from_its_maps() {
        let owner = DagId::root_in_package("test", "main");
        let template = DagId::root_in_package("test", "lib");
        let units = prelude_units();
        let bound_unit = ResolvedUnitName::from_def(template.clone(), UnitName::expect_valid("u"));
        let unit_bindings = HashMap::from([(unit("u"), bound_unit.clone())]);
        let name = ScopedName::from(DeclName::expect_valid("x"));
        let bound_decl = ResolvedDeclName::from_def(template.clone(), DeclName::expect_valid("x"));
        let decl_bindings = HashMap::from([(name.clone(), bound_decl.clone())]);
        let instance_templates = HashMap::from([(owner.clone(), template.clone())]);
        let overlay = BindingOverlay::Frozen(FrozenBindings {
            unit_registry: &units,
            unit_bindings: &unit_bindings,
            decl_bindings: &decl_bindings,
            instance_templates: &instance_templates,
        });

        assert_eq!(overlay.unit_binding(&unit("u")), Some(&bound_unit));
        assert_eq!(overlay.unit_binding(&unit("v")), None);
        assert_eq!(overlay.decl_binding(&name), Some(&bound_decl));
        assert_eq!(
            overlay.decl_binding(&ScopedName::from(DeclName::expect_valid("y"))),
            None
        );
        assert_eq!(overlay.instance_template(&owner), Some(&template));
        assert_eq!(overlay.instance_template(&template), None);
        assert_eq!(
            overlay.resolve_registry_unit_ref(&owner, &unit("km")),
            Some(ResolvedUnitName::from_def(
                owner.clone(),
                UnitName::expect_valid("km")
            ))
        );
    }

    #[test]
    fn registry_units_overlay_only_resolves_bare_registry_units() {
        let owner = DagId::root_in_package("test", "main");
        let units = prelude_units();
        let overlay = BindingOverlay::RegistryUnits(&units);

        assert_eq!(
            overlay.resolve_registry_unit_ref(&owner, &unit("km")),
            Some(ResolvedUnitName::from_def(
                owner.clone(),
                UnitName::expect_valid("km")
            ))
        );
        assert_eq!(
            overlay.resolve_registry_unit_ref(&owner, &unit("no_such_unit")),
            None
        );
        let qualified = SyntaxUnitRef::qualified(
            NonEmpty::singleton(NameAtom::parse("alias").unwrap()),
            UnitName::expect_valid("km"),
        );
        assert_eq!(overlay.resolve_registry_unit_ref(&owner, &qualified), None);
        assert_eq!(overlay.unit_binding(&unit("km")), None);
        assert_eq!(
            overlay.decl_binding(&ScopedName::from(DeclName::expect_valid("x"))),
            None
        );
        assert_eq!(overlay.instance_template(&owner), None);
    }

    #[test]
    fn empty_overlay_binds_nothing() {
        let owner = DagId::root_in_package("test", "main");
        let overlay = BindingOverlay::None;

        assert_eq!(overlay.resolve_registry_unit_ref(&owner, &unit("km")), None);
        assert_eq!(overlay.unit_binding(&unit("km")), None);
        assert_eq!(
            overlay.decl_binding(&ScopedName::from(DeclName::expect_valid("x"))),
            None
        );
        assert_eq!(overlay.instance_template(&owner), None);
    }
}
