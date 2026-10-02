//! Conversions from HIR lowering diagnostics to spanned [`SemanticError`]s.

use crate::desugar::desugared_ast::{TypeExpr, TypeExprKind};
use crate::hir::expr_lower::error::ExprLowerError;
use crate::hir::lower::{HirLowerError, TypePathSlot};
use crate::resolve::category::SymbolTable;
use crate::resolve::error::{ModuleResolveError, NameCategory};
use crate::semantic_error::SemanticError;
use crate::semantic_error::attribute::AttributeError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::domain::DomainError;
use crate::semantic_error::graph::DagReference;
use crate::semantic_error::index::IndexError;
use crate::semantic_error::module::ModuleError;
use crate::semantic_error::name::NameError;
use crate::semantic_error::plugin::PluginError;
use crate::semantic_error::structure::StructError;
use crate::semantic_error::visibility::VisibilityError;
use crate::source_id::SourceId;
use crate::syntax::index_name::IndexName;
use crate::syntax::names::NamePath;

/// Reject source-only type syntax that has no valid HIR representation.
pub fn validate_type_annotation(type_expr: &TypeExpr, src: SourceId) -> Result<(), SemanticError> {
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
                return Err(SemanticError::located(
                    src,
                    bound.span,
                    DomainError::GenericTypeArgDomainConstraint,
                ));
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
    src: SourceId,
) -> Result<(), SemanticError> {
    generic_args.into_iter().try_for_each(|arg| match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            if let Some(bound) = type_expr.constraints.first() {
                return Err(SemanticError::located(
                    src,
                    bound.span,
                    DomainError::GenericTypeArgDomainConstraint,
                ));
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
pub fn type_lower_error_to_graphcal(err: &HirLowerError, src: SourceId) -> SemanticError {
    if let HirLowerError::UnknownTypePath { path, slot, span } = err
        && let Some(atom) = path.as_bare()
    {
        return match slot {
            TypePathSlot::IndexAxis => SemanticError::located(
                src,
                *span,
                IndexError::UnknownIndex {
                    name: IndexName::classify(atom.clone()).into(),
                },
            ),
            TypePathSlot::DimensionTerm => SemanticError::located(
                src,
                *span,
                DimensionError::UnknownDimension {
                    name: NamePath::local(atom.clone()),
                },
            ),
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
pub fn expr_lower_error_to_semantic(err: &ExprLowerError, src: SourceId) -> SemanticError {
    match err {
        ExprLowerError::UnknownFunction { path, span } => SemanticError::located(
            src,
            *span,
            NameError::UnknownFunction { name: path.clone() },
        ),
        ExprLowerError::UnknownExternFunction { alias, name, span } => SemanticError::located(
            src,
            *span,
            PluginError::UnknownExternFunction {
                alias: alias.clone(),
                name: name.clone(),
            },
        ),
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
            SemanticError::located(
                src,
                *span,
                NameError::NamedArgumentsOnFunction {
                    positional_call: format!("{name}({positional_args})"),
                    name,
                },
            )
        }
        ExprLowerError::WrongArity {
            name,
            expected,
            got,
            span,
        } => SemanticError::located(
            src,
            *span,
            NameError::WrongArity {
                name: crate::semantic_error::name::CalledFunction::Builtin(*name),
                expected: *expected,
                got: *got,
            },
        ),
        ExprLowerError::InvalidStaticBindingValue { name, span } => SemanticError::located(
            src,
            *span,
            ModuleError::InvalidTypeLevelBindingValue {
                name: name.to_string(),
            },
        ),
        ExprLowerError::UnknownLocalRef { name, span } => SemanticError::located(
            src,
            *span,
            StructError::UnknownLocalRef {
                name: name.to_string(),
            },
        ),
        ExprLowerError::UnknownGraphRef { name, span } => SemanticError::located(
            src,
            *span,
            NameError::UnknownGraphRef { name: name.clone() },
        ),
        ExprLowerError::BareGraphDeclarationRef { name, kind, span } => SemanticError::located(
            src,
            *span,
            NameError::BareGraphDeclarationRef {
                name: name.clone(),
                kind: *kind,
            },
        ),
        ExprLowerError::TimeScaleInValuePosition { scale, span } => SemanticError::located(
            src,
            *span,
            NameError::TimeScaleInValuePosition { scale: *scale },
        ),
        ExprLowerError::UnknownUnit { name, span } => SemanticError::located(
            src,
            *span,
            DimensionError::UnknownUnit { name: name.clone() },
        ),
        ExprLowerError::InvalidTimezone {
            timezone,
            tzdb_version,
            span,
        } => SemanticError::located(
            src,
            *span,
            DimensionError::InvalidTimezone {
                timezone: timezone.clone(),
                tzdb_version,
            },
        ),
        ExprLowerError::InvalidDatetimeLiteral {
            expectation,
            reason,
            span,
        } => SemanticError::located(
            src,
            *span,
            DimensionError::InvalidDatetimeLiteral {
                expectation: *expectation,
                reason: reason.clone(),
            },
        ),
        ExprLowerError::NonexistentCivilDateTime {
            datetime,
            time_zone,
            before,
            after,
            datetime_span,
            time_zone_span,
        } => SemanticError::located(
            src,
            *datetime_span,
            DimensionError::NonexistentCivilDateTime {
                datetime: *datetime,
                time_zone: time_zone.clone(),
                before: *before,
                after: *after,
                time_zone_span: *time_zone_span,
            },
        ),
        ExprLowerError::RepeatedCivilDateTime {
            datetime,
            time_zone,
            before,
            after,
            datetime_span,
            time_zone_span,
        } => SemanticError::located(
            src,
            *datetime_span,
            DimensionError::RepeatedCivilDateTime {
                datetime: *datetime,
                time_zone: time_zone.clone(),
                before: *before,
                after: *after,
                time_zone_span: *time_zone_span,
            },
        ),
        ExprLowerError::TimeZoneRegistryInvariant {
            time_zone,
            reason,
            span,
        } => SemanticError::internal_error(
            format!("validated timezone `{time_zone}` could not be loaded: {reason}"),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::Source(*span),
        ),
        ExprLowerError::EpochTimeScaleArgumentCount { got, span } => SemanticError::located(
            src,
            *span,
            DimensionError::EpochTimeScaleArgumentCount { got: *got },
        ),
        ExprLowerError::InvalidEpochTimeScaleArgument { span } => SemanticError::located(
            src,
            *span,
            DimensionError::InvalidEpochTimeScaleArgument {
                expected: crate::semantic::time_scale::TimeScale::expected_names(),
            },
        ),
        ExprLowerError::UnsupportedEpochTimeScale { name, span } => SemanticError::located(
            src,
            *span,
            DimensionError::UnsupportedEpochTimeScale {
                name: name.clone(),
                expected: crate::semantic::time_scale::TimeScale::expected_names(),
            },
        ),
        ExprLowerError::ExtraMapVariant {
            index_name,
            variant_name,
            span,
        } => SemanticError::located(
            src,
            *span,
            IndexError::ExtraVariants {
                index_name: index_name.clone().into(),
                extra: vec![crate::syntax::index_name::IndexEntryKey::named(
                    variant_name.clone(),
                )],
            },
        ),
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownModuleAlias { alias, .. },
            span,
        } => SemanticError::located(
            src,
            *span,
            ModuleError::UnknownModule {
                name: alias.clone(),
            },
        ),
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnexpectedDeclKind {
                    name,
                    actual: crate::resolve::category::DeclSymbolKind::Assert,
                    ..
                },
            span,
        } => SemanticError::located(
            src,
            *span,
            AttributeError::GraphRefToAssert {
                name: name.to_unowned_def_name(),
            },
        ),
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::PrivateName { owner, name, .. },
            span,
        } => SemanticError::located(
            src,
            *span,
            VisibilityError::ImportPrivateItem {
                name: name.to_string(),
                file_path: DagReference::Dag(owner.clone()),
            },
        ),
        ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownIndexVariant { index, variant },
            span,
        } => SemanticError::located(
            src,
            *span,
            IndexError::UnknownVariant {
                index_name: index.to_unowned_def_name().into(),
                variant_name: variant.clone(),
            },
        ),
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    category: NameCategory::Table(SymbolTable::Index),
                    name,
                    ..
                },
            span,
        } => SemanticError::located(
            src,
            *span,
            IndexError::UnknownIndex {
                name: IndexName::classify(name.clone()).into(),
            },
        ),
        ExprLowerError::ModuleResolve {
            source:
                ModuleResolveError::UnknownName {
                    category: NameCategory::Table(SymbolTable::Decl),
                    name,
                    ..
                },
            span,
        } => SemanticError::located(
            src,
            *span,
            StructError::UnknownLocalRef {
                name: name.to_string(),
            },
        ),
        ExprLowerError::TooManyLocals { span } => {
            SemanticError::located(src, *span, NameError::TooManyLocals)
        }
        ExprLowerError::ExpressionIdentity { source, span } => SemanticError::located(
            src,
            *span,
            NameError::ExpressionIdentity {
                source: source.clone(),
            },
        ),
        ExprLowerError::EmptyMapEntry { span } => {
            SemanticError::located(src, *span, IndexError::EmptyMapEntry)
        }
        ExprLowerError::UnknownPattern { path, span } => {
            SemanticError::located(src, *span, NameError::UnknownPattern { path: path.clone() })
        }
        ExprLowerError::PositionalArgumentsOnConstructor { constructor, span } => {
            SemanticError::located(
                src,
                *span,
                NameError::PositionalArgumentsOnConstructor {
                    constructor: constructor.clone(),
                },
            )
        }
        ExprLowerError::UnsupportedFunctionGenericArgs { path, span } => SemanticError::located(
            src,
            *span,
            NameError::UnsupportedFunctionGenericArgs { path: path.clone() },
        ),
        ExprLowerError::DuplicateLocalBinding {
            name, duplicate, ..
        } => SemanticError::located(
            src,
            *duplicate,
            NameError::DuplicateLocalBinding { name: name.clone() },
        ),
        ExprLowerError::LocalBindingShadowsTerm {
            name, duplicate, ..
        } => SemanticError::located(
            src,
            *duplicate,
            NameError::LocalBindingShadowsTerm { name: name.clone() },
        ),
        ExprLowerError::EmptyParenthesizedConstructor { constructor, span } => {
            SemanticError::located(
                src,
                *span,
                StructError::EmptyParenthesizedConstructor {
                    constructor: constructor.to_unowned_def_name(),
                },
            )
        }
        ExprLowerError::ModuleResolve { source, span } => {
            SemanticError::located(src, *span, ModuleError::resolution(source.clone()))
        }
        ExprLowerError::Type(err) => hir_lower_error_to_graphcal(err, src),
    }
}

/// Convert a HIR type-lowering failure into a spanned diagnostic.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive mapping from type-lowering diagnostics to spanned errors"
)]
pub fn hir_lower_error_to_graphcal(err: &HirLowerError, src: SourceId) -> SemanticError {
    match err {
        HirLowerError::ExpectedIndexFoundNat { expression, span } => SemanticError::located(
            src,
            *span,
            IndexError::ExpectedIndexFoundNat {
                expression: expression.clone(),
            },
        ),
        HirLowerError::NestedIndexedType { span } => {
            SemanticError::located(src, *span, IndexError::NestedIndexedType)
        }
        HirLowerError::NatOverflow { source, span } => {
            SemanticError::located(src, *span, IndexError::NatOverflow { error: *source })
        }
        HirLowerError::UnknownTypePath { path, span, .. } => SemanticError::located(
            src,
            *span,
            NameError::UnknownTypeName { path: path.clone() },
        ),
        HirLowerError::IndexLabelAsType { index, label, span } => SemanticError::located(
            src,
            *span,
            NameError::IndexLabelAsType {
                index: index.clone(),
                label: label.clone(),
            },
        ),
        HirLowerError::IndexAsType { index } => SemanticError::located(
            src,
            index.span(),
            NameError::IndexAsType {
                index: index.clone(),
            },
        ),
        HirLowerError::GenericConstraintMismatch {
            name,
            actual,
            expected,
            span,
        } => SemanticError::located(
            src,
            *span,
            NameError::GenericConstraintMismatch {
                name: name.clone(),
                actual: *actual,
                expected,
            },
        ),
        HirLowerError::UnknownGenericParam { name, span } => SemanticError::located(
            src,
            *span,
            NameError::UnknownGenericParam { name: name.clone() },
        ),
        HirLowerError::WrongGenericArgCount {
            target,
            expected,
            got,
            span,
        } => SemanticError::located(
            src,
            *span,
            NameError::WrongGenericArgCount {
                target: target.clone(),
                expected: *expected,
                got: *got,
            },
        ),
        HirLowerError::GenericArgumentSortMismatch {
            parameter,
            expected,
            actual,
            span,
        } => SemanticError::located(
            src,
            *span,
            NameError::GenericArgumentSortMismatch {
                parameter: parameter.clone(),
                expected: *expected,
                actual,
            },
        ),
        HirLowerError::DuplicateGenericParam {
            name, duplicate, ..
        } => SemanticError::located(
            src,
            *duplicate,
            NameError::DuplicateGenericParam { name: name.clone() },
        ),
        HirLowerError::GenericParamShadowsStatic {
            name, duplicate, ..
        } => SemanticError::located(
            src,
            *duplicate,
            NameError::GenericParamShadowsStatic { name: name.clone() },
        ),
        HirLowerError::ModuleResolve { source, span } => {
            SemanticError::located(src, *span, ModuleError::resolution(source.clone()))
        }
        HirLowerError::ExpectedTimeScale { span } => {
            SemanticError::located(src, *span, DimensionError::ExpectedTimeScale)
        }
        HirLowerError::UnknownTimeScale {
            name,
            expected,
            span,
        } => SemanticError::located(
            src,
            *span,
            DimensionError::UnknownTimeScale {
                name: name.clone(),
                expected,
            },
        ),
        HirLowerError::WrongDatetimeArgCount { got, span } => SemanticError::located(
            src,
            *span,
            DimensionError::WrongDatetimeArgCount { got: *got },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_error::SemanticErrorKind;
    use crate::syntax::names::NameAtom;
    use crate::syntax::non_empty::NonEmpty;
    use crate::syntax::span::Span;

    fn unknown(path: NamePath, slot: TypePathSlot) -> SemanticError {
        let src = crate::source_registry::SourceRegistry::new()
            .register("main.gcl", std::sync::Arc::new(String::new()));
        let error = HirLowerError::UnknownTypePath {
            path,
            slot,
            span: Span::new(2, 3),
        };
        type_lower_error_to_graphcal(&error, src)
    }

    fn atom(name: &str) -> NameAtom {
        NameAtom::try_from(name).unwrap()
    }

    #[test]
    fn unknown_type_path_is_reported_by_its_slot() {
        let path = NamePath::local(atom("Foo"));
        assert!(matches!(
            unknown(path.clone(), TypePathSlot::IndexAxis),
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Index(IndexError::UnknownIndex { name, .. }), .. }) if name.to_string() == "Foo"
        ));
        assert!(matches!(
            unknown(path.clone(), TypePathSlot::DimensionTerm),
            SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Dimension(DimensionError::UnknownDimension { name, .. }), .. }) if name == path
        ));
    }

    #[test]
    fn qualified_unknown_type_path_keeps_the_generic_diagnostic() {
        let path = NamePath::qualified(NonEmpty::singleton(atom("lib")), atom("Foo"));
        for slot in [TypePathSlot::IndexAxis, TypePathSlot::DimensionTerm] {
            assert!(matches!(
                unknown(path.clone(), slot),
                SemanticError::Located(crate::diagnostic::Diagnostic { kind: SemanticErrorKind::Name(kind @ NameError::UnknownTypeName { .. }), .. })
                    if kind.to_string() == "unknown type-level name `lib::Foo`"
            ));
        }
    }
}
