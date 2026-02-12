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
        help("only built-in functions are available in Phase 0")
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
}
