//! Importer-side values of type-level include bindings (`index I: Target`,
//! `type T: Target`, `dim D: Target`), read from their parsed values.

use graphcal_compiler::desugar::desugared_ast::{BindingValue, IndexExpr};
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::static_interface::StaticInputKind;
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::span::Span;

use crate::compile_error::PipelineError;

/// Resolve the importer-side argument of an index-port binding.
///
/// The source expression crosses into the typed include core as either a
/// declared index name or a validated structural finite identity.
pub(super) fn extract_index_binding_target(
    index: &IndexExpr,
    span: Span,
    dep_index_name: &IndexName,
    file_src: SourceId,
) -> Result<IndexBindingTarget, PipelineError> {
    use graphcal_compiler::semantic::index_def::FiniteIndex;

    let invalid_binding = || {
        PipelineError::Semantic(SemanticError::located(
            file_src,
            span,
            ModuleError::InvalidTypeLevelBindingValue {
                kind: StaticInputKind::Index,
                name: dep_index_name.atom().clone(),
            },
        ))
    };
    match index {
        IndexExpr::Name(path) => path
            .value
            .as_bare()
            .map(|name| IndexBindingTarget::Declared(IndexName::classify(name.clone())))
            .ok_or_else(invalid_binding),
        IndexExpr::Finite { cardinality, .. } => {
            let cardinality = closed_binding_cardinality(cardinality, file_src)?;
            let finite = FiniteIndex::try_from_u64(cardinality).map_err(|error| {
                PipelineError::Semantic(SemanticError::located(
                    file_src,
                    span,
                    IndexError::InvalidFiniteIndexCardinality { error },
                ))
            })?;
            Ok(IndexBindingTarget::Finite(finite))
        }
        IndexExpr::BareNat(_) => Err(invalid_binding()),
    }
}

/// Evaluate the cardinality of an importer-side `Fin(...)` binding value.
///
/// An include binding has no generic scope, so a name here is an unknown
/// index and the expression must be closed.
fn closed_binding_cardinality(
    expr: &graphcal_compiler::desugar::desugared_ast::NatExpr,
    file_src: SourceId,
) -> Result<u64, SemanticError> {
    use graphcal_compiler::syntax::nat_eval::ClosedNat;
    match expr.closed_value() {
        Ok(ClosedNat::Value(value)) => Ok(value),
        Ok(ClosedNat::Open { first_var }) => Err(SemanticError::located(
            file_src,
            first_var.span,
            IndexError::UnknownIndex {
                name: IndexName::classify(first_var.name.atom().clone()).into(),
            },
        )),
        Err(overflow) => Err(SemanticError::located(
            file_src,
            overflow.span,
            IndexError::NatOverflow {
                error: graphcal_compiler::nat::NatOverflowError,
            },
        )),
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
) -> Result<&'v NameAtom, PipelineError> {
    value
        .static_name()
        .and_then(|name| name.value.as_bare())
        .ok_or_else(|| {
            PipelineError::Semantic(SemanticError::located(
                file_src,
                value.span(),
                ModuleError::InvalidTypeLevelBindingValue {
                    kind,
                    name: dep_name.clone(),
                },
            ))
        })
}
