//! Conversions from HIR lowering diagnostics to spanned [`GraphcalError`]s.

use crate::syntax::decl_name::DeclNameNamespace;
use crate::syntax::index_name::IndexNameNamespace;
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{TypeExpr, TypeExprKind};
use crate::hir;
use crate::registry::error::GraphcalError;
use crate::syntax::dimension::DimName;
use crate::syntax::index_name::IndexName;
use crate::syntax::module_resolve::ModuleResolveError;
use crate::syntax::names::{NameNamespace, NamePath};
use crate::syntax::span::Span;

/// Reject source-only type syntax that has no valid HIR representation.
pub fn validate_type_annotation(
    type_expr: &TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &type_expr.kind {
        TypeExprKind::Indexed { base, .. } => validate_type_annotation(base, src),
        TypeExprKind::TypeApplication { generic_args, .. } => {
            validate_generic_args(generic_args.iter(), src)
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            validate_generic_args(generic_args.iter(), src)
        }
        TypeExprKind::DatetimeApplication { type_args } => type_args.iter().try_for_each(|arg| {
            if let Some(bound) = arg.constraints.first() {
                return Err(GraphcalError::GenericTypeArgDomainConstraint {
                    src: src.clone(),
                    span: bound.span.into(),
                });
            }
            validate_type_annotation(arg, src)
        }),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => Ok(()),
    }
}

fn validate_generic_args<'a>(
    generic_args: impl IntoIterator<Item = &'a crate::desugar::desugared_ast::GenericArg>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    generic_args.into_iter().try_for_each(|arg| match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            if let Some(bound) = type_expr.constraints.first() {
                return Err(GraphcalError::GenericTypeArgDomainConstraint {
                    src: src.clone(),
                    span: bound.span.into(),
                });
            }
            validate_type_annotation(type_expr, src)
        }
        crate::desugar::desugared_ast::GenericArg::Index(_)
        | crate::desugar::desugared_ast::GenericArg::Nat(_)
        | crate::desugar::desugared_ast::GenericArg::Ambiguous(_) => Ok(()),
    })
}

/// Convert a HIR type-lowering failure while preserving source-category
/// diagnostics that are only knowable at the syntax-to-HIR boundary.
pub fn type_lower_error_to_graphcal(
    err: &hir::HirLowerError,
    type_ann: &TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let hir::HirLowerError::UnknownTypePath { path, span } = err {
        if type_expr_has_index_name_at_span(type_ann, *span)
            && let Ok(name) = IndexName::try_new(path.clone())
        {
            return GraphcalError::UnknownIndex {
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        if type_expr_has_dim_term_at_span(type_ann, *span)
            && let Ok(name) = DimName::try_new(path.clone())
        {
            return GraphcalError::UnknownDimension {
                name: NamePath::from(name.into_atom()),
                src: src.clone(),
                span: (*span).into(),
            };
        }
    }
    hir_lower_error_to_graphcal(err, src)
}

fn generic_args_have_index_name_at_span<'a>(
    generic_args: impl IntoIterator<Item = &'a crate::desugar::desugared_ast::GenericArg>,
    span: Span,
) -> bool {
    generic_args.into_iter().any(|arg| {
        matches!(
            arg,
            crate::desugar::desugared_ast::GenericArg::Type(type_expr)
                if type_expr_has_index_name_at_span(type_expr, span)
        )
    })
}

fn generic_args_have_dim_term_at_span<'a>(
    generic_args: impl IntoIterator<Item = &'a crate::desugar::desugared_ast::GenericArg>,
    span: Span,
) -> bool {
    generic_args.into_iter().any(|arg| {
        matches!(
            arg,
            crate::desugar::desugared_ast::GenericArg::Type(type_expr)
                if type_expr_has_dim_term_at_span(type_expr, span)
        ) || matches!(
            arg,
            crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous)
                if ambiguous.span() == span
        )
    })
}

fn type_expr_has_index_name_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::Indexed { base, indexes } => {
            type_expr_has_index_name_at_span(base, span)
                || indexes.iter().any(|index| match index {
                    crate::desugar::desugared_ast::IndexExpr::Name(name) => name.span == span,
                    crate::desugar::desugared_ast::IndexExpr::Finite { .. }
                    | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => false,
                })
        }
        TypeExprKind::TypeApplication { generic_args, .. } => {
            generic_args_have_index_name_at_span(generic_args, span)
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            generic_args_have_index_name_at_span(generic_args, span)
        }
        TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_index_name_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => false,
    }
}

fn type_expr_has_dim_term_at_span(type_ann: &TypeExpr, span: Span) -> bool {
    match &type_ann.kind {
        TypeExprKind::DimExpr(dim_expr) => dim_expr
            .terms
            .iter()
            .any(|item| item.term.name.span == span),
        TypeExprKind::Indexed { base, .. } => type_expr_has_dim_term_at_span(base, span),
        TypeExprKind::TypeApplication { generic_args, .. } => {
            generic_args_have_dim_term_at_span(generic_args, span)
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            generic_args_have_dim_term_at_span(generic_args, span)
        }
        TypeExprKind::DatetimeApplication { type_args } => type_args
            .iter()
            .any(|arg| type_expr_has_dim_term_at_span(arg, span)),
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => false,
    }
}

/// Convert a HIR expression-lowering failure into a spanned diagnostic.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive mapping from lowering diagnostics to spanned errors"
)]
#[must_use]
pub fn expr_lower_error_to_graphcal(
    err: &hir::ExprLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    match err {
        hir::ExprLowerError::UnknownFunction { path, span } => {
            return GraphcalError::UnknownFunction {
                name: path.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnknownExternFunction { alias, name, span } => {
            return GraphcalError::UnknownExternFunction {
                alias: alias.clone(),
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::NamedArgumentsOnFunction {
            function,
            argument_names,
            span,
        } => {
            let name = function.to_string();
            let positional_args = argument_names
                .iter()
                .map(|argument| format!("{argument}_value"))
                .collect::<Vec<_>>()
                .join(", ");
            return GraphcalError::NamedArgumentsOnFunction {
                positional_call: format!("{name}({positional_args})"),
                name,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::WrongArity {
            name,
            expected,
            got,
            span,
        } => {
            return GraphcalError::WrongArity {
                name: name.clone(),
                expected: *expected,
                got: *got,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnknownLocalRef { name, span } => {
            return GraphcalError::UnknownLocalRef {
                name: name.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnknownGraphRef { name, span } => {
            return GraphcalError::UnknownGraphRef {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::BareGraphDeclarationRef { name, kind, span } => {
            return GraphcalError::BareGraphDeclarationRef {
                name: name.clone(),
                kind: *kind,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnknownUnit { name, span } => {
            return GraphcalError::UnknownUnit {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::InvalidTimezone {
            timezone,
            tzdb_version,
            span,
        } => {
            return GraphcalError::InvalidTimezone {
                timezone: timezone.clone(),
                tzdb_version,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::InvalidDatetimeLiteral {
            expectation,
            reason,
            span,
        } => {
            return GraphcalError::InvalidDatetimeLiteral {
                expectation: *expectation,
                reason: reason.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::NonexistentCivilDateTime {
            datetime,
            time_zone,
            before,
            after,
            datetime_span,
            time_zone_span,
        } => {
            return GraphcalError::NonexistentCivilDateTime {
                datetime: *datetime,
                time_zone: time_zone.clone(),
                before: *before,
                after: *after,
                src: src.clone(),
                datetime_span: (*datetime_span).into(),
                time_zone_span: (*time_zone_span).into(),
            };
        }
        hir::ExprLowerError::RepeatedCivilDateTime {
            datetime,
            time_zone,
            before,
            after,
            datetime_span,
            time_zone_span,
        } => {
            return GraphcalError::RepeatedCivilDateTime {
                datetime: *datetime,
                time_zone: time_zone.clone(),
                before: *before,
                after: *after,
                src: src.clone(),
                datetime_span: (*datetime_span).into(),
                time_zone_span: (*time_zone_span).into(),
            };
        }
        hir::ExprLowerError::TimeZoneRegistryInvariant {
            time_zone,
            reason,
            span,
        } => {
            return GraphcalError::InternalError {
                message: format!("validated timezone `{time_zone}` could not be loaded: {reason}"),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::EpochTimeScaleArgumentCount { got, span } => {
            return GraphcalError::EpochTimeScaleArgumentCount {
                got: *got,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::InvalidEpochTimeScaleArgument { span } => {
            return GraphcalError::InvalidEpochTimeScaleArgument {
                expected: crate::registry::time_scale::TimeScale::expected_names(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnsupportedEpochTimeScale { name, span } => {
            return GraphcalError::UnsupportedEpochTimeScale {
                name: name.clone(),
                expected: crate::registry::time_scale::TimeScale::expected_names(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ExtraMapVariant {
            index_name,
            variant_name,
            span,
        } => {
            return GraphcalError::ExtraVariants {
                index_name: index_name.clone(),
                extra: vec![crate::syntax::index_name::IndexEntryKey::named(
                    variant_name.clone(),
                )],
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownModuleAlias { alias, .. },
            span,
        } => {
            return GraphcalError::UnknownModule {
                name: alias.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownIndexVariant { index, variant },
            span,
        } => {
            return GraphcalError::UnknownVariant {
                index_name: index.to_unowned_def_name(),
                variant_name: variant.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    namespace, name, ..
                },
            span,
        } if *namespace == IndexNameNamespace::DISPLAY_NAME => {
            if let Ok(index_name) = IndexName::try_new(name.clone()) {
                return GraphcalError::UnknownIndex {
                    name: index_name,
                    src: src.clone(),
                    span: (*span).into(),
                };
            }
        }
        hir::ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    namespace, name, ..
                },
            span,
        } if *namespace == DeclNameNamespace::DISPLAY_NAME => {
            return GraphcalError::UnknownLocalRef {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        _ => {}
    }
    let span = match err {
        hir::ExprLowerError::Type(err) => return hir_lower_error_to_graphcal(err, src),
        hir::ExprLowerError::ModuleResolve { span, .. }
        | hir::ExprLowerError::UnknownLocalRef { span, .. }
        | hir::ExprLowerError::UnknownGraphRef { span, .. }
        | hir::ExprLowerError::BareGraphDeclarationRef { span, .. }
        | hir::ExprLowerError::UnknownUnit { span, .. }
        | hir::ExprLowerError::TooManyLocals { span }
        | hir::ExprLowerError::EmptyMapEntry { span }
        | hir::ExprLowerError::InvalidMapEntryKey { span }
        | hir::ExprLowerError::ExtraMapVariant { span, .. }
        | hir::ExprLowerError::UnknownPattern { span, .. }
        | hir::ExprLowerError::UnknownFunction { span, .. }
        | hir::ExprLowerError::UnknownExternFunction { span, .. }
        | hir::ExprLowerError::NamedArgumentsOnFunction { span, .. }
        | hir::ExprLowerError::UnsupportedFunctionGenericArgs { span, .. }
        | hir::ExprLowerError::WrongArity { span, .. }
        | hir::ExprLowerError::InvalidTimezone { span, .. }
        | hir::ExprLowerError::EpochTimeScaleArgumentCount { span, .. }
        | hir::ExprLowerError::InvalidEpochTimeScaleArgument { span }
        | hir::ExprLowerError::UnsupportedEpochTimeScale { span, .. }
        | hir::ExprLowerError::InvalidDatetimeLiteral { span, .. }
        | hir::ExprLowerError::TimeZoneRegistryInvariant { span, .. } => *span,
        hir::ExprLowerError::NonexistentCivilDateTime { datetime_span, .. }
        | hir::ExprLowerError::RepeatedCivilDateTime { datetime_span, .. } => *datetime_span,
        hir::ExprLowerError::DuplicateLocalBinding { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

/// Convert a HIR type-lowering failure into a spanned diagnostic.
pub fn hir_lower_error_to_graphcal(
    err: &hir::HirLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let hir::HirLowerError::ExpectedIndexFoundNat { expression, span } = err {
        return GraphcalError::ExpectedIndexFoundNat {
            expression: expression.clone(),
            src: src.clone(),
            span: (*span).into(),
        };
    }
    let span = match &err {
        hir::HirLowerError::ModuleResolve { span, .. }
        | hir::HirLowerError::UnknownTypePath { span, .. }
        | hir::HirLowerError::GenericConstraintMismatch { span, .. }
        | hir::HirLowerError::ExpectedIndexFoundNat { span, .. }
        | hir::HirLowerError::UnknownGenericParam { span, .. }
        | hir::HirLowerError::WrongGenericArgCount { span, .. }
        | hir::HirLowerError::GenericArgumentSortMismatch { span, .. }
        | hir::HirLowerError::ExpectedTimeScale { span }
        | hir::HirLowerError::UnknownTimeScale { span, .. }
        | hir::HirLowerError::WrongDatetimeArgCount { span, .. } => *span,
        hir::HirLowerError::DuplicateGenericParam { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}
