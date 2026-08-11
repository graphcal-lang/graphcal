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

/// Structured presentation for entries of one indexed axis.
#[derive(Debug, Clone)]
pub enum IndexedPresentation {
    /// Every position has the same nested presentation shape. A comprehension
    /// records its lexical binder so runtime branch selectors can be evaluated
    /// for each concrete entry without re-deriving provenance.
    Uniform {
        local: Option<hir::LocalId>,
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
                local: None,
                element: Box::new(element),
            },
        }
    }

    /// Build one uniform indexed layer tied to a checked comprehension local.
    #[must_use]
    pub fn uniform_index_for_local(
        index: IndexTypeRef,
        local: hir::LocalId,
        element: Self,
    ) -> Self {
        Self::Indexed {
            index,
            elements: IndexedPresentation::Uniform {
                local: Some(local),
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
            Self::DagCall { .. } | Self::Select(_) => true,
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
