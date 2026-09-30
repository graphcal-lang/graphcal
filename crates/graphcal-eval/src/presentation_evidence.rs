//! Presentation leaves: how one quantity or datetime leaf of a value is
//! displayed, and the display failures reported beside the value.
//!
//! A leaf is [pending](PendingLeaf) while a display unit it requests still
//! waits for its owner's frame, and [resolved](ResolvedLeaf) once the request
//! was computed. Pending unit requests contain no lexical environment or
//! selector expression. The interpreter resolves them before the owning
//! invocation is discarded. A leaf's kind is part of its type, so resolving
//! a leaf never changes the kind of value it presents.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir::expr::{ResolvedUnitExpr, ResolvedUnitRef};
use graphcal_compiler::registry::time_zone::IanaTimeZoneId;
use graphcal_compiler::registry::unit::PositiveFiniteScale;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::type_name::FieldName;
use miette::NamedSource;
use std::sync::Arc;
use thiserror::Error;

/// A display-only computation, to be performed once the frame of the selected
/// value's owner is complete.
#[derive(Debug, Clone)]
pub struct PendingDisplayUnit {
    /// The DAG whose frame computed the value; the request is resolved against
    /// that frame's values.
    pub owner: DagId,
    pub source: NamedSource<Arc<String>>,
    /// The display unit, each term already resolved in the scope of the tree
    /// that names it.
    pub unit: ResolvedUnitExpr<ResolvedUnitRef>,
}

/// An ordinary presentation failure. Invariants and cancellation never inhabit this type.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PresentationFailure {
    #[error("display scale unavailable in {source_name}: {message}")]
    Scale {
        source_name: String,
        message: String,
    },
    #[error("display label unavailable in {source_name}: {error}")]
    Formatting {
        source_name: String,
        #[source]
        error: graphcal_compiler::registry::format::CanonicalUnitFormatError,
    },
    #[error("display projection unavailable: {message}")]
    Projection { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PresentationPathPart {
    Field(FieldName),
    Index(IndexEntryKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LeafPresentationDiagnostic {
    pub path: Vec<PresentationPathPart>,
    pub failure: PresentationFailure,
}

/// A separately reported display failure; the computational value remains available.
#[derive(Debug, Clone, PartialEq)]
pub struct PresentationDiagnostic {
    pub declaration: graphcal_compiler::resolved_name::ResolvedDeclName,
    pub channel: Option<graphcal_compiler::syntax::ast::EncodingChannel>,
    pub detail: LeafPresentationDiagnostic,
}

impl std::fmt::Display for PresentationDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.declaration)?;
        if let Some(channel) = self.channel {
            write!(f, " channel {channel}")?;
        }
        for part in &self.detail.path {
            match part {
                PresentationPathPart::Field(field) => write!(f, ".{field}")?,
                PresentationPathPart::Index(index) => write!(f, "[{index}]")?,
            }
        }
        write!(f, ": {} (SI value retained)", self.detail.failure)
    }
}

/// How a quantity leaf is displayed.
#[derive(Debug, Clone)]
pub enum QuantityDisplay {
    /// Display the quantity in a unit.
    Unit {
        label: String,
        scale: PositiveFiniteScale,
    },
    /// The requested display failed; the quantity keeps its SI value.
    Failed(PresentationFailure),
}

/// How one leaf of a value is displayed. The leaf kind is the kind of value
/// it presents: a quantity (or complex) leaf, or a datetime leaf.
#[derive(Debug, Clone)]
pub enum ResolvedLeaf {
    Quantity(QuantityDisplay),
    /// Display a datetime in a time zone.
    Datetime(IanaTimeZoneId),
}

/// How a quantity leaf is displayed while its display unit may still wait
/// for its owner's frame.
#[derive(Debug, Clone)]
pub enum PendingQuantityDisplay {
    /// A display that needs no further computation.
    Ready(QuantityDisplay),
    /// A display unit whose scale is computed once its owner's frame is complete.
    Requested(Box<PendingDisplayUnit>),
}

/// How one leaf of a value is displayed, while a display unit it requests may
/// still wait for its owner's frame.
#[derive(Debug, Clone)]
pub enum PendingLeaf {
    Quantity(PendingQuantityDisplay),
    /// Display a datetime in a time zone.
    Datetime(IanaTimeZoneId),
}

/// The kind of value leaf a presentation leaf presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafKind {
    /// A quantity or complex leaf.
    Quantity,
    /// A datetime leaf.
    Datetime,
}

/// A presentation leaf: pending or resolved.
pub trait PresentationLeaf: Clone {
    /// The kind of value leaf this presents.
    fn kind(&self) -> LeafKind;
}

impl PresentationLeaf for ResolvedLeaf {
    fn kind(&self) -> LeafKind {
        match self {
            Self::Quantity(_) => LeafKind::Quantity,
            Self::Datetime(_) => LeafKind::Datetime,
        }
    }
}

impl PresentationLeaf for PendingLeaf {
    fn kind(&self) -> LeafKind {
        match self {
            Self::Quantity(_) => LeafKind::Quantity,
            Self::Datetime(_) => LeafKind::Datetime,
        }
    }
}
