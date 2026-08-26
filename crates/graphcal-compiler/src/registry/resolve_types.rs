//! Data types used by the declaration-collection layer.
//!
//! These types have no dependency on the resolution logic itself, making them
//! suitable for use across all compilation phases.

use std::collections::{HashMap, HashSet};

use crate::dag_id::DagId;
use crate::desugar::desugared_ast::{AssertBody, Expr, FigureDecl, LayerDecl, PlotDecl};
use crate::registry::declared_type::IndexTypeRef;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

/// The kind of a declaration (used for source-order tracking).
#[derive(Debug, Clone, Copy)]
pub enum DeclCategory {
    Const,
    Param,
    Node,
    Assert,
    Plot,
    Figure,
    Layer,
}

impl std::fmt::Display for DeclCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Const => write!(f, "const"),
            Self::Param => write!(f, "param"),
            Self::Node => write!(f, "node"),
            Self::Assert => write!(f, "assert"),
            Self::Plot => write!(f, "plot"),
            Self::Figure => write!(f, "figure"),
            Self::Layer => write!(f, "layer"),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry types for collected declaration shells
// ---------------------------------------------------------------------------

/// A collected const declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedConstEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A collected parameter declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedParamEntry {
    pub(crate) name: DeclName,
    pub(crate) default_expr: Option<Expr>,
    pub(crate) span: Span,
}

/// A collected node declaration awaiting type and HIR lowering.
#[derive(Debug)]
pub struct CollectedNodeEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A collected assertion declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedAssertEntry {
    pub(crate) name: DeclName,
    pub(crate) body: AssertBody,
    pub(crate) span: Span,
}

/// A collected plot declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedPlotEntry {
    pub(crate) name: DeclName,
    pub(crate) decl: PlotDecl,
    pub(crate) span: Span,
}

/// A collected figure declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedFigureEntry {
    pub name: DeclName,
    pub decl: FigureDecl,
}

/// A collected layer declaration awaiting HIR lowering.
#[derive(Debug)]
pub struct CollectedLayerEntry {
    pub name: DeclName,
    pub decl: LayerDecl,
}

/// One axis segment in a per-variant `#[expected_fail(...)]` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedFailKeyPart<I = IndexTypeRef> {
    /// An `Index.Variant` / `module.Index.Variant` segment for a named axis.
    ///
    /// Before module-aware TIR resolution, `index` is the source [`NamePath`]
    /// written in the attribute. After resolution, `index` is the semantic
    /// [`IndexTypeRef`] used by runtime assertion checks.
    Named {
        index: I,
        variant: IndexVariantName,
        span: Span,
    },
    /// A `#N` segment for a finite structural axis (#816).
    ///
    /// Finite axes have no source-level index name, so the axis identity is
    /// positional: the segment binds to the assertion's axis at the same
    /// tuple position, validated at dim-check time.
    FinitePosition { position: u64, span: Span },
}

impl<I> ExpectedFailKeyPart<I> {
    /// The source span of this key segment.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } | Self::FinitePosition { span, .. } => *span,
        }
    }

    /// The variant key this segment selects within its axis.
    #[must_use]
    pub(crate) fn entry_key(&self) -> IndexEntryKey {
        match self {
            Self::Named { variant, .. } => IndexEntryKey::named(variant.clone()),
            Self::FinitePosition { position, .. } => IndexEntryKey::position(*position),
        }
    }
}

impl ExpectedFailKeyPart<NamePath> {
    #[must_use]
    pub(crate) const fn parsed(
        index_path: NamePath,
        variant: IndexVariantName,
        span: Span,
    ) -> Self {
        Self::Named {
            index: index_path,
            variant,
            span,
        }
    }

    /// The parsed index path, when this segment targets a named axis.
    #[must_use]
    pub(crate) const fn index_path(&self) -> Option<&NamePath> {
        match self {
            Self::Named { index, .. } => Some(index),
            Self::FinitePosition { .. } => None,
        }
    }
}

impl ExpectedFailKeyPart<IndexTypeRef> {
    #[must_use]
    pub fn with_owner(
        owner: DagId,
        index: IndexName,
        variant: IndexVariantName,
        span: Span,
    ) -> Self {
        Self::Named {
            index: IndexTypeRef::with_owner(owner, index),
            variant,
            span,
        }
    }

    #[must_use]
    pub(crate) fn resolved(resolved: ResolvedIndexVariant, span: Span) -> Self {
        let (index, variant) = resolved.into_parts();
        Self::Named {
            index: IndexTypeRef::from_resolved(index),
            variant,
            span,
        }
    }

    /// The named index reference, when this segment targets a named axis.
    #[must_use]
    pub(crate) const fn named_index(&self) -> Option<&IndexTypeRef> {
        match self {
            Self::Named { index, .. } => Some(index),
            Self::FinitePosition { .. } => None,
        }
    }

    /// Whether this segment selects the given entry of an indexed value.
    ///
    /// Named segments require the entry's index identity to match; `#N`
    /// segments match the `#N` entry of any finite structural axis (the axis itself
    /// was bound positionally at dim-check time).
    #[must_use]
    pub fn matches_entry(&self, index: &IndexTypeRef, key: &IndexEntryKey) -> bool {
        match (self, key) {
            (
                Self::Named {
                    index: expected,
                    variant: expected_variant,
                    ..
                },
                IndexEntryKey::Named(actual),
            ) => expected.matches_ref(index) && actual == expected_variant,
            (Self::FinitePosition { position, .. }, IndexEntryKey::Position(actual)) => {
                matches!(index, IndexTypeRef::Finite(_)) && actual == position
            }
            (Self::Named { .. }, IndexEntryKey::Position(_))
            | (Self::FinitePosition { .. }, IndexEntryKey::Named(_)) => false,
        }
    }

    /// Render this segment for diagnostics: `Index.Variant` or `#N`.
    #[must_use]
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Named { index, variant, .. } => format!("{}.{variant}", index.display_name()),
            Self::FinitePosition { position, .. } => format!("#{position}"),
        }
    }
}

/// A single expected-fail key: a list of index/variant pairs.
///
/// - Length 1 for single-index assertions: `[Mode.Boost]`
/// - Length >1 for multi-index assertions: `[(Mode.Boost, Phase.Launch)]`
pub type ExpectedFailKey<I = IndexTypeRef> = Vec<ExpectedFailKeyPart<I>>;

pub(crate) type ParsedExpectedFailKeyPart = ExpectedFailKeyPart<NamePath>;
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

pub type ResolvedExpectedFailKeyPart = ExpectedFailKeyPart<IndexTypeRef>;
pub type ResolvedExpectedFailKey = ExpectedFailKey<IndexTypeRef>;
pub type ResolvedExpectedFail = ExpectedFail<IndexTypeRef>;

/// Describes how an assertion is expected to fail.
#[derive(Debug, Clone)]
pub enum ExpectedFail<I = IndexTypeRef> {
    /// The entire assertion is expected to fail: `#[expected_fail]`.
    All,
    /// Specific index keys are expected to fail: `#[expected_fail(Index.Variant, ...)]`.
    Variants(Vec<ExpectedFailKey<I>>),
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

#[derive(Debug, Clone, Default)]
pub struct ExternalDeclSurface {
    roles: HashMap<DeclName, ExternalDeclRole>,
}

impl ExternalDeclSurface {
    /// Record a declaration carrying an explicit `pub` or `pub(bind)` export.
    pub fn insert_explicit_export(&mut self, name: DeclName) {
        self.insert(name, ExternalDeclRole::ExplicitExport);
    }

    /// Record a `param` declaration as a named input port.
    pub fn insert_input_port(&mut self, name: DeclName) {
        self.insert(name, ExternalDeclRole::InputPort);
    }

    fn insert(&mut self, name: DeclName, role: ExternalDeclRole) {
        match self.roles.entry(name) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(role);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                debug_assert_eq!(
                    *entry.get(),
                    role,
                    "a declaration cannot be both an explicit export and an input port"
                );
            }
        }
    }

    /// Whether the declaration carries an explicit `pub` or `pub(bind)` export.
    #[must_use]
    pub fn is_explicit_export(&self, name: &DeclName) -> bool {
        self.roles.get(name) == Some(&ExternalDeclRole::ExplicitExport)
    }

    /// Whether the declaration is a named `param` input port.
    #[must_use]
    pub fn is_input_port(&self, name: &DeclName) -> bool {
        self.roles.get(name) == Some(&ExternalDeclRole::InputPort)
    }

    /// Whether external syntax may resolve the declaration in either role.
    #[must_use]
    pub fn is_externally_nameable(&self, name: &DeclName) -> bool {
        self.roles.contains_key(name)
    }

    /// Whether the declaration may be selected from the external output surface.
    ///
    /// For an input port this selects its effective value after applying a
    /// call/include binding or its default.
    #[must_use]
    pub fn can_select_output(&self, name: &DeclName) -> bool {
        matches!(
            self.roles.get(name),
            Some(ExternalDeclRole::ExplicitExport | ExternalDeclRole::InputPort)
        )
    }
}

/// The result of declaration collection: declarations separated by category.
#[derive(Debug)]
pub(crate) struct CollectedFile {
    /// Const declarations in source order.
    pub(crate) consts: Vec<CollectedConstEntry>,
    /// Param declarations in source order.
    pub(crate) params: Vec<CollectedParamEntry>,
    /// Node declarations in source order.
    pub(crate) nodes: Vec<CollectedNodeEntry>,
    /// Assert declarations in source order.
    pub(crate) asserts: Vec<CollectedAssertEntry>,
    /// Plot declarations in source order.
    pub(crate) plots: Vec<CollectedPlotEntry>,
    /// Figure declarations in source order.
    pub(crate) figures: Vec<CollectedFigureEntry>,
    /// Layer declarations in source order.
    pub(crate) layers: Vec<CollectedLayerEntry>,
    /// All declaration names in source order with their category.
    pub(crate) source_order: Vec<(DeclName, DeclCategory)>,
    /// Set of all assert names (for checking `@assert_name` errors).
    pub(crate) assert_names: HashSet<DeclName>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub(crate) assumes_map: HashMap<DeclName, Vec<DeclName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub(crate) expected_fail: HashMap<DeclName, CollectedExpectedFail>,
    /// Plot names carrying `#[hidden]`: evaluated and referenceable from
    /// figures/layers, but excluded from standalone output (#847).
    pub(crate) hidden_plots: HashSet<DeclName>,
    /// Explicit exports and annotation-free `param` input ports, classified by role.
    pub(crate) external_surface: ExternalDeclSurface,
}
