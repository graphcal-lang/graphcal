//! Project diagnostics for module-resolution failures.

use graphcal_compiler::resolve::category::ExportedImportItemKind;
use graphcal_compiler::resolve::error::ModuleResolveError;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::graph::DagReference;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::semantic_error::name::{DuplicateDeclaration, NameError};
use graphcal_compiler::semantic_error::visibility::VisibilityError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;

use crate::compile_error::PipelineError;

/// N001 for a resolver duplicate, rendered with its spelled name.
fn duplicate_name(
    name: DuplicateDeclaration,
    first: Span,
    duplicate: Span,
    src: SourceId,
) -> PipelineError {
    PipelineError::Semantic(SemanticError::located(
        src,
        duplicate,
        NameError::DuplicateName { name, first },
    ))
}

/// N009 for a selective alias that takes a reserved spelling.
fn reserved_alias(
    kind: ExportedImportItemKind,
    name: NameAtom,
    span: Span,
    src: SourceId,
) -> PipelineError {
    let kind = match kind {
        ExportedImportItemKind::Decl(_) => "Term alias",
        ExportedImportItemKind::Constructor => "constructor alias",
        ExportedImportItemKind::Dimension => "dimension alias",
        ExportedImportItemKind::Unit(_) => "unit alias",
        ExportedImportItemKind::Type => "type alias",
        ExportedImportItemKind::Index => "index alias",
    };
    PipelineError::Semantic(SemanticError::located(
        src,
        span,
        NameError::BuiltinNameShadowed { kind, name },
    ))
}

pub(super) fn module_resolve_compile_error(
    err: ModuleResolveError,
    src: SourceId,
) -> PipelineError {
    match err {
        ModuleResolveError::PrivateName { owner, name, .. } => {
            PipelineError::Semantic(SemanticError::located(
                src,
                src.whole_span(),
                VisibilityError::ImportPrivateItem {
                    name,
                    file_path: DagReference::Dag(owner),
                },
            ))
        }
        ModuleResolveError::PrivateImportItem {
            owner, name, span, ..
        } => PipelineError::Semantic(SemanticError::located(
            src,
            span,
            VisibilityError::ImportPrivateItem {
                name,
                file_path: DagReference::Dag(owner),
            },
        )),
        ModuleResolveError::WrongImportCategory {
            owner,
            mismatch,
            span,
        } => PipelineError::Semantic(SemanticError::located(
            src,
            span,
            ModuleError::ImportCategoryMismatch {
                file_path: DagReference::Dag(owner),
                mismatch,
            },
        )),
        ModuleResolveError::IncludeItemNotProjectable { name, span, .. } => {
            PipelineError::Semantic(SemanticError::located(
                src,
                span,
                ModuleError::IncludeItemNotProjectable { name },
            ))
        }
        ModuleResolveError::ConstructorOwnerRebound {
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
        ModuleResolveError::ReservedImportName {
            kind, name, span, ..
        } => reserved_alias(kind, name, span, src),
        ModuleResolveError::DuplicateSymbol {
            name,
            first,
            duplicate,
            ..
        }
        | ModuleResolveError::DuplicateImportName {
            name,
            first,
            duplicate,
            ..
        } => duplicate_name(DuplicateDeclaration::Name(name), first, duplicate, src),
        ModuleResolveError::DuplicateIndexVariant {
            variant,
            first,
            duplicate,
            ..
        } => duplicate_name(
            DuplicateDeclaration::IndexVariant(variant),
            first,
            duplicate,
            src,
        ),
        ModuleResolveError::DuplicatePluginFunction {
            function,
            first,
            duplicate,
            ..
        } => duplicate_name(
            DuplicateDeclaration::Name(function.atom().clone()),
            first,
            duplicate,
            src,
        ),
        other => PipelineError::Semantic(SemanticError::located(
            src,
            src.whole_span(),
            ModuleError::resolution(other),
        )),
    }
}
