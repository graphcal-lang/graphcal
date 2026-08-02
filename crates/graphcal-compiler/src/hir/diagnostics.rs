//! Conversions from HIR lowering diagnostics to spanned [`GraphcalError`]s,
//! plus the canonical declaration-key derivation shared by the IR freeze
//! boundary and TIR.

use crate::syntax::decl_name::{DeclNameNamespace, ResolvedDeclName};
use crate::syntax::index_name::IndexNameNamespace;
use std::sync::Arc;

use miette::NamedSource;

use crate::hir;
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_resolve::ModuleResolveError;
use crate::syntax::names::NameNamespace;

/// Derive the canonical declaration key for an entry name under `owner`.
///
/// Qualified entry names (merged include instances) extend the owner with
/// their qualifier segments, matching the instance modules the loader
/// registers.
#[must_use]
pub fn resolved_decl_key(
    owner: &crate::dag_id::DagId,
    name: &ScopedName,
) -> Option<ResolvedDeclName> {
    let owner = name
        .qualifier()
        .iter()
        .fold(owner.clone(), |owner, segment| {
            owner.child(segment.as_ref())
        });
    let name = DeclName::try_new(name.member()).ok()?;
    Some(ResolvedDeclName::from_def(owner, name))
}

/// Convert a HIR expression-lowering failure into a spanned diagnostic.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive mapping from lowering diagnostics to spanned errors"
)]
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
                src: src.clone(),
                span: (*span).into(),
            };
        }
        hir::ExprLowerError::UnsupportedEpochTimeScale { name, span } => {
            return GraphcalError::UnsupportedEpochTimeScale {
                name: name.clone(),
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
        | hir::ExprLowerError::InvalidScopedNameSegment { span, .. }
        | hir::ExprLowerError::UnknownLocalRef { span, .. }
        | hir::ExprLowerError::UnknownGraphRef { span, .. }
        | hir::ExprLowerError::UnknownUnit { span, .. }
        | hir::ExprLowerError::TooManyLocals { span }
        | hir::ExprLowerError::EmptyMapEntry { span }
        | hir::ExprLowerError::InvalidMapEntryKey { span }
        | hir::ExprLowerError::ExtraMapVariant { span, .. }
        | hir::ExprLowerError::UnknownPattern { span, .. }
        | hir::ExprLowerError::UnknownFunction { span, .. }
        | hir::ExprLowerError::UnknownExternFunction { span, .. }
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
