//! Checked runtime interface of one directly authored entry DAG.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::hir::SourceDeclaration;
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::syntax::ast::Visibility;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::tir::typed::TIR;
use miette::NamedSource;

use crate::decl_key::RuntimeDeclKey;
use crate::eval::types::CompileError;

/// One checked entry-DAG parameter in direct source order.
#[derive(Debug)]
pub struct CheckedEntryParameter {
    name: DeclName,
    declared_type: DeclaredType,
    has_default: bool,
    runtime_key: RuntimeDeclKey,
    span: Span,
}

impl CheckedEntryParameter {
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }

    pub const fn has_default(&self) -> bool {
        self.has_default
    }

    pub const fn runtime_key(&self) -> &RuntimeDeclKey {
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
    declared_type: DeclaredType,
    visibility: Visibility,
    runtime_key: RuntimeDeclKey,
}

impl CheckedEntryOutput {
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    pub const fn declared_type(&self) -> &DeclaredType {
        &self.declared_type
    }

    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    pub const fn runtime_key(&self) -> &RuntimeDeclKey {
        &self.runtime_key
    }
}

/// The first unresolved required index that prevents runtime preparation.
#[derive(Debug)]
pub struct RequiredEntryIndex {
    name: IndexName,
    span: Span,
}

impl RequiredEntryIndex {
    pub const fn name(&self) -> &IndexName {
        &self.name
    }

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
    tir: &TIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    external_surface: &ExternalDeclSurface,
    source: &NamedSource<Arc<String>>,
) -> Result<CheckedEntryInterface, CompileError> {
    let mut parameters = Vec::new();
    let mut outputs = Vec::new();
    let mut required_index = None;

    for declaration in source_declarations {
        match declaration {
            SourceDeclaration::Parameter { name, span } => {
                let scoped = ScopedName::from(name);
                let entry = tir
                    .root()
                    .params()
                    .iter()
                    .find(|entry| entry.name == scoped)
                    .ok_or_else(|| {
                        missing_interface_fact(
                            format!("HIR entry parameter `{name}` is absent from checked TIR"),
                            source,
                            *span,
                        )
                    })?;
                let declared_type = declared_types.get(&scoped).cloned().ok_or_else(|| {
                    missing_interface_fact(
                        format!("declared type missing for entry parameter `{name}`"),
                        source,
                        *span,
                    )
                })?;
                parameters.push(CheckedEntryParameter {
                    name: name.clone(),
                    declared_type,
                    has_default: entry.default_expr.is_some(),
                    runtime_key: RuntimeDeclKey::for_local_decl(tir.root(), &scoped),
                    span: *span,
                });
            }
            SourceDeclaration::Node { name, span } => {
                let scoped = ScopedName::from(name);
                if !tir.root().nodes().iter().any(|entry| entry.name == scoped) {
                    return Err(missing_interface_fact(
                        format!("HIR entry node `{name}` is absent from checked TIR"),
                        source,
                        *span,
                    ));
                }
                let declared_type = declared_types.get(&scoped).cloned().ok_or_else(|| {
                    missing_interface_fact(
                        format!("declared type missing for entry node `{name}`"),
                        source,
                        *span,
                    )
                })?;
                outputs.push(CheckedEntryOutput {
                    name: name.clone(),
                    declared_type,
                    visibility: if external_surface.is_explicit_export(name) {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    runtime_key: RuntimeDeclKey::for_local_decl(tir.root(), &scoped),
                });
            }
            SourceDeclaration::Index { name, span } => {
                let definition = tir
                    .root_declared_indexes()
                    .find(|definition| definition.name == *name)
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
                        span: *span,
                    });
                }
            }
        }
    }

    if required_index.is_none() {
        required_index = tir
            .root()
            .semantic()
            .collection_refs
            .index_defs
            .values()
            .filter(|definition| definition.is_required())
            .min_by(|left, right| left.name.cmp(&right.name))
            .map(|definition| RequiredEntryIndex {
                name: definition.name.clone(),
                span: Span::new(0, 0),
            });
    }

    Ok(CheckedEntryInterface {
        parameters,
        outputs,
        required_index,
    })
}
