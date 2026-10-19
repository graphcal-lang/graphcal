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
use graphcal_compiler::syntax::module_name::ScopedName;
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

/// One checked entry-DAG output in source order: a node the entry DAG
/// declares, or a public node member of one of its whole-instance includes
/// (`include lib(..) as l` exposes `l::x`), at the include's position.
#[derive(Debug)]
pub struct CheckedEntryOutput {
    name: ScopedName,
    declared_type: CheckedType,
    visibility: Visibility,
    runtime_key: ResolvedDeclName,
}

impl CheckedEntryOutput {
    /// The name the entry source reads the output by: local for a declared
    /// node, qualified by the include alias for an instance member.
    pub const fn name(&self) -> &ScopedName {
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

/// The public node members of the entry DAG's whole-instance includes, each
/// with the source offset of its include, in source order.
///
/// A whole-instance include (`include lib(..) as l`) lets the entry source
/// read every node `lib` exports, including its `pub` re-exports, as `l::x`.
/// Params and consts of the instance are not outputs, as they are not for
/// the entry DAG itself. A selective include has no source-visible scope;
/// its `pub` items are entry nodes of their own.
fn instance_member_outputs(
    tir: &CheckedTir,
    source: SourceId,
) -> Result<Vec<(usize, CheckedEntryOutput)>, PipelineError> {
    let mut members = Vec::new();
    for record in tir.root().semantic_instances() {
        let scope = record.instance.id().scope();
        if scope.alias().is_none() {
            continue;
        }
        let Some(instance) = tir.dag_registry().semantic_instance(record) else {
            return Err(missing_interface_fact_internal_error(
                format!(
                    "include instance `{}` is absent from checked TIR",
                    record.instance.id().owner()
                ),
                source,
                record.include_span,
            ));
        };
        let dag = instance.dag();
        let mut nodes = dag
            .declarations()
            .filter(|declaration| dag.projectable_outputs().contains(declaration.name()))
            .filter_map(|declaration| {
                declaration
                    .value()
                    .filter(|value| value.category == ValueDeclCategory::Node)
                    .map(|value| (declaration, value))
            })
            .collect::<Vec<_>>();
        // Spans are in the template's source, the order its author wrote.
        nodes.sort_by_key(|(_, value)| value.span.offset());
        members.extend(nodes.into_iter().map(|(declaration, value)| {
            (
                record.include_span.offset(),
                CheckedEntryOutput {
                    name: ScopedName::in_scope(scope.clone(), declaration.name().clone()),
                    declared_type: value.annotation.checked().declared().clone(),
                    visibility: Visibility::Public,
                    runtime_key: declaration.identity().clone(),
                },
            )
        }));
    }
    members.sort_by_key(|(offset, _)| *offset);
    Ok(members)
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
    let mut members = instance_member_outputs(tir, source)?.into_iter().peekable();

    for declaration in source_declarations {
        while let Some((_, member)) =
            members.next_if(|(offset, _)| *offset <= declaration.span().offset())
        {
            outputs.push(member);
        }
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
                    name: ScopedName::local(name.clone()),
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
    outputs.extend(members.map(|(_, member)| member));

    Ok(CheckedEntryInterface {
        parameters,
        outputs,
        required_index,
    })
}
