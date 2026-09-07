//! Atomic value/evidence transport through the expression interpreter.

use crate::presentation_evidence::PresentationInstance;
use graphcal_compiler::registry::runtime_value::RuntimeValue;

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
    /// Unannotated recurrence computations retain the authored initial display.
    pub(crate) fn with_default_presentation(mut self, initial: &PresentationInstance) -> Self {
        if self.presentation.is_none() {
            self.presentation = initial.clone();
        }
        self
    }
    #[must_use]
    pub(crate) fn into_value(self) -> RuntimeValue {
        self.value
    }
    #[must_use]
    pub(crate) fn into_parts(self) -> (RuntimeValue, PresentationInstance) {
        (self.value, self.presentation)
    }
    pub(crate) const fn value(&self) -> &RuntimeValue {
        &self.value
    }
}
