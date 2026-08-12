//! Checked presentation provenance carried by TIR declarations and plot channels.
//!
//! Presentation is structured like the value it decorates. Unit expressions
//! retain their canonical declaration owner so dynamic scales are resolved in
//! the defining DAG rather than reconstructed from caller syntax.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::dag_id::DagId;
use crate::dimension::Dimension;
use crate::hir;
use crate::plot_shape::{PlotChannelShape, PlotLeafKind};
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::syntax::ast::EncodingChannel;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::index_name::{IndexEntryKey, ResolvedIndexVariant};
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName};

/// A checked display-unit expression and the declaration environment that owns it.
#[derive(Debug, Clone)]
pub struct UnitPresentation {
    defining_dag: DagId,
    owner: ResolvedDeclName,
    dimension: Dimension,
    unit: hir::ResolvedUnitExpr,
    requires_runtime_values: bool,
}

impl UnitPresentation {
    /// Construct a checked owner-qualified unit presentation fact.
    #[must_use]
    pub const fn new(
        defining_dag: DagId,
        owner: ResolvedDeclName,
        dimension: Dimension,
        unit: hir::ResolvedUnitExpr,
        requires_runtime_values: bool,
    ) -> Self {
        Self {
            defining_dag,
            owner,
            dimension,
            unit,
            requires_runtime_values,
        }
    }

    /// Checked DAG environment in which the declaration body executes.
    #[must_use]
    pub const fn defining_dag(&self) -> &DagId {
        &self.defining_dag
    }

    /// Declaration environment in which dynamic unit scales are evaluated.
    #[must_use]
    pub const fn owner(&self) -> &ResolvedDeclName {
        &self.owner
    }

    /// Statically checked physical dimension of the unit expression.
    #[must_use]
    pub const fn dimension(&self) -> &Dimension {
        &self.dimension
    }

    /// Canonical owner-qualified unit expression.
    #[must_use]
    pub const fn unit(&self) -> &hir::ResolvedUnitExpr {
        &self.unit
    }

    /// Whether resolving this unit needs live values from its defining DAG.
    #[must_use]
    pub const fn requires_runtime_values(&self) -> bool {
        self.requires_runtime_values
    }
}

/// Presentation metadata for one scalar value leaf.
#[derive(Debug, Clone)]
pub enum LeafPresentation {
    /// Quantity or complex-quantity display unit.
    Unit(UnitPresentation),
    /// Datetime display timezone.
    Timezone(crate::registry::time_zone::IanaTimeZoneId),
}

/// Owner-qualified identity of a lexical binder used by presentation metadata.
///
/// HIR local IDs are unique only within one lowered declaration body. Pairing
/// the ID with its declaration owner prevents projected presentation selectors
/// from accidentally reading an unrelated caller local with the same ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PresentationLocalBinder {
    owner: ResolvedDeclName,
    local: hir::LocalId,
}

impl PresentationLocalBinder {
    #[must_use]
    pub const fn new(owner: ResolvedDeclName, local: hir::LocalId) -> Self {
        Self { owner, local }
    }

    #[must_use]
    pub const fn owner(&self) -> &ResolvedDeclName {
        &self.owner
    }

    #[must_use]
    pub const fn local(&self) -> hir::LocalId {
        self.local
    }
}

/// A checked substitution from an indexed presentation binder to one access
/// argument evaluated in the declaration containing that access.
#[derive(Debug, Clone)]
pub struct PresentationIndexSubstitution {
    index: IndexTypeRef,
    binder: PresentationLocalBinder,
    argument: hir::expr::IndexArg,
}

impl PresentationIndexSubstitution {
    #[must_use]
    pub const fn new(
        index: IndexTypeRef,
        binder: PresentationLocalBinder,
        argument: hir::expr::IndexArg,
    ) -> Self {
        Self {
            index,
            binder,
            argument,
        }
    }

    #[must_use]
    pub const fn index(&self) -> &IndexTypeRef {
        &self.index
    }

    #[must_use]
    pub const fn binder(&self) -> &PresentationLocalBinder {
        &self.binder
    }

    #[must_use]
    pub const fn argument(&self) -> &hir::expr::IndexArg {
        &self.argument
    }

    #[must_use]
    pub fn argument_span(&self) -> Span {
        match &self.argument {
            hir::expr::IndexArg::Variant(variant) => variant.path_span(),
            hir::expr::IndexArg::Var(local) => local.span,
            hir::expr::IndexArg::Expr(expr) => expr.span,
        }
    }
}

/// Structured presentation for entries of one indexed axis.
#[derive(Debug, Clone)]
pub enum IndexedPresentation {
    /// Every position has the same nested presentation shape. A comprehension
    /// records its owner-qualified lexical binder so runtime branch selectors
    /// can be evaluated for each concrete entry without re-deriving provenance.
    Uniform {
        binder: Option<PresentationLocalBinder>,
        element: Box<PresentationProvenance>,
    },
    /// Positions retain independently authored presentation metadata.
    Entries(IndexMap<IndexEntryKey, PresentationProvenance>),
}

/// Checked runtime selector for presentation alternatives.
#[derive(Debug, Clone)]
pub enum PresentationSelection {
    /// Select presentation from the live branch of a checked `if` expression.
    If {
        defining_dag: DagId,
        owner: ResolvedDeclName,
        condition: hir::Expr,
        then_presentation: Box<PresentationProvenance>,
        else_presentation: Box<PresentationProvenance>,
    },
    /// Select presentation by the live typed `match` scrutinee.
    Match {
        defining_dag: DagId,
        owner: ResolvedDeclName,
        scrutinee: hir::Expr,
        arms: Vec<PresentationMatchArm>,
    },
}

/// Typed pattern identity and presentation for one checked match arm.
#[derive(Debug, Clone)]
pub struct PresentationMatchArm {
    pub pattern: PresentationMatchPattern,
    pub presentation: PresentationProvenance,
}

/// Closed semantic pattern identity used only for presentation selection.
#[derive(Debug, Clone)]
pub enum PresentationMatchPattern {
    IndexLabel(ResolvedIndexVariant),
    Constructor {
        owning_type: StructTypeRef,
        constructor: ConstructorName,
    },
}

/// Identity of one presentation-relevant inline DAG call site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PresentationCallKey {
    owner: ResolvedDeclName,
    span: Span,
}

impl PresentationCallKey {
    #[must_use]
    pub const fn new(owner: ResolvedDeclName, span: Span) -> Self {
        Self { owner, span }
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// Presentation metadata whose structure mirrors an evaluated public value.
#[derive(Debug, Clone, Default)]
pub enum PresentationProvenance {
    /// No authored presentation preference; render in the semantic default.
    #[default]
    None,
    /// Scalar quantity/datetime presentation.
    Leaf(LeafPresentation),
    /// Owner-qualified nominal value with per-field presentation.
    Struct {
        owning_type: StructTypeRef,
        fields: HashMap<FieldName, Self>,
    },
    /// Indexed value with its typed axis and nested entry presentation.
    Indexed {
        index: IndexTypeRef,
        elements: IndexedPresentation,
    },
    /// Inline DAG output plus the evaluated call environment needed by
    /// definition-owned dynamic units and branch selectors.
    DagCall {
        key: PresentationCallKey,
        output: Box<Self>,
    },
    /// Indexed access whose removed comprehension binders are substituted from
    /// arguments evaluated in the declaration containing the access.
    IndexProjection {
        defining_dag: DagId,
        owner: ResolvedDeclName,
        substitutions: Vec<PresentationIndexSubstitution>,
        output: Box<Self>,
    },
    /// Runtime branch selection over already-checked presentation alternatives.
    Select(Box<PresentationSelection>),
}

impl PresentationProvenance {
    /// Build one uniform indexed presentation layer.
    #[must_use]
    pub fn uniform_index(index: IndexTypeRef, element: Self) -> Self {
        Self::Indexed {
            index,
            elements: IndexedPresentation::Uniform {
                binder: None,
                element: Box::new(element),
            },
        }
    }

    /// Build one uniform indexed layer tied to a checked comprehension local.
    #[must_use]
    pub fn uniform_index_for_local(
        index: IndexTypeRef,
        binder: PresentationLocalBinder,
        element: Self,
    ) -> Self {
        Self::Indexed {
            index,
            elements: IndexedPresentation::Uniform {
                binder: Some(binder),
                element: Box::new(element),
            },
        }
    }

    /// Whether applying this fact needs values from its defining runtime DAG.
    #[must_use]
    pub fn requires_runtime_values(&self) -> bool {
        match self {
            Self::None | Self::Leaf(LeafPresentation::Timezone(_)) => false,
            Self::Leaf(LeafPresentation::Unit(unit)) => unit.requires_runtime_values(),
            Self::Struct { fields, .. } => fields.values().any(Self::requires_runtime_values),
            Self::Indexed { elements, .. } => match elements {
                IndexedPresentation::Uniform { element, .. } => element.requires_runtime_values(),
                IndexedPresentation::Entries(entries) => {
                    entries.values().any(Self::requires_runtime_values)
                }
            },
            Self::DagCall { .. } | Self::IndexProjection { .. } | Self::Select(_) => true,
        }
    }
}

/// Checked presentation facts for one plot encoding channel.
#[derive(Debug, Clone)]
pub struct PlotChannelPresentation {
    shape: PlotChannelShape,
    provenance: PresentationProvenance,
}

impl PlotChannelPresentation {
    /// Pair the inferred quantity dimension with checked display provenance.
    #[must_use]
    pub const fn new(shape: PlotChannelShape, provenance: PresentationProvenance) -> Self {
        Self { shape, provenance }
    }

    /// Already-inferred plottable type shape, including every checked axis.
    #[must_use]
    pub const fn shape(&self) -> &PlotChannelShape {
        &self.shape
    }

    /// Already-inferred plottable scalar leaf type.
    #[must_use]
    pub const fn leaf(&self) -> &PlotLeafKind {
        self.shape.leaf()
    }

    /// Quantity dimension inferred for the channel's scalar leaves.
    #[must_use]
    pub const fn dimension(&self) -> Option<&Dimension> {
        match self.shape.leaf() {
            PlotLeafKind::Quantity(dimension) => Some(dimension),
            PlotLeafKind::Int
            | PlotLeafKind::Bool
            | PlotLeafKind::Datetime(_)
            | PlotLeafKind::Key(_)
            | PlotLeafKind::ContextualString => None,
        }
    }

    /// Structured display provenance inferred for the channel expression.
    #[must_use]
    pub const fn provenance(&self) -> &PresentationProvenance {
        &self.provenance
    }
}

/// Per-DAG presentation facts installed only after static checking succeeds.
#[derive(Debug, Clone, Default)]
pub struct DagPresentationFacts {
    pub declarations: HashMap<ResolvedDeclName, PresentationProvenance>,
    pub plot_channels: HashMap<ResolvedDeclName, HashMap<EncodingChannel, PlotChannelPresentation>>,
}
