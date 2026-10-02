//! Importer-side values of type-level include bindings (`Index: Target`,
//! `Type: Target`), read from their desugared source expressions.

use graphcal_compiler::desugar::desugared_ast::{Expr, ExprKind};
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::index::IndexError;
use graphcal_compiler::semantic_error::module::ModuleError;
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::index_name::IndexName;

use crate::compile_error::PipelineError;

/// Resolve the importer-side argument of an index-port binding.
///
/// The source expression crosses into the typed include core as either a
/// declared index name or a validated structural finite identity.
pub(super) fn extract_index_binding_target(
    expr: &Expr,
    dep_index_name: &IndexName,
    file_src: SourceId,
) -> Result<IndexBindingTarget, PipelineError> {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::semantic::index_def::FiniteIndex;

    let invalid_binding = || {
        PipelineError::Semantic(SemanticError::located(
            file_src,
            expr.span,
            ModuleError::InvalidTypeLevelBindingValue {
                name: dep_index_name.atom().clone(),
            },
        ))
    };
    match expr.index_binding_arg().ok_or_else(invalid_binding)? {
        IndexExpr::Name(path) => path
            .value
            .as_bare()
            .map(|name| IndexBindingTarget::Declared(IndexName::classify(name.clone())))
            .ok_or_else(invalid_binding),
        IndexExpr::Finite { cardinality, .. } => {
            let cardinality = closed_binding_cardinality(&cardinality, file_src)?;
            let finite = FiniteIndex::try_from_u64(cardinality).map_err(|error| {
                PipelineError::Semantic(SemanticError::located(
                    file_src,
                    expr.span,
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

/// Extract a `PascalCase` type name from a binding expression.
///
/// Type bindings use the form `DepType: ImporterType` — the RHS is a bare
/// `PascalCase` identifier (an unresolved reference path in the desugared
/// AST) or a zero-arg `ConstructorCall` for constructor-shaped RHSs.
pub(super) fn extract_type_name_from_binding_expr(
    expr: &Expr,
    dep_type_name: &graphcal_compiler::syntax::names::NameAtom,
    file_src: SourceId,
) -> Result<String, PipelineError> {
    let invalid_binding = || {
        PipelineError::Semantic(SemanticError::located(
            file_src,
            expr.span,
            ModuleError::InvalidTypeLevelBindingValue {
                name: dep_type_name.clone(),
            },
        ))
    };
    match &expr.kind {
        ExprKind::UnresolvedRef(graphcal_compiler::syntax::ast::UnresolvedRef::Path(path)) => path
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(invalid_binding),
        ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } if generic_args.is_empty() && fields.is_empty() => callee
            .as_bare()
            .map(|ident| ident.name.to_string())
            .ok_or_else(invalid_binding),
        _ => Err(invalid_binding()),
    }
}
