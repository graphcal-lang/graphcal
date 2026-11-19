//! Importer-side values of type-level include bindings (`index I: Target`,
//! `type T: Target`, `dim D: Target`), read from their parsed values.

use graphcal_compiler::desugar::desugared_ast::{BindingValue, IndexExpr};
use graphcal_compiler::finite_index_binding::finite_index_binding;
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::static_interface::StaticInputKind;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;

/// Resolve the importer-side argument of an index-port binding.
///
/// The source expression crosses into the typed include core as either a
/// declared index name or a validated structural finite identity.
pub(super) fn extract_index_binding_target(
    index: &IndexExpr,
    span: Span,
    dep_index_name: &IndexName,
    file_src: SourceId,
) -> Result<IndexBindingTarget, SemanticError> {
    let invalid_binding = || {
        SemanticError::located(
            file_src,
            span,
            ModuleError::InvalidTypeLevelBindingValue {
                kind: StaticInputKind::Index,
                name: dep_index_name.atom().clone(),
            },
        )
    };
    match index {
        IndexExpr::Name(path) => path
            .value
            .as_bare()
            .map(|name| IndexBindingTarget::Declared(IndexName::classify(name.clone())))
            .ok_or_else(invalid_binding),
        IndexExpr::Finite { cardinality, .. } => finite_index_binding(cardinality, span)
            .map(IndexBindingTarget::Finite)
            .map_err(|error| SemanticError::located(file_src, error.span, error.index_error())),
        IndexExpr::BareNat(_) => Err(invalid_binding()),
    }
}

/// The importer-side static name a `type` or `dim` binding names.
///
/// An include binding names a declared type or dimension of the importer by
/// a bare name; anything else (an applied type, a dimension product, a
/// qualified path) is not a bindable value here.
pub(super) fn static_binding_name<'v>(
    value: &'v BindingValue,
    kind: StaticInputKind,
    dep_name: &NameAtom,
    file_src: SourceId,
) -> Result<&'v NameAtom, SemanticError> {
    value
        .static_name()
        .and_then(|name| name.value.as_bare())
        .ok_or_else(|| {
            SemanticError::located(
                file_src,
                value.span(),
                ModuleError::InvalidTypeLevelBindingValue {
                    kind,
                    name: dep_name.clone(),
                },
            )
        })
}
