use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use crate::syntax::names::{
    DeclName, DimName, FieldName, FnName, IndexName, IndexVariantName, ScopedName, StructTypeName,
    UnitName,
};

/// Rich diagnostic error types for graphcal evaluation.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum GraphcalError {
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

    #[error("{kind} `{name}` shadows a built-in name")]
    #[diagnostic(
        code(graphcal::N009),
        help("choose a different name; built-in dimensions, types, and units cannot be redefined")
    )]
    BuiltinNameShadowed {
        kind: &'static str,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("shadows a built-in name")]
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

    #[error("unknown constant `{name}`")]
    #[diagnostic(
        code(graphcal::N003),
        help("constant references must point to a `const` or built-in constant (PI, E)")
    )]
    UnknownConstRef {
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
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

    #[error("graph reference `@{name}` not allowed in function body")]
    #[diagnostic(code(graphcal::F001))]
    GraphRefInFn {
        name: ScopedName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("@ reference not allowed here")]
        span: SourceSpan,
        #[help]
        help: String,
    },

    #[error("recursive function `{name}` detected")]
    #[diagnostic(
        code(graphcal::F002),
        help("graphcal does not support recursive functions")
    )]
    RecursiveFunction {
        name: FnName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in recursion")]
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

    #[error("function `{name}` expects {expected} generic argument(s), got {got}")]
    #[diagnostic(
        code(graphcal::N007),
        help("provide all generic parameters or none (to infer)")
    )]
    WrongGenericArity {
        name: FnName,
        expected: usize,
        got: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wrong number of generic arguments")]
        span: SourceSpan,
    },

    #[error(
        "generic argument type mismatch for parameter `{param}` of function `{name}`: expected {expected} constraint, got {found}"
    )]
    #[diagnostic(code(graphcal::N008))]
    GenericArgMismatch {
        name: FnName,
        param: String,
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this generic argument")]
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
        span: SourceSpan,
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
            "element-wise operands must be indexed by the same axes in the same order; a scalar operand broadcasts to every key"
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

    #[error("unknown unit `{name}`")]
    #[diagnostic(
        code(graphcal::D003),
        help("unit must be declared or part of the prelude")
    )]
    UnknownUnit {
        name: UnitName,
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
        name: DimName,
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

    #[error("exponent in power must be a numeric literal for dimensional analysis")]
    #[diagnostic(
        code(graphcal::D005),
        help("use a literal exponent like `x ^ 2.0` so dimensions can be checked at compile time")
    )]
    NonLiteralExponent {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("non-literal exponent")]
        span: SourceSpan,
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
            "declare with `index Name = {{ Variant1, Variant2, ... }};` or `index Name = linspace(start, end, step: step);`"
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

    #[error("missing variant(s) {missing:?} in map literal for index `{index_name}`")]
    #[diagnostic(
        code(graphcal::I003),
        help("map literals must cover all variants of the index")
    )]
    MissingVariants {
        index_name: IndexName,
        missing: Vec<IndexVariantName>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("incomplete map literal")]
        span: SourceSpan,
    },

    #[error("extra variant(s) {extra:?} in map literal for index `{index_name}`")]
    #[diagnostic(
        code(graphcal::I004),
        help("only variants declared in the index are allowed")
    )]
    ExtraVariants {
        index_name: IndexName,
        extra: Vec<IndexVariantName>,
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

    #[error("filename `{stem}` is not a valid module name")]
    #[diagnostic(
        code(graphcal::M004),
        help(
            "module names must be lower_snake_case identifiers; use `as alias` to specify an explicit name"
        )
    )]
    InvalidModuleName {
        stem: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("imported here")]
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
        help("check that an `import` declaration imports this module")
    )]
    UnknownModule {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown module")]
        span: SourceSpan,
    },

    #[error("name `{name}` not found in module `{module}`")]
    #[diagnostic(
        code(graphcal::M007),
        help("check that the name is declared in the imported file")
    )]
    QualifiedNameNotFound {
        module: String,
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found in module")]
        span: SourceSpan,
    },

    #[error(
        "range index `{name}`: start, end, and step must have the same dimension (found {start_dim}, {end_dim}, {step_dim})"
    )]
    #[diagnostic(
        code(graphcal::I006),
        help("all three values in range(start, end, step: step) must share the same dimension")
    )]
    RangeIndexDimensionMismatch {
        name: IndexName,
        start_dim: String,
        end_dim: String,
        step_dim: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("dimension mismatch")]
        span: SourceSpan,
    },

    #[error("range index `{name}`: {message}")]
    #[diagnostic(code(graphcal::I007))]
    RangeIndexInvalid {
        name: IndexName,
        message: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("invalid range")]
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

    #[error("assumed assertion `{name}` failed")]
    #[diagnostic(code(graphcal::A002))]
    AssumedAssertionFailed {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this assertion failed")]
        span: SourceSpan,
        #[help]
        help: String,
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
        kind: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a node or param")]
        span: SourceSpan,
    },

    #[error("unknown attribute `{name}`")]
    #[diagnostic(
        code(graphcal::A007),
        help("recognized attributes are `#[assumes(...)]`, `#[lazy]`, and `#[expected_fail]`")
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
        kind: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not an assert")]
        span: SourceSpan,
    },

    #[error(
        "invalid argument in `#[expected_fail(...)]`: expected `Index.Variant`, `module.Index.Variant`, `#N` (range axes), or grouped variants"
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
            "use `#[expected_fail(Index.Variant, ...)]` (qualified `module.Index.Variant` also works) to specify which variants are expected to fail; for Nat range axes use `#[expected_fail(#N, ...)]`"
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
            "single-index assertions require `Index.Variant` keys; multi-index assertions require full tuple keys in assertion axis order"
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

    #[error("`#[expected_fail(...)]` range step `#{step}` is out of bounds")]
    #[diagnostic(
        code(graphcal::A016),
        help(
            "range steps in expected-fail keys must satisfy `0 <= N < size` for a `range(size)` axis"
        )
    )]
    ExpectedFailRangeStepOutOfBounds {
        step: u64,
        size: u64,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("step #{step} on an axis of size {size}")]
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

    #[error("cannot override `{name}`: it is a {actual_kind}, not a param")]
    #[diagnostic(
        code(graphcal::O001),
        help("only `param` declarations can be overridden with --set")
    )]
    OverrideNotAParam {
        name: DeclName,
        actual_kind: crate::registry::resolve_types::DeclCategory,
    },

    #[error("unknown parameter `{name}` in --set override")]
    #[diagnostic(
        code(graphcal::O002),
        help("the name must match a `param` declared in the file")
    )]
    OverrideUnknownParam { name: DeclName },

    #[error("required param `{name}` has no value")]
    #[diagnostic(
        code(graphcal::O003),
        help(
            "provide a value via `--set '{name}=<value>'`, `--input`, or a parameterized import binding"
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
        actual_kind: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("targets a {actual_kind}, not a param")]
        span: SourceSpan,
    },

    #[error("instantiated import requires an alias or selective names")]
    #[diagnostic(
        code(graphcal::M011),
        help("use `import \"path\"(...) as alias;` or `import \"path\"(...) {{ name1, name2 }};`")
    )]
    InstantiatedImportNeedsNamespace {
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("instantiated import without alias or selective names")]
        span: SourceSpan,
    },

    #[error("bare module import requires a graphcal.toml with [package].name")]
    #[diagnostic(
        code(graphcal::M012),
        help(
            "create a graphcal.toml file in the project root with:\n\n[package]\nname = \"your_package_name\""
        )
    )]
    BareImportWithoutManifest {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("bare module path requires a manifest")]
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
            "a Graphcal file is either part of a real package (lives at `<source_dir>/<package>.gcl` or under `<source_dir>/<package>/`) or a standalone virtual-package script. Standalone files may only reference their own top-level decls (via `import <file_stem>.{{...}};`) or their own inline DAGs. To pull symbols from a sibling file, add a `graphcal.toml` and place this file inside the package's namespace directory."
        )
    )]
    CrossFileImportInVirtualPackage {
        path: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not reachable from a virtual-package file")]
        span: SourceSpan,
    },

    #[error("binding target `{name}` is an index, not a param")]
    #[diagnostic(
        code(graphcal::M016),
        help("index bindings must use another index name as the value, e.g., `{name} = MyIndex`")
    )]
    BindingTargetsIndex {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("targets an index, not a param")]
        span: SourceSpan,
    },

    #[error("index binding `{dep_index} = {value}`: `{value}` is not a known index")]
    #[diagnostic(
        code(graphcal::M017),
        help(
            "the right-hand side of an index binding must be a `cat` or `range` index declared in the importing file or its transitive imports"
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
            "named indexes (`cat`) can only be bound to named indexes; range indexes can only be bound to range indexes"
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
        help("range index bindings must have matching dimensions")
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

    /// A required index was not bound via parameterized import.
    ///
    /// Required indexes (`index Foo;`, `index Foo: Time;`) must be bound when the
    /// file is imported. They cannot be evaluated standalone.
    #[error("required index `{name}` must be bound via parameterized include")]
    #[diagnostic(
        code(graphcal::I010),
        help("use `include \"./file.gcl\"({name}: SomeIndex)` to bind this index")
    )]
    RequiredIndexNotBound {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("required index declared here")]
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
            "use a valid IANA timezone name like \"UTC\", \"America/New_York\", or \"Asia/Tokyo\""
        )
    )]
    InvalidTimezone {
        timezone: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a recognized IANA timezone")]
        span: SourceSpan,
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
        help(
            "domain constraints (min/max) are only valid on scalar, Dimensionless, and Int types"
        )
    )]
    InvalidDomainTarget {
        type_kind: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("constraints not valid here")]
        span: SourceSpan,
    },

    #[error(
        "domain bound on Int `{name}` must be unitless: {bound_name} bound has type {bound_type}"
    )]
    #[diagnostic(
        code(graphcal::C005),
        help("Int values are unitless; their domain bounds must be Int or dimensionless")
    )]
    IntDomainBoundNotUnitless {
        name: String,
        bound_name: String,
        bound_type: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("Int bound is not unitless")]
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
    /// `param` is excluded: required `param` is implicitly bindable (A5);
    /// it never carries a visibility annotation.
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
    /// The `pub_kind` string is the declaration kind (e.g. `"param"`,
    /// `"pub node"`, `"pub type"`). `param` is always visible (A5 §4.0)
    /// and never carries an explicit annotation.
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
        pub_kind: String,
        pub_name: String,
        ref_kind: String,
        ref_name: String,
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
        "variant literal `{index}.{variant}` of `pub(bind) index` cannot be used in the defining file"
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

    /// A `pub include` / `pub import` (or selective `{ pub items }`)
    /// re-exports a declaration whose effective (post-substitution)
    /// signature mentions a symbol that is `V = private` at the
    /// importing site — A9 case 2 / visibility composition.
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
            "make `{leaked_name}` `pub` at the importing file, or drop the `pub` / `pub(..)` re-export marker on this include / import"
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

    #[error("unknown param `{name}` in inline dag call to `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G003),
        help("the binding name must match a `param` declared in the called dag")
    )]
    UnknownInlineDagParam {
        name: String,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a param in `{dag_name}`")]
        span: SourceSpan,
    },

    #[error("missing required binding(s) {missing:?} in inline dag call to `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G004),
        help("every `param` declared in the dag must be bound at each inline call site")
    )]
    MissingInlineDagBindings {
        missing: Vec<String>,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("missing binding(s)")]
        span: SourceSpan,
    },

    #[error("unknown output `{name}` in inline dag call to `{dag_name}`")]
    #[diagnostic(
        code(graphcal::G005),
        help("the projection after `).` must name a `node` declared in the called dag")
    )]
    UnknownInlineDagOutput {
        name: String,
        dag_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a node in `{dag_name}`")]
        span: SourceSpan,
    },

    #[error("inline dag call binding `{param_name}`: expected {expected}, found {found}")]
    #[diagnostic(
        code(graphcal::G006),
        help("the binding expression must have the same type as the dag's param declaration")
    )]
    InlineDagArgDimensionMismatch {
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
    /// Return the `NamedSource` embedded in this error, if any.
    ///
    /// Most variants carry a `#[source_code]` field naming the file and its
    /// full source text. Exposing it as a typed accessor lets diagnostic
    /// emitters pair the error's offsets with the exact source they index
    /// into — instead of inferring (name, source) from external context,
    /// which can silently desynchronize when an imported file is the origin.
    ///
    /// Returns `None` for the handful of variants that represent errors
    /// without a source location: file-system errors before parsing
    /// ([`Self::FileNotFound`], [`Self::CircularImport`],
    /// [`Self::ManifestError`]) and CLI override errors
    /// ([`Self::OverrideNotAParam`], [`Self::OverrideUnknownParam`]).
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive variant list; one arm per error variant"
    )]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        let src = match self {
            Self::FileNotFound { .. }
            | Self::CircularImport { .. }
            | Self::ManifestError { .. }
            | Self::OverrideNotAParam { .. }
            | Self::OverrideUnknownParam { .. } => return None,
            Self::DuplicateName { src, .. }
            | Self::BuiltinNameShadowed { src, .. }
            | Self::UnknownGraphRef { src, .. }
            | Self::UnknownConstRef { src, .. }
            | Self::UnknownFunction { src, .. }
            | Self::GraphRefInConst { src, .. }
            | Self::GraphRefInFn { src, .. }
            | Self::RecursiveFunction { src, .. }
            | Self::WrongArity { src, .. }
            | Self::WrongGenericArity { src, .. }
            | Self::GenericArgMismatch { src, .. }
            | Self::CyclicDependency { src, .. }
            | Self::EvalError { src, .. }
            | Self::InternalError { src, .. }
            | Self::DimensionOverflow { src, .. }
            | Self::DimensionMismatch { src, .. }
            | Self::IndexedShapeMismatch { src, .. }
            | Self::DimensionMismatchInAnnotation { src, .. }
            | Self::UnknownUnit { src, .. }
            | Self::UnknownDimension { src, .. }
            | Self::CyclicDimension { src, .. }
            | Self::CyclicUnit { src, .. }
            | Self::NonLiteralExponent { src, .. }
            | Self::ConversionDimensionMismatch { src, .. }
            | Self::NestedConversion { src, .. }
            | Self::UnknownStructType { src, .. }
            | Self::UnknownField { src, .. }
            | Self::MissingFields { src, .. }
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
            | Self::ImportNameNotFound { src, .. }
            | Self::InvalidModuleName { src, .. }
            | Self::DuplicateModuleName { src, .. }
            | Self::UnknownModule { src, .. }
            | Self::QualifiedNameNotFound { src, .. }
            | Self::RangeIndexDimensionMismatch { src, .. }
            | Self::RangeIndexInvalid { src, .. }
            | Self::GraphRefToAssert { src, .. }
            | Self::AssertBodyNotBool { src, .. }
            | Self::AssumedAssertionFailed { src, .. }
            | Self::UnknownAssertInAssumes { src, .. }
            | Self::InvalidAssumesTarget { src, .. }
            | Self::UnknownAttribute { src, .. }
            | Self::InvalidExpectedFailTarget { src, .. }
            | Self::ExpectedFailInvalidArg { src, .. }
            | Self::ExpectedFailNotIndexed { src, .. }
            | Self::ExpectedFailAllOnIndexed { src, .. }
            | Self::ExpectedFailDuplicateKey { src, .. }
            | Self::ExpectedFailKeyShapeMismatch { src, .. }
            | Self::ExpectedFailKeyIndexMismatch { src, .. }
            | Self::ExpectedFailRangeStepOutOfBounds { src, .. }
            | Self::NegativeTolerance { src, .. }
            | Self::ImportOutsideRoot { src, .. }
            | Self::RequiredParamNotProvided { src, .. }
            | Self::UnknownParamBinding { src, .. }
            | Self::BindingNotAParam { src, .. }
            | Self::InstantiatedImportNeedsNamespace { src, .. }
            | Self::BareImportWithoutManifest { src, .. }
            | Self::PackageNameMismatch { src, .. }
            | Self::StdlibNotImplemented { src, .. }
            | Self::CrossFileImportInVirtualPackage { src, .. }
            | Self::BindingTargetsIndex { src, .. }
            | Self::IndexBindingNotAnIndex { src, .. }
            | Self::IndexKindMismatch { src, .. }
            | Self::IndexBindingDimensionMismatch { src, .. }
            | Self::RequiredIndexNotBound { src, .. }
            | Self::ImportRuntimeItem { src, .. }
            | Self::InvalidTimezone { src, .. }
            | Self::DomainViolation { src, .. }
            | Self::DomainDimensionMismatch { src, .. }
            | Self::DomainMinExceedsMax { src, .. }
            | Self::InvalidDomainTarget { src, .. }
            | Self::IntDomainBoundNotUnitless { src, .. }
            | Self::GenericTypeArgDomainConstraint { src, .. }
            | Self::ImportPrivateItem { src, .. }
            | Self::RequiredItemMustBeBindable { src, .. }
            | Self::PrivateInPublic { src, .. }
            | Self::PubIndexVariantLiteral { src, .. }
            | Self::IncludeMustReconcileOverride { src, .. }
            | Self::GenericsLeakage { src, .. }
            | Self::UnknownDag { src, .. }
            | Self::UnknownInlineDagParam { src, .. }
            | Self::MissingInlineDagBindings { src, .. }
            | Self::UnknownInlineDagOutput { src, .. }
            | Self::InlineDagArgDimensionMismatch { src, .. } => src,
        };
        Some(src)
    }
}
