//! Selected value-shaped presentation evidence, owned by values, never by a call cache.
//!
//! A [`Presentation`] has the shape of the value it presents: a struct value's
//! presentation reuses the value's `StructValue` container and an indexed
//! value's its `IndexedValue` container, over the value's own axis. It is
//! read in the value's shape too, so it never disagrees with its value: a
//! field or an entry the presentation does not describe is plain.
//!
//! A presentation is [pending](PendingPresentation) while a display unit it
//! requests still waits for its owner's frame, and
//! [resolved](ResolvedPresentation) once every request was computed. Pending
//! unit requests contain no lexical environment or selector expression. The
//! interpreter resolves them before the owning invocation is discarded.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir::expr::{ResolvedUnitExpr, ResolvedUnitRef};
use graphcal_compiler::registry::time_zone::IanaTimeZoneId;
use graphcal_compiler::registry::unit::PositiveFiniteScale;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::type_name::FieldName;
use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use graphcal_compiler::resolved_name::ResolvedDeclName;

use crate::runtime_value::{IndexedValue, StructValue};

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

/// How one leaf of a value is displayed.
#[derive(Debug, Clone)]
pub enum ResolvedLeaf {
    /// Display a quantity leaf in a unit.
    Unit {
        label: String,
        scale: PositiveFiniteScale,
    },
    /// Display a datetime leaf in a time zone.
    Timezone(IanaTimeZoneId),
    /// The requested display failed; the leaf keeps its SI value.
    Failed(PresentationFailure),
}

/// How one leaf of a value is displayed, while a display unit it requests may
/// still wait for its owner's frame.
#[derive(Debug, Clone)]
pub enum PendingLeaf {
    /// A display that needs no further computation.
    Ready(ResolvedLeaf),
    /// A display unit whose scale is computed once its owner's frame is complete.
    Requested(Box<PendingDisplayUnit>),
}

/// The presentation of a value, in the shape of that value.
///
/// `L` is the leaf presentation: [`PendingLeaf`] or [`ResolvedLeaf`].
#[derive(Debug)]
pub enum Presentation<L> {
    /// No leaf of the value is presented.
    Plain,
    /// Every leaf of the value is presented alike (a conversion distributes
    /// element-wise over an indexed value).
    Uniform(L),
    /// The presentations of a struct value's fields, in the value's own
    /// constructor application.
    Struct(StructValue<Self>),
    /// The presentations of an indexed value's entries, over the value's own
    /// axis.
    Indexed(IndexedValue<Self>),
}

/// A presentation whose display-unit requests may still be pending.
pub type PendingPresentation = Presentation<PendingLeaf>;

/// A presentation whose every display unit was computed.
pub type ResolvedPresentation = Presentation<ResolvedLeaf>;

/// Pending presentations of evaluated declarations.
pub type PendingPresentationMap = HashMap<ResolvedDeclName, PendingPresentation>;

/// Resolved presentations of evaluated declarations.
pub type ResolvedPresentationMap = HashMap<ResolvedDeclName, ResolvedPresentation>;

impl<L: Clone> Clone for Presentation<L> {
    fn clone(&self) -> Self {
        #[cfg(test)]
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::PresentationEvidenceCopyNode,
        );
        match self {
            Self::Plain => Self::Plain,
            Self::Uniform(leaf) => Self::Uniform(leaf.clone()),
            Self::Struct(fields) => Self::Struct(fields.clone()),
            Self::Indexed(entries) => Self::Indexed(entries.clone()),
        }
    }
}

impl<L> Presentation<L> {
    /// The presentation of a struct value from its fields' presentations:
    /// plain when no field is presented.
    #[must_use]
    pub(crate) fn of_struct(fields: StructValue<Self>) -> Self {
        if fields.fields().all(|(_, field)| field.is_plain()) {
            Self::Plain
        } else {
            Self::Struct(fields)
        }
    }

    /// The presentation of an indexed value from its entries' presentations:
    /// plain when no entry is presented.
    #[must_use]
    pub(crate) fn of_indexed(entries: IndexedValue<Self>) -> Self {
        if entries.values().iter().all(Self::is_plain) {
            Self::Plain
        } else {
            Self::Indexed(entries)
        }
    }

    #[must_use]
    pub(crate) const fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }

    /// The presentation of `field` of the struct value this presents,
    /// borrowed so that siblings are never copied. A uniform presentation
    /// presents every field alike. `None` means plain: an indexed value's
    /// presentation presents no field.
    #[must_use]
    pub(crate) fn field_ref(&self, field: &FieldName) -> Option<&Self> {
        match self {
            Self::Struct(fields) => fields.field(field),
            Self::Plain | Self::Uniform(_) => Some(self),
            Self::Indexed(_) => None,
        }
    }

    /// The presentation of entry `key` of the indexed value this presents,
    /// borrowed so that siblings are never copied. A uniform presentation
    /// presents every entry alike. `None` means plain: a struct value's
    /// presentation presents no entry.
    #[must_use]
    pub(crate) fn entry_ref(&self, key: &IndexEntryKey) -> Option<&Self> {
        match self {
            Self::Indexed(entries) => entries.get(key),
            Self::Plain | Self::Uniform(_) => Some(self),
            Self::Struct(_) => None,
        }
    }

    /// The owned presentation of `field` of the struct value this presents.
    #[must_use]
    pub(crate) fn into_field(self, field: &FieldName) -> Self {
        match self {
            Self::Struct(fields) => fields.into_field(field).unwrap_or(Self::Plain),
            Self::Plain | Self::Uniform(_) => self,
            Self::Indexed(_) => Self::Plain,
        }
    }

    /// The owned presentation of entry `key` of the indexed value this
    /// presents.
    #[must_use]
    pub(crate) fn into_entry(self, key: &IndexEntryKey) -> Self {
        match self {
            Self::Indexed(entries) => entries.into_entry(key).unwrap_or(Self::Plain),
            Self::Plain | Self::Uniform(_) => self,
            Self::Struct(_) => Self::Plain,
        }
    }

    /// Replace every leaf presentation, keeping the shape.
    pub(crate) fn try_map_leaves<M, E>(
        self,
        leaf: &mut impl FnMut(L) -> Result<M, E>,
    ) -> Result<Presentation<M>, E> {
        match self {
            Self::Plain => Ok(Presentation::Plain),
            Self::Uniform(presented) => leaf(presented).map(Presentation::Uniform),
            Self::Struct(fields) => fields
                .try_map(|_, field| field.try_map_leaves(leaf))
                .map(Presentation::Struct),
            Self::Indexed(entries) => entries
                .try_map(|_, entry| entry.try_map_leaves(leaf))
                .map(Presentation::Indexed),
        }
    }

    /// Active retention metric: selected evidence nodes, excluding unrelated frame values.
    #[must_use]
    pub fn retained_nodes(&self) -> usize {
        match self {
            Self::Plain => 0,
            Self::Uniform(_) => 1,
            Self::Struct(fields) => fields
                .fields()
                .map(|(_, field)| field.retained_nodes())
                .fold(1, usize::saturating_add),
            Self::Indexed(entries) => entries
                .values()
                .iter()
                .map(Self::retained_nodes)
                .fold(1, usize::saturating_add),
        }
    }
}

impl<L: Clone> Presentation<L> {
    /// The presentation of `field` of the struct value this presents.
    #[must_use]
    pub(crate) fn field(&self, field: &FieldName) -> Self {
        self.field_ref(field).map_or(Self::Plain, Clone::clone)
    }

    /// The presentation of entry `key` of the indexed value this presents.
    #[must_use]
    pub(crate) fn entry(&self, key: &IndexEntryKey) -> Self {
        self.entry_ref(key).map_or(Self::Plain, Clone::clone)
    }
}
