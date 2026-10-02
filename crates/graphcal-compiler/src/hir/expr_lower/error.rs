//! Diagnostics produced while lowering syntax expressions into HIR.

use crate::resolved_name::ResolvedConstructorName;
use crate::syntax::dimension::UnitRef as SyntaxUnitRef;

use thiserror::Error;

use crate::datetime_literal::{CivilDateTimeLiteral, DatetimeLiteralExpectation};
use crate::resolve::category::DeclSymbolKind;
use crate::resolve::error::ModuleResolveError;
use crate::semantic::time_scale::TimeScale;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::type_name::FieldName;

use crate::hir::expr::UnappliedFunctionRef;
use crate::hir::lower::HirLowerError;

/// Errors produced while lowering syntax expressions into HIR.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExprLowerError {
    /// A type-level generic argument failed to lower.
    #[error(transparent)]
    Type(#[from] HirLowerError),
    /// A module-aware lookup failed at an expression use site.
    #[error("{source}")]
    ModuleResolve {
        #[source]
        source: ModuleResolveError,
        span: Span,
    },
    /// A categorized Static DAG-call binding did not contain a type-level value.
    #[error("invalid Static binding value for `{name}`")]
    InvalidStaticBindingValue { name: NameAtom, span: Span },
    /// A local reference had no lexical binding in scope.
    #[error("unknown local variable `{name}`")]
    UnknownLocalRef { name: LocalName, span: Span },
    /// A graph reference (`@name`) did not resolve to any declaration.
    #[error("unknown graph reference `@{name}`")]
    UnknownGraphRef { name: ScopedName, span: Span },
    /// A user graph declaration was referenced without the required `@` sigil.
    #[error("bare reference `{name}` names a {kind}; user graph declarations require `@`")]
    BareGraphDeclarationRef {
        name: ScopedName,
        kind: DeclSymbolKind,
        span: Span,
    },
    /// A Static-only time-scale spelling was used where a Term value was required.
    #[error("time scale `{scale}` cannot be used as a value")]
    TimeScaleInValuePosition { scale: TimeScale, span: Span },
    /// A single expression tree introduced more local bindings than HIR can index.
    #[error("too many local bindings in one expression")]
    TooManyLocals { span: Span },
    /// The body revision cannot allocate another expression occurrence.
    #[error("{source}")]
    ExpressionIdentity {
        source: crate::expression_source::ExpressionSourceError,
        span: Span,
    },
    /// A map literal entry unexpectedly had no keys after syntax lowering.
    #[error("map literal entry has no keys")]
    EmptyMapEntry { span: Span },
    /// A map literal used a key variant that is not declared by its index.
    #[error("extra variant `{variant_name}` in map literal for index `{index_name}`")]
    ExtraMapVariant {
        index_name: IndexName,
        variant_name: IndexVariantName,
        span: Span,
    },
    /// One lexical scope introduced the same local name twice.
    #[error("duplicate local binding `{name}`")]
    DuplicateLocalBinding {
        name: LocalName,
        first: Span,
        duplicate: Span,
    },
    /// A lexical Term binder reused a visible flat Term slot.
    #[error("local binding `{name}` shadows a visible Term")]
    LocalBindingShadowsTerm {
        name: LocalName,
        original: Option<Span>,
        duplicate: Span,
    },
    /// A function call could not be resolved to a built-in function.
    #[error("unknown function `{path}`")]
    UnknownFunction {
        path: crate::syntax::names::NamePath,
        span: Span,
    },
    /// A plugin alias is in scope, but does not declare the called function.
    #[error("plugin alias `{alias}` does not declare a function `{name}`")]
    UnknownExternFunction {
        alias: ModuleAliasName,
        name: crate::syntax::function_name::FnName,
        span: Span,
    },
    /// Named constructor-style arguments were supplied to a resolved function.
    #[error("function `{function}` uses positional arguments")]
    NamedArgumentsOnFunction {
        function: UnappliedFunctionRef,
        argument_names: Vec<FieldName>,
        span: Span,
    },
    /// A function call supplied generic arguments that no function signature consumes.
    #[error("function `{path}` does not accept generic arguments")]
    UnsupportedFunctionGenericArgs { path: NamePath, span: Span },
    /// Positional call syntax targeted a constructor, whose payload fields must
    /// be named explicitly.
    #[error("constructor `{constructor}` requires named field arguments")]
    PositionalArgumentsOnConstructor {
        constructor: ResolvedConstructorName,
        span: Span,
    },
    /// Empty parentheses targeted a constructor instead of a zero-argument function.
    #[error("constructor `{constructor}` cannot use empty parentheses")]
    EmptyParenthesizedConstructor {
        constructor: ResolvedConstructorName,
        span: Span,
    },
    /// A built-in function was called with the wrong number of arguments.
    #[error("function `{name}` expects {expected} argument(s), got {got}")]
    WrongArity {
        name: crate::builtin::BuiltinFn,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// A timezone literal was not present in Graphcal's bundled IANA registry.
    #[error("unknown timezone `{timezone}`")]
    InvalidTimezone {
        timezone: String,
        tzdb_version: &'static str,
        span: Span,
    },
    /// `epoch` had the wrong number of static time-scale arguments.
    #[error("epoch requires exactly one static time-scale argument, got {got}")]
    EpochTimeScaleArgumentCount { got: usize, span: Span },
    /// `epoch`'s static argument was not a bare source name.
    #[error("epoch's static time-scale argument must be a bare name")]
    InvalidEpochTimeScaleArgument { span: Span },
    /// `epoch`'s bare static argument did not name a supported time scale.
    #[error("unsupported epoch time scale `{name}`")]
    UnsupportedEpochTimeScale { name: NameAtom, span: Span },
    /// A contextual datetime literal did not satisfy its constructor contract.
    #[error("invalid datetime literal: {reason}")]
    InvalidDatetimeLiteral {
        expectation: DatetimeLiteralExpectation,
        reason: crate::semantic_error::dimension::DatetimeLiteralError,
        span: Span,
    },
    /// A timezone transition skips the requested local civil datetime.
    #[error("local civil datetime `{datetime}` does not exist in timezone `{time_zone}`")]
    NonexistentCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        datetime_span: Span,
        time_zone_span: Span,
    },
    /// A timezone transition repeats the requested local civil datetime.
    #[error("local civil datetime `{datetime}` occurs twice in timezone `{time_zone}`")]
    RepeatedCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        datetime_span: Span,
        time_zone_span: Span,
    },
    /// A validated timezone disappeared from the explicit registry.
    #[error("validated timezone `{time_zone}` could not be loaded: {reason}")]
    TimeZoneRegistryInvariant {
        time_zone: IanaTimeZoneId,
        reason: String,
        span: Span,
    },
    /// A unit reference was not found in the defining module's unit scope.
    #[error("unknown unit `{name}`")]
    UnknownUnit { name: SyntaxUnitRef, span: Span },
    /// A path-pattern could not be resolved to a constructor or index label.
    #[error("unknown match pattern `{path}`")]
    UnknownPattern { path: NamePath, span: Span },
}
