//! Failures binding values to the entry DAG's parameters from outside the
//! program (CLI `--param`, JSON parameter documents, model rows).

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::hir::closed_expr::ClosedExpressionError;
use graphcal_compiler::semantic::checked_type::{IndexTypeRef, TypeSpelling};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::index_name::IndexVariantName;

/// Why a literal value cannot be normalized onto its declared value schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BindingLiteralError {
    #[error("integer is not exactly representable as a real quantity")]
    InexactRealInteger,
    #[error("number is not exactly representable as Int")]
    InexactInt,
    #[error("map entry has more keys than the declared value has indexed axes")]
    OverNestedMapEntry,
}

/// Kind of value an external binding supplied, for kind mismatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingValueKind {
    Quantity,
    Int,
    Bool,
    Key,
    MapLiteral,
}

impl std::fmt::Display for BindingValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Quantity => "Quantity",
            Self::Int => "Int",
            Self::Bool => "Bool",
            Self::Key => "Key",
            Self::MapLiteral => "a map literal",
        })
    }
}

/// Why an external parameter binding was rejected.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum BindingError {
    #[error("cannot bind `{name}`: it is a {actual_kind}, not a param")]
    #[diagnostic(
        code(graphcal::O001),
        help("only `param` declarations can receive external parameter bindings")
    )]
    NotAParam {
        name: DeclName,
        actual_kind: DeclCategory,
    },

    #[error("unknown entry parameter `{name}` in external binding")]
    #[diagnostic(
        code(graphcal::O002),
        help("the name must match a `param` declared in the file")
    )]
    UnknownParam { name: DeclName },

    #[error("required param `{name}` has no value")]
    #[diagnostic(
        code(graphcal::O003),
        help(
            "supply the entry-DAG input via `--param '{name}=<value>'`, `--params-json`, or `--params-json-file`; otherwise bind this named input port at an include/call site"
        )
    )]
    RequiredParamNotProvided {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("declared here without a default value")]
        span: SourceSpan,
    },

    #[error("invalid binding for `{name}`: {reason}")]
    #[diagnostic(code(graphcal::O005))]
    InvalidLiteral {
        name: DeclName,
        reason: BindingLiteralError,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("invalid value")]
        span: SourceSpan,
    },

    #[error("binding for `{name}` is not a closed value: {reason}")]
    #[diagnostic(
        code(graphcal::O006),
        help("external bindings accept literal values only")
    )]
    NotClosed {
        name: DeclName,
        reason: ClosedExpressionError,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("not a closed value")]
        span: SourceSpan,
    },

    #[error("cannot bind {actual} to `{name}` of type `{expected}`")]
    #[diagnostic(code(graphcal::O007))]
    KindMismatch {
        name: DeclName,
        actual: BindingValueKind,
        expected: TypeSpelling,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expects `{expected}`")]
        span: SourceSpan,
    },

    #[error("quantity must be finite")]
    #[diagnostic(code(graphcal::O008))]
    NonFiniteQuantity {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("bound to a non-finite quantity")]
        span: SourceSpan,
    },

    #[error("parameter `{name}` is bound more than once")]
    #[diagnostic(code(graphcal::O009))]
    BoundMoreThanOnce {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("bound more than once")]
        span: SourceSpan,
    },

    #[error("{violation}")]
    #[diagnostic(code(graphcal::O010))]
    DomainViolation {
        name: DeclName,
        /// The domain checker's description of the violated bound.
        violation: String,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("value for `{name}` is out of its declared domain")]
        span: SourceSpan,
    },

    #[error("Tenax v2 requires a concrete named index")]
    #[diagnostic(code(graphcal::O011))]
    KeyIndexNotNamed {
        name: DeclName,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("this key's index is not a named index")]
        span: SourceSpan,
    },

    #[error("unknown category `{variant}` for index `{index}`")]
    #[diagnostic(code(graphcal::O012))]
    UnknownCategory {
        name: DeclName,
        variant: IndexVariantName,
        index: IndexTypeRef,
        #[source_code]
        src: NamedSource<Arc<String>>,
        #[label("expects a category of `{index}`")]
        span: SourceSpan,
    },
}

impl BindingError {
    /// The named source a located binding diagnostic points into.
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::NotAParam { .. } | Self::UnknownParam { .. } => None,
            Self::RequiredParamNotProvided { src, .. }
            | Self::InvalidLiteral { src, .. }
            | Self::NotClosed { src, .. }
            | Self::KindMismatch { src, .. }
            | Self::NonFiniteQuantity { src, .. }
            | Self::BoundMoreThanOnce { src, .. }
            | Self::DomainViolation { src, .. }
            | Self::KeyIndexNotNamed { src, .. }
            | Self::UnknownCategory { src, .. } => Some(src),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_failures_keep_their_codes() {
        let name = DeclName::expect_valid("mass");
        let src = NamedSource::new("main.gcl", Arc::new("param mass: Mass;".to_owned()));
        for (error, code, located) in [
            (
                BindingError::NotAParam {
                    name: name.clone(),
                    actual_kind: DeclCategory::Assert,
                },
                "graphcal::O001",
                false,
            ),
            (
                BindingError::UnknownParam { name: name.clone() },
                "graphcal::O002",
                false,
            ),
            (
                BindingError::RequiredParamNotProvided {
                    name,
                    src,
                    span: (6, 4).into(),
                },
                "graphcal::O003",
                true,
            ),
        ] {
            assert_eq!(
                error.code().map(|code| code.to_string()).as_deref(),
                Some(code)
            );
            assert_eq!(error.named_source().is_some(), located);
        }
    }
}
