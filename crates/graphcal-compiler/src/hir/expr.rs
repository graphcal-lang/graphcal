//! HIR expression/value reference types and lowering.
//!
//! This module is the expression-side counterpart to [`super::types`]. It
//! lowers the desugared syntax AST into a HIR expression tree whose reference
//! positions use canonical module identities or lexical local IDs. This is the
//! single name-resolution stage of the pipeline: syntactic reference paths
//! ([`crate::syntax::ast::UnresolvedRef`]) are classified and resolved here,
//! in one pass, against the lexical scope and the module-aware resolver.
//! Source paths (`NamePath` / `IdentPath` / `ScopedName`) are consumed at
//! this boundary. Unit references retain their structured source spelling only
//! for diagnostics and display labels, alongside a canonical resolved target;
//! semantic lookup never uses that spelling.
//!
//! Lowering is diagnostic-accumulating: a reference that cannot be resolved
//! becomes an explicit [`ExprKind::Error`] node and its diagnostic is
//! recorded, so IDE consumers can keep working on incomplete code. The strict
//! entry points (`lower_expr`, `lower_assert_body`) reject any tree that
//! contains an error node, so the batch pipeline never sees one.

use crate::syntax::decl_name::{DeclNameNamespace, ResolvedDeclName};
use crate::syntax::dimension::{ResolvedDimName, ResolvedUnitName, UnitRef as SyntaxUnitRef};
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::collections::{BTreeSet, HashMap};

use thiserror::Error;

use crate::builtin::{BuiltinConst, BuiltinFnName};
use crate::dag_id::DagId;
use crate::datetime_literal::{
    CivilDateTimeLiteral, DatetimeLiteralExpectation, OffsetDateTimeLiteral,
    ResolveZonedDateTimeLiteralError, ZonedDateTimeLiteral,
};
use crate::desugar::desugared_ast as ast;
use crate::registry::reserved_name::{ReservedNameNamespace, validate_reserved_name};
use crate::registry::time_scale::TimeScale;
use crate::registry::time_zone::{IanaTimeZoneId, TimeZoneRegistry};
use crate::registry::types::UnitRegistry;
use crate::syntax::ast::{Ident, IdentPath, InputBindingCategory, UnresolvedRef};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::{ModuleAliasName, ScopedName};
use crate::syntax::module_resolve::{
    DeclSymbolKind, ModuleAliasRole, ModuleResolveError, ModuleResolver,
};
use crate::syntax::names::{NameAtom, NameNamespace, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{FieldName, GenericParamName};

use super::lower::{
    GenericScope, HirLowerError, PreludeTypeScope, TypeLoweringContext, lower_generic_args,
    lower_nat_expr,
};
use super::types::{GenericArg, NatExpr};

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
    /// A map literal entry unexpectedly had no keys after syntax lowering.
    #[error("map literal entry has no keys")]
    EmptyMapEntry { span: Span },
    /// A syntax map key paired a named axis with a position or a Fin axis with a name.
    #[error("map entry key category does not match its index category")]
    InvalidMapEntryKey { span: Span },
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
    UnknownFunction { path: String, span: Span },
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
        function: FunctionRef,
        argument_names: Vec<FieldName>,
        span: Span,
    },
    /// A function call supplied generic arguments that no function signature consumes.
    #[error("function `{path}` does not accept generic arguments")]
    UnsupportedFunctionGenericArgs { path: String, span: Span },
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
        name: crate::syntax::function_name::FnName,
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
        reason: String,
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
    UnknownPattern { path: String, span: Span },
}

/// Context required to lower one expression tree into HIR.
#[derive(Debug, Clone, Copy)]
pub struct ExprLoweringContext<'a> {
    owner: &'a DagId,
    resolver: &'a ModuleResolver,
    generic_scope: &'a GenericScope,
    time_zones: &'a TimeZoneRegistry,
    prelude: Option<&'a PreludeTypeScope>,
    unit_registry: Option<&'a UnitRegistry>,
    unit_bindings: Option<&'a HashMap<SyntaxUnitRef, ResolvedUnitName>>,
    decl_bindings: Option<&'a HashMap<ScopedName, ResolvedDeclName>>,
    instance_templates: Option<&'a HashMap<DagId, DagId>>,
}

impl<'a> ExprLoweringContext<'a> {
    /// Create an expression-lowering context.
    #[must_use]
    pub const fn new(
        owner: &'a DagId,
        resolver: &'a ModuleResolver,
        generic_scope: &'a GenericScope,
        time_zones: &'a TimeZoneRegistry,
    ) -> Self {
        Self {
            owner,
            resolver,
            generic_scope,
            time_zones,
            prelude: None,
            unit_registry: None,
            unit_bindings: None,
            decl_bindings: None,
            instance_templates: None,
        }
    }

    /// Add implicit prelude type-system symbols for unit and generic-argument lowering.
    #[must_use]
    pub const fn with_prelude(self, prelude: &'a PreludeTypeScope) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: Some(prelude),
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add the frozen unit scope used for registry-created synthetic bindings
    /// that do not have a source module symbol.
    #[must_use]
    pub const fn with_unit_registry(self, unit_registry: &'a UnitRegistry) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: Some(unit_registry),
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add source-visible unit projections with their concrete semantic identities.
    #[must_use]
    pub(crate) const fn with_unit_bindings(
        self,
        unit_bindings: &'a HashMap<SyntaxUnitRef, ResolvedUnitName>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: Some(unit_bindings),
            decl_bindings: self.decl_bindings,
            instance_templates: self.instance_templates,
        }
    }

    /// Add canonical declaration bindings for declarations already visible in
    /// the lowered IR, such as prefixed dependency entries and DAG self-imports.
    #[must_use]
    pub(crate) const fn with_decl_bindings(
        self,
        decl_bindings: &'a HashMap<ScopedName, ResolvedDeclName>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: Some(decl_bindings),
            instance_templates: self.instance_templates,
        }
    }

    /// Add the concrete-instance to source-template identity map used to
    /// validate instance member access without flattening either identity.
    #[must_use]
    pub(crate) const fn with_instance_templates(
        self,
        instance_templates: &'a HashMap<DagId, DagId>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            time_zones: self.time_zones,
            prelude: self.prelude,
            unit_registry: self.unit_registry,
            unit_bindings: self.unit_bindings,
            decl_bindings: self.decl_bindings,
            instance_templates: Some(instance_templates),
        }
    }

    const fn type_context(self) -> TypeLoweringContext<'a> {
        let ctx = TypeLoweringContext::new(self.owner, self.resolver, self.generic_scope);
        match self.prelude {
            Some(prelude) => ctx.with_prelude(prelude),
            None => ctx,
        }
    }

    fn resolve_prelude_dimension_path(self, path: &NamePath) -> Option<ResolvedDimName> {
        self.prelude
            .and_then(|prelude| prelude.resolve_dimension_path(path))
    }

    fn resolve_prelude_unit_ref(self, reference: &SyntaxUnitRef) -> Option<ResolvedUnitName> {
        self.prelude
            .and_then(|prelude| prelude.resolve_unit_ref(reference))
    }

    fn resolve_registry_unit_ref(self, reference: &SyntaxUnitRef) -> Option<ResolvedUnitName> {
        if reference.is_qualified() {
            return None;
        }
        self.unit_registry?.get_unit(reference)?;
        Some(ResolvedUnitName::from_def(
            self.owner.clone(),
            reference.name().clone(),
        ))
    }
}

/// Lower a syntax expression into HIR, accumulating diagnostics.
///
/// References that cannot be resolved become [`ExprKind::Error`] nodes and
/// their diagnostics are returned alongside the lowered tree, so consumers
/// that must keep working on incomplete code (the LSP) still get a tree with
/// spans for every position that did resolve.
#[must_use]
pub fn lower_expr_tolerant(
    expr: &ast::Expr,
    ctx: ExprLoweringContext<'_>,
) -> (Expr, Vec<ExprLowerError>) {
    let mut lowerer = ExprLowerer::new(ctx);
    let hir_expr = lowerer.lower_expr(expr);
    (hir_expr, lowerer.diagnostics)
}

/// Lower a syntax expression into HIR, rejecting unresolved references.
///
/// This is the batch-pipeline boundary: the lowered tree is guaranteed to
/// contain no [`ExprKind::Error`] node.
///
/// # Errors
///
/// Returns the first [`ExprLowerError`] if any expression-level reference
/// cannot be resolved to a canonical module identity or lexical local binding.
pub(crate) fn lower_expr(
    expr: &ast::Expr,
    ctx: ExprLoweringContext<'_>,
) -> Result<CheckedExpr, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_expr_tolerant(expr, ctx);
    if diagnostics.is_empty() {
        Ok(CheckedExpr(lowered))
    } else {
        Err(diagnostics.swap_remove(0))
    }
}

/// Lower a syntax assertion body into HIR, accumulating diagnostics.
///
/// Each assertion body owns an independent lexical local-id space. Assertion
/// expressions cannot share locals across the `actual`/`expected`/`tolerance`
/// slots of a tolerance assertion, so each slot is lowered with a fresh lowerer.
#[must_use]
fn lower_assert_body_tolerant(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> (AssertBody, Vec<ExprLowerError>) {
    match body {
        ast::AssertBody::Expr(expr) => {
            let (lowered, diagnostics) = lower_expr_tolerant(expr, ctx);
            (AssertBody::Expr(Box::new(lowered)), diagnostics)
        }
        ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => {
            let (actual, mut diagnostics) = lower_expr_tolerant(actual, ctx);
            let (expected, expected_diags) = lower_expr_tolerant(expected, ctx);
            let (tolerance, tolerance_diags) = lower_expr_tolerant(tolerance, ctx);
            diagnostics.extend(expected_diags);
            diagnostics.extend(tolerance_diags);
            (
                AssertBody::Tolerance {
                    actual: Box::new(actual),
                    expected: Box::new(expected),
                    tolerance: Box::new(tolerance),
                },
                diagnostics,
            )
        }
    }
}

/// Lower a syntax assertion body into HIR, rejecting unresolved references.
///
/// # Errors
///
/// Returns the first [`ExprLowerError`] if any reference cannot be resolved.
pub(crate) fn lower_assert_body(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> Result<CheckedAssertBody, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_assert_body_tolerant(body, ctx);
    if diagnostics.is_empty() {
        Ok(CheckedAssertBody(lowered))
    } else {
        Err(diagnostics.swap_remove(0))
    }
}

/// Stable lexical identity for a local expression binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(u32);

impl LocalId {
    /// Numeric index unique within one lowered expression tree.
    #[must_use]
    pub(crate) const fn index(self) -> u32 {
        self.0
    }
}

/// A lexical local binding introduced by an expression form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDef {
    pub id: LocalId,
    pub name: LocalName,
    pub span: Span,
}

/// A layered lexical environment for HIR locals.
///
/// Each binder (for-comp, scan, unfold, match arm) layers a child frame
/// holding its few bindings over the enclosing environment instead of cloning
/// the full local map; lookup walks the parent chain. [`LocalId`]s are unique
/// within one lowered body, so frames never shadow one another — the chain is
/// purely an ownership layering, and nested binders cost O(own bindings)
/// instead of O(visible locals).
#[derive(Debug)]
pub struct LocalEnv<'a, V> {
    parent: Option<&'a Self>,
    bindings: Vec<(LocalId, V)>,
}

impl<'a, V> LocalEnv<'a, V> {
    /// Create an empty root environment.
    #[must_use]
    pub const fn root() -> Self {
        Self {
            parent: None,
            bindings: Vec::new(),
        }
    }

    /// Create a root environment holding the given bindings.
    #[must_use]
    pub const fn from_bindings(bindings: Vec<(LocalId, V)>) -> Self {
        Self {
            parent: None,
            bindings,
        }
    }

    /// Layer a child frame holding `bindings` over this environment.
    #[must_use]
    pub const fn child<'b>(&'b self, bindings: Vec<(LocalId, V)>) -> LocalEnv<'b, V>
    where
        'a: 'b,
    {
        LocalEnv {
            parent: Some(self),
            bindings,
        }
    }

    /// Look up a local by its lexical identity, innermost frame first.
    #[must_use]
    pub fn get(&self, id: LocalId) -> Option<&V> {
        self.bindings
            .iter()
            .rev()
            .find(|(bound, _)| *bound == id)
            .map(|(_, value)| value)
            .or_else(|| self.parent.and_then(|parent| parent.get(id)))
    }

    /// Bind or update a local in this frame.
    ///
    /// Iterating binders (for-comp elements, scan/unfold steps) rebind the
    /// same `LocalId` once per iteration without growing the frame.
    pub fn bind(&mut self, id: LocalId, value: V) {
        match self.bindings.iter_mut().find(|(bound, _)| *bound == id) {
            Some((_, slot)) => *slot = value,
            None => self.bindings.push((id, value)),
        }
    }
}

impl<V> Default for LocalEnv<'_, V> {
    fn default() -> Self {
        Self::root()
    }
}

/// An HIR assertion body proven not to contain tolerant-lowering error nodes.
#[derive(Debug, Clone)]
pub struct CheckedAssertBody(AssertBody);

impl std::ops::Deref for CheckedAssertBody {
    type Target = AssertBody;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl CheckedAssertBody {
    pub(crate) const fn from_assert_body_for_test(body: AssertBody) -> Self {
        Self(body)
    }
}

/// An HIR expression proven not to contain tolerant-lowering error nodes.
#[derive(Debug, Clone)]
pub struct CheckedExpr(Expr);

impl std::ops::Deref for CheckedExpr {
    type Target = Expr;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl CheckedExpr {
    pub(crate) fn into_expr_for_test(self) -> Expr {
        self.0
    }

    pub(crate) fn expr_mut_for_test(&mut self) -> &mut Expr {
        &mut self.0
    }
}

/// HIR expression node.
#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

// Manual impl instead of `#[derive(Clone)]`: derived clone glue recurses
// once per tree level without any stack-growth guard, so cloning a long
// left-nested operator chain overflows the stack. Routing each level
// through `with_stack_growth` lets the stack grow on demand (the derived
// `ExprKind` clone calls back into this impl through `Box<Expr>`).
impl Clone for Expr {
    fn clone(&self) -> Self {
        crate::stack::with_stack_growth(|| Self {
            kind: self.kind.clone(),
            span: self.span,
        })
    }
}

impl Expr {
    #[must_use]
    pub const fn new(kind: ExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// A resolved index-variant reference with its source spans kept separate.
///
/// The index path and variant segment are not necessarily adjacent: table
/// desugaring can associate a bare row label with the axis in `table[...]`.
/// Qualified slices and heterogeneous headers can also repeat that same index
/// path, so additional index occurrences are retained for editor features.
/// Diagnostics on a contiguous primary `Index#Variant` path can still use
/// [`IndexVariantRef::path_span`], while span-precise consumers address each
/// written occurrence independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVariantRef {
    /// The resolved index variant.
    pub variant: ResolvedIndexVariant,
    /// Span of the index path as written (`Maneuver` in `Maneuver#Departure`,
    /// or the axis token inside `table[...]` for desugared table rows).
    /// `None` when the variant is written without an index segment (a bare
    /// label in a match pattern whose index is inferred).
    pub index_span: Option<Span>,
    /// Other written occurrences of the same index path introduced by table
    /// syntax, such as the axis in `table[...]` beside a qualified slice.
    pub additional_index_spans: Vec<Span>,
    /// Span of just the variant segment (the final path segment / row label).
    pub variant_span: Span,
}

impl IndexVariantRef {
    /// Iterate over every written occurrence of the resolved index path.
    pub fn index_spans(&self) -> impl Iterator<Item = Span> + '_ {
        self.index_span
            .into_iter()
            .chain(self.additional_index_spans.iter().copied())
    }

    /// Whole-reference span for diagnostics on a contiguous primary
    /// `Index#Variant` path. For desugared table rows the primary index span
    /// may live elsewhere, so prefer [`Self::variant_span`] there.
    #[must_use]
    pub fn path_span(&self) -> Span {
        self.index_span
            .map_or(self.variant_span, |index| index.merge(self.variant_span))
    }
}

/// A unit reference after module-aware resolution.
///
/// `spelling` preserves the source alias for diagnostics and display labels;
/// `resolved` is the canonical definition identity used by the compiler core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitRef {
    spelling: SyntaxUnitRef,
    resolved: ResolvedUnitName,
}

impl ResolvedUnitRef {
    #[must_use]
    pub const fn new(spelling: SyntaxUnitRef, resolved: ResolvedUnitName) -> Self {
        Self { spelling, resolved }
    }

    /// Return the source spelling, including any module alias.
    #[must_use]
    pub const fn spelling(&self) -> &SyntaxUnitRef {
        &self.spelling
    }

    /// Return the canonical owner-qualified unit identity.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedUnitName {
        &self.resolved
    }
}

impl std::fmt::Display for ResolvedUnitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.spelling.fmt(f)
    }
}

/// One term in a module-resolved unit expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExprItem {
    pub op: ast::MulDivOp,
    pub name: Spanned<ResolvedUnitRef>,
    /// Exact semantic exponent, normalized at the AST-to-HIR boundary.
    pub power: crate::dimension::Rational,
}

/// A unit expression whose terms retain canonical defining-module identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnitExpr {
    pub terms: Vec<ResolvedUnitExprItem>,
    pub span: Span,
}

/// Resolved expression shape.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// A reference that failed to resolve.
    ///
    /// Produced only by tolerant lowering; the diagnostic for the failure is
    /// reported alongside the lowered tree. Independently lowerable descendant
    /// expressions are retained for IDE references and additional diagnostics.
    /// The strict entry points reject trees containing this node, so the batch
    /// pipeline never observes it.
    Error {
        children: Vec<Expr>,
    },
    Number(f64),
    Integer(i64),
    Bool(bool),
    StringLiteral(String),
    /// An offset-bearing instant parsed during HIR lowering.
    OffsetDateTimeLiteral(OffsetDateTimeLiteral),
    /// An offset/scale-free civil coordinate parsed during HIR lowering.
    CivilDateTimeLiteral(CivilDateTimeLiteral),
    /// A civil coordinate and timezone resolved to one unambiguous instant.
    ZonedDateTimeLiteral(ZonedDateTimeLiteral),
    /// An IANA timezone literal validated and canonicalized during HIR lowering.
    IanaTimeZoneLiteral(IanaTimeZoneId),
    TypeSystemRef(Spanned<TypeSystemRef>),
    GraphRef(Spanned<ResolvedDeclName>),
    ConstRef(Spanned<ConstRef>),
    LocalRef(Spanned<LocalId>),
    BinOp {
        op: ast::BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: ast::UnaryOp,
        operand: Box<Expr>,
    },
    FnCall {
        callee: Spanned<FunctionRef>,
        args: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    QuantityLiteral {
        value: f64,
        unit: ResolvedUnitExpr,
    },
    Convert {
        expr: Box<Expr>,
        target: ResolvedUnitExpr,
    },
    DisplayTimezone {
        expr: Box<Expr>,
        timezone: IanaTimeZoneId,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: Spanned<FieldName>,
    },
    ConstructorCall {
        callee: Spanned<ResolvedConstructorName>,
        generic_args: Vec<GenericArg>,
        fields: Vec<FieldInit>,
    },
    MapLiteral {
        entries: Vec<MapEntry>,
    },
    ForComp {
        bindings: Vec<ForBinding>,
        body: Box<Expr>,
    },
    IndexAccess {
        expr: Box<Expr>,
        args: NonEmpty<IndexArg>,
    },
    Scan {
        source: Box<Expr>,
        init: Box<Expr>,
        acc: LocalDef,
        val: LocalDef,
        body: Box<Expr>,
    },
    Unfold {
        recurrence: Box<UnfoldRecurrence>,
        init: Box<Expr>,
        body: Box<Expr>,
    },
    /// A key introduction form over a resolved axis:
    /// `key(Axis, spelling)`, `fin_key(Fin(N), e)`, or a coordinate search.
    KeyForm {
        kind: crate::syntax::ast::KeyFormKind,
        axis: ForBindingIndex,
        axis_span: Span,
        arg: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    VariantLiteral(IndexVariantRef),
    DagCall {
        target: Spanned<DagId>,
        args: Vec<ParamBinding>,
        static_bindings: DagCallStaticBindings,
        output: Spanned<ResolvedDeclName>,
    },
}

/// Resolved axis and lexical bindings for an `unfold` recurrence.
///
/// Boxed as one semantic unit inside [`ExprKind::Unfold`] to keep expression
/// variants compact while preserving the role of every binding.
#[derive(Debug, Clone)]
pub struct UnfoldRecurrence {
    pub axis: Spanned<ResolvedIndexName>,
    pub previous_state: LocalDef,
    pub previous_index: LocalDef,
    pub current_index: LocalDef,
}

/// Canonical declaration dependencies observed in one HIR expression tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExprDependencies {
    /// Runtime graph dependencies reached through `@name` references.
    pub graph_refs: BTreeSet<ResolvedDeclName>,
    /// Compile-time const dependencies reached through const-like value refs.
    pub(crate) const_refs: BTreeSet<ResolvedDeclName>,
}

/// Collect canonical declaration dependencies from an already-lowered HIR expression.
#[must_use]
pub fn collect_expr_dependencies(expr: &Expr) -> ExprDependencies {
    let mut deps = ExprDependencies::default();
    collect_expr_dependencies_into(expr, &mut deps);
    deps
}

fn collect_expr_dependencies_into(expr: &Expr, deps: &mut ExprDependencies) {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| collect_expr_dependencies_into_inner(expr, deps));
}

fn collect_expr_dependencies_into_inner(expr: &Expr, deps: &mut ExprDependencies) {
    match &expr.kind {
        ExprKind::Error { children } => {
            for child in children {
                collect_expr_dependencies_into(child, deps);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral(_)
        | ExprKind::QuantityLiteral { .. } => {}
        ExprKind::GraphRef(target) => {
            deps.graph_refs.insert(target.value.clone());
        }
        ExprKind::ConstRef(target) => {
            if let ConstRef::Decl(resolved) = &target.value {
                deps.const_refs.insert(resolved.clone());
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_expr_dependencies_into(lhs, deps);
            collect_expr_dependencies_into(rhs, deps);
        }
        ExprKind::UnaryOp { operand, .. }
        | ExprKind::Convert { expr: operand, .. }
        | ExprKind::DisplayTimezone { expr: operand, .. }
        | ExprKind::FieldAccess { expr: operand, .. } => {
            collect_expr_dependencies_into(operand, deps);
        }
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_expr_dependencies_into(arg, deps);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_dependencies_into(condition, deps);
            collect_expr_dependencies_into(then_branch, deps);
            collect_expr_dependencies_into(else_branch, deps);
        }
        ExprKind::ConstructorCall { fields, .. } => {
            for field in fields {
                collect_expr_dependencies_into(&field.value, deps);
            }
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_expr_dependencies_into(&entry.value, deps);
            }
        }
        ExprKind::ForComp { body, .. } => collect_expr_dependencies_into(body, deps),
        ExprKind::IndexAccess { expr, args } => {
            collect_expr_dependencies_into(expr, deps);
            for arg in args {
                if let IndexArg::Expr(expr) = arg {
                    collect_expr_dependencies_into(expr, deps);
                }
            }
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_expr_dependencies_into(source, deps);
            collect_expr_dependencies_into(init, deps);
            collect_expr_dependencies_into(body, deps);
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_expr_dependencies_into(init, deps);
            collect_expr_dependencies_into(body, deps);
        }
        ExprKind::KeyForm { arg, .. } => {
            collect_expr_dependencies_into(arg, deps);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_expr_dependencies_into(scrutinee, deps);
            for arm in arms {
                collect_expr_dependencies_into(&arm.body, deps);
            }
        }
        ExprKind::DagCall { args, .. } => {
            for arg in args {
                collect_expr_dependencies_into(&arg.value, deps);
            }
        }
    }
}

/// Visit every node in an HIR expression exactly once, in pre-order.
///
/// Semantic analyses should consume canonical facts directly from HIR through
/// this traversal instead of copying call-site data into span-keyed side tables.
pub(crate) fn visit_expr(expr: &Expr, visitor: &mut impl FnMut(&Expr)) {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| visit_expr_inner(expr, visitor));
}

fn visit_expr_inner(expr: &Expr, visitor: &mut impl FnMut(&Expr)) {
    visitor(expr);
    match &expr.kind {
        ExprKind::Error { children } => {
            for child in children {
                visit_expr(child, visitor);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::VariantLiteral(_) => {}
        ExprKind::BinOp { lhs, rhs, .. } => {
            visit_expr(lhs, visitor);
            visit_expr(rhs, visitor);
        }
        ExprKind::UnaryOp { operand, .. }
        | ExprKind::Convert { expr: operand, .. }
        | ExprKind::DisplayTimezone { expr: operand, .. }
        | ExprKind::FieldAccess { expr: operand, .. } => visit_expr(operand, visitor),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                visit_expr(arg, visitor);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            visit_expr(condition, visitor);
            visit_expr(then_branch, visitor);
            visit_expr(else_branch, visitor);
        }
        ExprKind::ConstructorCall { fields, .. } => fields
            .iter()
            .for_each(|field| visit_expr(&field.value, visitor)),
        ExprKind::MapLiteral { entries } => entries
            .iter()
            .for_each(|entry| visit_expr(&entry.value, visitor)),
        ExprKind::ForComp { body, .. } => visit_expr(body, visitor),
        ExprKind::IndexAccess { expr: inner, args } => {
            visit_expr(inner, visitor);
            args.iter().for_each(|arg| {
                if let IndexArg::Expr(arg) = arg {
                    visit_expr(arg, visitor);
                }
            });
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            visit_expr(source, visitor);
            visit_expr(init, visitor);
            visit_expr(body, visitor);
        }
        ExprKind::Unfold { init, body, .. } => {
            visit_expr(init, visitor);
            visit_expr(body, visitor);
        }
        ExprKind::KeyForm { arg, .. } => visit_expr(arg, visitor),
        ExprKind::Match { scrutinee, arms } => {
            visit_expr(scrutinee, visitor);
            for arm in arms {
                visit_expr(&arm.body, visitor);
            }
        }
        ExprKind::DagCall { args, .. } => args
            .iter()
            .for_each(|binding| visit_expr(&binding.value, visitor)),
    }
}

/// Return the first DAG call in an HIR expression, if any.
#[must_use]
pub fn find_dag_call(expr: &Expr) -> Option<(DagId, Span)> {
    let mut found = None;
    visit_expr(expr, &mut |candidate| {
        if found.is_none()
            && let ExprKind::DagCall { target, .. } = &candidate.kind
        {
            found = Some((target.value.clone(), candidate.span));
        }
    });
    found
}

/// Type-system identifier used as a value expression, usually in include bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSystemRef {
    Type(ResolvedStructTypeName),
    Dimension(ResolvedDimName),
    Index(ResolvedIndexName),
    IndexVariant(ResolvedIndexVariant),
}

impl TypeSystemRef {
    #[must_use]
    fn surface_description(&self) -> String {
        match self {
            Self::Type(name) => format!("type `{}`", name.as_str()),
            Self::Dimension(name) => format!("dimension `{}`", name.as_str()),
            Self::Index(name) => format!("index `{}`", name.as_str()),
            Self::IndexVariant(variant) => format!(
                "index label `{}#{}`",
                variant.index().as_str(),
                variant.variant()
            ),
        }
    }

    #[must_use]
    pub(crate) fn value_position_error(&self) -> String {
        format!("{} cannot be used as a value", self.surface_description())
    }
}

/// Resolved constant-like expression target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstRef {
    Decl(ResolvedDeclName),
    Constructor(ResolvedConstructorName),
    Builtin(BuiltinConst),
    GenericNatParam(super::types::GenericParamId),
}

/// Function call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionRef {
    Builtin(BuiltinFnName),
    /// `epoch<S>` with its required static time scale resolved at the HIR boundary.
    Epoch {
        scale: Spanned<TimeScale>,
    },
    /// An externally-provided function declared by an `import plugin` block.
    External(ExternFnRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedCallable {
    Constructor(ResolvedConstructorName),
    Function(FunctionRef),
}

impl FunctionRef {
    /// Return the built-in identity represented by this reference.
    #[must_use]
    pub const fn builtin_name(&self) -> Option<BuiltinFnName> {
        match self {
            Self::Builtin(name) => Some(*name),
            Self::Epoch { .. } => Some(BuiltinFnName::Epoch),
            Self::External(_) => None,
        }
    }
}

impl std::fmt::Display for FunctionRef {
    /// Render source-like function spelling at diagnostic and display boundaries.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin(name) => std::fmt::Display::fmt(name, f),
            Self::Epoch { scale } => write!(f, "epoch<{}>", scale.value),
            Self::External(extern_ref) => std::fmt::Display::fmt(extern_ref, f),
        }
    }
}

/// A resolved reference to an extern (plugin) function.
///
/// Carries the canonical plugin identity plus the source alias the call was
/// qualified with. The alias is for diagnostics and IDE features; semantic
/// lookups key on [`ExternFnRef::key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternFnRef {
    /// Canonical plugin identity (the `import plugin "…"` path string).
    pub plugin: crate::syntax::plugin::PluginPath,
    /// The module alias the call site was qualified with.
    pub alias: ModuleAliasName,
    /// The function leaf name.
    pub name: crate::syntax::function_name::FnName,
}

impl ExternFnRef {
    /// The canonical `(plugin, function)` lookup key.
    #[must_use]
    pub fn key(&self) -> crate::syntax::plugin::ExternFnKey {
        crate::syntax::plugin::ExternFnKey {
            plugin: self.plugin.clone(),
            name: self.name.clone(),
        }
    }
}

impl std::fmt::Display for ExternFnRef {
    /// Renders `alias::name` for diagnostics and display boundaries only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.alias, self.name)
    }
}

/// Find the first extern (plugin) function call in an expression tree.
///
/// Extern functions are runtime-provided; contexts that evaluate without a
/// host function registry (const expressions, domain bounds, dynamic unit
/// scales) use this to reject them with a spanned diagnostic.
#[must_use]
pub(crate) fn find_extern_call(expr: &Expr) -> Option<(&ExternFnRef, Span)> {
    // Recursion choke point: recurses once per tree level.
    crate::stack::with_stack_growth(|| find_extern_call_inner(expr))
}

fn find_extern_call_inner(expr: &Expr) -> Option<(&ExternFnRef, Span)> {
    match &expr.kind {
        ExprKind::Error { children } => children.iter().find_map(find_extern_call),
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::VariantLiteral(_) => None,
        ExprKind::FnCall { callee, args, .. } => {
            if let FunctionRef::External(ext) = &callee.value {
                return Some((ext, callee.span));
            }
            args.iter().find_map(find_extern_call)
        }
        ExprKind::BinOp { lhs, rhs, .. } => find_extern_call(lhs).or_else(|| find_extern_call(rhs)),
        ExprKind::UnaryOp { operand, .. }
        | ExprKind::Convert { expr: operand, .. }
        | ExprKind::DisplayTimezone { expr: operand, .. }
        | ExprKind::FieldAccess { expr: operand, .. } => find_extern_call(operand),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => find_extern_call(condition)
            .or_else(|| find_extern_call(then_branch))
            .or_else(|| find_extern_call(else_branch)),
        ExprKind::ConstructorCall { fields, .. } => fields
            .iter()
            .find_map(|field| find_extern_call(&field.value)),
        ExprKind::MapLiteral { entries } => entries
            .iter()
            .find_map(|entry| find_extern_call(&entry.value)),
        ExprKind::ForComp { body, .. } => find_extern_call(body),
        ExprKind::IndexAccess { expr: inner, args } => find_extern_call(inner).or_else(|| {
            args.iter().find_map(|arg| match arg {
                IndexArg::Expr(e) => find_extern_call(e),
                IndexArg::Variant(_) | IndexArg::Var(_) => None,
            })
        }),
        ExprKind::Scan {
            source, init, body, ..
        } => find_extern_call(source)
            .or_else(|| find_extern_call(init))
            .or_else(|| find_extern_call(body)),
        ExprKind::Unfold { init, body, .. } => {
            find_extern_call(init).or_else(|| find_extern_call(body))
        }
        ExprKind::KeyForm { arg, .. } => find_extern_call(arg),
        ExprKind::Match { scrutinee, arms } => find_extern_call(scrutinee)
            .or_else(|| arms.iter().find_map(|arm| find_extern_call(&arm.body))),
        ExprKind::DagCall { args, .. } => args
            .iter()
            .find_map(|binding| find_extern_call(&binding.value)),
    }
}

/// A lowered assertion body.
#[derive(Debug, Clone)]
pub enum AssertBody {
    Expr(Box<Expr>),
    Tolerance {
        actual: Box<Expr>,
        expected: Box<Expr>,
        tolerance: Box<Expr>,
    },
}

/// Field initializer after expression lowering.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Spanned<FieldName>,
    pub value: Expr,
}

/// A named param binding in a DAG call expression.
#[derive(Debug, Clone)]
pub struct ParamBinding {
    pub target: Spanned<ResolvedDeclName>,
    pub value: Expr,
}

/// Canonical Static substitutions supplied by one direct DAG call.
#[derive(Debug, Clone, Default)]
pub struct DagCallStaticBindings {
    pub types: HashMap<ResolvedStructTypeName, ResolvedStructTypeName>,
    pub dimensions: HashMap<ResolvedDimName, ResolvedDimName>,
    pub indexes: HashMap<ResolvedIndexName, DagCallIndexBinding>,
}

/// Concrete index target supplied to a direct DAG call.
#[derive(Debug, Clone)]
pub enum DagCallIndexBinding {
    Declared(ResolvedIndexName),
    Finite(crate::registry::types::FiniteIndex),
}

/// A resolved map literal entry.
#[derive(Debug, Clone)]
pub struct MapEntry {
    pub keys: NonEmpty<MapEntryKey>,
    pub value: Expr,
}

/// A single resolved map key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapEntryKey {
    IndexVariant(IndexVariantRef),
    FinitePosition { size: u64, position: Spanned<u64> },
}

/// A resolved for-comprehension binding.
#[derive(Debug, Clone)]
pub struct ForBinding {
    pub local: LocalDef,
    pub index: ForBindingIndex,
}

/// Index target in a for-comprehension binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForBindingIndex {
    Named(Spanned<ResolvedIndexName>),
    Finite { cardinality: NatExpr, span: Span },
}

/// A resolved index-access argument.
#[derive(Debug, Clone)]
pub enum IndexArg {
    Variant(IndexVariantRef),
    Var(Spanned<LocalId>),
    Expr(Box<Expr>),
}

/// One lowered match arm.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr,
    pub span: Span,
}

/// Resolved match pattern.
#[derive(Debug, Clone)]
pub enum MatchPattern {
    Constructor {
        constructor: Spanned<ResolvedConstructorName>,
        bindings: ast::PatternBindings<PatternBinding>,
        span: Span,
    },
    IndexLabel {
        variant: IndexVariantRef,
        span: Span,
    },
}

impl MatchPattern {
    fn bound_locals(&self) -> Vec<LocalDef> {
        match self {
            Self::Constructor { bindings, .. } => bindings
                .as_slice()
                .iter()
                .filter_map(|binding| match binding {
                    PatternBinding::Bind { local, .. } => Some(local.clone()),
                    PatternBinding::Wildcard { .. } => None,
                })
                .collect(),
            Self::IndexLabel { .. } => Vec::new(),
        }
    }
}

/// Binding inside a constructor match pattern.
#[derive(Debug, Clone)]
pub enum PatternBinding {
    Bind {
        field: Spanned<FieldName>,
        local: LocalDef,
    },
    Wildcard {
        field: Spanned<FieldName>,
        span: Span,
    },
}

/// Returns `true` when a custom type-inference rule, rather than the ordinary
/// built-in signature registry, owns arity validation.
const fn builtin_has_type_checker_arity(name: BuiltinFnName) -> bool {
    name.linear_algebra().is_some() || name.aggregation().is_some()
}

struct ExprLowerer<'a> {
    ctx: ExprLoweringContext<'a>,
    local_scopes: Vec<HashMap<LocalName, LocalDef>>,
    next_local: u32,
    diagnostics: Vec<ExprLowerError>,
}

fn static_binding_value_path(
    binding: &ast::ParamBinding,
    owner: &DagId,
) -> Result<NamePath, ExprLowerError> {
    match &binding.value.kind {
        ast::ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => Ok(path.to_name_path()),
        _ => Err(ExprLowerError::ModuleResolve {
            source: ModuleResolveError::UnknownName {
                owner: owner.clone(),
                namespace: "Static",
                name: binding.name.name.to_string(),
            },
            span: binding.value.span,
        }),
    }
}

impl<'a> ExprLowerer<'a> {
    const fn new(ctx: ExprLoweringContext<'a>) -> Self {
        Self {
            ctx,
            local_scopes: Vec::new(),
            next_local: 0,
            diagnostics: Vec::new(),
        }
    }

    /// Lower one expression level, localizing failures.
    ///
    /// A failed parent becomes an [`ExprKind::Error`] node that retains every
    /// independently lowerable descendant expression. Any tentative child
    /// lowering performed before the parent failed is rolled back first, so
    /// diagnostics and lexical IDs are emitted exactly once.
    fn lower_expr(&mut self, expr: &ast::Expr) -> Expr {
        let diagnostics_checkpoint = self.diagnostics.len();
        let next_local_checkpoint = self.next_local;
        let scope_depth_checkpoint = self.local_scopes.len();

        // Recursion choke point: lowering recurses once per tree level
        // (unbounded for left-nested operator chains).
        crate::stack::with_stack_growth(|| match self.lower_expr_inner(expr) {
            Ok(lowered) => lowered,
            Err(err) => {
                self.diagnostics.truncate(diagnostics_checkpoint);
                self.next_local = next_local_checkpoint;
                self.local_scopes.truncate(scope_depth_checkpoint);
                self.diagnostics.push(err);
                let children = self.lower_error_children(expr);
                Expr::new(ExprKind::Error { children }, expr.span)
            }
        })
    }

    /// Lower only expression-valued children of a failed parent.
    ///
    /// Binder metadata is intentionally ignored: when a `for`, `scan`,
    /// `unfold`, or match pattern fails, its lexical bindings were never
    /// established, so retaining a body must not fabricate those locals.
    fn lower_error_children(&mut self, expr: &ast::Expr) -> Vec<Expr> {
        let children: Vec<&ast::Expr> = match &expr.kind {
            ast::ExprKind::Number(_)
            | ast::ExprKind::Integer(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::StringLiteral(_)
            | ast::ExprKind::UnresolvedRef(_)
            | ast::ExprKind::GraphRef(_)
            | ast::ExprKind::QuantityLiteral { .. } => Vec::new(),
            ast::ExprKind::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
            ast::ExprKind::UnaryOp { operand, .. } => vec![operand],
            ast::ExprKind::FnCall { args, .. } => args.iter().collect(),
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => vec![condition, then_branch, else_branch],
            ast::ExprKind::Convert { expr, .. }
            | ast::ExprKind::DisplayTimezone { expr, .. }
            | ast::ExprKind::FieldAccess { expr, .. } => vec![expr],
            ast::ExprKind::ConstructorCall { fields, .. } => {
                fields.iter().map(|field| &field.value).collect()
            }
            ast::ExprKind::MapLiteral { entries } => {
                entries.iter().map(|entry| &entry.value).collect()
            }
            ast::ExprKind::ForComp { body, .. } => vec![body],
            ast::ExprKind::IndexAccess { expr, args } => std::iter::once(expr.as_ref())
                .chain(args.iter().filter_map(|arg| match arg {
                    ast::IndexArg::Expr(expr) => Some(expr.as_ref()),
                    ast::IndexArg::Variant { .. } | ast::IndexArg::Var(_) => None,
                }))
                .collect(),
            ast::ExprKind::Scan {
                source, init, body, ..
            } => vec![source, init, body],
            ast::ExprKind::Unfold { init, body, .. } => vec![init, body],
            ast::ExprKind::KeyForm { arg, .. } => vec![arg],
            ast::ExprKind::Match { scrutinee, arms } => std::iter::once(scrutinee.as_ref())
                .chain(arms.iter().map(|arm| &arm.body))
                .collect(),
            ast::ExprKind::InlineDagRef { args, .. } => args.iter().map(|arg| &arg.value).collect(),
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            ast::ExprKind::Sugar(sugar) => never(*sugar),
        };
        children
            .into_iter()
            .map(|child| self.lower_expr(child))
            .collect()
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind lowering")]
    fn lower_expr_inner(&mut self, expr: &ast::Expr) -> Result<Expr, ExprLowerError> {
        let kind = match &expr.kind {
            ast::ExprKind::Number(value) => ExprKind::Number(*value),
            ast::ExprKind::Integer(value) => ExprKind::Integer(*value),
            ast::ExprKind::Bool(value) => ExprKind::Bool(*value),
            ast::ExprKind::StringLiteral(value) => ExprKind::StringLiteral(value.clone()),
            ast::ExprKind::UnresolvedRef(unresolved) => match unresolved {
                UnresolvedRef::Path(path) => self.lower_unresolved_path(path)?,
                UnresolvedRef::IndexLabel { index, label, .. } => {
                    let index_path = index.to_name_path();
                    let resolved = self.resolve_index_variant_parts(
                        &index_path,
                        &label.value,
                        index.span(),
                        label.span,
                    )?;
                    ExprKind::VariantLiteral(IndexVariantRef {
                        variant: resolved,
                        index_span: Some(index.span()),
                        additional_index_spans: Vec::new(),
                        variant_span: label.span,
                    })
                }
            },
            ast::ExprKind::GraphRef(name) => self.lower_graph_ref(name)?,
            ast::ExprKind::BinOp { op, lhs, rhs } => ExprKind::BinOp {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)),
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::UnaryOp { op, operand } => ExprKind::UnaryOp {
                op: *op,
                operand: Box::new(self.lower_expr(operand)),
            },
            ast::ExprKind::FnCall {
                callee,
                generic_args,
                args,
            } => match self.resolve_callable(callee)? {
                ResolvedCallable::Function(function_ref) => {
                    let function_ref = Self::lower_function_application(
                        function_ref,
                        generic_args,
                        callee.display_path(),
                        callee.span(),
                    )?;
                    Self::check_function_arity(&function_ref, args.len(), callee.span())?;
                    let args = self.lower_function_args(&function_ref, args)?;
                    ExprKind::FnCall {
                        callee: Spanned::new(function_ref, callee.span()),
                        args,
                    }
                }
                ResolvedCallable::Constructor(constructor) => {
                    return Err(if args.is_empty() {
                        ExprLowerError::EmptyParenthesizedConstructor {
                            constructor,
                            span: expr.span,
                        }
                    } else {
                        ExprLowerError::PositionalArgumentsOnConstructor {
                            constructor,
                            span: expr.span,
                        }
                    });
                }
            },
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => ExprKind::If {
                condition: Box::new(self.lower_expr(condition)),
                then_branch: Box::new(self.lower_expr(then_branch)),
                else_branch: Box::new(self.lower_expr(else_branch)),
            },
            ast::ExprKind::QuantityLiteral { value, unit } => ExprKind::QuantityLiteral {
                value: *value,
                unit: self.lower_unit_expr(unit)?,
            },
            ast::ExprKind::Convert { expr, target } => ExprKind::Convert {
                expr: Box::new(self.lower_expr(expr)),
                target: self.lower_unit_expr(target)?,
            },
            ast::ExprKind::DisplayTimezone {
                expr: operand,
                timezone,
            } => ExprKind::DisplayTimezone {
                expr: Box::new(self.lower_expr(operand)),
                timezone: self.lower_iana_time_zone_id(timezone, expr.span)?,
            },
            ast::ExprKind::FieldAccess { expr, field } => ExprKind::FieldAccess {
                expr: Box::new(self.lower_expr(expr)),
                field: field.clone(),
            },
            ast::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => {
                let resolved = match self.resolve_callable(callee)? {
                    ResolvedCallable::Constructor(constructor) => constructor,
                    ResolvedCallable::Function(function) => {
                        return Err(ExprLowerError::NamedArgumentsOnFunction {
                            function,
                            argument_names: fields
                                .iter()
                                .map(|field| field.name.value.clone())
                                .collect(),
                            span: expr.span,
                        });
                    }
                };
                let params = self
                    .ctx
                    .resolver
                    .constructor_generic_params(&resolved)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: callee.span(),
                    })?;
                let lowered_args = lower_generic_args(
                    resolved.as_str(),
                    params,
                    generic_args,
                    expr.span,
                    self.ctx.type_context(),
                )?;
                ExprKind::ConstructorCall {
                    callee: Spanned::new(resolved, callee.span()),
                    generic_args: lowered_args,
                    fields: fields
                        .iter()
                        .map(|field| self.lower_field_init(field))
                        .collect(),
                }
            }
            ast::ExprKind::MapLiteral { entries } => ExprKind::MapLiteral {
                entries: entries
                    .iter()
                    .map(|entry| self.lower_map_entry(entry, expr.span))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::ForComp { bindings, body } => {
                let bindings = bindings
                    .iter()
                    .map(|binding| self.lower_for_binding(binding))
                    .collect::<Result<Vec<_>, _>>()?;
                let locals = bindings
                    .iter()
                    .map(|binding| binding.local.clone())
                    .collect::<Vec<_>>();
                self.push_scope(locals)?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::ForComp { bindings, body }
            }
            ast::ExprKind::IndexAccess { expr, args } => ExprKind::IndexAccess {
                expr: Box::new(self.lower_expr(expr)),
                args: args.try_map_ref(|arg| self.lower_index_arg(arg))?,
            },
            ast::ExprKind::Scan {
                source,
                init,
                acc_name,
                val_name,
                body,
            } => {
                let source = Box::new(self.lower_expr(source));
                let init = Box::new(self.lower_expr(init));
                let acc = self.allocate_local(acc_name.value.clone(), acc_name.span)?;
                let val = self.allocate_local(val_name.value.clone(), val_name.span)?;
                self.push_scope(vec![acc.clone(), val.clone()])?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::Scan {
                    source,
                    init,
                    acc,
                    val,
                    body,
                }
            }
            ast::ExprKind::Unfold {
                axis,
                init,
                prev_state_name,
                prev_index_name,
                index_name,
                body,
            } => {
                let resolved_axis = self
                    .ctx
                    .resolver
                    .resolve_index_path(self.ctx.owner, &axis.value)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: axis.span,
                    })?;
                let init = Box::new(self.lower_expr(init));
                let prev_state =
                    self.allocate_local(prev_state_name.value.clone(), prev_state_name.span)?;
                let prev_index =
                    self.allocate_local(prev_index_name.value.clone(), prev_index_name.span)?;
                let index = self.allocate_local(index_name.value.clone(), index_name.span)?;
                self.push_scope(vec![prev_state.clone(), prev_index.clone(), index.clone()])?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::Unfold {
                    recurrence: Box::new(UnfoldRecurrence {
                        axis: Spanned::new(resolved_axis, axis.span),
                        previous_state: prev_state,
                        previous_index: prev_index,
                        current_index: index,
                    }),
                    init,
                    body,
                }
            }
            ast::ExprKind::KeyForm { kind, axis, arg } => {
                let lowered_axis = match axis {
                    crate::syntax::ast::IndexExpr::Name(path) => {
                        let resolved = self
                            .ctx
                            .resolver
                            .resolve_index_path(self.ctx.owner, &path.value)
                            .map_err(|source| ExprLowerError::ModuleResolve {
                                source,
                                span: path.span,
                            })?;
                        ForBindingIndex::Named(Spanned::new(resolved, path.span))
                    }
                    crate::syntax::ast::IndexExpr::Finite { cardinality, span } => {
                        ForBindingIndex::Finite {
                            cardinality: lower_nat_expr(cardinality, self.ctx.type_context())?,
                            span: *span,
                        }
                    }
                    crate::syntax::ast::IndexExpr::BareNat(nat_expr) => {
                        return Err(crate::hir::lower::HirLowerError::ExpectedIndexFoundNat {
                            expression: nat_expr.to_string(),
                            span: nat_expr.span(),
                        }
                        .into());
                    }
                };
                ExprKind::KeyForm {
                    kind: *kind,
                    axis: lowered_axis,
                    axis_span: axis.span(),
                    arg: Box::new(self.lower_expr(arg)),
                }
            }
            ast::ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(self.lower_expr(scrutinee)),
                arms: arms
                    .iter()
                    .map(|arm| self.lower_match_arm(arm))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::InlineDagRef { path, args, output } => {
                let target = self
                    .ctx
                    .resolver
                    .resolve_module_path(self.ctx.owner, path)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: path.span(),
                    })?;
                let (lowered_args, static_bindings) =
                    self.lower_dag_call_bindings(&target, args)?;
                let output_path = NamePath::local(output.value.atom().clone());
                let lowered_output = self
                    .ctx
                    .resolver
                    .resolve_decl_path(&target, &output_path)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: output.span,
                    })?;
                ExprKind::DagCall {
                    target: Spanned::new(target, path.span()),
                    args: lowered_args,
                    static_bindings,
                    output: Spanned::new(lowered_output, output.span),
                }
            }
            // `Sugar(_)` payload is `Infallible` post-desugar.
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            ast::ExprKind::Sugar(s) => never(*s),
        };
        Ok(Expr::new(kind, expr.span))
    }

    fn lower_unit_expr(&self, unit: &ast::UnitExpr) -> Result<ResolvedUnitExpr, ExprLowerError> {
        let terms = unit
            .terms
            .iter()
            .map(|item| {
                let reference = &item.name.value;
                let path = reference.to_name_path();
                let resolved = match self
                    .ctx
                    .unit_bindings
                    .and_then(|bindings| bindings.get(reference))
                    .cloned()
                {
                    Some(resolved) => resolved,
                    None => match self.ctx.resolver.resolve_unit_path(self.ctx.owner, &path) {
                        Ok(resolved) => resolved,
                        Err(ModuleResolveError::UnknownName { .. }) => self
                            .ctx
                            .resolve_prelude_unit_ref(reference)
                            .or_else(|| self.ctx.resolve_registry_unit_ref(reference))
                            .ok_or_else(|| ExprLowerError::UnknownUnit {
                                name: reference.clone(),
                                span: item.name.span,
                            })?,
                        Err(source) => {
                            return Err(ExprLowerError::ModuleResolve {
                                source,
                                span: item.name.span,
                            });
                        }
                    },
                };
                Ok(ResolvedUnitExprItem {
                    op: item.op,
                    name: Spanned::new(
                        ResolvedUnitRef::new(reference.clone(), resolved),
                        item.name.span,
                    ),
                    power: item.effective_power(),
                })
            })
            .collect::<Result<Vec<_>, ExprLowerError>>()?;
        Ok(ResolvedUnitExpr {
            terms,
            span: unit.span,
        })
    }

    /// Resolve a syntactic reference path in value position.
    ///
    /// This is the single classification point of the compiler: it decides,
    /// in one pass, whether a path names a lexical local, a built-in constant,
    /// a constructor, a type-system entity, a generic `Nat` parameter, or a
    /// declaration — and resolves it to its canonical identity at the same
    /// time. Lexical scope shadows module symbols. Time scales are resolved
    /// only by the dedicated Static contexts that consume them.
    fn lower_unresolved_path(&self, path: &IdentPath) -> Result<ExprKind, ExprLowerError> {
        path.as_bare().map_or_else(
            || self.lower_dotted_path_ref(path),
            |ident| self.lower_bare_name_ref(ident),
        )
    }

    /// Resolve a bare identifier in value position.
    ///
    /// Priority:
    /// 1. Lexical locals (for/scan/unfold/match bindings)
    /// 2. Built-in Term constants (`PI`, `E`, ...)
    /// 3. Constructors (a bare constructor name is a nullary call)
    /// 4. Type-system names (struct types, dimensions, indexes, variants)
    /// 5. Generic `Nat` parameters and declarations (const/node/param)
    ///
    /// A time-scale spelling participates only after Term lookup fails, to
    /// produce a targeted wrong-namespace diagnostic rather than resolving a
    /// Static atom into the expression HIR.
    fn lower_bare_name_ref(&self, ident: &Ident) -> Result<ExprKind, ExprLowerError> {
        let span = ident.span;
        if let Ok(local) = self.lookup_local(&LocalName::from_atom(ident.name.clone()), span) {
            return Ok(ExprKind::LocalRef(Spanned::new(local, span)));
        }
        if let Some(builtin) = BuiltinConst::parse(ident.name.as_str()) {
            return Ok(ExprKind::ConstRef(Spanned::new(
                ConstRef::Builtin(builtin),
                span,
            )));
        }
        let path = NamePath::local(ident.name.clone());
        let constructor_result = self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &path);
        if let Ok(constructor) = constructor_result {
            return Ok(ExprKind::ConstructorCall {
                callee: Spanned::new(constructor, span),
                generic_args: Vec::new(),
                fields: Vec::new(),
            });
        }

        let scoped_name = ScopedName::from(ident.name.clone());
        match self.resolve_decl_scoped_name(&scoped_name, span) {
            Ok(resolved) => {
                let kind = self
                    .ctx
                    .resolver
                    .decl_symbol_kind(&resolved)
                    .map_err(|source| ExprLowerError::ModuleResolve { source, span })?;
                Err(ExprLowerError::BareGraphDeclarationRef {
                    name: scoped_name,
                    kind,
                    span,
                })
            }
            Err(ExprLowerError::UnknownGraphRef { .. }) => {
                ident.name.as_str().parse::<TimeScale>().map_or_else(
                    |_| {
                        Err(ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownName {
                                owner: self.ctx.owner.clone(),
                                namespace: "Term",
                                name: ident.name.to_string(),
                            },
                            span,
                        })
                    },
                    |scale| Err(ExprLowerError::TimeScaleInValuePosition { scale, span }),
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve a Term member selected by an explicit `::` boundary. Index
    /// labels never enter this path; the parser represents `Owner#Label`
    /// separately.
    fn lower_dotted_path_ref(&self, path: &IdentPath) -> Result<ExprKind, ExprLowerError> {
        let span = path.span();
        let scoped = ScopedName::from(path.to_name_path());
        self.lower_const_ref(&scoped, span)
            .map(|const_ref| ExprKind::ConstRef(Spanned::new(const_ref, span)))
    }

    fn lower_field_init(&mut self, field: &ast::FieldInit) -> FieldInit {
        FieldInit {
            name: field.name.clone(),
            value: self.lower_expr(&field.value),
        }
    }

    fn lower_dag_call_bindings(
        &mut self,
        target: &DagId,
        bindings: &[ast::ParamBinding],
    ) -> Result<(Vec<ParamBinding>, DagCallStaticBindings), ExprLowerError> {
        let mut params = Vec::new();
        let mut static_bindings = DagCallStaticBindings::default();
        for binding in bindings {
            let input_path = NamePath::local(binding.name.name.clone());
            match binding.category {
                InputBindingCategory::Unmarked => {
                    params.push(self.lower_param_binding(target, binding)?);
                }
                InputBindingCategory::Type => {
                    let input = self
                        .ctx
                        .resolver
                        .resolve_struct_type_path(target, &input_path)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: binding.name.span,
                        })?;
                    let value_path = static_binding_value_path(binding, self.ctx.owner)?;
                    let value = self
                        .ctx
                        .resolver
                        .resolve_struct_type_path(self.ctx.owner, &value_path)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: binding.value.span,
                        })?;
                    static_bindings.types.insert(input, value);
                }
                InputBindingCategory::Dimension => {
                    let input = self
                        .ctx
                        .resolver
                        .resolve_dimension_path(target, &input_path)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: binding.name.span,
                        })?;
                    let value_path = static_binding_value_path(binding, self.ctx.owner)?;
                    let value = match self
                        .ctx
                        .resolver
                        .resolve_dimension_path(self.ctx.owner, &value_path)
                    {
                        Ok(value) => value,
                        Err(source @ ModuleResolveError::UnknownName { .. }) => self
                            .ctx
                            .resolve_prelude_dimension_path(&value_path)
                            .ok_or(ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            })?,
                        Err(source) => {
                            return Err(ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            });
                        }
                    };
                    static_bindings.dimensions.insert(input, value);
                }
                InputBindingCategory::Index => {
                    let input = self
                        .ctx
                        .resolver
                        .resolve_index_path(target, &input_path)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: binding.name.span,
                        })?;
                    let value = match binding.value.index_binding_arg() {
                        Some(ast::IndexExpr::Name(path)) => self
                            .ctx
                            .resolver
                            .resolve_index_path(self.ctx.owner, &path.value)
                            .map(DagCallIndexBinding::Declared)
                            .map_err(|source| ExprLowerError::ModuleResolve {
                                source,
                                span: binding.value.span,
                            })?,
                        Some(ast::IndexExpr::Finite {
                            cardinality: ast::NatExpr::Literal(cardinality, _),
                            ..
                        }) => crate::registry::types::FiniteIndex::try_from_u64(cardinality)
                            .map(DagCallIndexBinding::Finite)
                            .map_err(|_| ExprLowerError::InvalidStaticBindingValue {
                                name: binding.name.name.clone(),
                                span: binding.value.span,
                            })?,
                        _ => {
                            return Err(ExprLowerError::InvalidStaticBindingValue {
                                name: binding.name.name.clone(),
                                span: binding.value.span,
                            });
                        }
                    };
                    static_bindings.indexes.insert(input, value);
                }
            }
        }
        Ok((params, static_bindings))
    }

    fn lower_param_binding(
        &mut self,
        target: &DagId,
        binding: &ast::ParamBinding,
    ) -> Result<ParamBinding, ExprLowerError> {
        let path = NamePath::local(binding.name.name.clone());
        let target_name = self
            .ctx
            .resolver
            .resolve_decl_path(target, &path)
            .map_err(|source| ExprLowerError::ModuleResolve {
                source,
                span: binding.name.span,
            })?;
        Ok(ParamBinding {
            target: Spanned::new(target_name, binding.name.span),
            value: self.lower_expr(&binding.value),
        })
    }

    fn lower_const_ref(&self, name: &ScopedName, span: Span) -> Result<ConstRef, ExprLowerError> {
        if !name.is_qualified() {
            if let Some(builtin) = BuiltinConst::parse(name.member().as_str()) {
                return Ok(ConstRef::Builtin(builtin));
            }
            let generic_name = GenericParamName::from_atom(name.member().atom().clone());
            if let Some(binding) = self.ctx.generic_scope.get(&generic_name)
                && binding.constraint == ast::GenericConstraint::Nat
            {
                return Ok(ConstRef::GenericNatParam(binding.id.clone()));
            }
        }

        let path = name.to_name_path();
        let mut first_error = None;

        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            self.ensure_bound_decl_access(name, &resolved, span)?;
            let actual = self
                .ctx
                .resolver
                .decl_symbol_kind(&resolved)
                .map_err(|source| ExprLowerError::ModuleResolve { source, span })?;
            return if actual.is_const() {
                Ok(ConstRef::Decl(resolved))
            } else {
                Err(ExprLowerError::ModuleResolve {
                    source: ModuleResolveError::UnexpectedDeclKind {
                        name: resolved,
                        expected: "const",
                        actual,
                    },
                    span,
                })
            };
        }

        match self
            .ctx
            .resolver
            .resolve_const_decl_path(self.ctx.owner, &path)
        {
            Ok(resolved) => return Ok(ConstRef::Decl(resolved)),
            Err(err) => first_error.get_or_insert(err),
        };
        if let Some(resolved) = self.resolve_synthetic_child_decl_path(&path)
            && self
                .ctx
                .resolver
                .decl_symbol_kind(&resolved)
                .is_ok_and(DeclSymbolKind::is_const)
        {
            return Ok(ConstRef::Decl(resolved));
        }
        match self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &path)
        {
            Ok(resolved) => return Ok(ConstRef::Constructor(resolved)),
            Err(err) => first_error.get_or_insert(err),
        };

        first_error.map_or_else(
            || {
                Err(ExprLowerError::ModuleResolve {
                    source: ModuleResolveError::UnknownName {
                        owner: self.ctx.owner.clone(),
                        namespace: DeclNameNamespace::DISPLAY_NAME,
                        name: name.to_string(),
                    },
                    span,
                })
            },
            |source| Err(ExprLowerError::ModuleResolve { source, span }),
        )
    }

    fn lower_graph_ref(&self, name: &Spanned<ScopedName>) -> Result<ExprKind, ExprLowerError> {
        let resolved = self.resolve_decl_scoped_name(&name.value, name.span)?;
        let kind_identity = match self
            .ctx
            .resolver
            .resolve_decl_path(self.ctx.owner, &name.value.to_name_path())
        {
            Ok(identity) => identity,
            Err(source @ ModuleResolveError::PrivateName { .. }) => {
                return Err(ExprLowerError::ModuleResolve {
                    source,
                    span: name.span,
                });
            }
            Err(_) => resolved.clone(),
        };
        let kind = match self.ctx.resolver.decl_symbol_kind(&kind_identity) {
            Ok(kind) => kind,
            Err(_)
                if self
                    .ctx
                    .decl_bindings
                    .is_some_and(|bindings| bindings.contains_key(&name.value)) =>
            {
                return Ok(ExprKind::GraphRef(Spanned::new(resolved, name.span)));
            }
            Err(source) => {
                return Err(ExprLowerError::ModuleResolve {
                    source,
                    span: name.span,
                });
            }
        };
        let role = name
            .value
            .qualifier()
            .first()
            .and_then(|alias| self.ctx.resolver.module_alias_role(self.ctx.owner, alias));
        let permitted = match role {
            Some(ModuleAliasRole::ImportedDag) => kind == DeclSymbolKind::Const,
            Some(ModuleAliasRole::IncludedInstance) | None => {
                matches!(
                    kind,
                    DeclSymbolKind::Const | DeclSymbolKind::Param | DeclSymbolKind::Node
                )
            }
        };
        if !permitted {
            return Err(ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnexpectedDeclKind {
                    name: resolved,
                    expected: match role {
                        Some(ModuleAliasRole::ImportedDag) => "instance-independent const",
                        Some(ModuleAliasRole::IncludedInstance) | None => "graph value",
                    },
                    actual: kind,
                },
                span: name.span,
            });
        }
        Ok(ExprKind::GraphRef(Spanned::new(resolved, name.span)))
    }

    fn resolve_decl_scoped_name(
        &self,
        name: &ScopedName,
        span: Span,
    ) -> Result<ResolvedDeclName, ExprLowerError> {
        let path = name.to_name_path();
        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            self.ensure_bound_decl_access(name, &resolved, span)?;
            return Ok(resolved);
        }
        let resolved = match self.ctx.resolver.resolve_decl_path(self.ctx.owner, &path) {
            Ok(resolved) => Ok(resolved),
            // Synthetic include-instance children intentionally are not module
            // aliases. Retry only when the qualifier itself is absent; once
            // the resolver reaches a real alias, its rejection is authoritative.
            Err(source @ ModuleResolveError::UnknownModuleAlias { .. }) => {
                self.resolve_synthetic_child_decl_path(&path).ok_or(source)
            }
            Err(source) => Err(source),
        };
        resolved.map_err(|source| match source {
            ModuleResolveError::UnknownName { .. } => ExprLowerError::UnknownGraphRef {
                name: name.clone(),
                span,
            },
            source => ExprLowerError::ModuleResolve { source, span },
        })
    }

    fn ensure_bound_decl_access(
        &self,
        name: &ScopedName,
        resolved: &ResolvedDeclName,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        if !name.is_qualified() {
            return Ok(());
        }

        match self
            .ctx
            .resolver
            .resolve_decl_path(self.ctx.owner, &name.to_name_path())
        {
            Ok(_) => Ok(()),
            Err(
                ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownName { .. },
            ) => {
                let Some(template) = self
                    .ctx
                    .instance_templates
                    .and_then(|templates| templates.get(resolved.owner()))
                else {
                    return Ok(());
                };
                if template == self.ctx.owner {
                    return Ok(());
                }

                let template_path = NamePath::local(resolved.atom().clone());
                let template_name = self
                    .ctx
                    .resolver
                    .resolve_decl_path(template, &template_path)
                    .unwrap_or_else(|_| {
                        ResolvedDeclName::from_def(template.clone(), resolved.to_unowned_def_name())
                    });
                if self
                    .ctx
                    .resolver
                    .decl_symbol_is_instance_accessible(&template_name)
                    .map_err(|source| ExprLowerError::ModuleResolve { source, span })?
                {
                    Ok(())
                } else {
                    Err(ExprLowerError::ModuleResolve {
                        source: ModuleResolveError::PrivateName {
                            owner: resolved.owner().clone(),
                            namespace: DeclNameNamespace::DISPLAY_NAME,
                            name: resolved.as_str().to_string(),
                        },
                        span,
                    })
                }
            }
            Err(source) => Err(ExprLowerError::ModuleResolve { source, span }),
        }
    }

    fn resolve_synthetic_child_decl_path(&self, path: &NamePath) -> Option<ResolvedDeclName> {
        let (qualifier, leaf) = path.qualifier_and_leaf()?;
        let owner = qualifier
            .iter()
            .fold(self.ctx.owner.clone(), |owner, segment| {
                owner.child(segment.as_str())
            });
        self.ctx.resolver.modules().get(&owner).and_then(|module| {
            let decl_name = DeclName::from_atom(leaf.clone());
            module
                .decls()
                .contains_key(&decl_name)
                .then(|| ResolvedDeclName::from_def(owner, decl_name))
        })
    }

    fn lower_function_application(
        function_ref: FunctionRef,
        generic_args: &[ast::GenericArg],
        path: String,
        callee_span: Span,
    ) -> Result<FunctionRef, ExprLowerError> {
        match function_ref {
            FunctionRef::Builtin(BuiltinFnName::Epoch) => {
                Self::lower_epoch_function_ref(generic_args, callee_span)
            }
            other if generic_args.is_empty() => Ok(other),
            _ => Err(ExprLowerError::UnsupportedFunctionGenericArgs {
                path,
                span: generic_args
                    .first()
                    .map_or(callee_span, ast::GenericArg::span),
            }),
        }
    }

    fn lower_epoch_function_ref(
        generic_args: &[ast::GenericArg],
        callee_span: Span,
    ) -> Result<FunctionRef, ExprLowerError> {
        let [arg] = generic_args else {
            return Err(ExprLowerError::EpochTimeScaleArgumentCount {
                got: generic_args.len(),
                span: generic_args
                    .first()
                    .map_or(callee_span, ast::GenericArg::span),
            });
        };
        let ast::GenericArg::Ambiguous(ast::AmbiguousGenericArg::Name(ident)) = arg else {
            return Err(ExprLowerError::InvalidEpochTimeScaleArgument { span: arg.span() });
        };
        let scale = ident.name.as_str().parse::<TimeScale>().map_err(|_| {
            ExprLowerError::UnsupportedEpochTimeScale {
                name: ident.name.clone(),
                span: ident.span,
            }
        })?;
        Ok(FunctionRef::Epoch {
            scale: Spanned::new(scale, ident.span),
        })
    }

    /// Lower constructor-context strings into parsed semantic datetime and
    /// timezone literals. Ordinary strings remain syntax-only HIR leaves.
    fn lower_function_args(
        &mut self,
        function_ref: &FunctionRef,
        args: &[ast::Expr],
    ) -> Result<Vec<Expr>, ExprLowerError> {
        match (function_ref, args) {
            (FunctionRef::Builtin(BuiltinFnName::Datetime), [datetime, time_zone]) => {
                self.lower_zoned_datetime_args(datetime, time_zone)
            }
            _ => args
                .iter()
                .enumerate()
                .map(
                    |(index, arg)| match (function_ref, index, args.len(), &arg.kind) {
                        (
                            FunctionRef::Builtin(BuiltinFnName::Datetime),
                            0,
                            1,
                            ast::ExprKind::StringLiteral(source),
                        ) => Self::lower_offset_datetime_literal(source, arg.span),
                        (
                            FunctionRef::Epoch { scale },
                            0,
                            1,
                            ast::ExprKind::StringLiteral(source),
                        ) => Self::lower_civil_datetime_literal(
                            source,
                            DatetimeLiteralExpectation::Epoch(scale.value),
                            arg.span,
                        ),
                        _ => Ok(self.lower_expr(arg)),
                    },
                )
                .collect(),
        }
    }

    fn lower_zoned_datetime_args(
        &mut self,
        datetime_arg: &ast::Expr,
        time_zone_arg: &ast::Expr,
    ) -> Result<Vec<Expr>, ExprLowerError> {
        let datetime = match &datetime_arg.kind {
            ast::ExprKind::StringLiteral(source) => Self::lower_civil_datetime_literal(
                source,
                DatetimeLiteralExpectation::ZonedCivilDateTime,
                datetime_arg.span,
            )?,
            _ => self.lower_expr(datetime_arg),
        };
        let time_zone = match &time_zone_arg.kind {
            ast::ExprKind::StringLiteral(source) => self
                .lower_iana_time_zone_id(source, time_zone_arg.span)
                .map(|time_zone| {
                    Expr::new(ExprKind::IanaTimeZoneLiteral(time_zone), time_zone_arg.span)
                })?,
            _ => self.lower_expr(time_zone_arg),
        };

        let resolution_inputs = match (&datetime.kind, &time_zone.kind) {
            (
                ExprKind::CivilDateTimeLiteral(datetime),
                ExprKind::IanaTimeZoneLiteral(time_zone),
            ) => Some((*datetime, time_zone.clone())),
            _ => None,
        };
        let datetime = match resolution_inputs {
            Some((datetime, time_zone_id)) => {
                ZonedDateTimeLiteral::resolve(datetime, time_zone_id, self.ctx.time_zones)
                    .map(|resolved| {
                        Expr::new(ExprKind::ZonedDateTimeLiteral(resolved), datetime_arg.span)
                    })
                    .map_err(|error| {
                        Self::lower_zoned_datetime_error(
                            error,
                            datetime_arg.span,
                            time_zone_arg.span,
                        )
                    })?
            }
            None => datetime,
        };
        Ok(vec![datetime, time_zone])
    }

    fn lower_zoned_datetime_error(
        error: ResolveZonedDateTimeLiteralError,
        datetime_span: Span,
        time_zone_span: Span,
    ) -> ExprLowerError {
        match error {
            ResolveZonedDateTimeLiteralError::Nonexistent {
                datetime,
                time_zone,
                before,
                after,
            } => ExprLowerError::NonexistentCivilDateTime {
                datetime,
                time_zone,
                before,
                after,
                datetime_span,
                time_zone_span,
            },
            ResolveZonedDateTimeLiteralError::Repeated {
                datetime,
                time_zone,
                before,
                after,
            } => ExprLowerError::RepeatedCivilDateTime {
                datetime,
                time_zone,
                before,
                after,
                datetime_span,
                time_zone_span,
            },
            ResolveZonedDateTimeLiteralError::TimeZoneRegistryInvariant { time_zone, source } => {
                ExprLowerError::TimeZoneRegistryInvariant {
                    time_zone,
                    reason: source.to_string(),
                    span: time_zone_span,
                }
            }
            error @ ResolveZonedDateTimeLiteralError::OutOfRange { .. } => {
                ExprLowerError::InvalidDatetimeLiteral {
                    expectation: DatetimeLiteralExpectation::ZonedCivilDateTime,
                    reason: error.to_string(),
                    span: datetime_span,
                }
            }
        }
    }

    fn lower_offset_datetime_literal(source: &str, span: Span) -> Result<Expr, ExprLowerError> {
        OffsetDateTimeLiteral::parse(source)
            .map(|literal| Expr::new(ExprKind::OffsetDateTimeLiteral(literal), span))
            .map_err(|error| ExprLowerError::InvalidDatetimeLiteral {
                expectation: DatetimeLiteralExpectation::OffsetDateTime,
                reason: error.to_string(),
                span,
            })
    }

    fn lower_civil_datetime_literal(
        source: &str,
        expectation: DatetimeLiteralExpectation,
        span: Span,
    ) -> Result<Expr, ExprLowerError> {
        CivilDateTimeLiteral::parse(source)
            .map(|literal| Expr::new(ExprKind::CivilDateTimeLiteral(literal), span))
            .map_err(|error| ExprLowerError::InvalidDatetimeLiteral {
                expectation,
                reason: error.to_string(),
                span,
            })
    }

    fn lower_iana_time_zone_id(
        &self,
        timezone: &str,
        span: Span,
    ) -> Result<IanaTimeZoneId, ExprLowerError> {
        self.ctx
            .time_zones
            .parse_iana_id(timezone)
            .map_err(|_| ExprLowerError::InvalidTimezone {
                timezone: timezone.to_string(),
                tzdb_version: self.ctx.time_zones.version().unwrap_or("unknown"),
                span,
            })
    }

    /// Validate a built-in call's argument count against the registry's
    /// arity table. Custom built-ins and externs defer shape checks to their
    /// typed rules.
    fn check_function_arity(
        function_ref: &FunctionRef,
        got: usize,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        let Some(builtin) = function_ref.builtin_name() else {
            return Ok(());
        };
        if builtin_has_type_checker_arity(builtin) {
            return Ok(());
        }
        let Some(function) = crate::registry::builtins::builtin_functions().get(&builtin) else {
            return Ok(());
        };
        if got != function.arity() {
            return Err(ExprLowerError::WrongArity {
                name: crate::syntax::function_name::FnName::expect_valid(builtin.as_str()),
                expected: function.arity(),
                got,
                span,
            });
        }
        Ok(())
    }

    /// Resolve one Term callee before argument-shape validation. The backing
    /// maps are an implementation detail; namespace construction guarantees
    /// that at most one callable category occupies the selected Term slot.
    fn resolve_callable(&self, callee: &IdentPath) -> Result<ResolvedCallable, ExprLowerError> {
        let constructor = self
            .ctx
            .resolver
            .resolve_constructor_ident_path(self.ctx.owner, callee);
        constructor.map(ResolvedCallable::Constructor).or_else(|_| {
            self.lower_function_ref(callee)
                .map(ResolvedCallable::Function)
        })
    }

    fn lower_function_ref(
        &self,
        callee: &crate::syntax::ast::IdentPath,
    ) -> Result<FunctionRef, ExprLowerError> {
        if let Some(ident) = callee.as_bare() {
            return BuiltinFnName::parse(ident.name.as_str())
                .map(FunctionRef::Builtin)
                .ok_or_else(|| ExprLowerError::UnknownFunction {
                    path: callee.display_path(),
                    span: callee.span(),
                });
        }
        // `alias::name(...)`: an extern call when `alias` is a plugin alias in
        // scope. Extern functions are only callable in this qualified form.
        if let Some((qualifiers, leaf)) = callee.qualifier_and_leaf()
            && let [qualifier] = qualifiers
            && let Some(target) = self
                .ctx
                .resolver
                .plugin_alias(self.ctx.owner, qualifier.name.as_str())
        {
            let name = crate::syntax::function_name::FnName::from_atom(leaf.name.clone());
            if !target.functions().contains_key(&name) {
                return Err(ExprLowerError::UnknownExternFunction {
                    alias: ModuleAliasName::from_atom(qualifier.name.clone()),
                    name,
                    span: callee.span(),
                });
            }
            return Ok(FunctionRef::External(ExternFnRef {
                plugin: target.path().clone(),
                alias: ModuleAliasName::from_atom(qualifier.name.clone()),
                name,
            }));
        }
        Err(ExprLowerError::UnknownFunction {
            path: callee.display_path(),
            span: callee.span(),
        })
    }

    fn lower_map_entry(
        &mut self,
        entry: &ast::MapEntry,
        map_span: Span,
    ) -> Result<MapEntry, ExprLowerError> {
        let keys = entry
            .keys
            .iter()
            .map(|key| self.lower_map_entry_key(key, map_span))
            .collect::<Result<Vec<_>, _>>()?;
        let mut keys = keys.into_iter();
        let Some(first) = keys.next() else {
            return Err(ExprLowerError::EmptyMapEntry {
                span: entry.value.span,
            });
        };
        Ok(MapEntry {
            keys: NonEmpty::new(first, keys.collect()),
            value: self.lower_expr(&entry.value),
        })
    }

    fn lower_map_entry_key(
        &self,
        key: &ast::MapEntryKey,
        map_span: Span,
    ) -> Result<MapEntryKey, ExprLowerError> {
        match (&key.index.value, &key.variant.value) {
            (
                crate::syntax::ast::MapEntryIndex::Named(index_path),
                IndexEntryKey::Named(variant_name),
            ) => {
                let variant = self
                    .resolve_index_variant_parts(
                        index_path,
                        variant_name,
                        key.index.span,
                        key.variant.span,
                    )
                    .map_err(|err| match err {
                        ExprLowerError::ModuleResolve {
                            source: ModuleResolveError::UnknownIndexVariant { index, variant },
                            ..
                        } => ExprLowerError::ExtraMapVariant {
                            index_name: index.to_unowned_def_name(),
                            variant_name: variant,
                            span: map_span,
                        },
                        err => err,
                    })?;
                Ok(MapEntryKey::IndexVariant(IndexVariantRef {
                    variant,
                    index_span: Some(key.index.span),
                    additional_index_spans: key.additional_index_spans.clone(),
                    variant_span: key.variant.span,
                }))
            }
            (
                crate::syntax::ast::MapEntryIndex::Finite(size),
                IndexEntryKey::Position(position),
            ) => Ok(MapEntryKey::FinitePosition {
                size: *size,
                position: Spanned::new(*position, key.variant.span),
            }),
            (crate::syntax::ast::MapEntryIndex::Named(_), IndexEntryKey::Position(_))
            | (crate::syntax::ast::MapEntryIndex::Finite(_), IndexEntryKey::Named(_)) => {
                Err(ExprLowerError::InvalidMapEntryKey {
                    span: key.variant.span,
                })
            }
        }
    }

    fn lower_for_binding(
        &mut self,
        binding: &ast::ForBinding,
    ) -> Result<ForBinding, ExprLowerError> {
        let local = self.allocate_local(binding.var.value.clone(), binding.var.span)?;
        let index = match &binding.index {
            ast::ForBindingIndex::Named(index) => {
                let resolved = self
                    .ctx
                    .resolver
                    .resolve_index_path(self.ctx.owner, &index.value)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: index.span,
                    })?;
                ForBindingIndex::Named(Spanned::new(resolved, index.span))
            }
            ast::ForBindingIndex::Finite { cardinality, span } => ForBindingIndex::Finite {
                cardinality: lower_nat_expr(cardinality, self.ctx.type_context())?,
                span: *span,
            },
        };
        Ok(ForBinding { local, index })
    }

    fn lower_index_arg(&mut self, arg: &ast::IndexArg) -> Result<IndexArg, ExprLowerError> {
        match arg {
            ast::IndexArg::Variant { index, variant } => {
                let resolved = self.resolve_index_variant_parts(
                    &index.value,
                    &variant.value,
                    index.span,
                    variant.span,
                )?;
                Ok(IndexArg::Variant(IndexVariantRef {
                    variant: resolved,
                    index_span: Some(index.span),
                    additional_index_spans: Vec::new(),
                    variant_span: variant.span,
                }))
            }
            ast::IndexArg::Var(ident) => Ok(IndexArg::Var(Spanned::new(
                self.lookup_local(&LocalName::from_atom(ident.name.clone()), ident.span)?,
                ident.span,
            ))),
            ast::IndexArg::Expr(expr) => Ok(IndexArg::Expr(Box::new(self.lower_expr(expr)))),
        }
    }

    fn lower_match_arm(&mut self, arm: &ast::MatchArm) -> Result<MatchArm, ExprLowerError> {
        let pattern = self.lower_match_pattern(&arm.pattern)?;
        self.push_scope(pattern.bound_locals())?;
        let body = self.lower_expr(&arm.body);
        self.pop_scope();
        Ok(MatchArm {
            pattern,
            body,
            span: arm.span,
        })
    }

    fn lower_match_pattern(
        &mut self,
        pattern: &ast::MatchPattern,
    ) -> Result<MatchPattern, ExprLowerError> {
        match pattern {
            ast::MatchPattern::Constructor {
                name,
                bindings,
                span,
            } => Ok(MatchPattern::Constructor {
                constructor: Spanned::new(
                    self.ctx
                        .resolver
                        .resolve_constructor_path(
                            self.ctx.owner,
                            &NamePath::local(name.value.atom().clone()),
                        )
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: name.span,
                        })?,
                    name.span,
                ),
                bindings: self.lower_pattern_bindings(bindings)?,
                span: *span,
            }),
            ast::MatchPattern::IndexLabel {
                index,
                variant,
                span,
            } => {
                let resolved = self.resolve_index_variant_parts(
                    &index.value,
                    &variant.value,
                    index.span,
                    variant.span,
                )?;
                Ok(MatchPattern::IndexLabel {
                    variant: IndexVariantRef {
                        variant: resolved,
                        index_span: Some(index.span),
                        additional_index_spans: Vec::new(),
                        variant_span: variant.span,
                    },
                    span: *span,
                })
            }
            ast::MatchPattern::Path {
                path,
                bindings,
                span,
            } => self.lower_path_pattern(path, bindings, *span),
        }
    }

    fn lower_path_pattern(
        &mut self,
        path: &crate::syntax::ast::IdentPath,
        bindings: &ast::PatternBindings<ast::PatternBinding>,
        span: Span,
    ) -> Result<MatchPattern, ExprLowerError> {
        let name_path = path.to_name_path();
        match self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &name_path)
        {
            Ok(constructor) => Ok(MatchPattern::Constructor {
                constructor: Spanned::new(constructor, path.span()),
                bindings: self.lower_pattern_bindings(bindings)?,
                span,
            }),
            Err(source) => match source {
                ModuleResolveError::UnknownName { .. }
                | ModuleResolveError::UnknownModuleAlias { .. }
                | ModuleResolveError::UnknownModule { .. } => Err(ExprLowerError::UnknownPattern {
                    path: path.display_path(),
                    span,
                }),
                source => Err(ExprLowerError::ModuleResolve { source, span }),
            },
        }
    }

    fn lower_pattern_bindings(
        &mut self,
        bindings: &ast::PatternBindings<ast::PatternBinding>,
    ) -> Result<ast::PatternBindings<PatternBinding>, ExprLowerError> {
        match bindings {
            ast::PatternBindings::Bare => Ok(ast::PatternBindings::Bare),
            ast::PatternBindings::Parenthesized(bindings) => bindings
                .iter()
                .map(|binding| self.lower_pattern_binding(binding))
                .collect::<Result<Vec<_>, _>>()
                .map(ast::PatternBindings::Parenthesized),
        }
    }

    fn lower_pattern_binding(
        &mut self,
        binding: &ast::PatternBinding,
    ) -> Result<PatternBinding, ExprLowerError> {
        match binding {
            ast::PatternBinding::Bind { field, var } => Ok(PatternBinding::Bind {
                field: field.clone(),
                local: self.allocate_local(LocalName::from_atom(var.name.clone()), var.span)?,
            }),
            ast::PatternBinding::Wildcard { field, span } => Ok(PatternBinding::Wildcard {
                field: field.clone(),
                span: *span,
            }),
        }
    }

    fn resolve_index_variant_parts(
        &self,
        index_path: &NamePath,
        variant: &IndexVariantName,
        index_span: Span,
        variant_span: Span,
    ) -> Result<ResolvedIndexVariant, ExprLowerError> {
        self.ctx
            .resolver
            .resolve_index_variant_parts(self.ctx.owner, index_path, variant)
            .map_err(|source| {
                let span = match source {
                    ModuleResolveError::UnknownIndexVariant { .. } => variant_span,
                    _ => index_span,
                };
                ExprLowerError::ModuleResolve { source, span }
            })
    }

    fn allocate_local(&mut self, name: LocalName, span: Span) -> Result<LocalDef, ExprLowerError> {
        let id = LocalId(self.next_local);
        let Some(next_local) = self.next_local.checked_add(1) else {
            return Err(ExprLowerError::TooManyLocals { span });
        };
        self.next_local = next_local;
        Ok(LocalDef { id, name, span })
    }

    fn push_scope(&mut self, bindings: Vec<LocalDef>) -> Result<(), ExprLowerError> {
        let mut scope = HashMap::new();
        for binding in bindings {
            if let Some(first) = scope.get(binding.name.as_str()).cloned().or_else(|| {
                self.local_scopes
                    .iter()
                    .rev()
                    .find_map(|visible| visible.get(binding.name.as_str()).cloned())
            }) {
                return Err(ExprLowerError::DuplicateLocalBinding {
                    name: binding.name,
                    first: first.span,
                    duplicate: binding.span,
                });
            }
            let atom = binding.name.atom();
            let builtin_occupied =
                validate_reserved_name(ReservedNameNamespace::Term, atom).is_err();
            let visible = self
                .ctx
                .resolver
                .visible_term_span(self.ctx.owner, atom)
                .map_err(|source| ExprLowerError::ModuleResolve {
                    source,
                    span: binding.span,
                })?;
            if builtin_occupied || visible.is_some() {
                return Err(ExprLowerError::LocalBindingShadowsTerm {
                    name: binding.name,
                    original: visible,
                    duplicate: binding.span,
                });
            }
            scope.insert(binding.name.clone(), binding);
        }
        self.local_scopes.push(scope);
        Ok(())
    }

    fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }

    fn lookup_local(&self, name: &LocalName, span: Span) -> Result<LocalId, ExprLowerError> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name.as_str()))
            .map(|def| def.id)
            .ok_or_else(|| ExprLowerError::UnknownLocalRef {
                name: name.clone(),
                span,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn desugared_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        crate::syntax::desugar::desugar_multi_decls_in_file(raw)
    }

    #[test]
    fn local_env_layers_frames_without_cloning() {
        let a = LocalId(0);
        let b = LocalId(1);
        let c = LocalId(2);

        let root: LocalEnv<'_, i32> = LocalEnv::root();
        assert_eq!(root.get(a), None);

        let outer = root.child(vec![(a, 1)]);
        assert_eq!(outer.get(a), Some(&1));
        assert_eq!(outer.get(b), None);

        let inner = outer.child(vec![(b, 2)]);
        assert_eq!(inner.get(a), Some(&1));
        assert_eq!(inner.get(b), Some(&2));

        // A child frame never leaks into its parent.
        assert_eq!(outer.get(b), None);

        let seeded = LocalEnv::from_bindings(vec![(c, 7)]);
        assert_eq!(seeded.get(c), Some(&7));
    }

    #[test]
    fn local_env_bind_rebinds_in_place() {
        let a = LocalId(0);
        let b = LocalId(1);
        let root: LocalEnv<'_, i32> = LocalEnv::root();
        let mut frame = root.child(Vec::new());

        // Iterating binders rebind the same id once per element.
        for value in 0..3 {
            frame.bind(a, value);
            assert_eq!(frame.get(a), Some(&value));
        }
        frame.bind(b, 10);
        assert_eq!(frame.get(a), Some(&2));
        assert_eq!(frame.get(b), Some(&10));
    }

    fn node_value<'a>(file: &'a ast::File, name: &str) -> &'a ast::Expr {
        file.declarations
            .iter()
            .find_map(|decl| match &decl.kind {
                ast::DeclKind::Node(node) if node.name.value.as_str() == name => Some(&node.value),
                _ => None,
            })
            .expect("source should contain requested node")
    }

    fn resolver_with_import(
        lib_id: &DagId,
        main_id: &DagId,
        lib: &ast::File,
        main: &ast::File,
    ) -> ModuleResolver {
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(lib_id.clone(), &lib.declarations)
            .unwrap();
        resolver
            .add_module(main_id.clone(), &main.declarations)
            .unwrap();
        for decl in &main.declarations {
            let ast::DeclKind::Import(import) = &decl.kind else {
                continue;
            };
            resolver
                .register_import(main_id, &import.path, &import.kind, lib_id)
                .unwrap();
        }
        resolver
    }

    #[test]
    fn lowers_qualified_index_variant_literal_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub index Phase = { Burn, Coast };");
        let main_source = "import lib as mission; node phase: Dimensionless = mission::Phase#Burn;";
        let main = desugared_source(main_source);
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "phase"),
            ExprLoweringContext::new(&main_id, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::VariantLiteral(variant) = expr.kind else {
            panic!("expected variant literal, got {expr:?}");
        };
        assert_eq!(variant.variant.index().owner(), &lib_id);
        assert_eq!(variant.variant.index().as_str(), "Phase");
        assert_eq!(variant.variant.variant().as_str(), "Burn");
        // Segment spans address exactly the written path parts.
        let slice = |span: crate::syntax::span::Span| {
            &main_source[span.offset()..span.offset() + span.len()]
        };
        assert_eq!(slice(variant.variant_span), "Burn");
        assert_eq!(
            slice(variant.index_span.expect("written index path")),
            "mission::Phase"
        );
    }

    #[test]
    fn lowers_qualified_quantity_literal_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub base dim Currency; pub base unit credit: Currency;");
        let main = desugared_source(
            "import lib as schema; node amount: Dimensionless = 1.0 schema::credit;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "amount"),
            ExprLoweringContext::new(&main_id, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::QuantityLiteral { unit, .. } = expr.kind else {
            panic!("expected quantity literal, got {expr:?}");
        };
        let [term] = unit.terms.as_slice() else {
            panic!("expected one unit term, got {:?}", unit.terms);
        };
        assert_eq!(term.name.value.spelling().to_string(), "schema::credit");
        assert_eq!(term.name.value.resolved().owner(), &lib_id);
        assert_eq!(term.name.value.resolved().as_str(), "credit");
    }

    #[test]
    fn lowers_qualified_nullary_constructor_const_ref_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type BurnKind { Impulsive, Coast }");
        let main = desugared_source(
            "import lib as mission; node burn: Dimensionless = mission::Impulsive;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "burn"),
            ExprLoweringContext::new(&main_id, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::ConstRef(target) = expr.kind else {
            panic!("expected const-like ref, got {expr:?}");
        };
        let ConstRef::Constructor(constructor) = target.value else {
            panic!("expected constructor, got {target:?}");
        };
        assert_eq!(constructor.owner(), &lib_id);
        assert_eq!(constructor.as_str(), "Impulsive");
    }

    #[test]
    fn lowers_unambiguous_timezone_datetime_to_resolved_hir() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source(
            "node t: Datetime = datetime(\"2024-07-15T17:30:00\", \"America/New_York\");",
        );
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&file, "t"),
            ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::FnCall { args, .. } = expr.kind else {
            panic!("expected function call, got {expr:?}");
        };
        let [
            Expr {
                kind: ExprKind::ZonedDateTimeLiteral(datetime),
                ..
            },
            Expr {
                kind: ExprKind::IanaTimeZoneLiteral(time_zone),
                ..
            },
        ] = args.as_slice()
        else {
            panic!("expected resolved datetime and timezone arguments, got {args:?}");
        };
        assert_eq!(datetime.time_zone(), time_zone);
        assert_eq!(
            datetime.timestamp(),
            "2024-07-15T21:30:00Z".parse::<jiff::Timestamp>().unwrap()
        );
    }

    #[test]
    fn lowers_epoch_static_scale_and_civil_literal_to_typed_hir() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("node t: Datetime<TT> = epoch<TT>(\"2024-11-05T12:00:00\");");
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&file, "t"),
            ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::FnCall { callee, args } = expr.kind else {
            panic!("expected function call, got {expr:?}");
        };
        let FunctionRef::Epoch { scale } = callee.value else {
            panic!("expected typed epoch reference, got {callee:?}");
        };
        assert_eq!(scale.value, TimeScale::TT);
        assert!(matches!(
            args.as_slice(),
            [Expr {
                kind: ExprKind::CivilDateTimeLiteral(_),
                ..
            }]
        ));
    }

    #[test]
    fn lowers_for_locals_to_lexical_ids() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source(
            "index Phase = { Burn }; node x: Dimensionless[Phase] = for p: Phase { p };",
        );
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&file, "x"),
            ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::ForComp { bindings, body } = expr.kind else {
            panic!("expected for comp, got {expr:?}");
        };
        let [binding] = bindings.as_slice() else {
            panic!("expected one binding, got {bindings:?}");
        };
        let ExprKind::LocalRef(local) = body.kind else {
            panic!("expected local ref, got {body:?}");
        };
        assert_eq!(binding.local.id, local.value);
    }

    #[test]
    fn lowers_qualified_constructor_match_pattern_and_binding() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib =
            desugared_source("pub type BurnKind { Impulsive(delta_v: Dimensionless), Coast }");
        let main = desugared_source(
            "import lib as mission; param burn: Dimensionless; \
             node dv: Dimensionless = match @burn { mission::Impulsive(delta_v: delta) => delta, mission::Coast => 0.0 };",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "dv"),
            ExprLoweringContext::new(&main_id, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap()
        .into_expr_for_test();

        let ExprKind::Match { arms, .. } = expr.kind else {
            panic!("expected match, got {expr:?}");
        };
        let [first, _second] = arms.as_slice() else {
            panic!("expected two arms, got {arms:?}");
        };
        let MatchPattern::Constructor {
            constructor,
            bindings,
            ..
        } = &first.pattern
        else {
            panic!("expected constructor pattern, got {:?}", first.pattern);
        };
        assert_eq!(constructor.value.owner(), &lib_id);
        assert_eq!(constructor.value.as_str(), "Impulsive");
        let [PatternBinding::Bind { local, .. }] = bindings.as_slice() else {
            panic!("expected one field binding, got {bindings:?}");
        };
        let ExprKind::LocalRef(body_ref) = &first.body.kind else {
            panic!("expected local ref body, got {:?}", first.body);
        };
        assert_eq!(local.id, body_ref.value);
    }

    #[test]
    fn collects_canonical_decl_dependencies_from_hir_expr() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib =
            desugared_source("pub const node C: Dimensionless = 1.0; param p: Dimensionless;");
        let main = desugared_source(
            "import lib as mission; import lib::{p}; node x: Dimensionless = @p + @mission::C;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "x"),
            ExprLoweringContext::new(&main_id, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
        .unwrap();
        let deps = collect_expr_dependencies(&expr);

        let graph_refs = deps.graph_refs.into_iter().collect::<Vec<_>>();
        assert!(deps.const_refs.is_empty());
        assert_eq!(graph_refs.len(), 2, "{graph_refs:?}");
        assert!(
            graph_refs
                .iter()
                .all(|reference| reference.owner() == &lib_id)
        );
        assert_eq!(
            graph_refs
                .iter()
                .map(crate::syntax::names::ResolvedName::as_str)
                .collect::<Vec<_>>(),
            ["C", "p"]
        );
    }

    fn lower_tolerant_node(source: &str, name: &str) -> (Expr, Vec<ExprLowerError>) {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source(source);
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();
        lower_expr_tolerant(
            node_value(&file, name),
            ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled()),
        )
    }

    fn graph_dependency_names(expr: &Expr) -> BTreeSet<String> {
        collect_expr_dependencies(expr)
            .graph_refs
            .into_iter()
            .map(|name| name.as_str().to_string())
            .collect()
    }

    #[test]
    fn failed_parent_nodes_retain_every_independent_expression_child() {
        let cases = [
            (
                "param a: Dimensionless; param b: Dimensionless; \
                 node out: Dimensionless = mystery(@a, @b);",
                2,
                BTreeSet::from(["a".to_string(), "b".to_string()]),
            ),
            (
                "param a: Dimensionless; param b: Dimensionless; \
                 node out: Dimensionless = Missing(first: @a, second: @b);",
                2,
                BTreeSet::from(["a".to_string(), "b".to_string()]),
            ),
            (
                "index Axis = { Good }; param a: Dimensionless; param b: Dimensionless; \
                 node out: Dimensionless[Axis] = \
                     { Axis#Missing: @a, Axis#Good: @b };",
                2,
                BTreeSet::from(["a".to_string(), "b".to_string()]),
            ),
            (
                "type State { Good } param state: State; \
                 param a: Dimensionless; param b: Dimensionless; \
                 node out: Dimensionless = match @state { \
                     Missing => @a, Good => @b \
                 };",
                3,
                BTreeSet::from(["state".to_string(), "a".to_string(), "b".to_string()]),
            ),
            (
                "param a: Dimensionless; param b: Dimensionless; \
                 node out: Dimensionless = @missing(first: @a, second: @b)::result;",
                2,
                BTreeSet::from(["a".to_string(), "b".to_string()]),
            ),
        ];

        for (source, expected_children, expected_dependencies) in cases {
            let (expr, diagnostics) = lower_tolerant_node(source, "out");
            let ExprKind::Error { children } = &expr.kind else {
                panic!("expected failed parent node, got {expr:?}");
            };
            assert_eq!(children.len(), expected_children, "source: {source}");
            assert!(!diagnostics.is_empty(), "source: {source}");
            assert_eq!(
                graph_dependency_names(&expr),
                expected_dependencies,
                "source: {source}"
            );
        }
    }

    #[test]
    fn failed_parent_accumulates_nested_diagnostics_without_duplicates() {
        let (expr, diagnostics) =
            lower_tolerant_node("node out: Dimensionless = mystery(also_missing);", "out");

        assert!(matches!(expr.kind, ExprKind::Error { .. }));
        assert_eq!(diagnostics.len(), 2, "diagnostics: {diagnostics:?}");
        assert!(matches!(
            diagnostics[0],
            ExprLowerError::UnknownFunction { .. }
        ));
        assert!(matches!(
            diagnostics[1],
            ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnknownName { .. },
                ..
            }
        ));
    }

    #[test]
    fn failed_binder_does_not_fabricate_lexical_locals_for_retained_body() {
        let (expr, diagnostics) = lower_tolerant_node(
            "param known: Dimensionless; \
             node out: Dimensionless = for item: Missing { item + @known };",
            "out",
        );

        assert!(matches!(expr.kind, ExprKind::Error { .. }));
        assert!(diagnostics.iter().any(|error| matches!(
            error,
            ExprLowerError::ModuleResolve {
                source: ModuleResolveError::UnknownName { name, .. },
                ..
            } if name == "item"
        )));
        assert_eq!(
            graph_dependency_names(&expr),
            BTreeSet::from(["known".to_string()])
        );
    }

    #[test]
    fn const_ref_binding_to_runtime_decl_is_rejected_by_decl_kind() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("param p: Dimensionless; node x: Dimensionless = p;");
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();
        let scoped_name = ScopedName::from(DeclName::expect_valid("p"));
        let bindings = HashMap::from([(
            scoped_name,
            ResolvedDeclName::from_def(owner.clone(), DeclName::expect_valid("p")),
        )]);

        let err = lower_expr(
            node_value(&file, "x"),
            ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled())
                .with_decl_bindings(&bindings),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ExprLowerError::BareGraphDeclarationRef {
                kind: DeclSymbolKind::Param,
                ..
            }
        ));
    }

    #[test]
    fn bare_graph_declaration_refs_require_at_sigil() {
        let cases = [
            ("param target: Dimensionless;", DeclSymbolKind::Param),
            ("node target: Dimensionless = 1.0;", DeclSymbolKind::Node),
            (
                "const node target: Dimensionless = 1.0;",
                DeclSymbolKind::Const,
            ),
        ];

        for (declaration, expected_kind) in cases {
            let owner = DagId::root_in_package("test", "main");
            let file = desugared_source(&format!(
                "{declaration} node output: Dimensionless = target;"
            ));
            let mut resolver = ModuleResolver::default();
            resolver
                .add_module(owner.clone(), &file.declarations)
                .unwrap();
            let scope = GenericScope::new();

            let err = lower_expr(
                node_value(&file, "output"),
                ExprLoweringContext::new(&owner, &resolver, &scope, &TimeZoneRegistry::bundled()),
            )
            .unwrap_err();

            assert!(matches!(
                err,
                ExprLowerError::BareGraphDeclarationRef { kind, .. } if kind == expected_kind
            ));
        }
    }
}
