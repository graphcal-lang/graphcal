//! Inputs that scope one expression-lowering run.

use crate::resolved_name::{ResolvedDeclName, ResolvedDimName, ResolvedUnitName};
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;
use std::collections::HashMap;

use crate::dag_id::DagId;
use crate::registry::time_zone::TimeZoneRegistry;
use crate::registry::types::UnitRegistry;
use crate::resolve::ModuleResolver;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;

use crate::hir::lower::{GenericScope, PreludeTypeScope, TypeLoweringContext};

/// Context required to lower one expression tree into HIR.
#[derive(Debug, Clone, Copy)]
pub struct ExprLoweringContext<'a> {
    pub(super) owner: &'a DagId,
    pub(super) resolver: &'a ModuleResolver,
    pub(super) generic_scope: &'a GenericScope,
    pub(super) time_zones: &'a TimeZoneRegistry,
    pub(super) prelude: Option<&'a PreludeTypeScope>,
    pub(super) unit_registry: Option<&'a UnitRegistry>,
    pub(super) unit_bindings: Option<&'a HashMap<SyntaxUnitRef, ResolvedUnitName>>,
    pub(super) decl_bindings: Option<&'a HashMap<ScopedName, ResolvedDeclName>>,
    pub(super) instance_templates: Option<&'a HashMap<DagId, DagId>>,
}

impl<'a> ExprLoweringContext<'a> {
    /// Create an expression-lowering context.
    #[must_use]
    pub const fn new(
        owner: &'a DagId,
        resolver: &'a ModuleResolver,
        generic_scope: &'a GenericScope,
        time_zones: &'a TimeZoneRegistry,
    ) -> Self {
        Self {
            owner,
            resolver,
            generic_scope,
            time_zones,
            prelude: None,
            unit_registry: None,
            unit_bindings: None,
            decl_bindings: None,
            instance_templates: None,
        }
    }

    /// Add implicit prelude type-system symbols for unit and generic-argument lowering.
    #[must_use]
    pub const fn with_prelude(self, prelude: &'a PreludeTypeScope) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: Some(prelude),
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add the frozen unit scope used for registry-created synthetic bindings
    /// that do not have a source module symbol.
    #[must_use]
    pub const fn with_unit_registry(self, unit_registry: &'a UnitRegistry) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: Some(unit_registry),
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add source-visible unit projections with their concrete semantic identities.
    #[must_use]
    pub(crate) const fn with_unit_bindings(
        self,
        unit_bindings: &'a HashMap<SyntaxUnitRef, ResolvedUnitName>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: Some(unit_bindings),
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add canonical declaration bindings for declarations already visible in
    /// the lowered IR, such as prefixed dependency entries and DAG self-imports.
    #[must_use]
    pub(crate) const fn with_decl_bindings(
        self,
        decl_bindings: &'a HashMap<ScopedName, ResolvedDeclName>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: Some(decl_bindings),
            instance_templates: self.instance_templates,
        }
    }

    /// Add the concrete-instance to source-template identity map used to
    /// validate instance member access without flattening either identity.
    #[must_use]
    pub(crate) const fn with_instance_templates(
        self,
        instance_templates: &'a HashMap<DagId, DagId>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: Some(instance_templates),
        }
    }

    pub(super) const fn type_context(self) -> TypeLoweringContext<'a> {
        let ctx = TypeLoweringContext::new(self.owner, self.resolver, self.generic_scope);
        match self.prelude {
            Some(prelude) => ctx.with_prelude(prelude),
            None => ctx,
        }
    }

    pub(super) fn resolve_prelude_dimension_path(self, path: &NamePath) -> Option<ResolvedDimName> {
        self.prelude
            .and_then(|prelude| prelude.resolve_dimension_path(path))
    }

    pub(super) fn resolve_prelude_unit_ref(
        self,
        reference: &SyntaxUnitRef,
    ) -> Option<ResolvedUnitName> {
        self.prelude
            .and_then(|prelude| prelude.resolve_unit_ref(reference))
    }

    pub(super) fn resolve_registry_unit_ref(
        self,
        reference: &SyntaxUnitRef,
    ) -> Option<ResolvedUnitName> {
        if reference.is_qualified() {
            return None;
        }
        self.unit_registry?.get_unit(reference)?;
        Some(ResolvedUnitName::from_def(
            self.owner.clone(),
            reference.leaf().clone(),
        ))
    }
}
