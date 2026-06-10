//! HIR expression/value reference types and lowering.
//!
//! This module is the expression-side counterpart to [`super::types`]. It
//! lowers the current locally-resolved syntax AST into a HIR expression tree
//! whose reference positions use canonical module identities or lexical local
//! IDs. Source paths (`NamePath` / `IdentPath` / `ScopedName`) are consumed at
//! this boundary and are not stored in HIR reference fields.

use std::collections::{BTreeSet, HashMap};

use thiserror::Error;

use crate::dag_id::DagId;
use crate::desugar::resolved_ast as ast;
use crate::registry::resolve_types::{
    AggregationFn, ConstructorFn, DatetimeExtractFn, DatetimeFromFn, DatetimeToFn, SpecialFnKind,
    TypeConversionFn,
};
use crate::registry::time_scale::TimeScale;
use crate::syntax::ast::TypeSystemRefKind as SyntaxTypeSystemRefKind;
use crate::syntax::module_resolve::{DeclSymbolKind, ModuleResolveError, ModuleResolver};
use crate::syntax::names::{
    DeclName, FieldName, GenericParamName, IndexName, IndexVariantName, LocalName, NameAtom,
    NameAtomError, NameNamespace, NamePath, ResolvedIndexVariant, ResolvedName, ScopedName,
    namespace,
};
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};

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
    pub decl_bindings: Option<&'a HashMap<ScopedName, ResolvedName<namespace::Decl>>>,
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
        decl_bindings: &'a HashMap<ScopedName, ResolvedName<namespace::Decl>>,
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

/// Lower a syntax expression into HIR.
///
/// # Errors
///
/// Returns [`ExprLowerError`] if any expression-level reference cannot be
/// resolved to a canonical module identity or lexical local binding.
pub fn lower_expr(expr: &ast::Expr, ctx: ExprLoweringContext<'_>) -> Result<Expr, ExprLowerError> {
    ExprLowerer::new(ctx).lower_expr(expr)
}

/// Lower a syntax assertion body into HIR.
///
/// Each assertion body owns an independent lexical local-id space. Assertion
/// expressions cannot share locals across the `actual`/`expected`/`tolerance`
/// slots of a tolerance assertion, so each slot is lowered with a fresh lowerer.
pub fn lower_assert_body(
    body: &crate::desugar::resolved_ast::AssertBody,
    ctx: ExprLoweringContext<'_>,
) -> Result<AssertBody, ExprLowerError> {
    match body {
        crate::desugar::resolved_ast::AssertBody::Expr(expr) => {
            lower_expr(expr, ctx).map(AssertBody::Expr)
        }
        crate::desugar::resolved_ast::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
            is_relative,
        } => Ok(AssertBody::Tolerance {
            actual: Box::new(lower_expr(actual, ctx)?),
            expected: Box::new(lower_expr(expected, ctx)?),
            tolerance: Box::new(lower_expr(tolerance, ctx)?),
            is_relative: *is_relative,
        }),
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

/// Built-in constants with closed semantic meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinConst {
    Pi,
    E,
    Tau,
    Sqrt2,
    Ln2,
    Ln10,
}

impl BuiltinConst {
    /// Parse a source name into a built-in constant.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "PI" => Some(Self::Pi),
            "E" => Some(Self::E),
            "TAU" => Some(Self::Tau),
            "SQRT2" => Some(Self::Sqrt2),
            "LN2" => Some(Self::Ln2),
            "LN10" => Some(Self::Ln10),
            _ => None,
        }
    }

    /// Canonical source spelling of the constant.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pi => "PI",
            Self::E => "E",
            Self::Tau => "TAU",
            Self::Sqrt2 => "SQRT2",
            Self::Ln2 => "LN2",
            Self::Ln10 => "LN10",
        }
    }
}

/// Built-in function names with closed semantic meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinFnName {
    Sqrt,
    Cbrt,
    Exp,
    Expm1,
    Ln,
    Log10,
    Log2,
    Log,
    Log1p,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    Abs,
    Floor,
    Ceil,
    Round,
    Trunc,
    Sign,
    Min,
    Max,
    Hypot,
    Clamp,
    Sum,
    Mean,
    Count,
    ToFloat,
    ToInt,
    ToUtc,
    ToTai,
    ToTt,
    ToTdb,
    ToEt,
    ToGpst,
    ToGst,
    ToBdt,
    ToQzsst,
    Datetime,
    Epoch,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Weekday,
    DayOfYear,
    FromJd,
    FromMjd,
    FromUnix,
    ToJd,
    ToMjd,
    ToUnix,
}

impl BuiltinFnName {
    /// Parse a source name into a built-in function.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "sqrt" => Some(Self::Sqrt),
            "cbrt" => Some(Self::Cbrt),
            "exp" => Some(Self::Exp),
            "expm1" => Some(Self::Expm1),
            "ln" => Some(Self::Ln),
            "log10" => Some(Self::Log10),
            "log2" => Some(Self::Log2),
            "log" => Some(Self::Log),
            "log1p" => Some(Self::Log1p),
            "sin" => Some(Self::Sin),
            "cos" => Some(Self::Cos),
            "tan" => Some(Self::Tan),
            "asin" => Some(Self::Asin),
            "acos" => Some(Self::Acos),
            "atan" => Some(Self::Atan),
            "atan2" => Some(Self::Atan2),
            "sinh" => Some(Self::Sinh),
            "cosh" => Some(Self::Cosh),
            "tanh" => Some(Self::Tanh),
            "asinh" => Some(Self::Asinh),
            "acosh" => Some(Self::Acosh),
            "atanh" => Some(Self::Atanh),
            "abs" => Some(Self::Abs),
            "floor" => Some(Self::Floor),
            "ceil" => Some(Self::Ceil),
            "round" => Some(Self::Round),
            "trunc" => Some(Self::Trunc),
            "sign" => Some(Self::Sign),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "hypot" => Some(Self::Hypot),
            "clamp" => Some(Self::Clamp),
            "sum" => Some(Self::Sum),
            "mean" => Some(Self::Mean),
            "count" => Some(Self::Count),
            "to_float" => Some(Self::ToFloat),
            "to_int" => Some(Self::ToInt),
            "to_utc" => Some(Self::ToUtc),
            "to_tai" => Some(Self::ToTai),
            "to_tt" => Some(Self::ToTt),
            "to_tdb" => Some(Self::ToTdb),
            "to_et" => Some(Self::ToEt),
            "to_gpst" => Some(Self::ToGpst),
            "to_gst" => Some(Self::ToGst),
            "to_bdt" => Some(Self::ToBdt),
            "to_qzsst" => Some(Self::ToQzsst),
            "datetime" => Some(Self::Datetime),
            "epoch" => Some(Self::Epoch),
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            "hour" => Some(Self::Hour),
            "minute" => Some(Self::Minute),
            "second" => Some(Self::Second),
            "weekday" => Some(Self::Weekday),
            "day_of_year" => Some(Self::DayOfYear),
            "from_jd" => Some(Self::FromJd),
            "from_mjd" => Some(Self::FromMjd),
            "from_unix" => Some(Self::FromUnix),
            "to_jd" => Some(Self::ToJd),
            "to_mjd" => Some(Self::ToMjd),
            "to_unix" => Some(Self::ToUnix),
            _ => None,
        }
    }

    /// Canonical source spelling of the function.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqrt => "sqrt",
            Self::Cbrt => "cbrt",
            Self::Exp => "exp",
            Self::Expm1 => "expm1",
            Self::Ln => "ln",
            Self::Log10 => "log10",
            Self::Log2 => "log2",
            Self::Log => "log",
            Self::Log1p => "log1p",
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Tan => "tan",
            Self::Asin => "asin",
            Self::Acos => "acos",
            Self::Atan => "atan",
            Self::Atan2 => "atan2",
            Self::Sinh => "sinh",
            Self::Cosh => "cosh",
            Self::Tanh => "tanh",
            Self::Asinh => "asinh",
            Self::Acosh => "acosh",
            Self::Atanh => "atanh",
            Self::Abs => "abs",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::Round => "round",
            Self::Trunc => "trunc",
            Self::Sign => "sign",
            Self::Min => "min",
            Self::Max => "max",
            Self::Hypot => "hypot",
            Self::Clamp => "clamp",
            Self::Sum => "sum",
            Self::Mean => "mean",
            Self::Count => "count",
            Self::ToFloat => "to_float",
            Self::ToInt => "to_int",
            Self::ToUtc => "to_utc",
            Self::ToTai => "to_tai",
            Self::ToTt => "to_tt",
            Self::ToTdb => "to_tdb",
            Self::ToEt => "to_et",
            Self::ToGpst => "to_gpst",
            Self::ToGst => "to_gst",
            Self::ToBdt => "to_bdt",
            Self::ToQzsst => "to_qzsst",
            Self::Datetime => "datetime",
            Self::Epoch => "epoch",
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            Self::Weekday => "weekday",
            Self::DayOfYear => "day_of_year",
            Self::FromJd => "from_jd",
            Self::FromMjd => "from_mjd",
            Self::FromUnix => "from_unix",
            Self::ToJd => "to_jd",
            Self::ToMjd => "to_mjd",
            Self::ToUnix => "to_unix",
        }
    }

    /// Return the existing typed special-function classification when this
    /// built-in is one of the special categories.
    #[must_use]
    pub const fn special_kind(self) -> Option<SpecialFnKind> {
        match self {
            Self::Sum => Some(SpecialFnKind::Aggregation(AggregationFn::Sum)),
            Self::Min => Some(SpecialFnKind::Aggregation(AggregationFn::Min)),
            Self::Max => Some(SpecialFnKind::Aggregation(AggregationFn::Max)),
            Self::Mean => Some(SpecialFnKind::Aggregation(AggregationFn::Mean)),
            Self::Count => Some(SpecialFnKind::Aggregation(AggregationFn::Count)),
            Self::ToFloat => Some(SpecialFnKind::TypeConversion(TypeConversionFn::ToFloat)),
            Self::ToInt => Some(SpecialFnKind::TypeConversion(TypeConversionFn::ToInt)),
            Self::ToUtc => Some(SpecialFnKind::TimeScaleConversion(TimeScale::UTC)),
            Self::ToTai => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TAI)),
            Self::ToTt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TT)),
            Self::ToTdb => Some(SpecialFnKind::TimeScaleConversion(TimeScale::TDB)),
            Self::ToEt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::ET)),
            Self::ToGpst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::GPST)),
            Self::ToGst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::GST)),
            Self::ToBdt => Some(SpecialFnKind::TimeScaleConversion(TimeScale::BDT)),
            Self::ToQzsst => Some(SpecialFnKind::TimeScaleConversion(TimeScale::QZSST)),
            Self::Datetime => Some(SpecialFnKind::Constructor(ConstructorFn::Datetime)),
            Self::Epoch => Some(SpecialFnKind::Constructor(ConstructorFn::Epoch)),
            Self::Year => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Year)),
            Self::Month => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Month)),
            Self::Day => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Day)),
            Self::Hour => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Hour)),
            Self::Minute => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Minute)),
            Self::Second => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Second)),
            Self::Weekday => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::Weekday)),
            Self::DayOfYear => Some(SpecialFnKind::DatetimeExtract(DatetimeExtractFn::DayOfYear)),
            Self::FromJd => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromJd)),
            Self::FromMjd => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromMjd)),
            Self::FromUnix => Some(SpecialFnKind::DatetimeFrom(DatetimeFromFn::FromUnix)),
            Self::ToJd => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToJd)),
            Self::ToMjd => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToMjd)),
            Self::ToUnix => Some(SpecialFnKind::DatetimeTo(DatetimeToFn::ToUnix)),
            _ => None,
        }
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

/// Resolved expression shape.
#[derive(Debug, Clone)]
pub enum ExprKind {
    Number(f64),
    Integer(i64),
    Bool(bool),
    StringLiteral(String),
    TypeSystemRef(Spanned<TypeSystemRef>),
    GraphRef(Spanned<ResolvedName<namespace::Decl>>),
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
        callee: Spanned<ResolvedName<namespace::Constructor>>,
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
    VariantLiteral(Spanned<ResolvedIndexVariant>),
    InlineDagRef {
        target: Spanned<DagId>,
        args: Vec<ParamBinding>,
        output: Spanned<ResolvedName<namespace::Decl>>,
    },
}

/// Canonical declaration dependencies observed in one HIR expression tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExprDependencies {
    /// Runtime graph dependencies reached through `@name` references.
    pub graph_refs: BTreeSet<ResolvedName<namespace::Decl>>,
    /// Compile-time const dependencies reached through const-like value refs.
    pub const_refs: BTreeSet<ResolvedName<namespace::Decl>>,
    /// Source-span keyed graph references for syntax-AST boundary consumers
    /// that still need to route references by canonical declaration identity.
    pub graph_ref_targets: HashMap<Span, ResolvedName<namespace::Decl>>,
    /// Source-span keyed const references for syntax-AST boundary consumers
    /// that still need to route references by canonical declaration identity.
    pub const_ref_targets: HashMap<Span, ResolvedName<namespace::Decl>>,
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
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral(_)
        | ExprKind::UnitLiteral { .. } => {}
        ExprKind::GraphRef(target) => {
            deps.graph_refs.insert(target.value.clone());
            deps.graph_ref_targets
                .insert(target.span, target.value.clone());
        }
        ExprKind::ConstRef(target) => {
            if let ConstRef::Decl(resolved) = &target.value {
                deps.const_refs.insert(resolved.clone());
                deps.const_ref_targets.insert(target.span, resolved.clone());
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

/// Type-system identifier used as a value expression, usually in include bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSystemRef {
    Type(ResolvedName<namespace::StructType>),
    Dimension(ResolvedName<namespace::Dim>),
    Index(ResolvedName<namespace::Index>),
    IndexVariant(ResolvedIndexVariant),
}

/// Resolved constant-like expression target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstRef {
    Decl(ResolvedName<namespace::Decl>),
    Constructor(ResolvedName<namespace::Constructor>),
    IndexVariant(ResolvedIndexVariant),
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
    pub target: Spanned<ResolvedName<namespace::Decl>>,
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
    IndexVariant(Spanned<ResolvedIndexVariant>),
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
    Named(Spanned<ResolvedName<namespace::Index>>),
    Range { arg: NatExpr, span: Span },
}

/// A resolved index-access argument.
#[derive(Debug, Clone)]
pub enum IndexArg {
    Variant(Spanned<ResolvedIndexVariant>),
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
        constructor: Spanned<ResolvedName<namespace::Constructor>>,
        bindings: Vec<PatternBinding>,
        span: Span,
    },
    IndexLabel {
        variant: Spanned<ResolvedIndexVariant>,
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

struct ExprLowerer<'a> {
    ctx: ExprLoweringContext<'a>,
    local_scopes: Vec<HashMap<LocalName, LocalDef>>,
    next_local: u32,
}

impl<'a> ExprLowerer<'a> {
    const fn new(ctx: ExprLoweringContext<'a>) -> Self {
        Self {
            ctx,
            local_scopes: Vec::new(),
            next_local: 0,
        }
    }

    fn lower_expr(&mut self, expr: &ast::Expr) -> Result<Expr, ExprLowerError> {
        // Recursion choke point: lowering recurses once per tree level
        // (unbounded for left-nested operator chains).
        crate::stack::with_stack_growth(|| self.lower_expr_inner(expr))
    }

    #[expect(clippy::too_many_lines, reason = "exhaustive ExprKind lowering")]
    fn lower_expr_inner(&mut self, expr: &ast::Expr) -> Result<Expr, ExprLowerError> {
        let kind = match &expr.kind {
            ast::ExprKind::Number(value) => ExprKind::Number(*value),
            ast::ExprKind::Integer(value) => ExprKind::Integer(*value),
            ast::ExprKind::Bool(value) => ExprKind::Bool(*value),
            ast::ExprKind::StringLiteral(value) => ExprKind::StringLiteral(value.clone()),
            ast::ExprKind::TypeSystemRef(value) => ExprKind::TypeSystemRef(Spanned::new(
                self.lower_type_system_ref(&value.value, value.span)?,
                value.span,
            )),
            ast::ExprKind::GraphRef(name) => ExprKind::GraphRef(Spanned::new(
                self.resolve_decl_scoped_name(&name.value, name.span)?,
                name.span,
            )),
            ast::ExprKind::ConstRef(name) => ExprKind::ConstRef(Spanned::new(
                self.lower_const_ref(&name.value, name.span)?,
                name.span,
            )),
            ast::ExprKind::LocalRef(ident) => match self
                .lookup_local(&LocalName::from_atom(ident.name.clone()), ident.span)
            {
                Ok(local) => ExprKind::LocalRef(Spanned::new(local, ident.span)),
                Err(ExprLowerError::UnknownLocalRef { .. }) => ExprKind::ConstRef(Spanned::new(
                    self.lower_const_ref(&ScopedName::local(ident.name.as_str()), ident.span)?,
                    ident.span,
                )),
                Err(err) => return Err(err),
            },
            ast::ExprKind::BinOp { op, lhs, rhs } => ExprKind::BinOp {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)?),
                rhs: Box::new(self.lower_expr(rhs)?),
            },
            ast::ExprKind::UnaryOp { op, operand } => ExprKind::UnaryOp {
                op: *op,
                operand: Box::new(self.lower_expr(operand)?),
            },
            ast::ExprKind::FnCall {
                callee,
                type_args,
                args,
            } => ExprKind::FnCall {
                callee: Spanned::new(Self::lower_function_ref(callee)?, callee.span()),
                type_args: type_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg))
                    .collect::<Result<Vec<_>, _>>()?,
                args: args
                    .iter()
                    .map(|arg| self.lower_expr(arg))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => ExprKind::If {
                condition: Box::new(self.lower_expr(condition)?),
                then_branch: Box::new(self.lower_expr(then_branch)?),
                else_branch: Box::new(self.lower_expr(else_branch)?),
            },
            ast::ExprKind::UnitLiteral { value, unit } => ExprKind::UnitLiteral {
                value: *value,
                unit: unit.clone(),
            },
            ast::ExprKind::Convert { expr, target } => ExprKind::Convert {
                expr: Box::new(self.lower_expr(expr)?),
                target: target.clone(),
            },
            ast::ExprKind::DisplayTimezone { expr, timezone } => ExprKind::DisplayTimezone {
                expr: Box::new(self.lower_expr(expr)?),
                timezone: timezone.clone(),
            },
            ast::ExprKind::FieldAccess { expr, field } => ExprKind::FieldAccess {
                expr: Box::new(self.lower_expr(expr)?),
                field: field.clone(),
            },
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
                    .collect::<Result<Vec<_>, _>>()?,
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
                let body = Box::new(self.lower_expr(body)?);
                self.pop_scope();
                ExprKind::ForComp { bindings, body }
            }
            ast::ExprKind::IndexAccess { expr, args } => ExprKind::IndexAccess {
                expr: Box::new(self.lower_expr(expr)?),
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
                let source = Box::new(self.lower_expr(source)?);
                let init = Box::new(self.lower_expr(init)?);
                let acc = self.allocate_local(acc_name.value.clone(), acc_name.span)?;
                let val = self.allocate_local(val_name.value.clone(), val_name.span)?;
                self.push_scope(vec![acc.clone(), val.clone()])?;
                let body = Box::new(self.lower_expr(body)?);
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
                let init = Box::new(self.lower_expr(init)?);
                let prev = self.allocate_local(prev_name.value.clone(), prev_name.span)?;
                let curr = self.allocate_local(curr_name.value.clone(), curr_name.span)?;
                self.push_scope(vec![prev.clone(), curr.clone()])?;
                let body = Box::new(self.lower_expr(body)?);
                self.pop_scope();
                ExprKind::Unfold {
                    init,
                    prev,
                    curr,
                    body,
                }
            }
            ast::ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                scrutinee: Box::new(self.lower_expr(scrutinee)?),
                arms: arms
                    .iter()
                    .map(|arm| self.lower_match_arm(arm))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::ExprKind::VariantLiteral { index, variant } => {
                ExprKind::VariantLiteral(Spanned::new(
                    self.resolve_index_variant_parts(
                        &index.value,
                        &variant.value,
                        index.span,
                        variant.span,
                    )?,
                    index.span.merge(variant.span),
                ))
            }
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
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar/UnresolvedRef(Infallible) proves this arm unreachable"
            )]
            ast::ExprKind::Sugar(s) | ast::ExprKind::UnresolvedRef(s) => never(*s),
        };
        Ok(Expr::new(kind, expr.span))
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

    fn lower_field_init(&mut self, field: &ast::FieldInit) -> Result<FieldInit, ExprLowerError> {
        Ok(FieldInit {
            name: field.name.clone(),
            value: self.lower_expr(&field.value)?,
        })
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
            value: self.lower_expr(&binding.value)?,
            span: binding.span,
        })
    }

    fn lower_type_system_ref(
        &self,
        kind: &SyntaxTypeSystemRefKind,
        span: Span,
    ) -> Result<TypeSystemRef, ExprLowerError> {
        match kind {
            SyntaxTypeSystemRefKind::Type(name) | SyntaxTypeSystemRefKind::Imported(name) => self
                .ctx
                .resolver
                .resolve_struct_type_path(self.ctx.owner, &NamePath::local(name.atom().clone()))
                .map(TypeSystemRef::Type)
                .map_err(|source| ExprLowerError::ModuleResolve { source, span }),
            SyntaxTypeSystemRefKind::Dimension(name) => self
                .ctx
                .resolver
                .resolve_dimension_path(self.ctx.owner, &NamePath::local(name.atom().clone()))
                .map(TypeSystemRef::Dimension)
                .map_err(|source| ExprLowerError::ModuleResolve { source, span }),
            SyntaxTypeSystemRefKind::Index(name) => self
                .ctx
                .resolver
                .resolve_index_path(self.ctx.owner, &NamePath::local(name.atom().clone()))
                .map(TypeSystemRef::Index)
                .map_err(|source| ExprLowerError::ModuleResolve { source, span }),
            SyntaxTypeSystemRefKind::BareVariant(variant) => self
                .ctx
                .resolver
                .resolve_bare_index_variant(self.ctx.owner, variant)
                .map(TypeSystemRef::IndexVariant)
                .map_err(|source| ExprLowerError::ModuleResolve { source, span }),
        }
    }

    fn lower_const_ref(&self, name: &ScopedName, span: Span) -> Result<ConstRef, ExprLowerError> {
        if !name.is_qualified() {
            if let Some(builtin) = BuiltinConst::parse(name.member()) {
                return Ok(ConstRef::Builtin(builtin));
            }
            if let Ok(scale) = name.member().parse::<TimeScale>() {
                return Ok(ConstRef::TimeScale(scale));
            }
            let generic_name = GenericParamName::new(name.member());
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
        match self
            .ctx
            .resolver
            .resolve_index_variant_path(self.ctx.owner, &path)
        {
            Ok(resolved) => return Ok(ConstRef::IndexVariant(resolved)),
            Err(err) => first_error.get_or_insert(err),
        };

        first_error.map_or_else(
            || {
                Err(ExprLowerError::ModuleResolve {
                    source: ModuleResolveError::UnknownName {
                        owner: self.ctx.owner.clone(),
                        namespace: namespace::Decl::DISPLAY_NAME,
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
    ) -> Result<ResolvedName<namespace::Decl>, ExprLowerError> {
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
            .or_else(|_| {
                self.resolve_synthetic_child_decl_path(&path)
                    .ok_or_else(|| ModuleResolveError::UnknownName {
                        owner: self.ctx.owner.clone(),
                        namespace: namespace::Decl::DISPLAY_NAME,
                        name: path.to_string(),
                    })
            })
            .map_err(|source| ExprLowerError::ModuleResolve { source, span })
    }

    fn resolve_synthetic_child_decl_path(
        &self,
        path: &NamePath,
    ) -> Option<ResolvedName<namespace::Decl>> {
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
            .then(|| ResolvedName::from_def(owner, DeclName::from_atom(leaf.clone())))
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
            value: self.lower_expr(&entry.value)?,
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
                Ok(MapEntryKey::IndexVariant(Spanned::new(
                    variant,
                    key.index.span.merge(key.variant.span),
                )))
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
            ast::ForBindingIndex::Named(index) => ForBindingIndex::Named(Spanned::new(
                self.ctx
                    .resolver
                    .resolve_index_path(self.ctx.owner, &index.value)
                    .map_err(|source| ExprLowerError::ModuleResolve {
                        source,
                        span: index.span,
                    })?,
                index.span,
            )),
            ast::ForBindingIndex::Range { arg, span } => ForBindingIndex::Range {
                arg: lower_nat_expr(arg, self.ctx.type_context())?,
                span: *span,
            },
        };
        Ok(ForBinding { local, index })
    }

    fn lower_index_arg(&mut self, arg: &ast::IndexArg) -> Result<IndexArg, ExprLowerError> {
        match arg {
            ast::IndexArg::Variant { index, variant } => Ok(IndexArg::Variant(Spanned::new(
                self.resolve_index_variant_parts(
                    &index.value,
                    &variant.value,
                    index.span,
                    variant.span,
                )?,
                index.span.merge(variant.span),
            ))),
            ast::IndexArg::Var(ident) => Ok(IndexArg::Var(Spanned::new(
                self.lookup_local(&LocalName::from_atom(ident.name.clone()), ident.span)?,
                ident.span,
            ))),
            ast::IndexArg::Expr(expr) => Ok(IndexArg::Expr(Box::new(self.lower_expr(expr)?))),
        }
    }

    fn lower_match_arm(&mut self, arm: &ast::MatchArm) -> Result<MatchArm, ExprLowerError> {
        let pattern = self.lower_match_pattern(&arm.pattern)?;
        self.push_scope(pattern.bound_locals())?;
        let body = self.lower_expr(&arm.body)?;
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
            } => Ok(MatchPattern::IndexLabel {
                variant: Spanned::new(
                    self.resolve_index_variant_parts(
                        &index.value,
                        &variant.value,
                        index.span,
                        variant.span,
                    )?,
                    index.span.merge(variant.span),
                ),
                span: *span,
            }),
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
        let name_path = ident_path_to_name_path(path);
        if bindings.is_empty()
            && let Ok(variant) = self
                .ctx
                .resolver
                .resolve_index_variant_path(self.ctx.owner, &name_path)
        {
            return Ok(MatchPattern::IndexLabel {
                variant: Spanned::new(variant, span),
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

fn ident_path_to_name_path(path: &crate::syntax::ast::IdentPath) -> NamePath {
    let segments = path.segments();
    NamePath::new(NonEmpty::new(
        segments[0].name.clone(),
        segments[1..]
            .iter()
            .map(|segment| segment.name.clone())
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn resolved_source(source: &str) -> ast::File {
        let raw = Parser::new(source).parse_file().unwrap();
        let desugared = crate::syntax::desugar::desugar_multi_decls_in_file(raw);
        crate::syntax::name_resolve::resolve_name_refs(desugared)
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
    fn lowers_qualified_index_variant_const_ref_to_canonical_owner() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub index Phase = { Burn, Coast };");
        let main = resolved_source(
            "import lib as mission; node phase: Dimensionless = mission.Phase.Burn;",
        );
        let resolver = resolver_with_import(&lib_id, &main_id, &lib, &main);
        let scope = GenericScope::new();

        let expr = lower_expr(
            node_value(&main, "phase"),
            ExprLoweringContext::new(&main_id, &resolver, &scope),
        )
        .unwrap();

        let ExprKind::ConstRef(target) = expr.kind else {
            panic!("expected const-like ref, got {expr:?}");
        };
        let ConstRef::IndexVariant(variant) = target.value else {
            panic!("expected index variant, got {target:?}");
        };
        assert_eq!(variant.index().owner(), &lib_id);
        assert_eq!(variant.index().as_str(), "Phase");
        assert_eq!(variant.variant().as_str(), "Burn");
    }

    #[test]
    fn lowers_qualified_nullary_constructor_const_ref_to_canonical_owner() {
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub type BurnKind { Impulsive, Coast }");
        let main =
            resolved_source("import lib as mission; node burn: Dimensionless = mission.Impulsive;");
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
        let owner = DagId::root("main");
        let file = resolved_source(
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
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub type BurnKind { Impulsive(delta_v: Dimensionless), Coast }");
        let main = resolved_source(
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
        let lib_id = DagId::root("lib");
        let main_id = DagId::root("main");
        let lib = resolved_source("pub const node C: Dimensionless = 1.0; param p: Dimensionless;");
        let main = resolved_source(
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
        let owner = DagId::root("main");
        let file = resolved_source("param p: Dimensionless; node x: Dimensionless = p;");
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
