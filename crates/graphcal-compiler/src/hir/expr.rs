//! HIR expression/value reference types and lowering.
//!
//! This module is the expression-side counterpart to [`super::types`]. It
//! lowers the desugared syntax AST into a HIR expression tree whose reference
//! positions use canonical module identities or lexical local IDs. This is the
//! single name-resolution stage of the pipeline: syntactic reference paths
//! ([`crate::syntax::ast::UnresolvedRef`]) are classified and resolved here,
//! in one pass, against the lexical scope and the module-aware resolver.
//! Source paths (`NamePath` / `IdentPath` / `ScopedName`) are consumed at
//! this boundary and are not stored in HIR reference fields.
//!
//! Lowering is diagnostic-accumulating: a reference that cannot be resolved
//! becomes an explicit [`ExprKind::Error`] node and its diagnostic is
//! recorded, so IDE consumers can keep working on incomplete code. The strict
//! entry points ([`lower_expr`], [`lower_assert_body`]) reject any tree that
//! contains an error node, so the batch pipeline never sees one.

use crate::syntax::decl_name::{DeclNameNamespace, ResolvedDeclName};
use crate::syntax::dimension::ResolvedDimName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::collections::{BTreeSet, HashMap};

use thiserror::Error;

use crate::builtin::{BuiltinConst, BuiltinFnName};
use crate::dag_id::DagId;
use crate::desugar::desugared_ast as ast;
use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::{Ident, IdentPath, UnresolvedRef};
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::{IndexName, IndexVariantName, ResolvedIndexVariant};
use crate::syntax::local_name::LocalName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_resolve::{DeclSymbolKind, ModuleResolveError, ModuleResolver};
use crate::syntax::names::{NameAtom, NameAtomError, NameNamespace, NamePath};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{FieldName, GenericParamName};

use super::lower::{
    GenericScope, HirLowerError, PreludeTypeScope, TypeLoweringContext, lower_nat_expr,
    lower_type_expr,
};
use super::types::{NatExpr, TypeExpr};

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
    /// A structured diagnostic name contained a segment that cannot be a source name atom.
    #[error("invalid scoped-name segment `{segment}`: {source}")]
    InvalidScopedNameSegment {
        segment: String,
        #[source]
        source: NameAtomError,
        span: Span,
    },
    /// A local reference had no lexical binding in scope.
    #[error("unknown local variable `{name}`")]
    UnknownLocalRef { name: LocalName, span: Span },
    /// A graph reference (`@name`) did not resolve to any declaration.
    #[error("unknown graph reference `@{name}`")]
    UnknownGraphRef { name: ScopedName, span: Span },
    /// A single expression tree introduced more local bindings than HIR can index.
    #[error("too many local bindings in one expression")]
    TooManyLocals { span: Span },
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
    /// A function call could not be resolved to a built-in function.
    #[error("unknown function `{path}`")]
    UnknownFunction { path: String, span: Span },
    /// A built-in function was called with the wrong number of arguments.
    #[error("function `{name}` expects {expected} argument(s), got {got}")]
    WrongArity {
        name: crate::syntax::function_name::FnName,
        expected: usize,
        got: usize,
        span: Span,
    },
    /// A path-pattern could not be resolved to a constructor or index label.
    #[error("unknown match pattern `{path}`")]
    UnknownPattern { path: String, span: Span },
}

/// Context required to lower one expression tree into HIR.
#[derive(Debug, Clone, Copy)]
pub struct ExprLoweringContext<'a> {
    pub owner: &'a DagId,
    pub resolver: &'a ModuleResolver,
    pub generic_scope: &'a GenericScope,
    pub prelude: Option<&'a PreludeTypeScope>,
    pub decl_bindings: Option<&'a HashMap<ScopedName, ResolvedDeclName>>,
}

impl<'a> ExprLoweringContext<'a> {
    /// Create an expression-lowering context.
    #[must_use]
    pub const fn new(
        owner: &'a DagId,
        resolver: &'a ModuleResolver,
        generic_scope: &'a GenericScope,
    ) -> Self {
        Self {
            owner,
            resolver,
            generic_scope,
            prelude: None,
            decl_bindings: None,
        }
    }

    /// Add implicit prelude type symbols for lowering type arguments.
    #[must_use]
    pub const fn with_prelude(self, prelude: &'a PreludeTypeScope) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            prelude: Some(prelude),
            decl_bindings: self.decl_bindings,
        }
    }

    /// Add canonical declaration bindings for declarations already visible in
    /// the lowered IR, such as prefixed dependency entries and DAG self-imports.
    #[must_use]
    pub const fn with_decl_bindings(
        self,
        decl_bindings: &'a HashMap<ScopedName, ResolvedDeclName>,
    ) -> Self {
        Self {
            owner: self.owner,
            resolver: self.resolver,
            generic_scope: self.generic_scope,
            prelude: self.prelude,
            decl_bindings: Some(decl_bindings),
        }
    }

    const fn type_context(self) -> TypeLoweringContext<'a> {
        let ctx = TypeLoweringContext::new(self.owner, self.resolver, self.generic_scope);
        match self.prelude {
            Some(prelude) => ctx.with_prelude(prelude),
            None => ctx,
        }
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
pub fn lower_expr(expr: &ast::Expr, ctx: ExprLoweringContext<'_>) -> Result<Expr, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_expr_tolerant(expr, ctx);
    if diagnostics.is_empty() {
        Ok(lowered)
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
pub fn lower_assert_body_tolerant(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> (AssertBody, Vec<ExprLowerError>) {
    match body {
        ast::AssertBody::Expr(expr) => {
            let (lowered, diagnostics) = lower_expr_tolerant(expr, ctx);
            (AssertBody::Expr(lowered), diagnostics)
        }
        ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
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
                    is_relative: *is_relative,
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
pub fn lower_assert_body(
    body: &ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> Result<AssertBody, ExprLowerError> {
    let (lowered, mut diagnostics) = lower_assert_body_tolerant(body, ctx);
    if diagnostics.is_empty() {
        Ok(lowered)
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
    pub const fn index(self) -> u32 {
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

/// A resolved index-variant reference with the source spans of its two
/// written segments kept separate.
///
/// The index segment and the variant segment are not necessarily adjacent:
/// table desugaring reuses the `table[Axis]` axis token's span as the index
/// span of every row key, so a single merged span would cover unrelated
/// source (and make different rows' spans contain each other). Keeping the
/// segments separate lets diagnostics on contiguous `Index.Variant` paths
/// use the whole written path ([`IndexVariantRef::path_span`]) while
/// span-precise consumers (rename, find-references) address exactly the
/// variant segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVariantRef {
    /// The resolved index variant.
    pub variant: ResolvedIndexVariant,
    /// Span of the index path as written (`Maneuver` in `Maneuver.Departure`,
    /// or the axis token inside `table[...]` for desugared table rows).
    /// `None` when the variant is written without an index segment (a bare
    /// label in a match pattern whose index is inferred).
    pub index_span: Option<Span>,
    /// Span of just the variant segment (the final path segment / row label).
    pub variant_span: Span,
}

impl IndexVariantRef {
    /// Whole-reference span for diagnostics on contiguous `Index.Variant`
    /// paths. For desugared table rows the index segment lives inside
    /// `table[...]`, so prefer [`Self::variant_span`] there.
    #[must_use]
    pub fn path_span(&self) -> Span {
        self.index_span
            .map_or(self.variant_span, |index| index.merge(self.variant_span))
    }
}

/// Resolved expression shape.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// A reference that failed to resolve.
    ///
    /// Produced only by tolerant lowering; the diagnostic for the failure is
    /// reported alongside the lowered tree. The strict entry points reject
    /// trees containing this node, so the batch pipeline never observes it.
    Error,
    Number(f64),
    Integer(i64),
    Bool(bool),
    StringLiteral(String),
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
        type_args: Vec<GenericArg>,
        args: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    UnitLiteral {
        value: f64,
        unit: ast::UnitExpr,
    },
    Convert {
        expr: Box<Expr>,
        target: ast::UnitExpr,
    },
    DisplayTimezone {
        expr: Box<Expr>,
        timezone: String,
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
        args: Vec<IndexArg>,
    },
    Scan {
        source: Box<Expr>,
        init: Box<Expr>,
        acc: LocalDef,
        val: LocalDef,
        body: Box<Expr>,
    },
    Unfold {
        init: Box<Expr>,
        prev: LocalDef,
        curr: LocalDef,
        body: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    VariantLiteral(IndexVariantRef),
    InlineDagRef {
        target: Spanned<DagId>,
        args: Vec<ParamBinding>,
        output: Spanned<ResolvedDeclName>,
    },
}

/// Canonical declaration dependencies observed in one HIR expression tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExprDependencies {
    /// Runtime graph dependencies reached through `@name` references.
    pub graph_refs: BTreeSet<ResolvedDeclName>,
    /// Compile-time const dependencies reached through const-like value refs.
    pub const_refs: BTreeSet<ResolvedDeclName>,
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
        ExprKind::Error
        | ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral(_)
        | ExprKind::UnitLiteral { .. } => {}
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
        ExprKind::Match { scrutinee, arms } => {
            collect_expr_dependencies_into(scrutinee, deps);
            for arm in arms {
                collect_expr_dependencies_into(&arm.body, deps);
            }
        }
        ExprKind::InlineDagRef { args, .. } => {
            for arg in args {
                collect_expr_dependencies_into(&arg.value, deps);
            }
        }
    }
}

/// Returns `true` if `expr` contains a graph reference to `name` that is
/// not dominated by an `Unfold` ancestor.
///
/// Unfold self-references access the previous step and are therefore not
/// true cyclic dependencies; a self-reference *outside* any unfold subtree
/// is a genuine cycle. Used to decide whether a declaration's self-edge can
/// be dropped from the dependency graph.
#[must_use]
pub fn has_ref_outside_unfold(expr: &Expr, name: &ResolvedDeclName) -> bool {
    // Recursion choke point: recurses once per tree level (unbounded for
    // left-nested operator chains).
    crate::stack::with_stack_growth(|| match &expr.kind {
        ExprKind::GraphRef(target) => target.value == *name,
        // Unfold body self-references access the previous step and are not a
        // dependency cycle, but `init` is evaluated before the previous-step
        // overlay exists, so self-references there are genuine cycles.
        ExprKind::Unfold { init, .. } => has_ref_outside_unfold(init, name),
        // The rest are leaves without graph references.
        ExprKind::Error
        | ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::ConstRef(_) => false,
        ExprKind::BinOp { lhs, rhs, .. } => {
            has_ref_outside_unfold(lhs, name) || has_ref_outside_unfold(rhs, name)
        }
        ExprKind::UnaryOp { operand, .. }
        | ExprKind::Convert { expr: operand, .. }
        | ExprKind::DisplayTimezone { expr: operand, .. }
        | ExprKind::FieldAccess { expr: operand, .. } => has_ref_outside_unfold(operand, name),
        ExprKind::FnCall { args, .. } => args.iter().any(|arg| has_ref_outside_unfold(arg, name)),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            has_ref_outside_unfold(condition, name)
                || has_ref_outside_unfold(then_branch, name)
                || has_ref_outside_unfold(else_branch, name)
        }
        ExprKind::ConstructorCall { fields, .. } => fields
            .iter()
            .any(|field| has_ref_outside_unfold(&field.value, name)),
        ExprKind::MapLiteral { entries } => entries
            .iter()
            .any(|entry| has_ref_outside_unfold(&entry.value, name)),
        ExprKind::ForComp { body, .. } => has_ref_outside_unfold(body, name),
        ExprKind::IndexAccess { expr, args } => {
            has_ref_outside_unfold(expr, name)
                || args.iter().any(|arg| match arg {
                    IndexArg::Expr(expr) => has_ref_outside_unfold(expr, name),
                    _ => false,
                })
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            has_ref_outside_unfold(source, name)
                || has_ref_outside_unfold(init, name)
                || has_ref_outside_unfold(body, name)
        }
        ExprKind::Match { scrutinee, arms } => {
            has_ref_outside_unfold(scrutinee, name)
                || arms
                    .iter()
                    .any(|arm| has_ref_outside_unfold(&arm.body, name))
        }
        ExprKind::InlineDagRef { args, .. } => args
            .iter()
            .any(|arg| has_ref_outside_unfold(&arg.value, name)),
    })
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
    pub fn surface_description(&self) -> String {
        match self {
            Self::Type(name) => format!("type `{}`", name.as_str()),
            Self::Dimension(name) => format!("dimension `{}`", name.as_str()),
            Self::Index(name) => format!("index `{}`", name.as_str()),
            Self::IndexVariant(variant) => format!(
                "index label `{}.{}`",
                variant.index().as_str(),
                variant.variant()
            ),
        }
    }

    #[must_use]
    pub fn value_position_error(&self) -> String {
        format!("{} cannot be used as a value", self.surface_description())
    }
}

/// Resolved constant-like expression target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstRef {
    Decl(ResolvedDeclName),
    Constructor(ResolvedConstructorName),
    Builtin(BuiltinConst),
    TimeScale(TimeScale),
    GenericNatParam(super::types::GenericParamId),
}

/// Function call target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionRef {
    Builtin(BuiltinFnName),
}

/// A lowered assertion body.
#[derive(Debug, Clone)]
pub enum AssertBody {
    Expr(Expr),
    Tolerance {
        actual: Box<Expr>,
        expected: Box<Expr>,
        tolerance: Box<Expr>,
        is_relative: bool,
    },
}

/// Generic argument at an expression call site.
#[derive(Debug, Clone)]
pub enum GenericArg {
    Type(TypeExpr),
    Nat(NatExpr),
}

/// Field initializer after expression lowering.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Spanned<FieldName>,
    pub value: Expr,
}

/// A param binding in an inline DAG invocation.
#[derive(Debug, Clone)]
pub struct ParamBinding {
    pub target: Spanned<ResolvedDeclName>,
    pub value: Expr,
    pub span: Span,
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
    NatRangeVariant {
        size: u64,
        variant: Spanned<IndexVariantName>,
    },
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
    Range { arg: NatExpr, span: Span },
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
        bindings: Vec<PatternBinding>,
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

/// Returns `true` when HIR cannot validate arity from the ordinary fixed-arity
/// built-in registry because the type checker may accept a different call shape.
const fn builtin_has_type_checker_arity(name: BuiltinFnName) -> bool {
    matches!(
        name,
        BuiltinFnName::Sum
            | BuiltinFnName::Min
            | BuiltinFnName::Max
            | BuiltinFnName::Mean
            | BuiltinFnName::Count
    )
}

struct ExprLowerer<'a> {
    ctx: ExprLoweringContext<'a>,
    local_scopes: Vec<HashMap<LocalName, LocalDef>>,
    next_local: u32,
    diagnostics: Vec<ExprLowerError>,
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
    /// A subtree whose references cannot be resolved becomes a single
    /// [`ExprKind::Error`] node with its diagnostic recorded; sibling
    /// subtrees keep lowering.
    fn lower_expr(&mut self, expr: &ast::Expr) -> Expr {
        // Recursion choke point: lowering recurses once per tree level
        // (unbounded for left-nested operator chains).
        crate::stack::with_stack_growth(|| match self.lower_expr_inner(expr) {
            Ok(lowered) => lowered,
            Err(err) => {
                self.diagnostics.push(err);
                Expr::new(ExprKind::Error, expr.span)
            }
        })
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind lowering")]
    fn lower_expr_inner(&mut self, expr: &ast::Expr) -> Result<Expr, ExprLowerError> {
        let kind = match &expr.kind {
            ast::ExprKind::Number(value) => ExprKind::Number(*value),
            ast::ExprKind::Integer(value) => ExprKind::Integer(*value),
            ast::ExprKind::Bool(value) => ExprKind::Bool(*value),
            ast::ExprKind::StringLiteral(value) => ExprKind::StringLiteral(value.clone()),
            ast::ExprKind::UnresolvedRef(unresolved) => {
                let UnresolvedRef::Path(path) = unresolved;
                self.lower_unresolved_path(path)?
            }
            ast::ExprKind::GraphRef(name) => ExprKind::GraphRef(Spanned::new(
                self.resolve_decl_scoped_name(&name.value, name.span)?,
                name.span,
            )),
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
                type_args,
                args,
            } => ExprKind::FnCall {
                callee: {
                    let function_ref = Self::lower_function_ref(callee)?;
                    Self::check_function_arity(function_ref, args.len(), callee.span())?;
                    Spanned::new(function_ref, callee.span())
                },
                type_args: type_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg))
                    .collect::<Result<Vec<_>, _>>()?,
                args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
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
            ast::ExprKind::UnitLiteral { value, unit } => ExprKind::UnitLiteral {
                value: *value,
                unit: unit.clone(),
            },
            ast::ExprKind::Convert { expr, target } => ExprKind::Convert {
                expr: Box::new(self.lower_expr(expr)),
                target: target.clone(),
            },
            ast::ExprKind::DisplayTimezone { expr, timezone } => ExprKind::DisplayTimezone {
                expr: Box::new(self.lower_expr(expr)),
                timezone: timezone.clone(),
            },
            // `@alias.member` parses as `FieldAccess(GraphRef(alias), member)`.
            // When `alias.member` resolves as a module-qualified declaration,
            // promote the access to a qualified graph reference — this is the
            // same promotion the project pipeline applies before evaluation,
            // done here so every consumer of HIR (LSP included) sees the
            // resolved identity. Otherwise it is a struct-field access.
            ast::ExprKind::FieldAccess { expr, field } => self
                .resolve_alias_field_access(expr, field)
                .unwrap_or_else(|| ExprKind::FieldAccess {
                    expr: Box::new(self.lower_expr(expr)),
                    field: field.clone(),
                }),
            ast::ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => ExprKind::ConstructorCall {
                callee: Spanned::new(
                    self.ctx
                        .resolver
                        .resolve_constructor_ident_path(self.ctx.owner, callee)
                        .map_err(|source| ExprLowerError::ModuleResolve {
                            source,
                            span: callee.span(),
                        })?,
                    callee.span(),
                ),
                generic_args: generic_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg))
                    .collect::<Result<Vec<_>, _>>()?,
                fields: fields
                    .iter()
                    .map(|field| self.lower_field_init(field))
                    .collect(),
            },
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
                args: args
                    .iter()
                    .map(|arg| self.lower_index_arg(arg))
                    .collect::<Result<Vec<_>, _>>()?,
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
                init,
                prev_name,
                curr_name,
                body,
            } => {
                let init = Box::new(self.lower_expr(init));
                let prev = self.allocate_local(prev_name.value.clone(), prev_name.span)?;
                let curr = self.allocate_local(curr_name.value.clone(), curr_name.span)?;
                self.push_scope(vec![prev.clone(), curr.clone()])?;
                let body = Box::new(self.lower_expr(body));
                self.pop_scope();
                ExprKind::Unfold {
                    init,
                    prev,
                    curr,
                    body,
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
                let lowered_args = args
                    .iter()
                    .map(|arg| self.lower_param_binding(&target, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let output_path = NamePath::local(output.value.atom().clone());
                let lowered_output = self
                    .ctx
                    .resolver
                    .resolve_decl_path(&target, &output_path)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: output.span,
                    })?;
                ExprKind::InlineDagRef {
                    target: Spanned::new(target, path.span()),
                    args: lowered_args,
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

    /// Resolve a syntactic reference path in value position.
    ///
    /// This is the single classification point of the compiler: it decides,
    /// in one pass, whether a path names a lexical local, a built-in constant
    /// or time scale, a constructor, a type-system entity, a generic `Nat`
    /// parameter, or a declaration — and resolves it to its canonical
    /// identity at the same time. Lexical scope shadows module symbols.
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
    /// 2. Built-in constants (`PI`, `E`, ...) and time scales (`UTC`, ...)
    /// 3. Constructors (a bare constructor name is a nullary call)
    /// 4. Type-system names (struct types, dimensions, indexes, variants)
    /// 5. Generic `Nat` parameters and declarations (const/node/param)
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
        if let Ok(scale) = ident.name.as_str().parse::<TimeScale>() {
            return Ok(ExprKind::ConstRef(Spanned::new(
                ConstRef::TimeScale(scale),
                span,
            )));
        }
        let path = NamePath::local(ident.name.clone());
        if let Ok(constructor) = self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &path)
        {
            return Ok(ExprKind::ConstructorCall {
                callee: Spanned::new(constructor, span),
                generic_args: Vec::new(),
                fields: Vec::new(),
            });
        }
        if let Some(type_system_ref) =
            self.resolve_bare_type_system_name(&path, &ident.name, span)?
        {
            return Ok(ExprKind::TypeSystemRef(Spanned::new(type_system_ref, span)));
        }
        self.lower_const_ref(&ScopedName::local(ident.name.as_str()), span)
            .map(|const_ref| ExprKind::ConstRef(Spanned::new(const_ref, span)))
    }

    /// Resolve a bare identifier against the type-system namespaces.
    ///
    /// Bare type-system names appear in value position in include-binding
    /// RHSs (`Speed: Velocity`); downstream type checking rejects them in
    /// genuine value positions with a precise diagnostic.
    fn resolve_bare_type_system_name(
        &self,
        path: &NamePath,
        name: &NameAtom,
        span: Span,
    ) -> Result<Option<TypeSystemRef>, ExprLowerError> {
        if let Ok(struct_type) = self
            .ctx
            .resolver
            .resolve_struct_type_path(self.ctx.owner, path)
        {
            return Ok(Some(TypeSystemRef::Type(struct_type)));
        }
        if let Ok(dimension) = self
            .ctx
            .resolver
            .resolve_dimension_path(self.ctx.owner, path)
        {
            return Ok(Some(TypeSystemRef::Dimension(dimension)));
        }
        if let Some(prelude) = self.ctx.prelude
            && let Some(dimension) = prelude.resolve_dimension_path(path)
        {
            return Ok(Some(TypeSystemRef::Dimension(dimension)));
        }
        if let Ok(index) = self.ctx.resolver.resolve_index_path(self.ctx.owner, path) {
            return Ok(Some(TypeSystemRef::Index(index)));
        }
        let variant_name = IndexVariantName::from_atom(name.clone());
        match self
            .ctx
            .resolver
            .resolve_bare_index_variant(self.ctx.owner, &variant_name)
        {
            Ok(variant) => Ok(Some(TypeSystemRef::IndexVariant(variant))),
            Err(ModuleResolveError::UnknownName { .. }) => Ok(None),
            Err(source) => Err(ExprLowerError::ModuleResolve { source, span }),
        }
    }

    /// Resolve a dotted reference path (`a.b`, `a.b.c`, ...) in value position.
    ///
    /// A two-segment path whose head names an index in scope is a variant
    /// literal; anything else is a qualified constant-like reference
    /// (declaration, constructor, or index variant of an imported module).
    fn lower_dotted_path_ref(&self, path: &IdentPath) -> Result<ExprKind, ExprLowerError> {
        let span = path.span();
        if let [qualifier, member] = path.segments() {
            let index_path = NamePath::local(qualifier.name.clone());
            if self
                .ctx
                .resolver
                .resolve_index_path(self.ctx.owner, &index_path)
                .is_ok()
            {
                let variant = IndexVariantName::from_atom(member.name.clone());
                let resolved = self.resolve_index_variant_parts(
                    &index_path,
                    &variant,
                    qualifier.span,
                    member.span,
                )?;
                return Ok(ExprKind::VariantLiteral(IndexVariantRef {
                    variant: resolved,
                    index_span: Some(qualifier.span),
                    variant_span: member.span,
                }));
            }
        }

        let (qualifier, member) = path.split_last();
        let scoped = ScopedName::qualified_path(
            qualifier.iter().map(|segment| segment.name.to_string()),
            member.name.to_string(),
        );
        match self.lower_const_ref(&scoped, span) {
            Ok(const_ref) => Ok(ExprKind::ConstRef(Spanned::new(const_ref, span))),
            // A qualified path that is not a const-like declaration can still
            // be an index variant (`m.Season.Winter`). Resolving it here keeps
            // the segment spans of the written path available for the literal.
            Err(const_err) => self
                .resolve_variant_literal_path(path)
                .ok_or(const_err)
                .map(ExprKind::VariantLiteral),
        }
    }

    /// Promote `FieldAccess(GraphRef(alias), member)` to a qualified graph
    /// reference when `alias.member` resolves as a module-qualified
    /// declaration. Returns `None` when it does not (a struct-field access).
    ///
    /// Resolution goes through [`ModuleResolver::resolve_decl_path`] only —
    /// it applies the alias boundary's visibility rule, so a private member
    /// of an imported/included module does not get promoted (and therefore
    /// stays an unresolved reference, exactly like writing the qualified
    /// path directly).
    fn resolve_alias_field_access(
        &self,
        inner: &ast::Expr,
        field: &Spanned<FieldName>,
    ) -> Option<ExprKind> {
        let ast::ExprKind::GraphRef(name) = &inner.kind else {
            return None;
        };
        if name.value.is_qualified() {
            return None;
        }
        let scoped = ScopedName::qualified(name.value.member(), field.value.as_str());
        let span = name.span.merge(field.span);
        let path = scoped_name_to_path(&scoped, span).ok()?;
        let resolved = self
            .ctx
            .resolver
            .resolve_decl_path(self.ctx.owner, &path)
            .ok()?;
        Some(ExprKind::GraphRef(Spanned::new(resolved, span)))
    }

    /// Resolve a dotted path as an index-variant literal, keeping the index
    /// and variant segment spans separate. Returns `None` when the path does
    /// not name an index variant in scope.
    fn resolve_variant_literal_path(&self, path: &IdentPath) -> Option<IndexVariantRef> {
        let (qualifier, member) = path.split_last();
        let (first, rest) = qualifier.split_first()?;
        let index_span = rest
            .iter()
            .fold(first.span, |merged, segment| merged.merge(segment.span));
        let resolved = self
            .ctx
            .resolver
            .resolve_index_variant_path(self.ctx.owner, &path.to_name_path())
            .ok()?;
        Some(IndexVariantRef {
            variant: resolved,
            index_span: Some(index_span),
            variant_span: member.span,
        })
    }

    fn lower_generic_arg(&self, arg: &ast::GenericArg) -> Result<GenericArg, ExprLowerError> {
        match arg {
            ast::GenericArg::Type(type_expr) => Ok(GenericArg::Type(lower_type_expr(
                type_expr,
                self.ctx.type_context(),
            )?)),
            ast::GenericArg::Nat(nat_expr) => Ok(GenericArg::Nat(lower_nat_expr(
                nat_expr,
                self.ctx.type_context(),
            )?)),
        }
    }

    fn lower_field_init(&mut self, field: &ast::FieldInit) -> FieldInit {
        FieldInit {
            name: field.name.clone(),
            value: self.lower_expr(&field.value),
        }
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
            span: binding.span,
        })
    }

    fn lower_const_ref(&self, name: &ScopedName, span: Span) -> Result<ConstRef, ExprLowerError> {
        if !name.is_qualified() {
            if let Some(builtin) = BuiltinConst::parse(name.member()) {
                return Ok(ConstRef::Builtin(builtin));
            }
            if let Ok(scale) = name.member().parse::<TimeScale>() {
                return Ok(ConstRef::TimeScale(scale));
            }
            let generic_name = GenericParamName::expect_valid(name.member());
            if let Some(binding) = self.ctx.generic_scope.get(&generic_name)
                && binding.constraint == ast::GenericConstraint::Nat
            {
                return Ok(ConstRef::GenericNatParam(binding.id.clone()));
            }
        }

        let path = scoped_name_to_path(name, span)?;
        let mut first_error = None;

        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            return Ok(ConstRef::Decl(resolved));
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

    fn resolve_decl_scoped_name(
        &self,
        name: &ScopedName,
        span: Span,
    ) -> Result<ResolvedDeclName, ExprLowerError> {
        let path = scoped_name_to_path(name, span)?;
        if let Some(resolved) = self
            .ctx
            .decl_bindings
            .and_then(|bindings| bindings.get(name))
            .cloned()
        {
            return Ok(resolved);
        }
        self.ctx
            .resolver
            .resolve_decl_path(self.ctx.owner, &path)
            .or_else(|err| self.resolve_synthetic_child_decl_path(&path).ok_or(err))
            .map_err(|source| match source {
                ModuleResolveError::UnknownName { .. } => ExprLowerError::UnknownGraphRef {
                    name: name.clone(),
                    span,
                },
                source => ExprLowerError::ModuleResolve { source, span },
            })
    }

    fn resolve_synthetic_child_decl_path(&self, path: &NamePath) -> Option<ResolvedDeclName> {
        let (qualifier, leaf) = path.qualifier_and_leaf()?;
        let owner = qualifier
            .iter()
            .fold(self.ctx.owner.clone(), |owner, segment| {
                owner.child(segment.as_str())
            });
        self.ctx
            .resolver
            .modules()
            .contains_key(&owner)
            .then(|| ResolvedDeclName::from_def(owner, DeclName::from_atom(leaf.clone())))
    }

    /// Validate a built-in call's argument count against the registry's
    /// arity table. Aggregations are variadic over collections and skip the
    /// check; their argument shapes are validated during type checking.
    fn check_function_arity(
        function_ref: FunctionRef,
        got: usize,
        span: Span,
    ) -> Result<(), ExprLowerError> {
        let FunctionRef::Builtin(builtin) = function_ref;
        if builtin_has_type_checker_arity(builtin) {
            return Ok(());
        }
        let Some(function) = crate::registry::builtins::builtin_functions().get(builtin.as_str())
        else {
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

    fn lower_function_ref(
        callee: &crate::syntax::ast::IdentPath,
    ) -> Result<FunctionRef, ExprLowerError> {
        let Some(ident) = callee.as_bare() else {
            return Err(ExprLowerError::UnknownFunction {
                path: callee.display_path(),
                span: callee.span(),
            });
        };
        BuiltinFnName::parse(ident.name.as_str())
            .map(FunctionRef::Builtin)
            .ok_or_else(|| ExprLowerError::UnknownFunction {
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
        match &key.index.value {
            crate::syntax::ast::MapEntryIndex::Named(index_path) => {
                let variant = self
                    .resolve_index_variant_parts(
                        index_path,
                        &key.variant.value,
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
                    variant_span: key.variant.span,
                }))
            }
            crate::syntax::ast::MapEntryIndex::NatRange(size) => Ok(MapEntryKey::NatRangeVariant {
                size: *size,
                variant: key.variant.clone(),
            }),
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
            ast::ForBindingIndex::Range { arg, span } => ForBindingIndex::Range {
                arg: lower_nat_expr(arg, self.ctx.type_context())?,
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
                bindings: bindings
                    .iter()
                    .map(|binding| self.lower_pattern_binding(binding))
                    .collect::<Result<Vec<_>, _>>()?,
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
        bindings: &[ast::PatternBinding],
        span: Span,
    ) -> Result<MatchPattern, ExprLowerError> {
        let name_path = path.to_name_path();
        if bindings.is_empty()
            && let Ok(variant) = self
                .ctx
                .resolver
                .resolve_index_variant_path(self.ctx.owner, &name_path)
        {
            let (qualifier, member) = path.split_last();
            let index_span = qualifier.split_first().map(|(first, rest)| {
                rest.iter()
                    .fold(first.span, |merged, segment| merged.merge(segment.span))
            });
            return Ok(MatchPattern::IndexLabel {
                variant: IndexVariantRef {
                    variant,
                    index_span,
                    variant_span: member.span,
                },
                span,
            });
        }

        match self
            .ctx
            .resolver
            .resolve_constructor_path(self.ctx.owner, &name_path)
        {
            Ok(constructor) => Ok(MatchPattern::Constructor {
                constructor: Spanned::new(constructor, path.span()),
                bindings: bindings
                    .iter()
                    .map(|binding| self.lower_pattern_binding(binding))
                    .collect::<Result<Vec<_>, _>>()?,
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
            if let Some(first) = scope.insert(binding.name.clone(), binding.clone()) {
                return Err(ExprLowerError::DuplicateLocalBinding {
                    name: binding.name,
                    first: first.span,
                    duplicate: binding.span,
                });
            }
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

fn scoped_name_to_path(name: &ScopedName, span: Span) -> Result<NamePath, ExprLowerError> {
    let qualifier = name
        .qualifier()
        .iter()
        .map(|segment| parse_atom(segment, span))
        .collect::<Result<Vec<_>, _>>()?;
    let leaf = parse_atom(name.member(), span)?;
    Ok(NamePath::qualified_path(qualifier, leaf))
}

fn parse_atom(segment: &str, span: Span) -> Result<NameAtom, ExprLowerError> {
    NameAtom::parse(segment).map_err(|source| ExprLowerError::InvalidScopedNameSegment {
        segment: segment.to_string(),
        source,
        span,
    })
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
        let main_source = "import lib as mission; node phase: Dimensionless = mission.Phase.Burn;";
        let main = desugared_source(main_source);
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "phase"),
            ExprLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();

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
            "mission.Phase"
        );
    }

    #[test]
    fn lowers_qualified_nullary_constructor_const_ref_to_canonical_owner() {
        let lib_id = DagId::root_in_package("test", "lib");
        let main_id = DagId::root_in_package("test", "main");
        let lib = desugared_source("pub type BurnKind { Impulsive, Coast }");
        let main = desugared_source(
            "import lib as mission; node burn: Dimensionless = mission.Impulsive;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "burn"),
            ExprLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();

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
            ExprLoweringContext::new(&owner, &resolver, &scope),
        )
        .unwrap();

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
             node dv: Dimensionless = match @burn { mission.Impulsive(delta_v: dv) => dv, mission.Coast => 0.0 };",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "dv"),
            ExprLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();

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
            "import lib as mission; import lib.{p}; node x: Dimensionless = @p + mission.C;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "x"),
            ExprLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();
        let deps = collect_expr_dependencies(&expr);

        let graph_refs = deps.graph_refs.into_iter().collect::<Vec<_>>();
        let const_refs = deps.const_refs.into_iter().collect::<Vec<_>>();
        let [graph_ref] = graph_refs.as_slice() else {
            panic!("expected one graph dep, got {graph_refs:?}");
        };
        let [const_ref] = const_refs.as_slice() else {
            panic!("expected one const dep, got {const_refs:?}");
        };
        assert_eq!(graph_ref.owner(), &lib_id);
        assert_eq!(graph_ref.as_str(), "p");
        assert_eq!(const_ref.owner(), &lib_id);
        assert_eq!(const_ref.as_str(), "C");
    }

    #[test]
    fn const_ref_to_runtime_decl_is_rejected_by_decl_kind() {
        let owner = DagId::root_in_package("test", "main");
        let file = desugared_source("param p: Dimensionless; node x: Dimensionless = p;");
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(owner.clone(), &file.declarations)
            .unwrap();
        let scope = GenericScope::new();

        let err = lower_expr(
            node_value(&file, "x"),
            ExprLoweringContext::new(&owner, &resolver, &scope),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("expected const declaration `main.p`, found param"),
            "unexpected error: {err}"
        );
    }
}
