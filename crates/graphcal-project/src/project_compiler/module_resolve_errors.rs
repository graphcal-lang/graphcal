//! Project diagnostics for module-resolution failures.

use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::NameError;
use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::span::Span;

use crate::compile_error::PipelineError;

/// N001 for a resolver duplicate, rendered with its spelled name.
fn duplicate_name(name: String, first: Span, duplicate: Span, src: SourceId) -> PipelineError {
    PipelineError::Semantic(SemanticError::located(
        src,
        duplicate,
        NameError::DuplicateName { name, first },
    ))
}

pub(super) fn module_resolve_compile_error(
    err: graphcal_compiler::resolve::error::ModuleResolveError,
    src: SourceId,
) -> PipelineError {
    match err {
        graphcal_compiler::resolve::error::ModuleResolveError::PrivateName {
            owner, name, ..
        } => PipelineError::Semantic(SemanticError::located(
            src,
            src.whole_span(),
            VisibilityError::ImportPrivateItem {
                name: name.to_string(),
                file_path: owner.to_string(),
            },
        )),
        graphcal_compiler::resolve::error::ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            span,
        } => PipelineError::Semantic(SemanticError::located(
            src,
            span,
            ModuleError::ImportCategoryMismatch {
                file_path: owner.to_string(),
                mismatch,
            },
        )),
        graphcal_compiler::resolve::error::ModuleResolveError::IncludeItemNotProjectable {
            name,
            span,
            ..
        } => PipelineError::Semantic(SemanticError::located(
            src,
            span,
            ModuleError::IncludeItemNotProjectable { name },
        )),
        graphcal_compiler::resolve::error::ModuleResolveError::ConstructorOwnerRebound {
            constructor,
            owner_type,
            span,
            ..
        } => PipelineError::Semantic(SemanticError::located(
            src,
            span,
            ModuleError::IncludeConstructorOwnerRebound {
                constructor,
                owner_type,
            },
        )),
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
        other => PipelineError::Semantic(SemanticError::located(
            src,
            src.whole_span(),
            ModuleError::resolution(other),
        )),
    }
}
