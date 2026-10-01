use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};

use crate::diagnostic::{Diagnostic as LocatedDiagnostic, DiagnosticKind as _};
use crate::semantic_error::SemanticErrorKind;
use crate::source_id::SourceId;
use crate::source_registry::SourceRegistry;
use crate::syntax::span::Span;
use thiserror::Error;

use crate::declaration_kind::DeclarationKind;
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::outcome::Outcome;
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::names::NameAtom;

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
            Self::ImportPlotItem { src, .. }
            | Self::ImportAssertionItem { src, .. }
            | Self::ImportRuntimeUnit { src, .. }
            | Self::ImportRequiredStaticInput { src, .. }
            | Self::ImportUnresolvedStaticDependency { src, .. }
            | Self::InvalidStaticBindingTarget { src, .. }
            | Self::IncludeItemNotProjectable { src, .. }
            | Self::IncludeConstructorOwnerRebound { src, .. }
            | Self::DuplicateIncludeSelection { src, .. }
            | Self::EvalError { src, .. }
            | Self::EvaluationUnavailable { src, .. }
            | Self::InternalError { src, .. }
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
            | Self::ImportRuntimeItem { src, .. } => *src,
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
