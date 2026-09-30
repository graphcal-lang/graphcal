//! Checked runtime interface of one directly authored entry DAG.

use std::sync::Arc;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::hir::SourceDeclaration;
use graphcal_compiler::ir::resolve::collected::ExternalDeclSurface;
use graphcal_compiler::semantic::checked_type::CheckedType;
use graphcal_compiler::syntax::ast::Visibility;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::CheckedTir;
use miette::NamedSource;

use crate::compile_error::CompileError;
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
    anchor: DiagnosticAnchor,
}

impl RequiredEntryIndex {
    pub const fn name(&self) -> &IndexName {
        &self.name
    }

    pub const fn anchor(&self) -> DiagnosticAnchor {
        self.anchor
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

fn missing_interface_fact(
    message: String,
    source: &NamedSource<Arc<String>>,
    span: Span,
) -> CompileError {
    CompileError::Eval(GraphcalError::InternalError {
        message,
        src: source.clone(),
        span: span.into(),
    })
}

/// Attach checked types and runtime identities to HIR source-interface records.
pub(super) fn build_checked_entry_interface(
    source_declarations: &[SourceDeclaration],
    tir: &CheckedTir,
    external_surface: &ExternalDeclSurface,
    source: &NamedSource<Arc<String>>,
) -> Result<CheckedEntryInterface, CompileError> {
    let mut parameters = Vec::new();
    let mut outputs = Vec::new();
    let mut required_index = None;

    for declaration in source_declarations {
        match declaration {
            SourceDeclaration::Parameter { name, span } => {
                let entry = tir
                    .root()
                    .params()
                    .find(|entry| entry.name() == name)
                    .ok_or_else(|| {
                        missing_interface_fact(
                            format!("HIR entry parameter `{name}` is absent from checked TIR"),
                            source,
                            *span,
                        )
                    })?;
                parameters.push(CheckedEntryParameter {
                    name: name.clone(),
                    declared_type: entry.type_ann.checked().declared().clone(),
                    has_default: entry.default.is_some(),
                    runtime_key: entry.identity(),
                    span: *span,
                });
            }
            SourceDeclaration::Node { name, span } => {
                let entry = tir
                    .root()
                    .nodes()
                    .find(|entry| entry.name() == name)
                    .ok_or_else(|| {
                        missing_interface_fact(
                            format!("HIR entry node `{name}` is absent from checked TIR"),
                            source,
                            *span,
                        )
                    })?;
                outputs.push(CheckedEntryOutput {
                    name: name.clone(),
                    declared_type: entry.type_ann.checked().declared().clone(),
                    visibility: if external_surface.is_explicit_export(name) {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    runtime_key: entry.identity(),
                });
            }
            SourceDeclaration::Index { name, span } => {
                let definition = tir
                    .root_declared_indexes()
                    .find(|definition| definition.name.declared_name() == Some(name))
                    .ok_or_else(|| {
                        missing_interface_fact(
                            format!("HIR entry index `{name}` is absent from checked TIR"),
                            source,
                            *span,
                        )
                    })?;
                if required_index.is_none() && definition.is_required() {
                    required_index = Some(RequiredEntryIndex {
                        name: name.clone(),
                        anchor: DiagnosticAnchor::Source(*span),
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
