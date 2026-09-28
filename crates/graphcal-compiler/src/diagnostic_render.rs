//! Shell-side `miette` adapter for core [`Diagnostic`]s.
//!
//! [`RenderableDiagnostic`] resolves a diagnostic's [`SourceId`] through a
//! [`SourceRegistry`] (or takes the one source a single-source shell already
//! holds) and exposes the result as a [`miette::Diagnostic`], so CLI, LSP, and
//! snapshot renderers keep using `miette` while the core stays free of it. The adapter reproduces what `#[derive(miette::Diagnostic)]`
//! yields for an equivalent hand-written error: the same code, message, help,
//! and non-primary labels in declaration order (primary first).
//!
//! [`SourceId`]: crate::source_id::SourceId

use std::fmt;
use std::sync::Arc;

use miette::{LabeledSpan, NamedSource, SourceCode};

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::source_registry::{ForeignSourceId, SourceRegistry};
use crate::syntax::span::Span;

/// A core diagnostic payload with its source attached, ready for `miette`.
#[derive(Debug)]
pub struct RenderableDiagnostic<K> {
    kind: K,
    primary: Span,
    source: NamedSource<Arc<String>>,
}

impl<K> RenderableDiagnostic<K> {
    /// Attach the source `diagnostic` refers to.
    ///
    /// # Errors
    ///
    /// Returns [`ForeignSourceId`] if `registry` did not issue the
    /// diagnostic's source id.
    pub fn new(
        diagnostic: Diagnostic<K>,
        registry: &SourceRegistry,
    ) -> Result<Self, ForeignSourceId> {
        let source = registry.named_source(diagnostic.src)?.clone();
        Ok(Self::in_source(diagnostic.kind, diagnostic.primary, source))
    }

    /// Attach `source` to a payload located at `primary` in it.
    ///
    /// For shells that hold exactly the one source the payload was produced
    /// from (such as a parser run over one file), so there is no source id
    /// to resolve.
    pub const fn in_source(kind: K, primary: Span, source: NamedSource<Arc<String>>) -> Self {
        Self {
            kind,
            primary,
            source,
        }
    }

    /// The typed payload.
    pub const fn kind(&self) -> &K {
        &self.kind
    }

    /// The span the diagnostic is primarily about.
    pub const fn primary(&self) -> Span {
        self.primary
    }

    /// The named source the diagnostic's spans index into.
    pub const fn named_source(&self) -> &NamedSource<Arc<String>> {
        &self.source
    }
}

impl<K: DiagnosticKind> fmt::Display for RenderableDiagnostic<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl<K: DiagnosticKind + fmt::Debug> std::error::Error for RenderableDiagnostic<K> {}

impl<K: DiagnosticKind + fmt::Debug> miette::Diagnostic for RenderableDiagnostic<K> {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.kind.code()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.kind
            .help()
            .map(|help| Box::new(help) as Box<dyn fmt::Display + 'a>)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let primary = LabeledSpan::new_with_span(self.kind.primary_label(), self.primary);
        let secondary = self
            .kind
            .secondary_labels()
            .into_iter()
            .map(|label| LabeledSpan::new_with_span(Some(label.text), label.span));
        Some(Box::new(std::iter::once(primary).chain(secondary)))
    }
}

#[cfg(test)]
mod tests {
    use miette::{NarratableReportHandler, SourceSpan};

    use super::*;
    use crate::syntax::ast::{DomainBoundKind, NatExpr};
    use crate::syntax::decl_name::DeclName;
    use crate::syntax::index_name::{IndexEntryKey, IndexVariantName};
    use crate::syntax::names::{NameAtom, NamePath};
    use crate::syntax::parser::{
        CompositionKind, Expected, Found, InvalidNumberReason, ParseError, ParseErrorKind,
        PlotFieldContext, UnsupportedMultiDeclShape,
    };
    use crate::syntax::token::{SourceIdentifier, Token};

    /// Frozen copy of the derived `ParseError` that `ParseErrorKind` replaced.
    #[derive(Debug, thiserror::Error, miette::Diagnostic)]
    enum LegacyParseError {
        #[error("unexpected token `{found}`")]
        #[diagnostic(code(graphcal::P001), help("expected {expected}"))]
        UnexpectedToken {
            expected: String,
            found: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("here")]
            span: SourceSpan,
        },

        #[error("unexpected end of file")]
        #[diagnostic(code(graphcal::P002), help("expected {expected}"))]
        UnexpectedEof {
            expected: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("here")]
            span: SourceSpan,
        },

        #[error("invalid number literal")]
        #[diagnostic(code(graphcal::P003))]
        InvalidNumber {
            reason: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("{reason}")]
            span: SourceSpan,
        },

        #[error("table row has {got} value(s), but the header has {expected} column(s)")]
        #[diagnostic(code(graphcal::P004))]
        TableRowLengthMismatch {
            expected: u64,
            got: u64,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("this row has {got} value(s)")]
            span: SourceSpan,
        },

        #[error("unknown domain constraint key `{key}`")]
        #[diagnostic(
            code(graphcal::P005),
            help("valid domain constraint keys are `min` and `max`")
        )]
        InvalidDomainBoundKey {
            key: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("unknown key")]
            span: SourceSpan,
        },

        #[error("duplicate domain constraint `{bound}`")]
        #[diagnostic(
            code(graphcal::P021),
            help("each domain constraint may appear at most once")
        )]
        DuplicateDomainBound {
            bound: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("duplicate bound here")]
            span: SourceSpan,
        },

        #[error("duplicate DAG binding `{name}`")]
        #[diagnostic(
            code(graphcal::P025),
            help("each name may appear at most once in a DAG binding list")
        )]
        DuplicateDagBinding {
            name: NameAtom,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("duplicate binding")]
            duplicate: SourceSpan,
            #[label("first bound here")]
            first: SourceSpan,
        },

        #[error("stray character in source")]
        #[diagnostic(
            code(graphcal::P006),
            help("remove or replace this character; it is not part of the graphcal grammar")
        )]
        UnknownToken {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("stray character")]
            span: SourceSpan,
        },

        #[error(
            "multi-decl slot tuple has {tuple_count} entr{}, but the multi-decl declares {slot_count} slot{}",
            if *tuple_count == 1 { "y" } else { "ies" },
            if *slot_count == 1 { "" } else { "s" }
        )]
        #[diagnostic(
            code(graphcal::P007),
            help(
                "the slot tuple in `table[..., (…)]` must contain exactly one entry per declared slot"
            )
        )]
        MultiDeclTupleArity {
            slot_count: usize,
            tuple_count: usize,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("slot tuple here")]
            span: SourceSpan,
        },

        #[error(
            "multi-decl header row has {header_count} cell{}, but the multi-decl declares {slot_count} slot{}",
            if *header_count == 1 { "" } else { "s" },
            if *slot_count == 1 { "" } else { "s" }
        )]
        #[diagnostic(
            code(graphcal::P008),
            help("the header row (`: _, _, …;`) must have exactly one cell per slot")
        )]
        MultiDeclHeaderArity {
            slot_count: usize,
            header_count: usize,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("header row here")]
            span: SourceSpan,
        },

        #[error(
            "multi-decl row `{row_label}` has {got} value(s), but the header row declares {expected_count} value column{}",
            if *expected_count == 1 { "" } else { "s" }
        )]
        #[diagnostic(
            code(graphcal::P009),
            help("each row must have exactly one value per header column")
        )]
        MultiDeclRowArity {
            expected_count: usize,
            got: usize,
            row_label: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("this row has {got} value(s)")]
            span: SourceSpan,
        },

        #[error("multi-decl requires at least one shared axis")]
        #[diagnostic(
            code(graphcal::P011),
            help("declare the row axis in `table[SharedAxis, (…)]`")
        )]
        MultiDeclNoSharedAxis {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("missing shared axis")]
            span: SourceSpan,
        },

        #[error("{reason}")]
        #[diagnostic(
            code(graphcal::P012),
            help(
                "this multi-decl shape is scheduled for a later version; see issue #481 for the incremental plan"
            )
        )]
        MultiDeclUnsupportedShape {
            reason: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("here")]
            span: SourceSpan,
        },

        #[error("inline DAG call requires `::<out>` projection")]
        #[diagnostic(
            code(graphcal::P014),
            help(
                "add `::<output_name>` after the call; an instantiated DAG without a projection is not a graph value"
            )
        )]
        InlineDagCallMissingProjection {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("expected `::<out>` projection here")]
            span: SourceSpan,
        },

        #[error("syntax nesting is too deep")]
        #[diagnostic(
            code(graphcal::P015),
            help("the parser limits nesting to 256 levels; simplify the nested syntax")
        )]
        TooDeeplyNested {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("nesting exceeds the limit here")]
            span: SourceSpan,
        },

        #[error("`^0` exponent has no effect")]
        #[diagnostic(
            code(graphcal::P016),
            help(
                "a zero power erases its term; remove the term (or the exponent) instead of raising to zero"
            )
        )]
        ZeroExponent {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("exponent must be a non-zero integer")]
            span: SourceSpan,
        },

        #[error("Nat subtraction is not supported")]
        #[diagnostic(
            code(graphcal::P022),
            help(
                "express the larger size additively instead, for example use `D[Fin(N + 1)]` for the input and `D[Fin(N)]` for the smaller output"
            )
        )]
        NatSubtractionUnsupported {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("`-` is not part of the Nat polynomial algebra")]
            span: SourceSpan,
        },

        #[error("expected Index, found Nat `{expression}`")]
        #[diagnostic(
            code(graphcal::P023),
            help("write `{suggestion}` for an explicit finite structural index")
        )]
        ExpectedIndexFoundNat {
            expression: String,
            suggestion: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("Nat is not implicitly converted to Index")]
            span: SourceSpan,
        },

        #[error("`range(N)` is no longer a structural index constructor")]
        #[diagnostic(
            code(graphcal::P024),
            help(
                "write `Fin({cardinality})`; `range` is reserved for coordinate indexes with an explicit `step:`"
            )
        )]
        ObsoleteStructuralRange {
            cardinality: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("use `Fin(...)` here")]
            span: SourceSpan,
        },

        #[error("duplicate `{field}` in {context}")]
        #[diagnostic(
            code(graphcal::P018),
            help("each field may appear at most once; remove or rename the duplicate")
        )]
        DuplicatePlotField {
            field: String,
            context: String,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("duplicate field here")]
            span: SourceSpan,
        },

        #[error("plot declaration has no encoding channels")]
        #[diagnostic(
            code(graphcal::P019),
            help(
                "add an `encode:` block with at least one channel, e.g. `encode: {{ x: ..., y: ... }}`"
            )
        )]
        MissingPlotEncoding {
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("this plot has an empty or missing `encode:` block")]
            span: SourceSpan,
        },

        #[error("{kind} declaration has no plots")]
        #[diagnostic(
            code(graphcal::P020),
            help("add a non-empty `plots:` list, e.g. `plots: [my_plot]`")
        )]
        EmptyCompositionPlots {
            kind: &'static str,
            #[source_code]
            src: NamedSource<Arc<String>>,
            #[label("this {kind} has an empty or missing `plots:` list")]
            span: SourceSpan,
        },
    }

    fn render(diagnostic: &dyn miette::Diagnostic) -> String {
        let mut rendered = String::new();
        NarratableReportHandler::new()
            .render_report(&mut rendered, diagnostic)
            .expect("rendering into a String cannot fail");
        rendered
    }

    fn atom(text: &str) -> NameAtom {
        NameAtom::parse(text).expect("valid name atom")
    }

    fn ident(text: &str) -> SourceIdentifier {
        SourceIdentifier::parse(text).expect("valid identifier")
    }

    fn path(text: &str) -> NamePath {
        NamePath::local(atom(text))
    }

    const SOURCE: &str = "dag d(a = 1, a = 2) {}\nnode x = ;\nparam p: Dimensionless = 1.0;\n";

    /// Every legacy variant paired with the typed kind that replaced it.
    #[expect(
        clippy::too_many_lines,
        reason = "one pair per former `ParseError` variant keeps the parity table complete"
    )]
    fn parity_cases(
        named: impl Fn() -> NamedSource<Arc<String>>,
    ) -> Vec<(ParseErrorKind, Span, LegacyParseError)> {
        let span = Span::new(32, 1);
        let at = || SourceSpan::from(span);
        let first = Span::new(6, 1);
        let nat = NatExpr::Literal(3, Span::new(0, 1));
        let slot = DeclName::try_new("slot").expect("valid declaration name");
        vec![
            (
                ParseErrorKind::UnexpectedToken {
                    expected: Expected::Expression,
                    found: Found::Token(Token::Semicolon),
                },
                span,
                LegacyParseError::UnexpectedToken {
                    expected: "expression".to_string(),
                    found: ";".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::UnexpectedToken {
                    expected: Expected::MarkType,
                    found: Found::Name(ident("circle")),
                },
                span,
                LegacyParseError::UnexpectedToken {
                    expected: "`point`, `line`, `bar`, `area`, `rect`, or `tick`".to_string(),
                    found: "circle".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::SliceAxisMismatch {
                    expected: path("A"),
                    found: path("B"),
                },
                span,
                LegacyParseError::UnexpectedToken {
                    expected: "slice axis `A`".to_string(),
                    found: "B".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::UnexpectedEof {
                    expected: Expected::Token(Token::RBrace),
                },
                span,
                LegacyParseError::UnexpectedEof {
                    expected: "`}`".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::InvalidNumber {
                    reason: InvalidNumberReason::SliceIndexOutOfRange {
                        value: 4,
                        cardinality: 3,
                    },
                },
                span,
                LegacyParseError::InvalidNumber {
                    reason: "slice index #4 out of range for Fin(3)".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::TableRowLengthMismatch {
                    expected: 3,
                    got: 1,
                },
                span,
                LegacyParseError::TableRowLengthMismatch {
                    expected: 3,
                    got: 1,
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::InvalidDomainBoundKey { key: ident("mid") },
                span,
                LegacyParseError::InvalidDomainBoundKey {
                    key: "mid".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::DuplicateDomainBound {
                    bound: DomainBoundKind::Min,
                },
                span,
                LegacyParseError::DuplicateDomainBound {
                    bound: "min".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::DuplicateDagBinding {
                    name: atom("a"),
                    first,
                },
                Span::new(13, 1),
                LegacyParseError::DuplicateDagBinding {
                    name: atom("a"),
                    src: named(),
                    duplicate: Span::new(13, 1).into(),
                    first: first.into(),
                },
            ),
            (
                ParseErrorKind::UnknownToken,
                span,
                LegacyParseError::UnknownToken {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclTupleArity {
                    slot_count: 2,
                    tuple_count: 1,
                },
                span,
                LegacyParseError::MultiDeclTupleArity {
                    slot_count: 2,
                    tuple_count: 1,
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclHeaderArity {
                    slot_count: 1,
                    header_count: 2,
                },
                span,
                LegacyParseError::MultiDeclHeaderArity {
                    slot_count: 1,
                    header_count: 2,
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclRowArity {
                    expected_count: 1,
                    got: 2,
                    row_label: IndexEntryKey::named(IndexVariantName::try_new("X").expect("valid variant name")),
                },
                span,
                LegacyParseError::MultiDeclRowArity {
                    expected_count: 1,
                    got: 2,
                    row_label: "X".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclNoSharedAxis,
                span,
                LegacyParseError::MultiDeclNoSharedAxis {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclUnsupportedShape {
                    shape: UnsupportedMultiDeclShape::MissingVariantCells { slot },
                },
                span,
                LegacyParseError::MultiDeclUnsupportedShape {
                    reason: "slot `slot` is declared with an extra axis but has zero variant cells in the header row"
                        .to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MultiDeclUnsupportedShape {
                    shape: UnsupportedMultiDeclShape::MultipleExtraAxisSlots,
                },
                span,
                LegacyParseError::MultiDeclUnsupportedShape {
                    reason: "multi-decl with more than one extra-axis slot is not yet supported (v3)"
                        .to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::InlineDagCallMissingProjection,
                span,
                LegacyParseError::InlineDagCallMissingProjection {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::TooDeeplyNested,
                span,
                LegacyParseError::TooDeeplyNested {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::ZeroExponent,
                span,
                LegacyParseError::ZeroExponent {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::NatSubtractionUnsupported,
                span,
                LegacyParseError::NatSubtractionUnsupported {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::ExpectedIndexFoundNat {
                    expression: "3".to_string(),
                },
                span,
                LegacyParseError::ExpectedIndexFoundNat {
                    expression: "3".to_string(),
                    suggestion: "Fin(3)".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::ObsoleteStructuralRange { cardinality: nat },
                span,
                LegacyParseError::ObsoleteStructuralRange {
                    cardinality: "3".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::DuplicatePlotField {
                    field: ident("encode"),
                    context: PlotFieldContext::Composition(CompositionKind::Layer),
                },
                span,
                LegacyParseError::DuplicatePlotField {
                    field: "encode".to_string(),
                    context: "layer".to_string(),
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::MissingPlotEncoding,
                span,
                LegacyParseError::MissingPlotEncoding {
                    src: named(),
                    span: at(),
                },
            ),
            (
                ParseErrorKind::EmptyCompositionPlots {
                    kind: CompositionKind::Figure,
                },
                span,
                LegacyParseError::EmptyCompositionPlots {
                    kind: "figure",
                    src: named(),
                    span: at(),
                },
            ),
        ]
    }

    #[test]
    fn parse_errors_render_identically_to_the_former_derived_diagnostics() {
        let text = Arc::new(SOURCE.to_string());
        let mut registry = SourceRegistry::new();
        let id = registry.register("main.gcl", Arc::clone(&text));
        let named = || NamedSource::new("main.gcl", Arc::clone(&text));

        for (kind, primary, legacy) in parity_cases(named) {
            let expected = render(&legacy);
            let registered = RenderableDiagnostic::new(
                ParseError::new(kind.clone(), primary).located(id),
                &registry,
            )
            .expect("own source id");
            assert_eq!(render(&registered), expected);
            let single = RenderableDiagnostic::in_source(kind, primary, named());
            assert_eq!(render(&single), expected);
        }
    }

    #[test]
    fn accessors_expose_the_typed_payload() {
        let text = Arc::new(SOURCE.to_string());
        let rendered = RenderableDiagnostic::in_source(
            ParseErrorKind::UnknownToken,
            Span::new(3, 1),
            NamedSource::new("main.gcl", Arc::clone(&text)),
        );
        assert!(matches!(rendered.kind(), ParseErrorKind::UnknownToken));
        assert_eq!(rendered.primary(), Span::new(3, 1));
        assert_eq!(rendered.named_source().name(), "main.gcl");
    }

    #[test]
    fn foreign_source_ids_are_rejected() {
        let mut issuing = SourceRegistry::new();
        let id = issuing.register("main.gcl", Arc::new(String::new()));
        let diagnostic = Diagnostic::new(id, Span::new(0, 0), ParseErrorKind::UnknownToken);

        assert!(matches!(
            RenderableDiagnostic::new(diagnostic, &SourceRegistry::new()),
            Err(ForeignSourceId)
        ));
    }
}
