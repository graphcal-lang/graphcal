//! Project diagnostics for module-resolution failures.

use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::syntax::span::Span;

use crate::eval::types::CompileError;

/// N001 for a resolver duplicate, rendered with its spelled name.
fn duplicate_name(
    name: String,
    first: Span,
    duplicate: Span,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    CompileError::Eval(GraphcalError::DuplicateName {
        name,
        src: src.clone(),
        duplicate: duplicate.into(),
        first: first.into(),
    })
}

pub(super) fn module_resolve_compile_error(
    err: graphcal_compiler::resolve::error::ModuleResolveError,
    src: &NamedSource<Arc<String>>,
) -> CompileError {
    match err {
        graphcal_compiler::resolve::error::ModuleResolveError::PrivateName {
            owner, name, ..
        } => CompileError::Eval(GraphcalError::ImportPrivateItem {
            name: name.to_string(),
            file_path: owner.to_string(),
            src: src.clone(),
            span: Span::new(0, src.inner().len()).into(),
        }),
        graphcal_compiler::resolve::error::ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            span,
        } => CompileError::Eval(GraphcalError::ImportCategoryMismatch {
            file_path: owner.to_string(),
            mismatch,
            src: src.clone(),
            span: span.into(),
        }),
        graphcal_compiler::resolve::error::ModuleResolveError::IncludeItemNotProjectable {
            name,
            span,
            ..
        } => CompileError::Eval(GraphcalError::IncludeItemNotProjectable {
            name: name.to_string(),
            src: src.clone(),
            span: span.into(),
        }),
        graphcal_compiler::resolve::error::ModuleResolveError::ConstructorOwnerRebound {
            constructor,
            owner_type,
            span,
            ..
        } => CompileError::Eval(GraphcalError::IncludeConstructorOwnerRebound {
            constructor: constructor.to_string(),
            owner_type: owner_type.to_string(),
            src: src.clone(),
            span: span.into(),
        }),
        graphcal_compiler::resolve::error::ModuleResolveError::DuplicateSymbol {
            name,
            first,
            duplicate,
            ..
        }
        | graphcal_compiler::resolve::error::ModuleResolveError::DuplicateImportName {
            name,
            first,
            duplicate,
            ..
        } => duplicate_name(name.to_string(), first, duplicate, src),
        graphcal_compiler::resolve::error::ModuleResolveError::DuplicateIndexVariant {
            variant,
            first,
            duplicate,
            ..
        } => duplicate_name(variant.to_string(), first, duplicate, src),
        graphcal_compiler::resolve::error::ModuleResolveError::DuplicatePluginFunction {
            function,
            first,
            duplicate,
            ..
        } => duplicate_name(function.to_string(), first, duplicate, src),
        other => CompileError::Eval(GraphcalError::EvalError {
            message: other.to_string(),
            src: src.clone(),
            span: Span::new(0, src.inner().len()).into(),
        }),
    }
}
