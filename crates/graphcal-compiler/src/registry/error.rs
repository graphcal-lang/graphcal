use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::builtin::BuiltinFnName;
use crate::datetime_literal::CivilDateTimeLiteral;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::resolve_types::{AttributeTarget, DeclarationKind};
use crate::registry::time_scale::TimeScale;
use crate::registry::time_zone::IanaTimeZoneId;
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitName, UnitRef};
use crate::syntax::function_name::{FnName, FnParamName};
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::index_name::{IndexEntryKey, IndexName, IndexVariantName};
use crate::syntax::module_name::ScopedName;
use crate::syntax::module_resolve::DeclSymbolKind;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

fn format_index_entry_keys(keys: &[IndexEntryKey]) -> String {
    keys.iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rich diagnostic error types for graphcal evaluation.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GraphcalError {
    /// Cooperative cancellation requested by the embedding shell.
    ///
    /// This variant is intercepted at the operation boundary and is never
    /// rendered as a source diagnostic.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Cancelled(#[from] crate::cancellation::Cancelled),

    #[error("duplicate name `{name}`")]
    #[diagnostic(code(graphcal::N001), help("each name must be unique within a file"))]
    DuplicateName {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate definition here")]
        duplicate: SourceSpan,
        #[label("first defined here")]
        first: SourceSpan,
    },

    /// A constructor payload repeats a field declaration.
    #[error("constructor `{constructor}` declares field `{field}` more than once")]
    #[diagnostic(
        code(graphcal::N016),
        help("constructor field names must be unique; remove or rename one declaration")
    )]
    DuplicateConstructorField {
        type_name: StructTypeName,
        constructor: ConstructorName,
        field: FieldName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate `{field}` field in `{type_name}.{constructor}`")]
        duplicate: SourceSpan,
        #[label("first `{field}` field declared here")]
        first: SourceSpan,
    },

    #[error("{kind} `{name}` shadows a built-in name")]
    #[diagnostic(
        code(graphcal::N009),
        help(
            "choose a different name; prelude dimensions, built-in types, prelude units, and built-in numeric constants cannot be redefined in their namespaces"
        )
    )]
    BuiltinNameShadowed {
        kind: &'static str,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("shadows a built-in name")]
        span: SourceSpan,
    },

    #[error("conflicting definitions of unit `{name}` reach this file through includes")]
    #[diagnostic(
        code(graphcal::N010),
        help(
            "included modules share this file's unit scope, and two of them define `{name}` with different dimensions or scales, so references would be ambiguous — rename one of the definitions"
        )
    )]
    ConflictingImportedUnit {
        name: UnitRef,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("import brings in a conflicting `{name}`")]
        span: SourceSpan,
    },

    #[error("property `{property}` is not valid in {context}")]
    #[diagnostic(code(graphcal::N011), help("{valid}"))]
    InvalidPlotProperty {
        property: String,
        context: &'static str,
        /// Preformatted help listing the valid property set for `context`.
        valid: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a {context} property")]
        span: SourceSpan,
    },

    #[error("property `{property}` expects {expected}")]
    #[diagnostic(code(graphcal::D015))]
    PlotPropertyTypeMismatch {
        property: &'static str,
        expected: &'static str,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("a plain `unit` belongs to a runtime instance")]
        span: SourceSpan,
    },

    #[error("cannot `import` required {kind} input `{name}`")]
    #[diagnostic(
        code(graphcal::M028),
        help("supply this typed input through `include` or a direct DAG call")
    )]
    ImportRequiredStaticInput {
        kind: crate::ir::static_interface::StaticInputKind,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        dependency_kind: crate::ir::static_interface::StaticInputKind,
        dependency: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("declaration is not blueprint-closed")]
        span: SourceSpan,
    },

    #[error("cannot bind {kind} input `{name}` to non-concrete {kind} `{target}`")]
    #[diagnostic(
        code(graphcal::M030),
        help("bind to a fixed declaration or an optional `pub(bind)` declaration with a default")
    )]
    InvalidStaticBindingTarget {
        kind: crate::ir::static_interface::StaticInputKind,
        name: String,
        target: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate producer selection")]
        duplicate: SourceSpan,
        #[label("first selected here")]
        first: SourceSpan,
    },

    #[error("attribute `hidden` does not apply to include item `{name}`")]
    #[diagnostic(
        code(graphcal::A018),
        help("`#[hidden]` on an include item is only valid when the item names a plot")
    )]
    HiddenIncludeItemNotAPlot {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a plot item")]
        span: SourceSpan,
    },

    #[error("{owner_kind} `{owner}` references unknown plot `{name}`")]
    #[diagnostic(
        code(graphcal::N012),
        help("`plots:` entries must name `plot` declarations visible in this file")
    )]
    UnknownPlotReference {
        owner_kind: &'static str,
        owner: ScopedName,
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no plot with this name")]
        span: SourceSpan,
    },

    #[error("`{name}` is a {actual_kind}, not a plot")]
    #[diagnostic(
        code(graphcal::N013),
        help("{owner_kind}s compose `plot` declarations; they cannot nest other {actual_kind}s")
    )]
    CompositionReferencesNonPlot {
        owner_kind: &'static str,
        actual_kind: &'static str,
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this names a {actual_kind}")]
        span: SourceSpan,
    },

    #[error("{owner_kind} `{owner}` lists plot `{name}` more than once")]
    #[diagnostic(
        code(graphcal::N014),
        help("each plot may appear at most once in a `plots:` list")
    )]
    DuplicatePlotReference {
        owner_kind: &'static str,
        owner: ScopedName,
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate entry")]
        span: SourceSpan,
    },

    #[error("unknown graph reference `@{name}`")]
    #[diagnostic(
        code(graphcal::N002),
        help("graph references must point to a `param` or `node`")
    )]
    UnknownGraphRef {
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },

    #[error("bare reference `{name}` names a {kind}; user graph declarations require `@`")]
    #[diagnostic(code(graphcal::N017), help("write `@{name}` to reference this {kind}"))]
    BareGraphDeclarationRef {
        name: ScopedName,
        kind: DeclSymbolKind,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("missing `@` sigil")]
        span: SourceSpan,
    },

    #[error("time scale `{scale}` cannot be used as a value")]
    #[diagnostic(
        code(graphcal::N018),
        help(
            "time scales are Static atoms; use `{scale}` in `Datetime<{scale}>` or `epoch<{scale}>(...)`"
        )
    )]
    TimeScaleInValuePosition {
        scale: TimeScale,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no Term named `{scale}` is in scope")]
        span: SourceSpan,
    },

    #[error("unknown function `{name}`")]
    #[diagnostic(
        code(graphcal::N004),
        help("check function name and ensure it is defined")
    )]
    UnknownFunction {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown function")]
        span: SourceSpan,
    },

    #[error("plugin alias `{alias}` does not declare a function `{name}`")]
    #[diagnostic(
        code(graphcal::P002),
        help(
            "extern functions must be declared in the plugin's `import plugin ... {{ ... }}` block"
        )
    )]
    UnknownExternFunction {
        alias: crate::syntax::module_name::ModuleAliasName,
        name: crate::syntax::function_name::FnName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown extern function")]
        span: SourceSpan,
    },

    #[error("function `{name}` uses positional arguments")]
    #[diagnostic(code(graphcal::N015), help("write `{positional_call}`"))]
    NamedArgumentsOnFunction {
        name: String,
        positional_call: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("named arguments are not allowed in function calls")]
        span: SourceSpan,
    },

    #[error("invalid extern function signature: {message}")]
    #[diagnostic(
        code(graphcal::P001),
        help(
            "extern signatures support Bool, Int, quantity types, indexed collections of those scalar kinds over one or more declared index variables, and record struct returns with concrete fields; each dimension variable must be declared in the `<...>` binder list and bound by a bare quantity parameter or bare quantity-array element before compound uses, and every result axis must reuse an index variable that indexes some parameter"
        )
    )]
    InvalidExternSignature {
        message: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("invalid signature")]
        span: SourceSpan,
    },

    #[error("duplicate parameter `{name}` in extern function signature")]
    #[diagnostic(
        code(graphcal::P011),
        help("each extern function parameter name must be unique")
    )]
    DuplicateExternParameter {
        name: FnParamName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate parameter")]
        duplicate: SourceSpan,
        #[label("first declared here")]
        first: SourceSpan,
    },

    #[error("extern function `{name}` (plugin \"{plugin}\") is not provided by the host")]
    #[diagnostic(
        code(graphcal::P003),
        help(
            "the embedder's host function registry has no entry for this declared extern function"
        )
    )]
    MissingHostFunction {
        plugin: crate::syntax::plugin::PluginPath,
        name: crate::syntax::function_name::FnName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("missing host function")]
        span: SourceSpan,
    },

    #[error("extern function call `{name}` not allowed in {context}")]
    #[diagnostic(
        code(graphcal::P004),
        help(
            "extern functions are runtime-provided and can only be called from runtime expressions (nodes, param defaults, and asserts)"
        )
    )]
    ExternCallNotAllowed {
        name: String,
        context: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("extern call not allowed here")]
        span: SourceSpan,
    },

    #[error(
        "extern function `{name}` is declared with signature {declared}, but plugin \"{plugin}\" provides {provided}"
    )]
    #[diagnostic(
        code(graphcal::P005),
        help(
            "the extern declaration must structurally match the signature in the plugin's manifest (dimension-variable and parameter names may differ; the dimensional shape may not); note that plugin manifests cannot express user-defined base dimensions"
        )
    )]
    ExternSignatureMismatch {
        plugin: crate::syntax::plugin::PluginPath,
        name: crate::syntax::function_name::FnName,
        declared: String,
        provided: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("signature does not match the plugin manifest")]
        span: SourceSpan,
    },

    #[error("failed to load plugin \"{plugin}\": {reason}")]
    #[diagnostic(
        code(graphcal::P006),
        help(
            "plugin paths ending in `.wasm` resolve relative to the project root and must name a vendored WebAssembly module with an embedded graphcal manifest"
        )
    )]
    PluginLoadFailed {
        plugin: crate::syntax::plugin::PluginPath,
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("plugin failed to load")]
        span: SourceSpan,
    },

    #[error("plugin \"{plugin}\" imports `{import_module}::{import_name}`, which is not allowed")]
    #[diagnostic(
        code(graphcal::P007),
        help(
            "graphcal plugins must be pure: they may import nothing except `graphcal::fail`. A module importing WASI or other host APIs is not a graphcal plugin (rebuild for a bare wasm32 target or stub the imports out)"
        )
    )]
    PluginForbiddenImport {
        plugin: crate::syntax::plugin::PluginPath,
        import_module: String,
        import_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("plugin declares a forbidden import")]
        span: SourceSpan,
    },

    #[error("plugin \"{plugin}\" is imported by a dependency package")]
    #[diagnostic(
        code(graphcal::P008),
        help(
            "WebAssembly plugin files can currently be imported only by the root package; dependency packages may still use embedder-provided (host registry) plugins"
        )
    )]
    PluginInDependencyPackage {
        plugin: crate::syntax::plugin::PluginPath,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wasm plugin import in a dependency package")]
        span: SourceSpan,
    },

    #[error("plugin \"{plugin}\" is not pinned in graphcal.lock")]
    #[diagnostic(
        code(graphcal::P009),
        help(
            "projects with a graphcal.toml load plugin binaries only through lockfile pins; run `graphcal deps lock` to record this plugin's hash"
        )
    )]
    PluginNotPinned {
        plugin: crate::syntax::plugin::PluginPath,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("plugin has no graphcal.lock pin")]
        span: SourceSpan,
    },

    #[error(
        "plugin \"{plugin}\" does not match its graphcal.lock pin: file hashes to {actual}, lockfile pins {expected}"
    )]
    #[diagnostic(
        code(graphcal::P010),
        help(
            "the lockfile is the trust boundary for plugin code — a changed binary must arrive together with a reviewed pin update; if this change is intentional, rerun `graphcal deps lock`"
        )
    )]
    PluginHashMismatch {
        plugin: crate::syntax::plugin::PluginPath,
        expected: String,
        actual: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("plugin file does not match its pin")]
        span: SourceSpan,
    },

    #[error("graph reference `@{name}` not allowed in const expression")]
    #[diagnostic(
        code(graphcal::N005),
        help(
            "const expressions are evaluated at compile time and cannot reference params or nodes"
        )
    )]
    GraphRefInConst {
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("@ reference not allowed here")]
        span: SourceSpan,
    },

    #[error("DAG call `{name}` is not allowed in a compile-time expression")]
    #[diagnostic(
        code(graphcal::G007),
        help(
            "a DAG call is an anonymous runtime include; call it from a `node`, `param` default, assertion, or visualization expression instead"
        )
    )]
    DagCallInCompileTime {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("runtime DAG instantiation is not allowed here")]
        span: SourceSpan,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unit is not const")]
        span: SourceSpan,
    },

    #[error("function `{name}` expects {expected} argument(s), got {got}")]
    #[diagnostic(code(graphcal::N006))]
    WrongArity {
        name: FnName,
        expected: usize,
        got: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wrong number of arguments")]
        span: SourceSpan,
    },

    #[error("cyclic dependency involving `{name}`")]
    #[diagnostic(
        code(graphcal::G001),
        help("declarations cannot form dependency cycles")
    )]
    CyclicDependency {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("{message}")]
    #[diagnostic(code(graphcal::E001))]
    EvalError {
        message: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unexpected state here")]
        span: Option<Span>,
    },

    #[error("dimension exponent overflow")]
    #[diagnostic(
        code(graphcal::D010),
        help("dimension exponents are stored as `i32`; reduce the magnitude of the exponent")
    )]
    DimensionOverflow {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("overflow here")]
        span: SourceSpan,
    },

    #[error("dimension mismatch: expected {expected}, found {found}")]
    #[diagnostic(code(graphcal::D001))]
    DimensionMismatch {
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("has type {rhs}")]
        span: SourceSpan,
    },

    #[error("incompatible indexed shape for `{function}()`: expected {expected}, found {found}")]
    #[diagnostic(code(graphcal::D022), help("{help}"))]
    LinearAlgebraShapeMismatch {
        function: BuiltinFnName,
        expected: String,
        found: String,
        help: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        function: BuiltinFnName,
        rank: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        function: BuiltinFnName,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("cyclic unit dependency involving `{name}`")]
    #[diagnostic(code(graphcal::D009), help("units cannot form dependency cycles"))]
    CyclicUnit {
        name: UnitName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("a dimensioned base requires a statically exact rational exponent")]
    #[diagnostic(
        code(graphcal::D005),
        help("use an exact integer such as `2` or a parenthesized rational such as `(3/2)`")
    )]
    RuntimeExponentForDimensionedBase {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("runtime exponent cannot determine the result dimension")]
        span: SourceSpan,
    },

    #[error("float syntax cannot be the exponent of a dimensioned base")]
    #[diagnostic(code(graphcal::D020))]
    FloatPowerExponent {
        /// Exact source replacement when the decimal value fits the dimension
        /// rational model. Also carried as structured LSP diagnostic data.
        replacement: Option<String>,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unit defined on an affine-prone dimension")]
        span: SourceSpan,
    },

    #[error("unknown struct type `{name}`")]
    #[diagnostic(
        code(graphcal::S002),
        help("struct types must be declared with `type` before use")
    )]
    UnknownStructType {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },

    #[error("unknown field `{field_name}` on struct `{type_name}`")]
    #[diagnostic(code(graphcal::S003))]
    UnknownField {
        type_name: StructTypeName,
        field_name: FieldName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no such field")]
        span: SourceSpan,
    },

    #[error("missing field(s) {missing:?} in construction of `{type_name}`")]
    #[diagnostic(
        code(graphcal::S004),
        help("all fields are required when constructing a struct")
    )]
    MissingFields {
        type_name: StructTypeName,
        missing: Vec<FieldName>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("incomplete construction")]
        span: SourceSpan,
    },

    #[error("missing field(s) {missing:?} in match pattern for `{constructor}`")]
    #[diagnostic(
        code(graphcal::S009),
        help(
            "all constructor fields must be bound as `field: variable` or discarded with `field: _`"
        )
    )]
    MissingPatternFields {
        constructor: ConstructorName,
        missing: Vec<FieldName>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("incomplete pattern")]
        span: SourceSpan,
    },

    #[error("extra field(s) {extra:?} in construction of `{type_name}`")]
    #[diagnostic(
        code(graphcal::S005),
        help("only fields declared in the struct type are allowed")
    )]
    ExtraFields {
        type_name: StructTypeName,
        extra: Vec<FieldName>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unexpected fields")]
        span: SourceSpan,
    },

    #[error("field `{field_name}` of `{type_name}`: expected dimension {expected}, found {found}")]
    #[diagnostic(code(graphcal::S006))]
    FieldDimensionMismatch {
        type_name: StructTypeName,
        field_name: FieldName,
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("has dimension {found}")]
        span: SourceSpan,
    },

    #[error("cannot access field of non-struct value `{name}`")]
    #[diagnostic(
        code(graphcal::S007),
        help("field access `.field` is only valid on struct values")
    )]
    NotAStruct {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a struct")]
        span: SourceSpan,
    },

    #[error("unknown local variable `{name}`")]
    #[diagnostic(
        code(graphcal::S008),
        help(
            "local variables are introduced by `for`, `scan`, `unfold`, `match`, or function parameters"
        )
    )]
    UnknownLocalRef {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },

    #[error("unknown index `{name}`")]
    #[diagnostic(
        code(graphcal::I001),
        help(
            "declare a named or coordinate index, or write `Fin(N)` explicitly for a structural axis; coordinate constructors are `range(start, end, step: delta)` and `linspace(start, end, points: N)`"
        )
    )]
    UnknownIndex {
        name: IndexName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown index")]
        span: SourceSpan,
    },

    #[error("unknown variant `{variant_name}` in index `{index_name}`")]
    #[diagnostic(code(graphcal::I002))]
    UnknownVariant {
        index_name: IndexName,
        variant_name: IndexVariantName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a variant of `{index_name}`")]
        span: SourceSpan,
    },

    #[error(
        "missing variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(missing)
    )]
    #[diagnostic(
        code(graphcal::I003),
        help("map literals must cover all variants of the index")
    )]
    MissingVariants {
        index_name: IndexName,
        missing: Vec<IndexEntryKey>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("incomplete map literal")]
        span: SourceSpan,
    },

    #[error(
        "extra variant(s) [{}] in map literal for index `{index_name}`",
        format_index_entry_keys(extra)
    )]
    #[diagnostic(
        code(graphcal::I004),
        help("only variants declared in the index are allowed")
    )]
    ExtraVariants {
        index_name: IndexName,
        extra: Vec<IndexEntryKey>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unexpected variants")]
        span: SourceSpan,
    },

    #[error("index mismatch: expected `{expected}`, found `{found}`")]
    #[diagnostic(code(graphcal::I005))]
    IndexMismatch {
        expected: IndexName,
        found: IndexName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wrong index")]
        span: SourceSpan,
    },

    #[error("file not found: {path}")]
    #[diagnostic(code(graphcal::M000), help("check that the file path is correct"))]
    FileNotFound { path: String },

    #[error("invalid source path `{path}`: {reason}")]
    #[diagnostic(
        code(graphcal::M023),
        help("Graphcal source files must be UTF-8 `.gcl` files")
    )]
    InvalidSourcePath { path: String, reason: String },

    #[error("circular import detected: {cycle}")]
    #[diagnostic(
        code(graphcal::M001),
        help("files cannot import each other in a cycle")
    )]
    CircularImport { cycle: String },

    #[error("imported file not found: {path}")]
    #[diagnostic(code(graphcal::M002))]
    ImportFileNotFound {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("referenced here")]
        span: SourceSpan,
    },

    #[error("a file-root `import` cannot target its own file `{path}`")]
    #[diagnostic(
        code(graphcal::M033),
        help(
            "top-level declarations are already in scope; self-imports are only meaningful inside isolated inline DAG bodies"
        )
    )]
    FileRootSelfImport {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this import resolves to its own file root")]
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found in imported file")]
        span: SourceSpan,
    },

    #[error("in imported file `{file_path}`, {mismatch}")]
    #[diagnostic(code(graphcal::M022))]
    ImportCategoryMismatch {
        file_path: String,
        mismatch: ImportItemCategoryMismatch,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wrong import category")]
        span: SourceSpan,
    },

    #[error("duplicate module name `{name}`")]
    #[diagnostic(code(graphcal::M005))]
    DuplicateModuleName {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown module")]
        span: SourceSpan,
    },

    #[error("coordinate index `{name}`: {message}")]
    #[diagnostic(
        code(graphcal::I006),
        help("coordinate constructor arguments must have exactly the same dimension")
    )]
    CoordinateIndexDimensionMismatch {
        name: IndexName,
        message: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("dimension mismatch")]
        span: SourceSpan,
    },

    #[error("coordinate index `{name}`: {message}")]
    #[diagnostic(code(graphcal::I007), help("{help}"))]
    CoordinateIndexInvalid {
        name: IndexName,
        message: String,
        help: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("invalid coordinate index")]
        span: SourceSpan,
    },

    #[error("expected Index, found Nat `{expression}`")]
    #[diagnostic(
        code(graphcal::I008),
        help("write `Fin({expression})` for an explicit finite structural index")
    )]
    ExpectedIndexFoundNat {
        expression: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("Nat is not implicitly converted to Index")]
        span: SourceSpan,
    },

    #[error("cannot reference assert `{name}` with `@`")]
    #[diagnostic(
        code(graphcal::A003),
        help("assert declarations are post-evaluation checks and cannot be referenced with `@`")
    )]
    GraphRefToAssert {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("`@{name}` is an assert, not a param or node")]
        span: SourceSpan,
    },

    #[error("assert body must evaluate to Bool, got {found}")]
    #[diagnostic(
        code(graphcal::A004),
        help("assert declarations must have a body that evaluates to Bool")
    )]
    AssertBodyNotBool {
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected Bool, found {found}")]
        span: SourceSpan,
    },

    #[error("unknown assert `{name}` in #[assumes(...)]")]
    #[diagnostic(
        code(graphcal::A005),
        help("`#[assumes(...)]` arguments must reference `assert` declarations")
    )]
    UnknownAssertInAssumes {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not an assert declaration")]
        span: SourceSpan,
    },

    #[error("`#[assumes(...)]` is not valid on `{kind}` declarations")]
    #[diagnostic(
        code(graphcal::A006),
        help("`#[assumes(...)]` is only valid on `node` and `param` declarations")
    )]
    InvalidAssumesTarget {
        kind: AttributeTarget,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a node or param")]
        span: SourceSpan,
    },

    #[error("attribute `#[{name}]` appears more than once")]
    #[diagnostic(
        code(graphcal::A019),
        help("`#[{name}]` is singleton metadata; combine its contents into one attribute")
    )]
    RepeatedSingletonAttribute {
        name: AttributeName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate `#[{name}]` attribute")]
        duplicate: SourceSpan,
        #[label("first `#[{name}]` attribute")]
        first: SourceSpan,
    },

    #[error("`#[assumes(...)]` requires at least one assertion name")]
    #[diagnostic(
        code(graphcal::A020),
        help("name one or more distinct assertions, or remove the inert attribute")
    )]
    EmptyAssumes {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no assertions named")]
        span: SourceSpan,
    },

    #[error("assertion `{name}` appears more than once in `#[assumes(...)]`")]
    #[diagnostic(
        code(graphcal::A021),
        help("each assertion may be named at most once by one declaration")
    )]
    DuplicateAssumesArgument {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate assertion name")]
        duplicate: SourceSpan,
        #[label("first named here")]
        first: SourceSpan,
    },

    #[error("`#[assumes(...)]` arguments must be plain identifiers")]
    #[diagnostic(
        code(graphcal::A022),
        help("name assertions directly, for example `#[assumes(first_check, second_check)]`")
    )]
    InvalidAssumesArgument {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a plain assertion name")]
        span: SourceSpan,
    },

    #[error("`#[lazy]` is reserved but not supported")]
    #[diagnostic(
        code(graphcal::A023),
        help("remove `#[lazy]`; Graphcal currently evaluates nodes eagerly")
    )]
    LazyNotSupported {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("lazy evaluation is not implemented")]
        span: SourceSpan,
    },

    #[error("attribute `hidden` does not apply to `{kind}` declarations")]
    #[diagnostic(
        code(graphcal::A017),
        help(
            "`#[hidden]` suppresses a plot's standalone output; it is only valid on `plot` declarations"
        )
    )]
    InvalidHiddenTarget {
        kind: AttributeTarget,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a plot")]
        span: SourceSpan,
    },

    #[error("unknown attribute `{name}`")]
    #[diagnostic(
        code(graphcal::A007),
        help(
            "recognized attributes are `#[assumes(...)]`, `#[expected_fail]`, `#[hidden]`, and `#[lazy]`"
        )
    )]
    UnknownAttribute {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown attribute")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail]` is not valid on `{kind}` declarations")]
    #[diagnostic(
        code(graphcal::A008),
        help("`#[expected_fail]` is only valid on `assert` declarations")
    )]
    InvalidExpectedFailTarget {
        kind: AttributeTarget,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not an assert")]
        span: SourceSpan,
    },

    #[error(
        "invalid argument in `#[expected_fail(...)]`: expected `Index#Variant`, `module::Index#Variant`, `#N` (Fin axes), or grouped variants"
    )]
    #[diagnostic(code(graphcal::A009))]
    ExpectedFailInvalidArg {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("invalid argument")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail(...)]` on non-indexed assertion")]
    #[diagnostic(
        code(graphcal::A010),
        help("use `#[expected_fail]` without arguments for non-indexed assertions")
    )]
    ExpectedFailNotIndexed {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this assertion is not indexed")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail]` without arguments on indexed assertion")]
    #[diagnostic(
        code(graphcal::A011),
        help(
            "use `#[expected_fail(Index#Variant, ...)]` (qualified `module::Index#Variant` also works) to specify which variants are expected to fail; for finite structural axes use `#[expected_fail(#N, ...)]`"
        )
    )]
    ExpectedFailAllOnIndexed {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this assertion is indexed")]
        span: SourceSpan,
    },

    #[error("duplicate key in `#[expected_fail(...)]`")]
    #[diagnostic(code(graphcal::A012), help("each expected-fail key must be unique"))]
    ExpectedFailDuplicateKey {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate expected-fail key")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail(...)]` key has the wrong index shape")]
    #[diagnostic(
        code(graphcal::A013),
        help(
            "single-index assertions require `Index#Variant` keys; multi-index assertions require full tuple keys in assertion axis order"
        )
    )]
    ExpectedFailKeyShapeMismatch {
        expected: usize,
        found: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected {expected} index axis/axes, found {found}")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail(...)]` key does not belong to the assertion index")]
    #[diagnostic(
        code(graphcal::A014),
        help("expected-fail keys must use the assertion's indexes in axis order")
    )]
    ExpectedFailKeyIndexMismatch {
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected index `{expected}`, found `{found}`")]
        span: SourceSpan,
    },

    #[error("`#[expected_fail(...)]` finite-index position `#{position}` is out of bounds")]
    #[diagnostic(
        code(graphcal::A016),
        help(
            "finite-index positions in expected-fail keys must satisfy `0 <= N < size` for a `Fin(size)` axis"
        )
    )]
    ExpectedFailFinitePositionOutOfBounds {
        position: u64,
        size: u64,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("position #{position} on an axis of size {size}")]
        span: SourceSpan,
    },

    #[error("negative tolerance in tolerance assertion")]
    #[diagnostic(
        code(graphcal::A015),
        help(
            "the tolerance in `~= expected +/- tolerance` must be non-negative; use `0` for exact-match semantics"
        )
    )]
    NegativeTolerance {
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("tolerance is {found}")]
        span: SourceSpan,
    },

    #[error("import path `{path}` resolves outside the project root")]
    #[diagnostic(
        code(graphcal::M008),
        help(
            "imports must reference files within the project directory tree; place a `graphcal.toml` in an ancestor directory to widen the project root"
        )
    )]
    ImportOutsideRoot {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("resolves outside project root")]
        span: SourceSpan,
    },

    #[error("cannot bind `{name}`: it is a {actual_kind}, not a param")]
    #[diagnostic(
        code(graphcal::O001),
        help("only `param` declarations can receive external parameter bindings")
    )]
    OverrideNotAParam {
        name: DeclName,
        actual_kind: crate::registry::resolve_types::DeclCategory,
    },

    #[error("unknown entry parameter `{name}` in external binding")]
    #[diagnostic(
        code(graphcal::O002),
        help("the name must match a `param` declared in the file")
    )]
    OverrideUnknownParam { name: DeclName },

    #[error("required param `{name}` has no value")]
    #[diagnostic(
        code(graphcal::O003),
        help(
            "supply the entry-DAG input via `--param '{name}=<value>'`, `--params-json`, or `--params-json-file`; otherwise bind this named input port at an include/call site"
        )
    )]
    RequiredParamNotProvided {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("declared here without a default value")]
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no {expected} input with this name")]
        span: SourceSpan,
    },

    #[error("module path starts with `{path_first}` but package name is `{package_name}`")]
    #[diagnostic(
        code(graphcal::M013),
        help("module paths must start with the package name from graphcal.toml")
    )]
    PackageNameMismatch {
        path_first: String,
        package_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("should start with `{package_name}`")]
        span: SourceSpan,
    },

    #[error("standard library modules are not yet implemented")]
    #[diagnostic(
        code(graphcal::M014),
        help(
            "the graphcal standard library (graphcal/math, etc.) will be available in a future release"
        )
    )]
    StdlibNotImplemented {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("stdlib not yet available")]
        span: SourceSpan,
    },

    #[error("failed to parse graphcal.toml: {message}")]
    #[diagnostic(code(graphcal::M015))]
    ManifestError { message: String },

    #[error("cross-file import `{path}` from a file outside any package")]
    #[diagnostic(
        code(graphcal::M017),
        help(
            "a Graphcal file is either part of a real package (lives at `<source_dir>/<package>.gcl` or under `<source_dir>/<package>/`) or a standalone virtual-package script. Standalone files may only reference their own top-level decls (via `import <file_stem>::{{...}};`) or their own inline DAGs. To pull symbols from a sibling file, add a `graphcal.toml` and place this file inside the package's namespace directory."
        )
    )]
    CrossFileImportInVirtualPackage {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not reachable from a virtual-package file")]
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("kind mismatch")]
        span: SourceSpan,
    },

    #[error(
        "index dimension mismatch: `{dep_index}` requires dimension {expected_dim} but `{bound_index}` has dimension {found_dim}"
    )]
    #[diagnostic(
        code(graphcal::I009),
        help("coordinate-index bindings must have matching dimensions")
    )]
    IndexBindingDimensionMismatch {
        dep_index: String,
        expected_dim: String,
        bound_index: String,
        found_dim: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("dimension mismatch")]
        span: SourceSpan,
    },

    /// A required typed Static input was not bound at a DAG instantiation boundary.
    #[error("required {kind} `{name}` must be bound at DAG instantiation")]
    #[diagnostic(
        code(graphcal::I010),
        help(
            "bind the input with its explicit `{kind}` marker at the include or direct-call site"
        )
    )]
    RequiredStaticInputNotBound {
        kind: crate::ir::static_interface::StaticInputKind,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("required {kind} input is not bound")]
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a recognized IANA timezone")]
        span: SourceSpan,
    },

    #[error("invalid datetime literal: {reason}")]
    #[diagnostic(code(graphcal::D028), help("{expectation}"))]
    InvalidDatetimeLiteral {
        expectation: crate::datetime_literal::DatetimeLiteralExpectation,
        reason: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected exactly one time scale here")]
        span: SourceSpan,
    },

    #[error("epoch's static time-scale argument must be a bare name")]
    #[diagnostic(code(graphcal::D029), help("use one of {expected}"))]
    InvalidEpochTimeScaleArgument {
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expected a supported bare time-scale name")]
        span: SourceSpan,
    },

    #[error("unsupported epoch time scale `{name}`")]
    #[diagnostic(code(graphcal::D030), help("use one of {expected}"))]
    UnsupportedEpochTimeScale {
        name: NameAtom,
        expected: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
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
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this local time is repeated")]
        datetime_span: SourceSpan,
        #[label("fold occurs in this timezone")]
        time_zone_span: SourceSpan,
    },

    #[error("domain violation: `{name}` value {value} is {violation}")]
    #[diagnostic(
        code(graphcal::C001),
        help("the value must satisfy the domain constraints declared on the type")
    )]
    DomainViolation {
        name: String,
        value: String,
        violation: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("value out of declared domain")]
        span: SourceSpan,
    },

    #[error(
        "domain bound dimension mismatch on `{name}`: type has dimension {type_dim}, but {bound_name} bound has dimension {bound_dim}"
    )]
    #[diagnostic(
        code(graphcal::C002),
        help("domain bounds must have the same dimension as the constrained type")
    )]
    DomainDimensionMismatch {
        name: String,
        type_dim: String,
        bound_name: String,
        bound_dim: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("dimension mismatch in domain bound")]
        span: SourceSpan,
    },

    #[error("domain constraint on `{name}`: min ({min}) exceeds max ({max})")]
    #[diagnostic(
        code(graphcal::C003),
        help("the min bound must be less than or equal to the max bound")
    )]
    DomainMinExceedsMax {
        name: String,
        min: String,
        max: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("min > max")]
        span: SourceSpan,
    },

    #[error("domain constraints are not valid on `{type_kind}` types")]
    #[diagnostic(
        code(graphcal::C004),
        help("domain constraints (min/max) are only valid on quantity, Int, and Datetime types")
    )]
    InvalidDomainTarget {
        type_kind: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("constraints not valid here")]
        span: SourceSpan,
    },

    #[error("domain bound type mismatch on Int `{name}`: {bound_name} bound has type {bound_type}")]
    #[diagnostic(
        code(graphcal::C005),
        help("Int domain bounds must be Int so their full range is preserved exactly")
    )]
    IntDomainBoundTypeMismatch {
        name: String,
        bound_name: String,
        bound_type: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("Int bound must have type Int")]
        span: SourceSpan,
    },

    #[error("domain constraints are not supported on generic type arguments")]
    #[diagnostic(
        code(graphcal::C006),
        help(
            "put the constraint on the field in the struct definition, not on the generic type argument"
        )
    )]
    GenericTypeArgDomainConstraint {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("constraint not allowed here")]
        span: SourceSpan,
    },

    #[error(
        "datetime domain bound type mismatch on `{name}`: target is {target_type}, but {bound_name} bound is {bound_type}"
    )]
    #[diagnostic(
        code(graphcal::C007),
        help(
            "datetime bounds must have exactly the constrained Datetime<S> type; use an explicit time-scale conversion"
        )
    )]
    DatetimeDomainBoundTypeMismatch {
        name: String,
        target_type: String,
        bound_name: String,
        bound_type: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("datetime bound has the wrong time scale or value type")]
        span: SourceSpan,
    },

    // --- Visibility errors ---
    /// Attempting to import a private (non-`pub`) item from another file.
    #[error("cannot import private item `{name}` from `{file_path}`")]
    #[diagnostic(
        code(graphcal::V001),
        help("add `pub` to the declaration in the source file to make it importable")
    )]
    ImportPrivateItem {
        name: String,
        file_path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not visible — item is private")]
        span: SourceSpan,
    },

    /// A required `index`, `type`, or `dim` is not marked `pub(bind)`.
    ///
    /// `param` is excluded: the declaration kind itself creates a required or
    /// defaulted input port and never carries a visibility annotation.
    #[error("required {kind} `{name}` must be declared `pub(bind)`")]
    #[diagnostic(
        code(graphcal::V002),
        help(
            "required indexes, types, and dimensions form the bindable interface — add `pub(bind)` before the declaration"
        )
    )]
    RequiredItemMustBeBindable {
        kind: String,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("required item must be `pub(bind)`")]
        span: SourceSpan,
    },

    /// A visible declaration references a private type-system item in
    /// its written signature (A9 case 1).
    ///
    /// `pub_kind` is the externally visible declaration category. A `param`
    /// contributes an input-port signature rather than an explicitly exported
    /// signature.
    #[error(
        "`{pub_kind}` `{pub_name}` references private {ref_kind} `{ref_name}` in its signature"
    )]
    #[diagnostic(
        code(graphcal::V003),
        help(
            "add `pub` to `{ref_name}` so it is visible across the include boundary, or stop exposing `{pub_name}`"
        )
    )]
    PrivateInPublic {
        pub_kind: DeclarationKind,
        pub_name: NameAtom,
        ref_kind: DeclarationKind,
        ref_name: NameAtom,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("references private `{ref_name}`")]
        ref_span: SourceSpan,
        #[label("visible declaration is here")]
        pub_span: SourceSpan,
    },

    /// A `pub(bind)` index with concrete variants has its variants used
    /// in a non-bindable body (`node` / `const`) or a public sink
    /// declaration in the defining file.
    ///
    /// Per axiom A10(c) / A10(b), a bindable index's variant literals
    /// must not appear in bodies that cannot themselves be re-bound by
    /// importers (the defining library must abstract over the index).
    #[error(
        "variant literal `{index}#{variant}` of `pub(bind) index` cannot be used in the defining file"
    )]
    #[diagnostic(
        code(graphcal::V004),
        help(
            "pub(bind) indexes may be overridden by importers; use `param` declarations for variant-specific values, or abstract over the index via `for p : I {{ … }}`"
        )
    )]
    PubIndexVariantLiteral {
        index: String,
        variant: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("variant literal of pub(bind) index")]
        span: SourceSpan,
    },

    /// An include overrides a bindable symbol `s`, but some kept
    /// declaration's body or default mentions a name nominally tied to
    /// `s` and was not itself re-bound by the same include statement
    /// (A8).
    ///
    /// Nominally-tied mentions today are: variant literals `s.v` for
    /// an overridden `index`, and constructors / field accesses of `s`
    /// for an overridden `type`. `dim` and `param` overrides are
    /// vacuous for A8 — their substitution is total — so they never
    /// trigger this error.
    #[error(
        "include overrides {overridden_kind} `{overridden}` but does not re-bind `{orphan_decl}`, whose default mentions `{detail}`"
    )]
    #[diagnostic(
        code(graphcal::V005),
        help(
            "add a binding for `{orphan_decl}` to this include, or keep `{overridden}` bound to its default"
        )
    )]
    IncludeMustReconcileOverride {
        overridden: String,
        overridden_kind: String,
        orphan_decl: String,
        detail: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("include is missing a binding for `{orphan_decl}`")]
        span: SourceSpan,
    },

    /// A selectively re-exported import/include item (`{ pub item }`)
    /// has an effective (post-substitution) signature that mentions a symbol
    /// that is `V = private` at the importing site — A9 case 2 / visibility
    /// composition.
    ///
    /// Concretely: an include binding renames a bindable symbol `s`
    /// in the dep to a name that is private at the importer, and the
    /// re-exported surface of the include carries that name into the
    /// importer's public API. Downstream consumers of the importer
    /// would see a signature referring to a symbol they cannot name.
    #[error(
        "re-exported {reexport_kind} `{reexport_name}`'s signature references private {leaked_kind} `{leaked_name}`"
    )]
    #[diagnostic(
        code(graphcal::V006),
        help(
            "make `{leaked_name}` `pub` at the importing file, or drop the per-item `pub` marker on this include / import"
        )
    )]
    GenericsLeakage {
        reexport_kind: String,
        reexport_name: String,
        leaked_kind: String,
        leaked_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("leaks private `{leaked_name}` across the include boundary")]
        span: SourceSpan,
    },

    #[error("unknown dag `{name}`")]
    #[diagnostic(
        code(graphcal::G002),
        help("the inline call references a dag that is not declared in this file")
    )]
    UnknownDag {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown dag")]
        span: SourceSpan,
    },

    #[error("unknown param `{name}` in DAG call to `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G003),
        help("the binding name must match a `param` declared in the called DAG")
    )]
    UnknownDagParam {
        name: String,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a param in `{dag_name}`")]
        span: SourceSpan,
    },

    #[error("missing required binding(s) {missing:?} when instantiating DAG `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G004),
        help(
            "every required `param` declared in the DAG must be bound at each `include` or call site"
        )
    )]
    MissingDagBindings {
        missing: Vec<String>,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("missing binding(s)")]
        span: SourceSpan,
    },

    #[error("unknown output `{name}` in DAG call to `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G005),
        help(
            "the projection after `).` must name a param input port or an explicitly exported node in the called DAG"
        )
    )]
    UnknownDagOutput {
        name: String,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a projectable value in `{dag_name}`")]
        span: SourceSpan,
    },

    #[error("DAG call binding `{param_name}`: expected {expected}, found {found}")]
    #[diagnostic(
        code(graphcal::G006),
        help("the binding expression must have the same type as the DAG's param declaration")
    )]
    DagArgTypeMismatch {
        param_name: String,
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("type mismatch")]
        span: SourceSpan,
    },
}

impl GraphcalError {
    /// Construct an internal diagnostic with an explicit source-anchor policy.
    #[must_use]
    pub fn internal_error(
        message: impl Into<String>,
        src: &NamedSource<Arc<String>>,
        anchor: DiagnosticAnchor,
    ) -> Self {
        Self::InternalError {
            message: message.into(),
            src: src.clone(),
            span: anchor.resolve(src.inner().len()),
        }
    }

    /// Whether this outcome represents cooperative cancellation rather than a
    /// Graphcal source error.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled(_))
    }

    /// Return the `NamedSource` embedded in this error, if any.
    ///
    /// Most variants carry a `#[source_code]` field naming the file and its
    /// full source text. Exposing it as a typed accessor lets diagnostic
    /// emitters pair the error's offsets with the exact source they index
    /// into — instead of inferring (name, source) from external context,
    /// which can silently desynchronize when an imported file is the origin.
    ///
    /// Returns `None` for control flow and the handful of variants without a
    /// source location: [`Self::Cancelled`], file-system errors before parsing
    /// ([`Self::FileNotFound`], [`Self::InvalidSourcePath`],
    /// [`Self::CircularImport`], [`Self::ManifestError`]) and CLI override errors
    /// ([`Self::OverrideNotAParam`], [`Self::OverrideUnknownParam`]).
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive variant list; one arm per error variant"
    )]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        let src = match self {
            Self::Cancelled(_)
            | Self::FileNotFound { .. }
            | Self::InvalidSourcePath { .. }
            | Self::CircularImport { .. }
            | Self::ManifestError { .. }
            | Self::OverrideNotAParam { .. }
            | Self::OverrideUnknownParam { .. } => return None,
            Self::DuplicateName { src, .. }
            | Self::DuplicateConstructorField { src, .. }
            | Self::BuiltinNameShadowed { src, .. }
            | Self::ConflictingImportedUnit { src, .. }
            | Self::InvalidPlotProperty { src, .. }
            | Self::PlotPropertyTypeMismatch { src, .. }
            | Self::PlotPropertyDimensioned { src, .. }
            | Self::PlotEncodingTypeMismatch { src, .. }
            | Self::PlotEncodingAxisMismatch { src, .. }
            | Self::UnknownPlotReference { src, .. }
            | Self::CompositionReferencesNonPlot { src, .. }
            | Self::DuplicatePlotReference { src, .. }
            | Self::ImportPlotItem { src, .. }
            | Self::ImportAssertionItem { src, .. }
            | Self::ImportRuntimeUnit { src, .. }
            | Self::ImportRequiredStaticInput { src, .. }
            | Self::ImportUnresolvedStaticDependency { src, .. }
            | Self::InvalidStaticBindingTarget { src, .. }
            | Self::IncludeItemNotProjectable { src, .. }
            | Self::IncludeConstructorOwnerRebound { src, .. }
            | Self::DuplicateIncludeSelection { src, .. }
            | Self::HiddenIncludeItemNotAPlot { src, .. }
            | Self::UnknownGraphRef { src, .. }
            | Self::BareGraphDeclarationRef { src, .. }
            | Self::TimeScaleInValuePosition { src, .. }
            | Self::UnknownFunction { src, .. }
            | Self::UnknownExternFunction { src, .. }
            | Self::NamedArgumentsOnFunction { src, .. }
            | Self::ExternSignatureMismatch { src, .. }
            | Self::PluginLoadFailed { src, .. }
            | Self::PluginForbiddenImport { src, .. }
            | Self::PluginInDependencyPackage { src, .. }
            | Self::PluginNotPinned { src, .. }
            | Self::PluginHashMismatch { src, .. }
            | Self::InvalidExternSignature { src, .. }
            | Self::DuplicateExternParameter { src, .. }
            | Self::MissingHostFunction { src, .. }
            | Self::ExternCallNotAllowed { src, .. }
            | Self::GraphRefInConst { src, .. }
            | Self::DagCallInCompileTime { src, .. }
            | Self::GraphRefInConstUnit { src, .. }
            | Self::NonConstUnitInConst { src, .. }
            | Self::WrongArity { src, .. }
            | Self::CyclicDependency { src, .. }
            | Self::EvalError { src, .. }
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
            | Self::UnknownStructType { src, .. }
            | Self::UnknownField { src, .. }
            | Self::MissingFields { src, .. }
            | Self::MissingPatternFields { src, .. }
            | Self::ExtraFields { src, .. }
            | Self::FieldDimensionMismatch { src, .. }
            | Self::NotAStruct { src, .. }
            | Self::UnknownLocalRef { src, .. }
            | Self::UnknownIndex { src, .. }
            | Self::UnknownVariant { src, .. }
            | Self::MissingVariants { src, .. }
            | Self::ExtraVariants { src, .. }
            | Self::IndexMismatch { src, .. }
            | Self::ImportFileNotFound { src, .. }
            | Self::FileRootSelfImport { src, .. }
            | Self::ImportNameNotFound { src, .. }
            | Self::ImportCategoryMismatch { src, .. }
            | Self::DuplicateModuleName { src, .. }
            | Self::UnknownModule { src, .. }
            | Self::CoordinateIndexDimensionMismatch { src, .. }
            | Self::CoordinateIndexInvalid { src, .. }
            | Self::ExpectedIndexFoundNat { src, .. }
            | Self::GraphRefToAssert { src, .. }
            | Self::AssertBodyNotBool { src, .. }
            | Self::UnknownAssertInAssumes { src, .. }
            | Self::InvalidAssumesTarget { src, .. }
            | Self::RepeatedSingletonAttribute { src, .. }
            | Self::EmptyAssumes { src, .. }
            | Self::DuplicateAssumesArgument { src, .. }
            | Self::InvalidAssumesArgument { src, .. }
            | Self::LazyNotSupported { src, .. }
            | Self::InvalidHiddenTarget { src, .. }
            | Self::UnknownAttribute { src, .. }
            | Self::InvalidExpectedFailTarget { src, .. }
            | Self::ExpectedFailInvalidArg { src, .. }
            | Self::ExpectedFailNotIndexed { src, .. }
            | Self::ExpectedFailAllOnIndexed { src, .. }
            | Self::ExpectedFailDuplicateKey { src, .. }
            | Self::ExpectedFailKeyShapeMismatch { src, .. }
            | Self::ExpectedFailKeyIndexMismatch { src, .. }
            | Self::ExpectedFailFinitePositionOutOfBounds { src, .. }
            | Self::NegativeTolerance { src, .. }
            | Self::ImportOutsideRoot { src, .. }
            | Self::RequiredParamNotProvided { src, .. }
            | Self::UnknownParamBinding { src, .. }
            | Self::BindingNotAParam { src, .. }
            | Self::DagInputCategoryMismatch { src, .. }
            | Self::PackageNameMismatch { src, .. }
            | Self::StdlibNotImplemented { src, .. }
            | Self::CrossFileImportInVirtualPackage { src, .. }
            | Self::InvalidTypeLevelBindingValue { src, .. }
            | Self::IndexBindingNotAnIndex { src, .. }
            | Self::IndexKindMismatch { src, .. }
            | Self::IndexBindingDimensionMismatch { src, .. }
            | Self::RequiredStaticInputNotBound { src, .. }
            | Self::ImportRuntimeItem { src, .. }
            | Self::InvalidTimezone { src, .. }
            | Self::InvalidDatetimeLiteral { src, .. }
            | Self::EpochTimeScaleArgumentCount { src, .. }
            | Self::InvalidEpochTimeScaleArgument { src, .. }
            | Self::UnsupportedEpochTimeScale { src, .. }
            | Self::NonexistentCivilDateTime { src, .. }
            | Self::RepeatedCivilDateTime { src, .. }
            | Self::DomainViolation { src, .. }
            | Self::DomainDimensionMismatch { src, .. }
            | Self::DomainMinExceedsMax { src, .. }
            | Self::InvalidDomainTarget { src, .. }
            | Self::IntDomainBoundTypeMismatch { src, .. }
            | Self::GenericTypeArgDomainConstraint { src, .. }
            | Self::DatetimeDomainBoundTypeMismatch { src, .. }
            | Self::ImportPrivateItem { src, .. }
            | Self::RequiredItemMustBeBindable { src, .. }
            | Self::PrivateInPublic { src, .. }
            | Self::PubIndexVariantLiteral { src, .. }
            | Self::IncludeMustReconcileOverride { src, .. }
            | Self::GenericsLeakage { src, .. }
            | Self::UnknownDag { src, .. }
            | Self::UnknownDagParam { src, .. }
            | Self::MissingDagBindings { src, .. }
            | Self::UnknownDagOutput { src, .. }
            | Self::DagArgTypeMismatch { src, .. } => src,
        };
        Some(src)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use miette::{Diagnostic as _, NamedSource};

    use super::GraphcalError;
    use crate::diagnostic_anchor::DiagnosticAnchor;

    fn diagnostic_code_catalog() -> BTreeMap<String, String> {
        let source = include_str!("error.rs");
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
        let source = NamedSource::new("test.gcl", Arc::new("node x".to_string()));

        let whole_file =
            GraphcalError::internal_error("whole file", &source, DiagnosticAnchor::WholeFile);
        let whole_file_labels = whole_file
            .labels()
            .expect("internal diagnostics expose a label iterator")
            .collect::<Vec<_>>();
        assert_eq!(whole_file_labels.len(), 1);
        assert_eq!(whole_file_labels[0].offset(), 0);
        assert_eq!(whole_file_labels[0].len(), source.inner().len());

        let builtin = GraphcalError::internal_error("builtin", &source, DiagnosticAnchor::Builtin);
        assert_eq!(
            builtin
                .labels()
                .expect("internal diagnostics expose a label iterator")
                .count(),
            0
        );
    }

    #[test]
    fn diagnostic_codes_are_unique_and_reassignments_are_pinned() {
        let catalog = diagnostic_code_catalog();
        assert!(catalog.len() > 100, "incomplete catalog: {catalog:?}");

        let mut variants_by_code = BTreeMap::new();
        for (variant, code) in &catalog {
            if let Some(previous) = variants_by_code.insert(code, variant) {
                panic!("diagnostic code `{code}` is shared by `{previous}` and `{variant}`");
            }
        }

        for (variant, expected) in [
            ("InvalidSourcePath", "M023"),
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
}
