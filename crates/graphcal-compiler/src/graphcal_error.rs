use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};

use crate::diagnostic::{Diagnostic as LocatedDiagnostic, DiagnosticKind as _};
use crate::semantic_error::SemanticErrorKind;
use crate::source_id::SourceId;
use crate::source_registry::SourceRegistry;
use crate::syntax::span::Span;
use thiserror::Error;

use crate::builtin::{AggregationFn, LinearAlgebraFn};
use crate::datetime_literal::CivilDateTimeLiteral;
use crate::declaration_kind::DeclarationKind;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;
use crate::semantic::time_zone::IanaTimeZoneId;
use crate::syntax::dimension::{DimName, UnitName, UnitRef};
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};

/// Rich diagnostic error types for graphcal evaluation.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GraphcalError {
    /// A diagnostic of a typed family, located by source id and span.
    #[error("{}", .0.kind)]
    Located(LocatedDiagnostic<SemanticErrorKind>),

    /// Runtime propagation of an unavailable projected value. This is not a
    /// static checking error; evaluator boundaries retain the typed reason.
    #[error("{reason}")]
    #[diagnostic(code(graphcal::E050))]
    EvaluationUnavailable {
        reason: crate::node_unavailable::NodeUnavailable,
        src: SourceId,
        #[label("value unavailable here")]
        span: SourceSpan,
    },

    #[error("property `{property}` expects {expected}")]
    #[diagnostic(code(graphcal::D015))]
    PlotPropertyTypeMismatch {
        property: &'static str,
        expected: &'static str,
        found: String,
        src: SourceId,
        #[label("this is {found}")]
        span: SourceSpan,
    },

    #[error(
        "property `{property}` must be dimensionless, but this value has dimension {dimension}"
    )]
    #[diagnostic(
        code(graphcal::D016),
        help(
            "plot properties are raw rendering quantities (pixels, ratios); write a plain number instead of a dimensioned value"
        )
    )]
    PlotPropertyDimensioned {
        property: &'static str,
        dimension: String,
        src: SourceId,
        #[label("dimensioned value")]
        span: SourceSpan,
    },

    #[error("encoding channel `{channel}` cannot plot values of type {found}")]
    #[diagnostic(
        code(graphcal::D033),
        help(
            "plot quantities, Int, Bool, Datetime, index keys, or a contextual string literal; project algebraic or Complex values to a plottable field first"
        )
    )]
    PlotEncodingTypeMismatch {
        channel: crate::syntax::ast::EncodingChannel,
        found: String,
        src: SourceId,
        #[label("not a plottable value")]
        span: SourceSpan,
    },

    #[error("plot encoding channels range over incompatible index axes")]
    #[diagnostic(
        code(graphcal::D034),
        help("{channels}; every channel must range over a subset of one channel's axes")
    )]
    PlotEncodingAxisMismatch {
        /// Preformatted channel/axis list at the diagnostic boundary.
        channels: String,
        src: SourceId,
        #[label("this channel cannot align with the shared plot rows")]
        span: SourceSpan,
    },

    #[error("cannot `import` plot `{name}`")]
    #[diagnostic(
        code(graphcal::M021),
        help(
            "plots are runtime sinks evaluated against an instance; request them through an include brace list instead: `include path(...)::{{ {name} }}`"
        )
    )]
    ImportPlotItem {
        name: String,
        src: SourceId,
        #[label("plots cannot travel through `import`")]
        span: SourceSpan,
    },

    #[error("cannot `import` assertion `{name}` from a module blueprint")]
    #[diagnostic(
        code(graphcal::M024),
        help(
            "assertions run in concrete DAG instances; use `include path(...)::{{ {name} }}` or evaluate the library as an entry DAG"
        )
    )]
    ImportAssertionItem {
        name: String,
        src: SourceId,
        #[label("an imported module has no assertion outcome")]
        span: SourceSpan,
    },

    #[error("cannot `import` runtime unit `{name}`")]
    #[diagnostic(
        code(graphcal::M025),
        help(
            "use the unit through a concrete `include` or direct DAG call instance; otherwise declare a `const unit` to grant blueprint import capability"
        )
    )]
    ImportRuntimeUnit {
        name: String,
        src: SourceId,
        #[label("a plain `unit` belongs to a runtime instance")]
        span: SourceSpan,
    },

    #[error("cannot `import` required {kind} input `{name}`")]
    #[diagnostic(
        code(graphcal::M028),
        help("supply this typed input through `include` or a direct DAG call")
    )]
    ImportRequiredStaticInput {
        kind: crate::static_interface::StaticInputKind,
        name: String,
        src: SourceId,
        #[label("required Static input has no blueprint-stable target")]
        span: SourceSpan,
    },

    #[error(
        "cannot `import` `{name}` because it depends on required {dependency_kind} input `{dependency}`"
    )]
    #[diagnostic(
        code(graphcal::M029),
        help(
            "supply the required Static input through `include` before projecting this declaration"
        )
    )]
    ImportUnresolvedStaticDependency {
        name: String,
        dependency_kind: crate::static_interface::StaticInputKind,
        dependency: String,
        src: SourceId,
        #[label("declaration is not blueprint-closed")]
        span: SourceSpan,
    },

    #[error("cannot bind {kind} input `{name}` to non-concrete {kind} `{target}`")]
    #[diagnostic(
        code(graphcal::M030),
        help("bind to a fixed declaration or an optional `pub(bind)` declaration with a default")
    )]
    InvalidStaticBindingTarget {
        kind: crate::static_interface::StaticInputKind,
        name: String,
        target: String,
        src: SourceId,
        #[label("required Static inputs are holes, not concrete targets")]
        span: SourceSpan,
    },

    #[error("cannot project `{name}` from a configured DAG instance")]
    #[diagnostic(
        code(graphcal::M031),
        help(
            "DAGs are reusable blueprints, not instance members; address the child with a dotted blueprint path such as `module.child`"
        )
    )]
    IncludeItemNotProjectable {
        name: String,
        src: SourceId,
        #[label("this declaration is not an instance projection")]
        span: SourceSpan,
    },

    #[error(
        "cannot project constructor `{constructor}` because its owning type `{owner_type}` is rebound"
    )]
    #[diagnostic(
        code(graphcal::M032),
        help(
            "project constructors only when their source nominal type keeps its canonical identity; use constructors of the replacement type instead"
        )
    )]
    IncludeConstructorOwnerRebound {
        constructor: String,
        owner_type: String,
        src: SourceId,
        #[label("source constructor no longer belongs to the projected type")]
        span: SourceSpan,
    },

    #[error("selective include chooses {namespace} producer `{name}` more than once")]
    #[diagnostic(
        code(graphcal::M027),
        help("select each producer at most once in an include list")
    )]
    DuplicateIncludeSelection {
        namespace: ImportItemNamespace,
        name: NameAtom,
        src: SourceId,
        #[label("duplicate producer selection")]
        duplicate: SourceSpan,
        #[label("first selected here")]
        first: SourceSpan,
    },

    #[error("graph reference `@{name}` not allowed in const unit scale")]
    #[diagnostic(
        code(graphcal::D017),
        help(
            "`const unit` scales are compile-time constants; use plain `unit` for runtime-dependent units"
        )
    )]
    GraphRefInConstUnit {
        name: ScopedName,
        src: SourceId,
        #[label("@ reference not allowed in a const unit")]
        span: SourceSpan,
    },

    #[error("non-const unit `{name}` not allowed in const expression")]
    #[diagnostic(
        code(graphcal::D018),
        help(
            "`const node` bodies and `const unit` definitions can only use prelude units, `base unit`, or `const unit` declarations; use `node` or plain `unit` for runtime-unit calculations"
        )
    )]
    NonConstUnitInConst {
        name: UnitRef,
        src: SourceId,
        #[label("unit is not const")]
        span: SourceSpan,
    },

    #[error("{message}")]
    #[diagnostic(code(graphcal::E001))]
    EvalError {
        message: String,
        src: SourceId,
        #[label("error here")]
        span: SourceSpan,
    },

    /// An internal invariant violation that should never be reached if earlier
    /// compiler phases (parsing, resolution, `dim_check`) are correct.
    #[error("internal error: {message}")]
    #[diagnostic(
        code(graphcal::X001),
        help("this is a compiler bug — please report it")
    )]
    InternalError {
        message: String,
        src: SourceId,
        /// Where the violation is reported; resolved against the source text
        /// only when rendered.
        anchor: DiagnosticAnchor,
    },

    #[error("dimension exponent overflow")]
    #[diagnostic(
        code(graphcal::D010),
        help("dimension exponents are stored as `i32`; reduce the magnitude of the exponent")
    )]
    DimensionOverflow {
        src: SourceId,
        #[label("overflow here")]
        span: SourceSpan,
    },

    #[error("dimension mismatch: expected {expected}, found {found}")]
    #[diagnostic(code(graphcal::D001))]
    DimensionMismatch {
        expected: String,
        found: String,
        src: SourceId,
        #[label("has dimension {found}")]
        span: SourceSpan,
        #[help]
        help: String,
    },

    #[error("mismatched index axes in {context}: {lhs} vs {rhs}")]
    #[diagnostic(
        code(graphcal::D011),
        help(
            "element-wise operands must be indexed by the same axes in the same order; an unindexed operand broadcasts to every key"
        )
    )]
    IndexedShapeMismatch {
        context: String,
        lhs: String,
        rhs: String,
        src: SourceId,
        #[label("has type {rhs}")]
        span: SourceSpan,
    },

    #[error("incompatible indexed shape for `{function}()`: expected {expected}, found {found}")]
    #[diagnostic(code(graphcal::D022), help("{help}"))]
    LinearAlgebraShapeMismatch {
        function: LinearAlgebraFn,
        expected: String,
        found: String,
        help: String,
        src: SourceId,
        #[label("found {found}")]
        span: SourceSpan,
    },

    #[error("comparison operators require unindexed operands, found {found}")]
    #[diagnostic(
        code(graphcal::D019),
        help(
            "comparison operators do not broadcast; use an explicit `for` comprehension and compare individual indexed elements in its body"
        )
    )]
    IndexedComparisonOperand {
        found: String,
        src: SourceId,
        #[label("indexed operand has type {found}")]
        span: SourceSpan,
    },

    #[error("`{function}()` does not accept a rank-{rank} indexed value")]
    #[diagnostic(
        code(graphcal::D021),
        help(
            "reduce one axis at a time with an explicit `for` comprehension; total- and partial-axis aggregation are not yet defined"
        )
    )]
    MultiAxisAggregation {
        function: AggregationFn,
        rank: usize,
        src: SourceId,
        #[label("rank-{rank} input")]
        span: SourceSpan,
    },

    #[error("`scan()` requires a rank-one source, found rank-{rank}")]
    #[diagnostic(
        code(graphcal::D026),
        help(
            "scan does not choose an axis implicitly; use an explicit `for` comprehension to select each rank-one series before scanning it"
        )
    )]
    MultiAxisScanSource {
        rank: usize,
        src: SourceId,
        #[label("rank-{rank} source")]
        span: SourceSpan,
    },

    #[error(
        "`{function}()` cannot determine the result dimension without a concrete axis cardinality"
    )]
    #[diagnostic(
        code(graphcal::D027),
        help(
            "apply product() where the index is concrete, or reduce dimensionless values whose result does not depend on cardinality"
        )
    )]
    AggregationCardinalityUnknown {
        function: AggregationFn,
        src: SourceId,
        #[label("axis cardinality is abstract here")]
        span: SourceSpan,
    },

    #[error("materialized indexed value exceeds the eager limit of {maximum} scalar values")]
    #[diagnostic(
        code(graphcal::D035),
        help("reduce one or more axis cardinalities so their product is at most {maximum}")
    )]
    MaterializedShapeTooLarge {
        maximum: usize,
        src: SourceId,
        #[label("this indexed expression would materialize too many values")]
        span: SourceSpan,
    },

    #[error("type annotation mismatch: declared {declared}, inferred {inferred}")]
    #[diagnostic(
        code(graphcal::D002),
        help("the declared type must match the inferred dimension of the expression")
    )]
    DimensionMismatchInAnnotation {
        declared: String,
        inferred: String,
        src: SourceId,
        #[label("declared as {declared}")]
        span: SourceSpan,
    },

    #[error("unit `{name}` is declared as {declared}, but its definition uses {definition}")]
    #[diagnostic(
        code(graphcal::D031),
        help(
            "the unit expression on the right-hand side must have exactly the declared dimension"
        )
    )]
    UnitDefinitionDimensionMismatch {
        name: UnitName,
        declared: String,
        definition: String,
        src: SourceId,
        #[label("this unit expression has dimension {definition}")]
        span: SourceSpan,
    },

    #[error("dynamic unit `{name}` requires a scalar Dimensionless scale, but found {found}")]
    #[diagnostic(
        code(graphcal::D032),
        help(
            "use an unindexed Dimensionless quantity; Bool, Int, structures, indexed values, and dimensioned quantities are not valid unit scales"
        )
    )]
    DynamicUnitScaleTypeMismatch {
        name: UnitRef,
        found: String,
        src: SourceId,
        #[label("this scale expression has type {found}")]
        span: SourceSpan,
    },

    #[error("unknown unit `{name}`")]
    #[diagnostic(
        code(graphcal::D003),
        help(
            "a bare unit name must be declared in this file, selectively imported, or part of the prelude; units of a module imported with an alias are referenced as `alias::unit`"
        )
    )]
    UnknownUnit {
        name: UnitRef,
        src: SourceId,
        #[label("unknown unit")]
        span: SourceSpan,
    },

    #[error("unknown dimension `{name}`")]
    #[diagnostic(
        code(graphcal::D004),
        help("dimension must be declared or part of the prelude")
    )]
    UnknownDimension {
        name: NamePath,
        src: SourceId,
        #[label("unknown dimension")]
        span: SourceSpan,
    },

    #[error("cyclic dimension dependency involving `{name}`")]
    #[diagnostic(
        code(graphcal::D008),
        help("derived dimensions cannot form dependency cycles")
    )]
    CyclicDimension {
        name: DimName,
        src: SourceId,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("cyclic unit dependency involving `{name}`")]
    #[diagnostic(code(graphcal::D009), help("units cannot form dependency cycles"))]
    CyclicUnit {
        name: UnitName,
        src: SourceId,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("a dimensioned base requires a statically exact rational exponent")]
    #[diagnostic(
        code(graphcal::D005),
        help("use an exact integer such as `2` or a parenthesized rational such as `(3/2)`")
    )]
    RuntimeExponentForDimensionedBase {
        src: SourceId,
        #[label("runtime exponent cannot determine the result dimension")]
        span: SourceSpan,
    },

    #[error("float syntax cannot be the exponent of a dimensioned base")]
    #[diagnostic(code(graphcal::D020))]
    FloatPowerExponent {
        /// Exact source replacement when the decimal value fits the dimension
        /// rational model. Also carried as structured LSP diagnostic data.
        replacement: Option<String>,
        src: SourceId,
        #[label("exact rational syntax is required here")]
        span: SourceSpan,
        #[help]
        help: String,
    },

    #[error("conversion target dimension {target} does not match expression dimension {expr_dim}")]
    #[diagnostic(
        code(graphcal::D006),
        help("the `->` conversion operator can only change units within the same dimension")
    )]
    ConversionDimensionMismatch {
        target: String,
        expr_dim: String,
        src: SourceId,
        #[label("target unit has different dimension")]
        span: SourceSpan,
    },

    #[error("`->` cannot be applied to an expression that already has a display target")]
    #[diagnostic(
        code(graphcal::D012),
        help(
            "an expression carries at most one `->` target; remove the inner conversion — only the outermost target takes effect"
        )
    )]
    NestedConversion {
        src: SourceId,
        #[label("the operand of this conversion is itself a conversion")]
        span: SourceSpan,
    },

    #[error("`->` has no effect in this position")]
    #[diagnostic(
        code(graphcal::D013),
        help(
            "a conversion only affects how a declaration's final value is displayed; move it to the top level of the declaration (or a selected `if`/`match` branch, constructor field, map entry, for-comprehension body, or scan/unfold init), or remove it"
        )
    )]
    IneffectiveConversion {
        src: SourceId,
        #[label("this conversion's display target is discarded")]
        span: SourceSpan,
    },

    #[error("cannot declare `{name}` as the base unit of dimension `{dim}`")]
    #[diagnostic(code(graphcal::D036), help("{help}"))]
    InvalidBaseUnitDeclaration {
        name: UnitName,
        dim: String,
        reason: String,
        help: String,
        src: SourceId,
        #[label("{reason}")]
        span: SourceSpan,
    },

    #[error("user-defined units on dimension `{dim}` are not supported")]
    #[diagnostic(
        code(graphcal::D014),
        help(
            "common units of this dimension (e.g. \u{b0}C, \u{b0}F for Temperature) are affine scales with an offset; a purely multiplicative `unit` definition would display silently wrong values. Keep values in the base unit, or model the offset explicitly in your expressions"
        )
    )]
    AffineProneUnitDefinition {
        dim: String,
        src: SourceId,
        #[label("unit defined on an affine-prone dimension")]
        span: SourceSpan,
    },

    #[error("name `{name}` not found in imported file `{file_path}`")]
    #[diagnostic(
        code(graphcal::M003),
        help("check that the name is declared in the imported file")
    )]
    ImportNameNotFound {
        name: String,
        file_path: String,
        src: SourceId,
        #[label("not found in imported file")]
        span: SourceSpan,
    },

    #[error("in imported file `{file_path}`, {mismatch}")]
    #[diagnostic(code(graphcal::M022))]
    ImportCategoryMismatch {
        file_path: String,
        mismatch: ImportItemCategoryMismatch,
        src: SourceId,
        #[label("wrong import category")]
        span: SourceSpan,
    },

    #[error("duplicate module name `{name}`")]
    #[diagnostic(code(graphcal::M005))]
    DuplicateModuleName {
        name: String,
        src: SourceId,
        #[label("duplicate module import")]
        span: SourceSpan,
        #[label("first imported here")]
        first: SourceSpan,
    },

    #[error("unknown module `{name}`")]
    #[diagnostic(
        code(graphcal::M006),
        help(
            "module-qualified references start with a local name introduced by `import`; call an aliased module through that alias, for example `import pkg.module as m; @m(...)::out`"
        )
    )]
    UnknownModule {
        name: String,
        src: SourceId,
        #[label("unknown module")]
        span: SourceSpan,
    },

    #[error("unknown param `{name}` in import binding for `{file_path}`")]
    #[diagnostic(
        code(graphcal::M009),
        help("param bindings must reference `param` declarations in the imported file")
    )]
    UnknownParamBinding {
        name: String,
        file_path: String,
        src: SourceId,
        #[label("not a param in the imported file")]
        span: SourceSpan,
    },

    #[error("binding target `{name}` is a {actual_kind}, not a param")]
    #[diagnostic(
        code(graphcal::M010),
        help("only `param` declarations can be overridden in import bindings")
    )]
    BindingNotAParam {
        name: String,
        actual_kind: DeclarationKind,
        src: SourceId,
        #[label("targets a {actual_kind}, not a param")]
        span: SourceSpan,
    },

    #[error("`{name}` is not a {expected} input of the invoked DAG")]
    #[diagnostic(
        code(graphcal::M011),
        help(
            "the binding marker selects exactly one input category; correct the marker or target name"
        )
    )]
    DagInputCategoryMismatch {
        name: String,
        expected: &'static str,
        src: SourceId,
        #[label("no {expected} input with this name")]
        span: SourceSpan,
    },

    #[error("invalid type-level binding value for `{name}`")]
    #[diagnostic(
        code(graphcal::M016),
        help(
            "index bindings accept a compatible index name or structural axis such as `index {name}: Fin(3)`; type and dimension bindings use the explicit `type` and `dim` markers"
        )
    )]
    InvalidTypeLevelBindingValue {
        name: String,
        src: SourceId,
        #[label("expected a compatible type-level binding argument")]
        span: SourceSpan,
    },

    #[error("index binding `{dep_index}: {value}`: `{value}` is not a known index")]
    #[diagnostic(
        code(graphcal::M019),
        help(
            "the right-hand side must name a compatible index visible in the including DAG or use a structural Fin(N) axis"
        )
    )]
    IndexBindingNotAnIndex {
        dep_index: String,
        value: String,
        src: SourceId,
        #[label("not a known index")]
        span: SourceSpan,
    },

    #[error("index kind mismatch: `{dep_index}` is {dep_kind} but `{bound_index}` is {bound_kind}")]
    #[diagnostic(
        code(graphcal::M018),
        help(
            "unconstrained required indexes accept named or Fin(N) axes; concrete named indexes remain named-only, and coordinate indexes remain coordinate-only"
        )
    )]
    IndexKindMismatch {
        dep_index: String,
        dep_kind: String,
        bound_index: String,
        bound_kind: String,
        src: SourceId,
        #[label("kind mismatch")]
        span: SourceSpan,
    },

    #[error("cannot import runtime item `{name}`; use `include` for runtime nodes and params")]
    #[diagnostic(
        code(graphcal::M020),
        help(
            "`import` only allows compile-time items (const, dimension, unit, type, index, dag); use `include` for runtime nodes and params"
        )
    )]
    ImportRuntimeItem {
        name: String,
        src: SourceId,
        #[label("runtime item cannot be imported")]
        span: SourceSpan,
    },

    // --- Domain constraint errors ---
    #[error("unknown timezone `{timezone}`")]
    #[diagnostic(
        code(graphcal::D007),
        help(
            "use a name present in Graphcal's bundled IANA tzdb {tzdb_version}, such as \"UTC\", \"America/New_York\", or \"Asia/Tokyo\""
        )
    )]
    InvalidTimezone {
        timezone: String,
        tzdb_version: &'static str,
        src: SourceId,
        #[label("not a recognized IANA timezone")]
        span: SourceSpan,
    },

    #[error("invalid datetime literal: {reason}")]
    #[diagnostic(code(graphcal::D028), help("{expectation}"))]
    InvalidDatetimeLiteral {
        expectation: crate::datetime_literal::DatetimeLiteralExpectation,
        reason: String,
        src: SourceId,
        #[label("does not satisfy this constructor's datetime literal contract")]
        span: SourceSpan,
    },

    #[error("epoch requires exactly one static time-scale argument, got {got}")]
    #[diagnostic(
        code(graphcal::D023),
        help(
            "write a supported scale in angle brackets, for example `epoch<TT>(\"2024-11-05T12:00:00\")`"
        )
    )]
    EpochTimeScaleArgumentCount {
        got: usize,
        src: SourceId,
        #[label("expected exactly one time scale here")]
        span: SourceSpan,
    },

    #[error("epoch's static time-scale argument must be a bare name")]
    #[diagnostic(code(graphcal::D029), help("use one of {expected}"))]
    InvalidEpochTimeScaleArgument {
        expected: String,
        src: SourceId,
        #[label("expected a supported bare time-scale name")]
        span: SourceSpan,
    },

    #[error("unsupported epoch time scale `{name}`")]
    #[diagnostic(code(graphcal::D030), help("use one of {expected}"))]
    UnsupportedEpochTimeScale {
        name: NameAtom,
        expected: String,
        src: SourceId,
        #[label("not a supported time scale")]
        span: SourceSpan,
    },

    #[error("local civil datetime `{datetime}` does not exist in timezone `{time_zone}`")]
    #[diagnostic(
        code(graphcal::D024),
        help(
            "the timezone offset jumps from {before} to {after} across this gap; choose an existing local time or use one-argument `datetime` with an explicit offset"
        )
    )]
    NonexistentCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        src: SourceId,
        #[label("this local time is skipped")]
        datetime_span: SourceSpan,
        #[label("gap occurs in this timezone")]
        time_zone_span: SourceSpan,
    },

    #[error("local civil datetime `{datetime}` occurs twice in timezone `{time_zone}`")]
    #[diagnostic(
        code(graphcal::D025),
        help(
            "the repeated time can use offset {before} or {after}; use one-argument `datetime` with an explicit offset to select an instant"
        )
    )]
    RepeatedCivilDateTime {
        datetime: CivilDateTimeLiteral,
        time_zone: IanaTimeZoneId,
        before: jiff::tz::Offset,
        after: jiff::tz::Offset,
        src: SourceId,
        #[label("this local time is repeated")]
        datetime_span: SourceSpan,
        #[label("fold occurs in this timezone")]
        time_zone_span: SourceSpan,
    },
}

/// A cancellable operation that fails with a [`GraphcalError`] reports it as
/// [`Outcome::Failed`]; cancellation only ever
/// comes from [`Cancelled`](crate::cancellation::Cancelled).
impl From<GraphcalError> for Outcome<GraphcalError> {
    fn from(error: GraphcalError) -> Self {
        Self::Failed(error)
    }
}

impl GraphcalError {
    /// Locate a typed family diagnostic at `primary` in `src`.
    #[must_use]
    pub fn located(src: SourceId, primary: Span, kind: impl Into<SemanticErrorKind>) -> Self {
        Self::Located(LocatedDiagnostic::new(src, primary, kind.into()))
    }

    /// Construct an internal diagnostic with an explicit source-anchor policy.
    #[must_use]
    #[cold]
    pub fn internal_error(
        message: impl Into<String>,
        src: SourceId,
        anchor: DiagnosticAnchor,
    ) -> Self {
        Self::InternalError {
            message: message.into(),
            src,
            anchor,
        }
    }

    /// The source this error's spans index into.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        match self {
            Self::Located(diagnostic) => diagnostic.src,
            Self::PlotPropertyTypeMismatch { src, .. }
            | Self::PlotPropertyDimensioned { src, .. }
            | Self::PlotEncodingTypeMismatch { src, .. }
            | Self::PlotEncodingAxisMismatch { src, .. }
            | Self::ImportPlotItem { src, .. }
            | Self::ImportAssertionItem { src, .. }
            | Self::ImportRuntimeUnit { src, .. }
            | Self::ImportRequiredStaticInput { src, .. }
            | Self::ImportUnresolvedStaticDependency { src, .. }
            | Self::InvalidStaticBindingTarget { src, .. }
            | Self::IncludeItemNotProjectable { src, .. }
            | Self::IncludeConstructorOwnerRebound { src, .. }
            | Self::DuplicateIncludeSelection { src, .. }
            | Self::GraphRefInConstUnit { src, .. }
            | Self::NonConstUnitInConst { src, .. }
            | Self::EvalError { src, .. }
            | Self::EvaluationUnavailable { src, .. }
            | Self::InternalError { src, .. }
            | Self::DimensionOverflow { src, .. }
            | Self::DimensionMismatch { src, .. }
            | Self::IndexedShapeMismatch { src, .. }
            | Self::LinearAlgebraShapeMismatch { src, .. }
            | Self::IndexedComparisonOperand { src, .. }
            | Self::MultiAxisAggregation { src, .. }
            | Self::MultiAxisScanSource { src, .. }
            | Self::AggregationCardinalityUnknown { src, .. }
            | Self::MaterializedShapeTooLarge { src, .. }
            | Self::DimensionMismatchInAnnotation { src, .. }
            | Self::UnitDefinitionDimensionMismatch { src, .. }
            | Self::DynamicUnitScaleTypeMismatch { src, .. }
            | Self::UnknownUnit { src, .. }
            | Self::UnknownDimension { src, .. }
            | Self::CyclicDimension { src, .. }
            | Self::CyclicUnit { src, .. }
            | Self::RuntimeExponentForDimensionedBase { src, .. }
            | Self::FloatPowerExponent { src, .. }
            | Self::ConversionDimensionMismatch { src, .. }
            | Self::NestedConversion { src, .. }
            | Self::IneffectiveConversion { src, .. }
            | Self::InvalidBaseUnitDeclaration { src, .. }
            | Self::AffineProneUnitDefinition { src, .. }
            | Self::ImportNameNotFound { src, .. }
            | Self::ImportCategoryMismatch { src, .. }
            | Self::DuplicateModuleName { src, .. }
            | Self::UnknownModule { src, .. }
            | Self::UnknownParamBinding { src, .. }
            | Self::BindingNotAParam { src, .. }
            | Self::DagInputCategoryMismatch { src, .. }
            | Self::InvalidTypeLevelBindingValue { src, .. }
            | Self::IndexBindingNotAnIndex { src, .. }
            | Self::IndexKindMismatch { src, .. }
            | Self::ImportRuntimeItem { src, .. }
            | Self::InvalidTimezone { src, .. }
            | Self::InvalidDatetimeLiteral { src, .. }
            | Self::EpochTimeScaleArgumentCount { src, .. }
            | Self::InvalidEpochTimeScaleArgument { src, .. }
            | Self::UnsupportedEpochTimeScale { src, .. }
            | Self::NonexistentCivilDateTime { src, .. }
            | Self::RepeatedCivilDateTime { src, .. } => *src,
        }
    }
}

/// A [`GraphcalError`] together with the source text its spans index into,
/// ready for `miette`.
///
/// The error itself names its source only by [`SourceId`]; the shell resolves
/// that id through the [`SourceRegistry`] that issued it.
#[derive(Debug)]
pub struct RenderedGraphcalError {
    /// The rendered error; readable (and matchable) but only constructed with
    /// its source through [`Self::new`].
    pub error: GraphcalError,
    source: NamedSource<Arc<String>>,
}

impl RenderedGraphcalError {
    /// Attach the source `error` points into, resolved through `registry`.
    ///
    /// An id from another registry has no text to point into; the error is
    /// then rendered against an empty, explicitly unknown source.
    #[must_use]
    pub fn new(error: GraphcalError, registry: &SourceRegistry) -> Self {
        let source = registry.renderable(error.source());
        Self { error, source }
    }

    /// The rendered error.
    #[must_use]
    pub const fn error(&self) -> &GraphcalError {
        &self.error
    }

    /// The named source the error's labels index into.
    #[must_use]
    pub const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.source
    }
}

impl std::fmt::Display for RenderedGraphcalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for RenderedGraphcalError {}

impl Diagnostic for RenderedGraphcalError {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match &self.error {
            GraphcalError::Located(diagnostic) => Some(Box::new(diagnostic.kind.code())),
            error => error.code(),
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match &self.error {
            GraphcalError::Located(diagnostic) => diagnostic
                .kind
                .help()
                .map(|help| Box::new(help) as Box<dyn std::fmt::Display + 'a>),
            error => error.help(),
        }
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        match &self.error {
            GraphcalError::Located(diagnostic) => {
                let primary = miette::LabeledSpan::new_with_span(
                    diagnostic.kind.primary_label(),
                    diagnostic.primary,
                );
                let secondary =
                    diagnostic.kind.secondary_labels().into_iter().map(|label| {
                        miette::LabeledSpan::new_with_span(Some(label.text), label.span)
                    });
                Some(Box::new(std::iter::once(primary).chain(secondary)))
            }
            GraphcalError::InternalError { anchor, .. } => Some(Box::new(
                anchor
                    .resolve(self.source.inner().len())
                    .map(|span| {
                        miette::LabeledSpan::new_with_span(
                            Some("unexpected state here".to_owned()),
                            span,
                        )
                    })
                    .into_iter(),
            )),
            error => error.labels(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use miette::Diagnostic as _;

    use super::GraphcalError;
    use crate::diagnostic_anchor::DiagnosticAnchor;
    use crate::source_registry::SourceRegistry;

    fn diagnostic_code_catalog() -> BTreeMap<String, String> {
        let source = include_str!("graphcal_error.rs");
        let prefix = concat!("code(", "graphcal::");
        let mut pending_code = None;
        let mut catalog = BTreeMap::new();

        for line in source.lines() {
            if let Some((_, suffix)) = line.split_once(prefix) {
                let code = suffix
                    .split_once(')')
                    .map(|(code, _)| code)
                    .expect("diagnostic code must end with `)`");
                assert!(pending_code.replace(code.to_string()).is_none());
                continue;
            }

            let Some(code) = pending_code.as_ref() else {
                continue;
            };
            let Some(candidate) = line.strip_prefix("    ") else {
                continue;
            };
            if candidate.starts_with(' ') || candidate.starts_with('#') {
                continue;
            }
            let Some(delimiter) = candidate.find(['{', '(']) else {
                continue;
            };
            let variant = candidate[..delimiter].trim();
            if variant.is_empty()
                || !variant.starts_with(char::is_uppercase)
                || !variant.chars().all(char::is_alphanumeric)
            {
                continue;
            }

            let previous = catalog.insert(variant.to_string(), code.clone());
            assert!(
                previous.is_none(),
                "duplicate variant `{variant}` in catalog"
            );
            pending_code = None;
        }

        assert!(pending_code.is_none(), "diagnostic code without a variant");
        catalog
    }

    #[test]
    fn internal_error_renders_whole_file_and_builtin_anchors_honestly() {
        let mut registry = SourceRegistry::new();
        let text = "node x";
        let source = registry.register("test.gcl", Arc::new(text.to_string()));

        let whole_file = super::RenderedGraphcalError::new(
            GraphcalError::internal_error("whole file", source, DiagnosticAnchor::WholeFile),
            &registry,
        );
        let whole_file_labels = whole_file
            .labels()
            .expect("internal diagnostics expose a label iterator")
            .collect::<Vec<_>>();
        assert_eq!(whole_file_labels.len(), 1);
        assert_eq!(whole_file_labels[0].offset(), 0);
        assert_eq!(whole_file_labels[0].len(), text.len());
        assert_eq!(whole_file.named_source().name(), "test.gcl");

        let builtin = super::RenderedGraphcalError::new(
            GraphcalError::internal_error("builtin", source, DiagnosticAnchor::Builtin),
            &registry,
        );
        assert_eq!(
            builtin
                .labels()
                .expect("internal diagnostics expose a label iterator")
                .count(),
            0
        );
    }

    #[test]
    fn foreign_source_ids_render_against_an_explicitly_unknown_source() {
        let source = SourceRegistry::new().register("other.gcl", Arc::new("x".to_string()));
        let rendered = super::RenderedGraphcalError::new(
            GraphcalError::internal_error("foreign", source, DiagnosticAnchor::WholeFile),
            &SourceRegistry::new(),
        );
        assert_eq!(rendered.named_source().name(), "<unknown source>");
        assert_eq!(rendered.error().source(), source);
    }

    #[test]
    fn diagnostic_codes_are_unique_and_reassignments_are_pinned() {
        let mut catalog = diagnostic_code_catalog();
        for (variant, code) in crate::semantic_error::tests::family_code_catalog() {
            assert!(
                catalog.insert(variant.clone(), code).is_none(),
                "variant `{variant}` is defined twice"
            );
        }
        assert!(catalog.len() > 100, "incomplete catalog: {catalog:?}");

        let mut variants_by_code = BTreeMap::new();
        for (variant, code) in &catalog {
            if let Some(previous) = variants_by_code.insert(code, variant) {
                panic!("diagnostic code `{code}` is shared by `{previous}` and `{variant}`");
            }
        }

        for (variant, expected) in [
            ("LinearAlgebraShapeMismatch", "D022"),
            ("AggregationCardinalityUnknown", "D027"),
            ("MaterializedShapeTooLarge", "D035"),
            ("InvalidDatetimeLiteral", "D028"),
            ("EpochTimeScaleArgumentCount", "D023"),
            ("InvalidEpochTimeScaleArgument", "D029"),
            ("UnsupportedEpochTimeScale", "D030"),
            ("ImportRuntimeItem", "M020"),
        ] {
            assert_eq!(catalog.get(variant).map(String::as_str), Some(expected));
        }
    }

    #[test]
    fn typed_member_and_function_payloads_render_their_source_spelling() {
        use crate::builtin::{BuiltinFn, ScalarFn};
        use crate::semantic_error::name::CalledFunction;
        use crate::semantic_error::structure::NominalMember;
        use crate::syntax::function_name::FnName;
        use crate::syntax::type_name::{ConstructorName, FieldName};

        assert_eq!(
            CalledFunction::Builtin(BuiltinFn::Scalar(ScalarFn::Sqrt)).to_string(),
            "sqrt"
        );
        assert_eq!(
            CalledFunction::Extern(FnName::expect_valid("lerp")).to_string(),
            "lerp"
        );
        assert_eq!(
            NominalMember::Field(FieldName::expect_valid("dv")).to_string(),
            "dv"
        );
        assert_eq!(
            NominalMember::Constructor(ConstructorName::expect_valid("Coast")).to_string(),
            "Coast"
        );
    }
}
