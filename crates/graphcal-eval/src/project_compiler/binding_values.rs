//! Importer-side values of type-level include bindings (`Index: Target`,
//! `Type: Target`), read from their desugared source expressions.

use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::{Expr, ExprKind};
use graphcal_compiler::graphcal_error::GraphcalError;
use graphcal_compiler::semantic::index_def::IndexBindingTarget;
use graphcal_compiler::syntax::index_name::IndexName;

use crate::eval::types::CompileError;

/// Resolve the importer-side argument of an index-port binding.
///
/// The source expression crosses into the typed include core as either a
/// declared index name or a validated structural finite identity.
pub(super) fn extract_index_binding_target(
    expr: &Expr,
    dep_index_name: &IndexName,
    file_src: &NamedSource<Arc<String>>,
) -> Result<IndexBindingTarget, CompileError> {
    use graphcal_compiler::desugar::desugared_ast::IndexExpr;
    use graphcal_compiler::semantic::index_def::FiniteIndex;

    let invalid_binding = || {
        CompileError::Eval(GraphcalError::InvalidTypeLevelBindingValue {
            name: dep_index_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })
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
                CompileError::Eval(GraphcalError::EvalError {
                    message: error.describe_finite_index(),
                    src: file_src.clone(),
                    span: expr.span.into(),
                })
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
    file_src: &NamedSource<Arc<String>>,
) -> Result<u64, GraphcalError> {
    use graphcal_compiler::desugar::desugared_ast::NatExpr;
    let overflow = |span: graphcal_compiler::syntax::span::Span| GraphcalError::EvalError {
        message: graphcal_compiler::nat::NatOverflowError.to_string(),
        src: file_src.clone(),
        span: span.into(),
    };
    match expr {
        NatExpr::Literal(value, _) => Ok(*value),
        NatExpr::Var(ident) => Err(GraphcalError::UnknownIndex {
            name: IndexName::classify(ident.name.atom().clone()).into(),
            src: file_src.clone(),
            span: ident.span.into(),
        }),
        NatExpr::Add(operands, span) => operands.iter().try_fold(0_u64, |sum, operand| {
            sum.checked_add(closed_binding_cardinality(operand, file_src)?)
                .ok_or_else(|| overflow(*span))
        }),
        NatExpr::Mul(operands, span) => operands.iter().try_fold(1_u64, |product, operand| {
            product
                .checked_mul(closed_binding_cardinality(operand, file_src)?)
                .ok_or_else(|| overflow(*span))
        }),
    }
}

/// Extract a `PascalCase` type name from a binding expression.
///
/// Type bindings use the form `DepType: ImporterType` — the RHS is a bare
/// `PascalCase` identifier (an unresolved reference path in the desugared
/// AST) or a zero-arg `ConstructorCall` for constructor-shaped RHSs.
pub(super) fn extract_type_name_from_binding_expr(
    expr: &Expr,
    dep_type_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    let invalid_binding = || {
        CompileError::Eval(GraphcalError::InvalidTypeLevelBindingValue {
            name: dep_type_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })
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
