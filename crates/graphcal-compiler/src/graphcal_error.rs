use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};

use crate::diagnostic::{Diagnostic as LocatedDiagnostic, DiagnosticKind as _};
use crate::semantic_error::SemanticErrorKind;
use crate::source_id::SourceId;
use crate::source_registry::SourceRegistry;
use crate::syntax::span::Span;
use thiserror::Error;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;

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
            Self::EvalError { src, .. }
            | Self::EvaluationUnavailable { src, .. }
            | Self::InternalError { src, .. } => *src,
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
