use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_syntax::names::{
    DeclName, DimName, FieldName, FnName, IndexName, StructTypeName, UnitName, VariantName,
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

    #[error("unknown graph reference `@{name}`")]
    #[diagnostic(
        code(graphcal::N002),
        help("graph references must point to a `param` or `node`")
    )]
    UnknownGraphRef {
        name: DeclName,
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
        name: DeclName,
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
        name: FnName,
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
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("@ reference not allowed here")]
        span: SourceSpan,
    },

    #[error("graph reference `@{name}` not allowed in function body")]
    #[diagnostic(code(graphcal::F001))]
    GraphRefInFn {
        name: DeclName,
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

    #[error("cyclic dependency involving `{name}`")]
    #[diagnostic(
        code(graphcal::G001),
        help("declarations cannot form dependency cycles")
    )]
    CyclicDependency {
        name: DeclName,
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

    #[error("duplicate `let` binding `{name}`")]
    #[diagnostic(
        code(graphcal::S001),
        help("each `let` binding must have a unique name within a block")
    )]
    DuplicateLetBinding {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("duplicate binding here")]
        duplicate: SourceSpan,
        #[label("first defined here")]
        first: SourceSpan,
    },

    #[error("unknown struct type `{name}`")]
    #[diagnostic(
        code(graphcal::S002),
        help("struct types must be declared with `type` before use")
    )]
    UnknownStructType {
        name: StructTypeName,
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
        help("local variables must be defined with `let` before use")
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
        help("index must be declared with `index Name = {{ Variant1, Variant2, ... }}`")
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
        variant_name: VariantName,
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
        missing: Vec<VariantName>,
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
        extra: Vec<VariantName>,
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
        help("recognized attributes are `#[assumes(...)]` and `#[lazy]`")
    )]
    UnknownAttribute {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown attribute")]
        span: SourceSpan,
    },

    #[error("cannot override `{name}`: it is a {actual_kind}, not a param")]
    #[diagnostic(
        code(graphcal::O001),
        help("only `param` declarations can be overridden with --set")
    )]
    OverrideNotAParam { name: DeclName, actual_kind: String },

    #[error("unknown parameter `{name}` in --set override")]
    #[diagnostic(
        code(graphcal::O002),
        help("the name must match a `param` declared in the file")
    )]
    OverrideUnknownParam { name: DeclName },
}
