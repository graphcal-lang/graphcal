//! Failures binding values to the entry DAG's parameters from outside the
//! program (CLI `--param`, JSON parameter documents, model rows).

use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

use graphcal_compiler::declaration_category::DeclCategory;
use graphcal_compiler::syntax::decl_name::DeclName;

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
}

impl BindingError {
    /// The named source a located binding diagnostic points into.
    #[must_use]
    pub const fn named_source(&self) -> Option<&NamedSource<Arc<String>>> {
        match self {
            Self::NotAParam { .. } | Self::UnknownParam { .. } => None,
            Self::RequiredParamNotProvided { src, .. } => Some(src),
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
