//! Runtime identity and value-shaped sidecars for checked presentation metadata.
//!
//! Static presentation facts identify inline DAG call sites. This module adds
//! the evaluation-scoped invocation identity that distinguishes concrete calls
//! and carries that identity through value-preserving projections.

use std::collections::HashMap;
use std::fmt;

use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::syntax::index_name::IndexEntryKey;
use graphcal_compiler::syntax::type_name::FieldName;
use indexmap::IndexMap;
use thiserror::Error;

use crate::decl_key::RuntimeDeclKey;

/// Opaque identity of one presentation-relevant inline DAG invocation.
///
/// IDs are allocated by one [`crate::execution_facts::EvaluatedPresentationCalls`]
/// store and are meaningful only for that evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationInvocationId(u64);

impl PresentationInvocationId {
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }
}

impl fmt::Display for PresentationInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Runtime instantiation of the invocation-bearing parts of checked
/// presentation provenance.
///
/// `None` deliberately collapses any subtree with no inline-call identity.
/// Struct and indexed variants therefore contain only non-empty descendants.
#[derive(Debug, Clone, Default)]
pub enum PresentationInstance {
    #[default]
    None,
    Struct {
        fields: HashMap<FieldName, Self>,
    },
    Indexed {
        entries: IndexMap<IndexEntryKey, Self>,
    },
    DagCall {
        invocation: PresentationInvocationId,
        output: Box<Self>,
    },
}

impl PresentationInstance {
    #[must_use]
    pub(crate) fn fields(fields: impl IntoIterator<Item = (FieldName, Self)>) -> Self {
        let fields = fields
            .into_iter()
            .filter(|(_, instance)| !instance.is_none())
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
            .filter(|(_, instance)| !instance.is_none())
            .collect::<IndexMap<_, _>>();
        if entries.is_empty() {
            Self::None
        } else {
            Self::Indexed { entries }
        }
    }

    #[must_use]
    pub(crate) fn dag_call(invocation: PresentationInvocationId, output: Self) -> Self {
        Self::DagCall {
            invocation,
            output: Box::new(output),
        }
    }

    #[must_use]
    pub(crate) const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Preserve call wrappers while selecting one struct field.
    pub(crate) fn project_field(
        self,
        field: &FieldName,
    ) -> Result<Self, PresentationInstanceProjectionError> {
        match self {
            Self::None => Ok(Self::None),
            Self::Struct { mut fields } => Ok(fields.remove(field).unwrap_or_else(Self::default)),
            Self::DagCall { invocation, output } => output
                .project_field(field)
                .map(|output| Self::dag_call(invocation, output)),
            Self::Indexed { .. } => Err(PresentationInstanceProjectionError::ExpectedStruct),
        }
    }

    /// Preserve call wrappers while selecting concrete indexed entries.
    pub(crate) fn project_indexes(
        self,
        keys: &[IndexEntryKey],
    ) -> Result<Self, PresentationInstanceProjectionError> {
        let Some((key, remaining)) = keys.split_first() else {
            return Ok(self);
        };
        match self {
            Self::None => Ok(Self::None),
            Self::Indexed { mut entries } => entries.swap_remove(key).map_or_else(
                || Ok(Self::None),
                |instance| instance.project_indexes(remaining),
            ),
            Self::DagCall { invocation, output } => output
                .project_indexes(keys)
                .map(|output| Self::dag_call(invocation, output)),
            Self::Struct { .. } => Err(PresentationInstanceProjectionError::ExpectedIndexed),
        }
    }
}

/// Structural mismatch while applying a value projection to its presentation
/// instance. A checked program reaching either variant is an internal error.
#[derive(Debug, Clone, Copy, Error)]
pub enum PresentationInstanceProjectionError {
    #[error("field projection expected struct presentation provenance")]
    ExpectedStruct,
    #[error("index projection expected indexed presentation provenance")]
    ExpectedIndexed,
}

/// One semantic runtime value paired atomically with its presentation instance.
#[derive(Debug, Clone)]
pub struct EvaluatedRuntimeValue {
    value: RuntimeValue,
    presentation: PresentationInstance,
}

impl EvaluatedRuntimeValue {
    #[must_use]
    pub(crate) const fn new(value: RuntimeValue, presentation: PresentationInstance) -> Self {
        Self {
            value,
            presentation,
        }
    }

    #[must_use]
    pub(crate) const fn plain(value: RuntimeValue) -> Self {
        Self::new(value, PresentationInstance::None)
    }

    #[must_use]
    pub(crate) fn into_value(self) -> RuntimeValue {
        self.value
    }

    #[must_use]
    pub(crate) fn into_parts(self) -> (RuntimeValue, PresentationInstance) {
        (self.value, self.presentation)
    }
}

/// Presentation instances retained beside declaration runtime values.
pub type PresentationInstanceMap = HashMap<RuntimeDeclKey, PresentationInstance>;
