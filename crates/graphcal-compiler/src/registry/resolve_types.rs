//! Data types used by the declaration-collection layer.
//!
//! These types have no dependency on the resolution logic itself, making them
//! suitable for use across all compilation phases.

use std::collections::{HashMap, HashSet};

use crate::dag_id::DagId;
use crate::desugar::desugared_ast::{AssertBody, Expr, FigureDecl, LayerDecl, PlotDecl};
use crate::registry::declared_type::IndexTypeRef;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::{IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::span::Span;

/// Returns `true` if `name` is a time scale identifier (`UTC`, `TT`, `TAI`, etc.).
#[must_use]
pub(crate) fn is_time_scale_name(name: &str) -> bool {
    crate::registry::time_scale::TimeScale::ALL_NAMES.contains(&name)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// Unlike `ImportedNames` which carries AST expressions, this carries
/// evaluated values. Used in per-file evaluation where each file is
/// compiled and evaluated independently.
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
// Entry types for resolved declarations
// ---------------------------------------------------------------------------

/// A resolved const declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedConstEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A resolved param declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedParamEntry {
    pub(crate) name: DeclName,
    pub(crate) default_expr: Option<Expr>,
    pub(crate) span: Span,
}

/// A resolved node declaration (before type annotation is added).
#[derive(Debug)]
pub struct ResolvedNodeEntry {
    pub(crate) name: DeclName,
    pub(crate) expr: Expr,
    pub(crate) span: Span,
}

/// A resolved assert declaration.
#[derive(Debug)]
pub struct ResolvedAssertEntry {
    pub(crate) name: DeclName,
    pub(crate) body: AssertBody,
    pub(crate) span: Span,
}

/// A resolved plot declaration.
#[derive(Debug)]
pub struct ResolvedPlotEntry {
    pub(crate) name: DeclName,
    pub(crate) decl: PlotDecl,
    pub(crate) span: Span,
}

/// A resolved figure declaration.
#[derive(Debug)]
pub struct ResolvedFigureEntry {
    pub name: DeclName,
    pub decl: FigureDecl,
}

/// A resolved layer declaration.
#[derive(Debug)]
pub struct ResolvedLayerEntry {
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
    /// A `#N` segment for a Nat range axis (#816).
    ///
    /// Range axes have no source-level index name, so the axis identity is
    /// positional: the segment binds to the assertion's axis at the same
    /// tuple position, validated at dim-check time.
    RangeStep { step: u64, span: Span },
}

impl<I> ExpectedFailKeyPart<I> {
    /// The source span of this key segment.
    #[must_use]
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } | Self::RangeStep { span, .. } => *span,
        }
    }

    /// The variant key this segment selects within its axis.
    #[must_use]
    pub(crate) fn variant(&self) -> IndexVariantName {
        match self {
            Self::Named { variant, .. } => variant.clone(),
            Self::RangeStep { step, .. } => IndexVariantName::range_step(*step),
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
            Self::RangeStep { .. } => None,
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
            Self::RangeStep { .. } => None,
        }
    }

    /// Whether this segment selects the given entry of an indexed value.
    ///
    /// Named segments require the entry's index identity to match; `#N`
    /// segments match the `#N` entry of any Nat range axis (the axis itself
    /// was bound positionally at dim-check time).
    #[must_use]
    pub fn matches_entry(&self, index: &IndexTypeRef, variant: &IndexVariantName) -> bool {
        match self {
            Self::Named {
                index: expected,
                variant: expected_variant,
                ..
            } => expected.matches_ref(index) && variant == expected_variant,
            Self::RangeStep { step, .. } => {
                matches!(index, IndexTypeRef::NatRange(_))
                    && *variant == IndexVariantName::range_step(*step)
            }
        }
    }

    /// Render this segment for diagnostics: `Index.Variant` or `#N`.
    #[must_use]
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Named { index, variant, .. } => format!("{}.{variant}", index.display_name()),
            Self::RangeStep { step, .. } => format!("#{step}"),
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

/// The result of declaration collection: declarations separated by category.
#[derive(Debug)]
pub(crate) struct ResolvedFile {
    /// Const declarations in source order.
    pub(crate) consts: Vec<ResolvedConstEntry>,
    /// Param declarations in source order.
    pub(crate) params: Vec<ResolvedParamEntry>,
    /// Node declarations in source order.
    pub(crate) nodes: Vec<ResolvedNodeEntry>,
    /// Assert declarations in source order.
    pub(crate) asserts: Vec<ResolvedAssertEntry>,
    /// Plot declarations in source order.
    pub(crate) plots: Vec<ResolvedPlotEntry>,
    /// Figure declarations in source order.
    pub(crate) figures: Vec<ResolvedFigureEntry>,
    /// Layer declarations in source order.
    pub(crate) layers: Vec<ResolvedLayerEntry>,
    /// All declaration names in source order with their category.
    pub(crate) source_order: Vec<(DeclName, DeclCategory)>,
    /// Set of all assert names (for checking `@assert_name` errors).
    pub(crate) assert_names: HashSet<DeclName>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub(crate) assumes_map: HashMap<DeclName, Vec<DeclName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub(crate) expected_fail: HashMap<DeclName, ParsedExpectedFail>,
    /// Plot names carrying `#[hidden]`: evaluated and referenceable from
    /// figures/layers, but excluded from standalone output (#847).
    pub(crate) hidden_plots: HashSet<DeclName>,
    /// Names of all declarations marked `pub` in this file (values + type-system).
    pub(crate) pub_names: HashSet<DeclName>,
}
