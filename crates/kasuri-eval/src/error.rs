use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

/// Rich diagnostic error types for kasuri evaluation.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum KasuriError {
    #[error("duplicate name `{name}`")]
    #[diagnostic(code(kasuri::N001), help("each name must be unique within a file"))]
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
        code(kasuri::N002),
        help("graph references must point to a `param` or `node`")
    )]
    UnknownGraphRef {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },

    #[error("unknown constant `{name}`")]
    #[diagnostic(
        code(kasuri::N003),
        help("constant references must point to a `const` or built-in constant (PI, E)")
    )]
    UnknownConstRef {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },

    #[error("unknown function `{name}`")]
    #[diagnostic(
        code(kasuri::N004),
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
        code(kasuri::N005),
        help(
            "const expressions are evaluated at compile time and cannot reference params or nodes"
        )
    )]
    GraphRefInConst {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("@ reference not allowed here")]
        span: SourceSpan,
    },

    #[error("graph reference `@{name}` not allowed in function body")]
    #[diagnostic(code(kasuri::F001))]
    GraphRefInFn {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("@ reference not allowed here")]
        span: SourceSpan,
        #[help]
        help: String,
    },

    #[error("recursive function `{name}` detected")]
    #[diagnostic(
        code(kasuri::F002),
        help("kasuri does not support recursive functions")
    )]
    RecursiveFunction {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in recursion")]
        span: SourceSpan,
    },

    #[error("function `{name}` expects {expected} argument(s), got {got}")]
    #[diagnostic(code(kasuri::N006))]
    WrongArity {
        name: String,
        expected: usize,
        got: usize,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("wrong number of arguments")]
        span: SourceSpan,
    },

    #[error("cyclic dependency involving `{name}`")]
    #[diagnostic(code(kasuri::G001), help("declarations cannot form dependency cycles"))]
    CyclicDependency {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("involved in cycle")]
        span: SourceSpan,
    },

    #[error("{message}")]
    #[diagnostic(code(kasuri::E001))]
    EvalError {
        message: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("error here")]
        span: SourceSpan,
    },

    #[error("dimension mismatch: expected {expected}, found {found}")]
    #[diagnostic(code(kasuri::D001))]
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
        code(kasuri::D002),
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
        code(kasuri::D003),
        help("unit must be declared or part of the prelude")
    )]
    UnknownUnit {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown unit")]
        span: SourceSpan,
    },

    #[error("unknown dimension `{name}`")]
    #[diagnostic(
        code(kasuri::D004),
        help("dimension must be declared or part of the prelude")
    )]
    UnknownDimension {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unknown dimension")]
        span: SourceSpan,
    },

    #[error("exponent in power must be a numeric literal for dimensional analysis")]
    #[diagnostic(
        code(kasuri::D005),
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
        code(kasuri::D006),
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
        code(kasuri::S001),
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
        code(kasuri::S002),
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
    #[diagnostic(code(kasuri::S003))]
    UnknownField {
        type_name: String,
        field_name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("no such field")]
        span: SourceSpan,
    },

    #[error("missing field(s) {missing:?} in construction of `{type_name}`")]
    #[diagnostic(
        code(kasuri::S004),
        help("all fields are required when constructing a struct")
    )]
    MissingFields {
        type_name: String,
        missing: Vec<String>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("incomplete construction")]
        span: SourceSpan,
    },

    #[error("extra field(s) {extra:?} in construction of `{type_name}`")]
    #[diagnostic(
        code(kasuri::S005),
        help("only fields declared in the struct type are allowed")
    )]
    ExtraFields {
        type_name: String,
        extra: Vec<String>,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("unexpected fields")]
        span: SourceSpan,
    },

    #[error("field `{field_name}` of `{type_name}`: expected dimension {expected}, found {found}")]
    #[diagnostic(code(kasuri::S006))]
    FieldDimensionMismatch {
        type_name: String,
        field_name: String,
        expected: String,
        found: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("has dimension {found}")]
        span: SourceSpan,
    },

    #[error("cannot access field of non-struct value `{name}`")]
    #[diagnostic(
        code(kasuri::S007),
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
        code(kasuri::S008),
        help("local variables must be defined with `let` before use")
    )]
    UnknownLocalRef {
        name: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not found")]
        span: SourceSpan,
    },
}
