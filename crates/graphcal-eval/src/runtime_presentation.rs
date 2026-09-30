//! Atomic value/evidence transport through the expression interpreter.

use crate::presentation_evidence::{PendingPresentation, Presentation};
use crate::runtime_value::{IndexedValue, RuntimeValue, StructValue};

#[derive(Debug, Clone)]
pub struct EvaluatedRuntimeValue {
    value: RuntimeValue,
    presentation: PendingPresentation,
}

impl EvaluatedRuntimeValue {
    #[must_use]
    pub(crate) const fn new(value: RuntimeValue, presentation: PendingPresentation) -> Self {
        Self {
            value,
            presentation,
        }
    }
    #[must_use]
    pub(crate) const fn plain(value: RuntimeValue) -> Self {
        Self::new(value, Presentation::Plain)
    }
    /// A struct value whose fields were evaluated with their presentations;
    /// its presentation keeps the value's own constructor application.
    #[must_use]
    pub(crate) fn from_struct(fields: StructValue<Self>) -> Self {
        let (value, presentation) = fields.map(Self::into_parts).unzip();
        Self::new(
            RuntimeValue::Struct(value),
            Presentation::of_struct(presentation),
        )
    }
    /// An indexed value whose entries were evaluated with their
    /// presentations; its presentation keeps the value's own axis.
    #[must_use]
    pub(crate) fn from_indexed(entries: IndexedValue<Self>) -> Self {
        let (value, presentation) = entries.map(Self::into_parts).unzip();
        Self::new(
            RuntimeValue::Indexed(value),
            Presentation::of_indexed(presentation),
        )
    }
    /// Unannotated recurrence computations retain the authored initial display.
    pub(crate) fn with_default_presentation(mut self, initial: &PendingPresentation) -> Self {
        if self.presentation.is_plain() {
            self.presentation = initial.clone();
        }
        self
    }
    #[must_use]
    pub(crate) fn into_value(self) -> RuntimeValue {
        self.value
    }
    #[must_use]
    pub(crate) fn into_parts(self) -> (RuntimeValue, PendingPresentation) {
        (self.value, self.presentation)
    }
    pub(crate) const fn value(&self) -> &RuntimeValue {
        &self.value
    }
    pub(crate) const fn presentation(&self) -> &PendingPresentation {
        &self.presentation
    }
}
