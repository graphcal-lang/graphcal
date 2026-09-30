//! Conversions from HIR lowering diagnostics to spanned [`GraphcalError`]s.

use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{TypeExpr, TypeExprKind};
use crate::graphcal_error::GraphcalError;
use crate::hir::expr_lower::error::ExprLowerError;
use crate::hir::lower::{HirLowerError, TypePathSlot};
use crate::resolve::category::SymbolTable;
use crate::resolve::error::{ModuleResolveError, NameCategory};
use crate::syntax::index_name::IndexName;
use crate::syntax::names::NamePath;

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
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
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

/// Convert a failure to lower a declaration type annotation.
///
/// An unknown path in an index axis or a dimension term is reported as the
/// missing index or dimension; its slot says which namespace was searched.
pub fn type_lower_error_to_graphcal(
    err: &HirLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let HirLowerError::UnknownTypePath { path, slot, span } = err
        && let Some(atom) = path.as_bare()
    {
        return match slot {
            TypePathSlot::IndexAxis => GraphcalError::UnknownIndex {
                name: IndexName::classify(atom.clone()).into(),
                src: src.clone(),
                span: (*span).into(),
            },
            TypePathSlot::DimensionTerm => GraphcalError::UnknownDimension {
                name: NamePath::local(atom.clone()),
                src: src.clone(),
                span: (*span).into(),
            },
        };
    }
    hir_lower_error_to_graphcal(err, src)
}

/// Convert a HIR expression-lowering failure into a spanned diagnostic.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive mapping from lowering diagnostics to spanned errors"
)]
#[must_use]
pub fn expr_lower_error_to_graphcal(
    err: &ExprLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    match err {
        ExprLowerError::UnknownFunction { path, span } => {
            return GraphcalError::UnknownFunction {
                name: path.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::UnknownExternFunction { alias, name, span } => {
            return GraphcalError::UnknownExternFunction {
                alias: alias.clone(),
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::NamedArgumentsOnFunction {
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
        ExprLowerError::WrongArity {
            name,
            expected,
            got,
            span,
        } => {
            return GraphcalError::WrongArity {
                name: crate::graphcal_error::CalledFunction::Builtin(*name),
                expected: *expected,
                got: *got,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::InvalidStaticBindingValue { name, span } => {
            return GraphcalError::InvalidTypeLevelBindingValue {
                name: name.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::UnknownLocalRef { name, span } => {
            return GraphcalError::UnknownLocalRef {
                name: name.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::UnknownGraphRef { name, span } => {
            return GraphcalError::UnknownGraphRef {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::BareGraphDeclarationRef { name, kind, span } => {
            return GraphcalError::BareGraphDeclarationRef {
                name: name.clone(),
                kind: *kind,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::TimeScaleInValuePosition { scale, span } => {
            return GraphcalError::TimeScaleInValuePosition {
                scale: *scale,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::UnknownUnit { name, span } => {
            return GraphcalError::UnknownUnit {
                name: name.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::InvalidTimezone {
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
        ExprLowerError::InvalidDatetimeLiteral {
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
        ExprLowerError::NonexistentCivilDateTime {
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
        ExprLowerError::RepeatedCivilDateTime {
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
        ExprLowerError::TimeZoneRegistryInvariant {
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
        ExprLowerError::EpochTimeScaleArgumentCount { got, span } => {
            return GraphcalError::EpochTimeScaleArgumentCount {
                got: *got,
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::InvalidEpochTimeScaleArgument { span } => {
            return GraphcalError::InvalidEpochTimeScaleArgument {
                expected: crate::semantic::time_scale::TimeScale::expected_names(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::UnsupportedEpochTimeScale { name, span } => {
            return GraphcalError::UnsupportedEpochTimeScale {
                name: name.clone(),
                expected: crate::semantic::time_scale::TimeScale::expected_names(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ExtraMapVariant {
            index_name,
            variant_name,
            span,
        } => {
            return GraphcalError::ExtraVariants {
                index_name: index_name.clone().into(),
                extra: vec![crate::syntax::index_name::IndexEntryKey::named(
                    variant_name.clone(),
                )],
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownModuleAlias { alias, .. },
            span,
        } => {
            return GraphcalError::UnknownModule {
                name: alias.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnexpectedDeclKind {
                    name,
                    actual: crate::resolve::category::DeclSymbolKind::Assert,
                    ..
                },
            span,
        } => {
            return GraphcalError::GraphRefToAssert {
                name: name.to_unowned_def_name(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::PrivateName { owner, name, .. },
            span,
        } => {
            return GraphcalError::ImportPrivateItem {
                name: name.to_string(),
                file_path: owner.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownIndexVariant { index, variant },
            span,
        } => {
            return GraphcalError::UnknownVariant {
                index_name: index.to_unowned_def_name().into(),
                variant_name: variant.clone(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    category: NameCategory::Table(SymbolTable::Index),
                    name,
                    ..
                },
            span,
        } => {
            return GraphcalError::UnknownIndex {
                name: IndexName::classify(name.clone()).into(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    category: NameCategory::Table(SymbolTable::Decl),
                    name,
                    ..
                },
            span,
        } => {
            return GraphcalError::UnknownLocalRef {
                name: name.to_string(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        ExprLowerError::EmptyParenthesizedConstructor { constructor, span } => {
            return GraphcalError::EmptyParenthesizedConstructor {
                constructor: constructor.to_unowned_def_name(),
                src: src.clone(),
                span: (*span).into(),
            };
        }
        _ => {}
    }
    let span = match err {
        ExprLowerError::Type(err) => return hir_lower_error_to_graphcal(err, src),
        ExprLowerError::ModuleResolve { span, .. }
        | ExprLowerError::InvalidStaticBindingValue { span, .. }
        | ExprLowerError::UnknownLocalRef { span, .. }
        | ExprLowerError::UnknownGraphRef { span, .. }
        | ExprLowerError::BareGraphDeclarationRef { span, .. }
        | ExprLowerError::TimeScaleInValuePosition { span, .. }
        | ExprLowerError::UnknownUnit { span, .. }
        | ExprLowerError::TooManyLocals { span }
        | ExprLowerError::ExpressionIdentity { span, .. }
        | ExprLowerError::EmptyMapEntry { span }
        | ExprLowerError::ExtraMapVariant { span, .. }
        | ExprLowerError::UnknownPattern { span, .. }
        | ExprLowerError::UnknownFunction { span, .. }
        | ExprLowerError::UnknownExternFunction { span, .. }
        | ExprLowerError::NamedArgumentsOnFunction { span, .. }
        | ExprLowerError::PositionalArgumentsOnConstructor { span, .. }
        | ExprLowerError::EmptyParenthesizedConstructor { span, .. }
        | ExprLowerError::UnsupportedFunctionGenericArgs { span, .. }
        | ExprLowerError::WrongArity { span, .. }
        | ExprLowerError::InvalidTimezone { span, .. }
        | ExprLowerError::EpochTimeScaleArgumentCount { span, .. }
        | ExprLowerError::InvalidEpochTimeScaleArgument { span }
        | ExprLowerError::UnsupportedEpochTimeScale { span, .. }
        | ExprLowerError::InvalidDatetimeLiteral { span, .. }
        | ExprLowerError::TimeZoneRegistryInvariant { span, .. } => *span,
        ExprLowerError::NonexistentCivilDateTime { datetime_span, .. }
        | ExprLowerError::RepeatedCivilDateTime { datetime_span, .. } => *datetime_span,
        ExprLowerError::DuplicateLocalBinding { duplicate, .. }
        | ExprLowerError::LocalBindingShadowsTerm { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

/// Convert a HIR type-lowering failure into a spanned diagnostic.
pub fn hir_lower_error_to_graphcal(
    err: &HirLowerError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    if let HirLowerError::ExpectedIndexFoundNat { expression, span } = err {
        return GraphcalError::ExpectedIndexFoundNat {
            expression: expression.clone(),
            src: src.clone(),
            span: (*span).into(),
        };
    }
    let span = match &err {
        HirLowerError::ModuleResolve { span, .. }
        | HirLowerError::UnknownTypePath { span, .. }
        | HirLowerError::IndexLabelAsType { span, .. }
        | HirLowerError::NestedIndexedType { span }
        | HirLowerError::GenericConstraintMismatch { span, .. }
        | HirLowerError::ExpectedIndexFoundNat { span, .. }
        | HirLowerError::UnknownGenericParam { span, .. }
        | HirLowerError::NatOverflow { span, .. }
        | HirLowerError::WrongGenericArgCount { span, .. }
        | HirLowerError::GenericArgumentSortMismatch { span, .. }
        | HirLowerError::ExpectedTimeScale { span }
        | HirLowerError::UnknownTimeScale { span, .. }
        | HirLowerError::WrongDatetimeArgCount { span, .. } => *span,
        HirLowerError::IndexAsType { index } => index.span(),
        HirLowerError::DuplicateGenericParam { duplicate, .. }
        | HirLowerError::GenericParamShadowsStatic { duplicate, .. } => *duplicate,
    };
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::names::NameAtom;
    use crate::syntax::non_empty::NonEmpty;
    use crate::syntax::span::Span;

    fn unknown(path: NamePath, slot: TypePathSlot) -> GraphcalError {
        let src = NamedSource::new("main.gcl", Arc::new(String::new()));
        let error = HirLowerError::UnknownTypePath {
            path,
            slot,
            span: Span::new(2, 3),
        };
        type_lower_error_to_graphcal(&error, &src)
    }

    fn atom(name: &str) -> NameAtom {
        NameAtom::try_from(name).unwrap()
    }

    #[test]
    fn unknown_type_path_is_reported_by_its_slot() {
        let path = NamePath::local(atom("Foo"));
        assert!(matches!(
            unknown(path.clone(), TypePathSlot::IndexAxis),
            GraphcalError::UnknownIndex { name, .. } if name.to_string() == "Foo"
        ));
        assert!(matches!(
            unknown(path.clone(), TypePathSlot::DimensionTerm),
            GraphcalError::UnknownDimension { name, .. } if name == path
        ));
    }

    #[test]
    fn qualified_unknown_type_path_keeps_the_generic_diagnostic() {
        let path = NamePath::qualified(NonEmpty::singleton(atom("lib")), atom("Foo"));
        for slot in [TypePathSlot::IndexAxis, TypePathSlot::DimensionTerm] {
            assert!(matches!(
                unknown(path.clone(), slot),
                GraphcalError::EvalError { message, .. }
                    if message == "unknown type-level name `lib::Foo`"
            ));
        }
    }
}
