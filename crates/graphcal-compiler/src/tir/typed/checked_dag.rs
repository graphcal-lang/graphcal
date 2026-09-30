//! A checked DAG body: one body paired with every fact its check published.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::graphcal_error::GraphcalError;
use crate::resolved_name::{ResolvedDeclName, ResolvedStructTypeName};
use crate::tir::presentation::DagPresentationFacts;
use crate::tir::schedule::RuntimeSchedule;
use crate::tir::texpr::CheckedBodies;

use super::model::DagTIR;

/// A DAG body together with everything its check published: the checked tree
/// of every expression root, presentation facts, and its runtime schedule as a
/// callable.
///
/// A canonical body's trees are the ones its inference emitted; a semantic
/// instance's are its template's, specialized with the instance's Static
/// substitution (their types differ per instance, so they cannot be shared).
///
/// Created only when an [`InstantiatedTir`](super::program::InstantiatedTir) is
/// checked, so its checked trees are always present and cover exactly this
/// body's expression roots.
#[derive(Debug, Clone)]
pub struct CheckedDag {
    pub(super) body: DagTIR,
    bodies: CheckedBodies,
    presentation: DagPresentationFacts,
    runtime_schedule: RuntimeSchedule,
}

/// The facts one check published for one local body.
pub(super) struct PublishedDag {
    pub(super) bodies: CheckedBodies,
    pub(super) presentation: DagPresentationFacts,
    pub(super) runtime_schedule: RuntimeSchedule,
}

impl CheckedDag {
    /// Pair a checked body with the facts published for it.
    pub(super) fn new(
        body: DagTIR,
        published: PublishedDag,
        src: &NamedSource<Arc<String>>,
    ) -> Result<Self, GraphcalError> {
        let internal = |message: String| {
            GraphcalError::internal_error(
                format!("DAG `{}`: {message}", body.dag_id()),
                src,
                DiagnosticAnchor::WholeFile,
            )
        };
        if !published.bodies.cover(body.owned_expression_roots()) {
            return Err(internal(
                "typed bodies do not cover exactly its expression roots".to_owned(),
            ));
        }
        Ok(Self {
            body,
            bodies: published.bodies,
            presentation: published.presentation,
            runtime_schedule: published.runtime_schedule,
        })
    }

    /// The checked tree of every expression root this body owns.
    #[must_use]
    pub(crate) const fn bodies(&self) -> &CheckedBodies {
        &self.bodies
    }

    /// Every concrete constructor application this body's checked trees
    /// make; see [`CheckedBodies::concrete_applications`].
    #[must_use]
    pub fn concrete_constructor_applications(
        &self,
    ) -> Vec<(
        &ResolvedStructTypeName,
        Vec<crate::semantic::checked_type::CheckedGenericArg>,
    )> {
        self.bodies.concrete_applications()
    }

    /// The checked tree of every expression root this body owns, for tests
    /// outside the compiler.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub const fn bodies_for_test(&self) -> &CheckedBodies {
        &self.bodies
    }

    /// Runtime schedule of this DAG as a callable.
    #[must_use]
    pub const fn runtime_schedule(&self) -> &RuntimeSchedule {
        &self.runtime_schedule
    }

    /// Checked structured display and plot-channel presentation facts.
    #[must_use]
    pub const fn presentation(&self) -> &DagPresentationFacts {
        &self.presentation
    }

    /// Look up checked plot-channel presentation facts.
    #[must_use]
    pub fn plot_channel_presentations(
        &self,
        plot: &ResolvedDeclName,
    ) -> Option<&HashMap<crate::syntax::ast::EncodingChannel, crate::plot_shape::PlotChannelShape>>
    {
        self.presentation.plot_channels.get(plot)
    }

    /// Release the body, dropping its facts, to re-resolve it in a derived
    /// checking view.
    pub(crate) fn into_body(self) -> DagTIR {
        self.body
    }
}

/// The unchecked body, for the compiler's own passes.
///
/// Outside the compiler a checked DAG exposes only the declaration data
/// below, not its body's HIR expressions, frame, or name lookups.
impl CheckedDag {
    /// The unchecked body this DAG was checked from.
    #[must_use]
    pub(crate) const fn body(&self) -> &DagTIR {
        &self.body
    }

    /// The unchecked body, for tests outside the compiler.
    #[cfg(any(test, feature = "test-identities"))]
    #[must_use]
    pub const fn body_for_test(&self) -> &DagTIR {
        &self.body
    }

    /// Canonical identity of this DAG.
    #[must_use]
    pub const fn dag_id(&self) -> &crate::dag_id::DagId {
        self.body.dag_id()
    }

    /// Whether this DAG is a semantic instance of a template.
    #[must_use]
    pub const fn is_semantic_instance(&self) -> bool {
        self.body.is_semantic_instance()
    }

    /// The semantic include edges this DAG owns.
    #[must_use]
    pub fn semantic_instances(&self) -> &[crate::ir::instance::HirInstanceRecord] {
        self.body.semantic_instances()
    }

    /// Output names an importer may select from this DAG.
    #[must_use]
    pub const fn projectable_outputs(
        &self,
    ) -> &std::collections::HashSet<crate::syntax::decl_name::DeclName> {
        self.body.projectable_outputs()
    }

    /// Every declaration of this DAG, keyed by canonical identity.
    #[must_use]
    pub const fn decls(&self) -> &crate::ir::decl_table::DeclTable<super::model::Typed> {
        self.body.decls()
    }

    /// Const declarations, in source order.
    pub fn consts(&self) -> impl Iterator<Item = &super::model::TypedConstEntry> {
        self.body.consts()
    }

    /// Param declarations, in source order.
    pub fn params(&self) -> impl Iterator<Item = &super::model::TypedParamEntry> {
        self.body.params()
    }

    /// Node declarations, in source order.
    pub fn nodes(&self) -> impl Iterator<Item = &super::model::TypedNodeEntry> {
        self.body.nodes()
    }

    /// Assertions, in source order.
    pub fn asserts(&self) -> impl Iterator<Item = &super::model::TypedAssertEntry> {
        self.body.asserts()
    }

    /// Plots, in source order.
    pub fn plots(&self) -> impl Iterator<Item = &super::model::TypedPlotEntry> {
        self.body.plots()
    }

    /// Figures, in source order.
    pub fn figures(&self) -> impl Iterator<Item = &super::model::TypedFigureEntry> {
        self.body.figures()
    }

    /// Layers, in source order.
    pub fn layers(&self) -> impl Iterator<Item = &super::model::TypedLayerEntry> {
        self.body.layers()
    }

    /// Semantic facts of this DAG's body.
    #[must_use]
    pub const fn semantic(&self) -> &super::model::DagSemanticBody {
        self.body.semantic()
    }

    /// The checked declared type of a value declaration of this DAG.
    #[must_use]
    pub fn value_decl_type(
        &self,
        key: &ResolvedDeclName,
    ) -> Option<&super::model::CheckedDeclType> {
        self.body.value_decl_type(key)
    }

    /// Every value declaration of this DAG with its checked type annotation.
    pub fn value_decl_types(
        &self,
    ) -> impl Iterator<Item = (ResolvedDeclName, &super::model::CheckedTypeAnnotation)> {
        self.body.value_decl_types()
    }

    /// Identities of this DAG's value declarations, in source order.
    pub fn value_declaration_identities(&self) -> impl Iterator<Item = &ResolvedDeclName> {
        self.body.value_declaration_identities()
    }

    /// Whether `key` is a const declaration of this DAG.
    #[must_use]
    pub fn is_constant(&self, key: &ResolvedDeclName) -> bool {
        self.body.const_expr(key).is_some()
    }

    /// The unfinished-definition marker of a declaration, if it is a TODO.
    #[must_use]
    pub fn todo(
        &self,
        key: &ResolvedDeclName,
    ) -> Option<&crate::syntax::span::Spanned<Vec<crate::syntax::span::Spanned<ResolvedDeclName>>>>
    {
        self.body.todo(key)
    }

    /// Each assertion with the declarations that assume it.
    #[must_use]
    pub const fn assumes_map(&self) -> &HashMap<ResolvedDeclName, Vec<ResolvedDeclName>> {
        self.body.assumes_map()
    }

    /// Imported declarations keyed by their source-visible lexical binding.
    #[must_use]
    pub const fn imported_bindings(
        &self,
    ) -> &HashMap<
        crate::syntax::module_name::ScopedName,
        crate::ir::imported_binding::ImportedBinding,
    > {
        self.body.imported_bindings()
    }

    /// The declaration of this DAG an imported `target` is bound to.
    #[must_use]
    pub fn imported_destination(&self, target: &ResolvedDeclName) -> ResolvedDeclName {
        self.body.imported_destination(target)
    }
}
