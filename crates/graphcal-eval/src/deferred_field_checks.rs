//! Field-validation obligations produced while compile-time bounds are unresolved.

use std::cell::RefCell;

use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::model::StructFieldConstraintKey;

use crate::runtime_value::RuntimeValue;

/// One executed, constrained constructor field, retained even if its temporary
/// struct is discarded. Unconstrained fields never create an obligation.
pub struct DeferredFieldCheck {
    pub key: StructFieldConstraintKey,
    pub value: RuntimeValue,
    pub source: SourceId,
    pub span: Span,
}

/// Shared by provisional sessions and their source/declaration views. Sealing
/// must discharge every obligation after all bounds have been evaluated.
#[derive(Default)]
pub struct DeferredFieldChecks {
    checks: RefCell<Vec<DeferredFieldCheck>>,
}

impl DeferredFieldChecks {
    pub(crate) fn record(&self, check: DeferredFieldCheck) {
        self.checks.borrow_mut().push(check);
    }

    pub(crate) fn take(&self) -> Vec<DeferredFieldCheck> {
        self.checks.take()
    }
}
