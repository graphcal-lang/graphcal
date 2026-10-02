//! Checked runtime interface of one directly authored entry DAG.

use graphcal_compiler::declaration_category::ValueDeclCategory;
use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::hir::source_interface::SourceDeclaration;
use graphcal_compiler::ir::resolve::collected::ExternalDeclSurface;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::ast::Visibility;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::CheckedTir;
use graphcal_compiler::tir::typed::declaration_view::{DeclarationView, ValueDeclaration};

use crate::compile_error::PipelineError;
use graphcal_compiler::resolved_name::ResolvedDeclName;

/// One checked entry-DAG parameter in direct source order.
#[derive(Debug)]
pub struct CheckedEntryParameter {
    name: DeclName,
    declared_type: CheckedType,
    has_default: bool,
    runtime_key: ResolvedDeclName,
    span: Span,
}

impl CheckedEntryParameter {
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    pub const fn declared_type(&self) -> &CheckedType {
        &self.declared_type
    }

    pub const fn has_default(&self) -> bool {
        self.has_default
    }

    pub const fn runtime_key(&self) -> &ResolvedDeclName {
        &self.runtime_key
    }

    pub const fn span(&self) -> Span {
        self.span
    }
}

/// One checked directly authored entry-DAG node in source order.
#[derive(Debug)]
pub struct CheckedEntryOutput {
    name: DeclName,
    declared_type: CheckedType,
    visibility: Visibility,
    runtime_key: ResolvedDeclName,
}

impl CheckedEntryOutput {
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    pub const fn declared_type(&self) -> &CheckedType {
        &self.declared_type
    }

    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    pub const fn runtime_key(&self) -> &ResolvedDeclName {
        &self.runtime_key
    }
}

/// The first unresolved required index that prevents runtime preparation.
#[derive(Debug)]
pub struct RequiredEntryIndex {
    name: IndexName,
    /// The index declaration in the entry source.
    span: Span,
}

impl RequiredEntryIndex {
    pub const fn name(&self) -> &IndexName {
        &self.name
    }

    /// The index declaration in the entry source.
    pub const fn span(&self) -> Span {
        self.span
    }
}

/// Fully checked, syntax-independent runtime interface of an entry DAG.
#[derive(Debug)]
pub struct CheckedEntryInterface {
    parameters: Vec<CheckedEntryParameter>,
    outputs: Vec<CheckedEntryOutput>,
    required_index: Option<RequiredEntryIndex>,
}

impl CheckedEntryInterface {
    pub fn parameters(&self) -> &[CheckedEntryParameter] {
        &self.parameters
    }

    pub fn outputs(&self) -> &[CheckedEntryOutput] {
        &self.outputs
    }

    pub const fn required_index(&self) -> Option<&RequiredEntryIndex> {
        self.required_index.as_ref()
    }
}

fn missing_interface_fact_internal_error(
    message: String,
    source: SourceId,
    span: Span,
) -> PipelineError {
    PipelineError::Semantic(SemanticError::internal_error(
        message,
        source,
        DiagnosticAnchor::Source(span),
    ))
}

/// Attach checked types and runtime identities to HIR source-interface records.
pub(super) fn build_checked_entry_interface(
    source_declarations: &[SourceDeclaration],
    tir: &CheckedTir,
    external_surface: &ExternalDeclSurface,
    source: SourceId,
) -> Result<CheckedEntryInterface, PipelineError> {
    let mut parameters = Vec::new();
    let mut outputs = Vec::new();
    let mut required_index = None;

    for declaration in source_declarations {
        match declaration {
            SourceDeclaration::Parameter { identity, span } => {
                let Some(ValueDeclaration {
                    category: ValueDeclCategory::Param,
                    annotation,
                    has_default,
                    ..
                }) = tir
                    .root()
                    .declaration(identity)
                    .and_then(DeclarationView::value)
                else {
                    return Err(missing_interface_fact_internal_error(
                        format!("HIR entry parameter `{identity}` is absent from checked TIR"),
                        source,
                        *span,
                    ));
                };
                parameters.push(CheckedEntryParameter {
                    name: identity.leaf().clone(),
                    declared_type: annotation.checked().declared().clone(),
                    has_default,
                    runtime_key: identity.clone(),
                    span: *span,
                });
            }
            SourceDeclaration::Node { identity, span } => {
                let Some(ValueDeclaration {
                    category: ValueDeclCategory::Node,
                    annotation,
                    ..
                }) = tir
                    .root()
                    .declaration(identity)
                    .and_then(DeclarationView::value)
                else {
                    return Err(missing_interface_fact_internal_error(
                        format!("HIR entry node `{identity}` is absent from checked TIR"),
                        source,
                        *span,
                    ));
                };
                let name = identity.leaf();
                outputs.push(CheckedEntryOutput {
                    name: name.clone(),
                    declared_type: annotation.checked().declared().clone(),
                    visibility: if external_surface.is_explicit_export(name) {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    runtime_key: identity.clone(),
                });
            }
            SourceDeclaration::Index { identity, span } => {
                let definition = tir.declared_index_def(identity).ok_or_else(|| {
                    missing_interface_fact_internal_error(
                        format!("HIR entry index `{identity}` is absent from checked TIR"),
                        source,
                        *span,
                    )
                })?;
                if required_index.is_none() && definition.is_required() {
                    required_index = Some(RequiredEntryIndex {
                        name: identity.leaf().clone(),
                        span: *span,
                    });
                }
            }
        }
    }

    Ok(CheckedEntryInterface {
        parameters,
        outputs,
        required_index,
    })
}
