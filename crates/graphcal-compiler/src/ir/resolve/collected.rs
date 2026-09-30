//! Data types used by the declaration-collection layer.
//!
//! Source collection shells stay here; declaration kinds and attribute
//! targets are the independent [`crate::declaration_kind`] contract.

use std::collections::HashMap;

use crate::assertion_expectation::{ExpectedFail, ExpectedFailKey};
use crate::resolve::namespace::Namespace;
use crate::syntax::ast::{DeclExposure, IntroducedName};
use crate::syntax::decl_name::DeclName;
use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// These names carry no parallel AST-expression import route. Canonical
/// targets cross the HIR boundary separately as typed imported bindings.
#[derive(Debug, Default, Clone)]
pub struct ImportedValueNames {
    /// Imported const names (for scope checking only — actual values are in the exec plan).
    pub const_names: Vec<(ScopedName, Span)>,
    /// Imported param names.
    pub param_names: Vec<(ScopedName, Span)>,
    /// Imported node names.
    pub node_names: Vec<(ScopedName, Span)>,
    /// Imported assert names (for `#[assumes]` validation).
    pub assert_names: Vec<(DeclName, Span)>,
    /// Plot aliases requested by include brace lists (#847). Registered in
    /// the value namespace for collision checking and recorded on the DAG so
    /// figures/layers can reference them.
    pub plot_names: Vec<(ScopedName, Span)>,
}

pub(crate) type ParsedExpectedFailKey = ExpectedFailKey<NamePath>;
pub type ParsedExpectedFail = ExpectedFail<NamePath>;

/// Source-level expected-fail configuration retained by declaration collection.
///
/// The attribute span is kept separately from key-part spans because the
/// blanket form has no keys but may still need a diagnostic at a later phase.
#[derive(Debug, Clone)]
pub(crate) struct CollectedExpectedFail {
    pub(crate) expected: ParsedExpectedFail,
    pub(crate) attribute_span: Span,
}

/// Roles that form a file or DAG's externally addressable declaration surface.
///
/// An explicit `pub`/`pub(bind)` export and an annotation-free `param` input
/// port have different provenance and capabilities. Both are externally
/// readable and may be selected as effective outputs, while only input ports
/// accept value bindings. Keeping the roles distinct prevents output selection
/// from erasing binding semantics or making a param look explicitly `pub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalDeclRole {
    ExplicitExport,
    InputPort,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExternalDeclSlot {
    namespace: Namespace,
    name: NameAtom,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalDeclSurface {
    roles: HashMap<ExternalDeclSlot, ExternalDeclRole>,
}

impl ExternalDeclSurface {
    /// Record one name by its import namespace and external exposure.
    ///
    /// Private names are not part of the surface. Only `param` declarations
    /// carry [`DeclExposure::InputPort`], so an input port is always a Term.
    pub fn record(
        &mut self,
        namespace: ImportItemNamespace,
        name: NameAtom,
        exposure: DeclExposure,
    ) {
        match exposure {
            DeclExposure::Private => {}
            DeclExposure::InputPort => {
                self.insert(Namespace::Term, name, ExternalDeclRole::InputPort);
            }
            DeclExposure::ExplicitExport => {
                self.insert(
                    Namespace::of(namespace),
                    name,
                    ExternalDeclRole::ExplicitExport,
                );
            }
        }
    }

    /// Record a declaration's own name with its declared exposure.
    pub fn record_declared(&mut self, name: IntroducedName<'_>) {
        self.record(name.namespace(), name.atom().clone(), name.exposure());
    }

    /// Record an explicitly exported flat Term.
    pub fn insert_explicit_export(&mut self, name: DeclName) {
        self.insert(
            Namespace::Term,
            name.into_atom(),
            ExternalDeclRole::ExplicitExport,
        );
    }

    fn insert(&mut self, namespace: Namespace, name: NameAtom, role: ExternalDeclRole) {
        let slot = ExternalDeclSlot { namespace, name };
        match self.roles.entry(slot) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(role);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                debug_assert_eq!(*entry.get(), role, "one external slot has one role");
            }
        }
    }

    fn role(&self, namespace: Namespace, name: &NameAtom) -> Option<ExternalDeclRole> {
        self.roles
            .get(&ExternalDeclSlot {
                namespace,
                name: name.clone(),
            })
            .copied()
    }

    /// Whether a flat Term carries an explicit export.
    #[must_use]
    pub fn is_explicit_export(&self, name: &DeclName) -> bool {
        self.role(Namespace::Term, name.atom()) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether a Static entity carries an explicit export.
    #[must_use]
    pub fn is_static_explicit_export(&self, name: &NameAtom) -> bool {
        self.role(Namespace::Static, name) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether a Unit carries an explicit export.
    #[must_use]
    pub fn is_unit_explicit_export(&self, name: &NameAtom) -> bool {
        self.role(Namespace::Unit, name) == Some(ExternalDeclRole::ExplicitExport)
    }

    /// Whether the Term is a named `param` input port.
    #[must_use]
    pub fn is_input_port(&self, name: &DeclName) -> bool {
        self.role(Namespace::Term, name.atom()) == Some(ExternalDeclRole::InputPort)
    }

    /// Whether external Term syntax can resolve the declaration in either role.
    #[must_use]
    pub fn is_externally_nameable(&self, name: &DeclName) -> bool {
        self.role(Namespace::Term, name.atom()).is_some()
    }

    /// Whether a Term may be selected from an instance output surface.
    #[must_use]
    pub fn can_select_output(&self, name: &DeclName) -> bool {
        matches!(
            self.role(Namespace::Term, name.atom()),
            Some(ExternalDeclRole::ExplicitExport | ExternalDeclRole::InputPort)
        )
    }
}
