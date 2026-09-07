//! Selected value-shaped presentation evidence, owned by values, never by a call cache.
//!
//! Pending unit requests contain no lexical environment or selector expression.
//! The interpreter resolves them before the owning invocation is discarded.

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::hir::expr::ResolvedUnitExpr;
use graphcal_compiler::registry::time_zone::IanaTimeZoneId;
use graphcal_compiler::registry::unit::PositiveFiniteScale;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::type_name::FieldName;
use indexmap::IndexMap;
use miette::NamedSource;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;

/// A display-only computation, to be performed in the selected value's owner frame.
#[derive(Debug, Clone)]
pub struct PendingDisplayUnit {
    pub owner: DagId,
    pub source: NamedSource<Arc<String>>,
    pub unit: ResolvedUnitExpr,
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
    pub declaration: graphcal_compiler::syntax::decl_name::ResolvedDeclName,
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

#[derive(Debug, Default)]
pub enum PresentationInstance {
    #[default]
    None,
    Unit {
        label: String,
        scale: PositiveFiniteScale,
    },
    Timezone(IanaTimeZoneId),
    Pending(Box<PendingDisplayUnit>),
    Failed(PresentationFailure),
    Struct {
        fields: HashMap<FieldName, Self>,
    },
    Indexed {
        entries: IndexMap<IndexEntryKey, Self>,
    },
}

impl Clone for PresentationInstance {
    fn clone(&self) -> Self {
        #[cfg(test)]
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::PresentationEvidenceCopyNode,
        );
        match self {
            Self::None => Self::None,
            Self::Unit { label, scale } => Self::Unit {
                label: label.clone(),
                scale: *scale,
            },
            Self::Timezone(timezone) => Self::Timezone(timezone.clone()),
            Self::Pending(request) => Self::Pending(request.clone()),
            Self::Failed(failure) => Self::Failed(failure.clone()),
            Self::Struct { fields } => Self::Struct {
                fields: fields.clone(),
            },
            Self::Indexed { entries } => Self::Indexed {
                entries: entries.clone(),
            },
        }
    }
}

impl PresentationInstance {
    #[must_use]
    pub(crate) fn fields(fields: impl IntoIterator<Item = (FieldName, Self)>) -> Self {
        let fields = fields
            .into_iter()
            .filter(|(_, evidence)| !evidence.is_none())
            .collect::<HashMap<_, _>>();
        if fields.is_empty() {
            Self::None
        } else {
            Self::Struct { fields }
        }
    }

    #[must_use]
    pub(crate) fn entries(entries: impl IntoIterator<Item = (IndexEntryKey, Self)>) -> Self {
        let entries = entries
            .into_iter()
            .filter(|(_, evidence)| !evidence.is_none())
            .collect::<IndexMap<_, _>>();
        if entries.is_empty() {
            Self::None
        } else {
            Self::Indexed { entries }
        }
    }

    #[must_use]
    pub(crate) const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Borrow only the selected subtree: repeated projections cannot copy siblings.
    pub(crate) fn project_field_ref(
        &self,
        field: &FieldName,
    ) -> Result<&Self, PresentationInstanceProjectionError> {
        match self {
            Self::None => Ok(self),
            Self::Struct { fields } => Ok(fields.get(field).unwrap_or(&Self::None)),
            _ => Err(PresentationInstanceProjectionError::ExpectedStruct),
        }
    }

    /// Missing sparse entries are valid absence; scalar annotations broadcast.
    pub(crate) fn project_indexes_ref(
        &self,
        keys: &[IndexEntryKey],
    ) -> Result<&Self, PresentationInstanceProjectionError> {
        match (self, keys.split_first()) {
            (evidence, None) => Ok(evidence),
            (Self::Indexed { entries }, Some((key, rest))) => entries
                .get(key)
                .unwrap_or(&Self::None)
                .project_indexes_ref(rest),
            (Self::Struct { .. }, Some(_)) => {
                Err(PresentationInstanceProjectionError::ExpectedIndexed)
            }
            (leaf, Some(_)) => Ok(leaf),
        }
    }

    pub(crate) fn project_field(
        self,
        field: &FieldName,
    ) -> Result<Self, PresentationInstanceProjectionError> {
        match self {
            Self::None => Ok(Self::None),
            Self::Struct { mut fields } => Ok(fields.remove(field).unwrap_or(Self::None)),
            _ => Err(PresentationInstanceProjectionError::ExpectedStruct),
        }
    }

    pub(crate) fn project_indexes(
        self,
        keys: &[IndexEntryKey],
    ) -> Result<Self, PresentationInstanceProjectionError> {
        match (self, keys.split_first()) {
            (evidence, None) => Ok(evidence),
            (Self::Indexed { mut entries }, Some((key, rest))) => entries
                .swap_remove(key)
                .map_or_else(|| Ok(Self::None), |evidence| evidence.project_indexes(rest)),
            (Self::Struct { .. }, Some(_)) => {
                Err(PresentationInstanceProjectionError::ExpectedIndexed)
            }
            // A conversion or timezone annotation broadcasts over every leaf.
            (leaf, Some(_)) => Ok(leaf),
        }
    }

    /// Active retention metric: selected evidence nodes, excluding unrelated frame values.
    #[must_use]
    pub fn retained_nodes(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Struct { fields } => fields
                .values()
                .map(Self::retained_nodes)
                .fold(1, usize::saturating_add),
            Self::Indexed { entries } => entries
                .values()
                .map(Self::retained_nodes)
                .fold(1, usize::saturating_add),
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Error)]
pub enum PresentationInstanceProjectionError {
    #[error("field projection expected struct presentation evidence")]
    ExpectedStruct,
    #[error("index projection expected indexed presentation evidence")]
    ExpectedIndexed,
}

pub type PresentationInstanceMap = HashMap<RuntimeDeclKey, PresentationInstance>;
